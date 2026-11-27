use std::{
    fs::File,
    io::BufWriter,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use coreaudio::{
    AppTapAggregateDevice, AudioObject, DEVICE_IS_ALIVE, DEVICE_NOMINAL_SAMPLE_RATE, Device,
    IOProc, PROCESS_BUNDLE_ID, ProcessTap, Scope, System, TapMute,
};
use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};
use rtrb::{Consumer, RingBuffer};

pub(crate) const RATE: u32 = 16_000;
pub(crate) const MIC_FILE: &str = "mic.wav";
pub(crate) const TEAMS_FILE: &str = "teams.wav";
const PLAYBACK_FILE: &str = "meeting.wav";
const BLOCK_SAMPLES: usize = 512;
const SECOND: u64 = 1_000_000_000;

#[link(name = "CoreAudio", kind = "framework")]
unsafe extern "C" {
    fn AudioGetCurrentHostTime() -> u64;
    fn AudioConvertHostTimeToNanos(time: u64) -> u64;
}

fn host_nanos(time: u64) -> u64 {
    // SAFETY: Core Audio accepts any host timestamp value.
    unsafe { AudioConvertHostTimeToNanos(time) }
}

fn now() -> u64 {
    // SAFETY: This Core Audio clock query has no arguments or preconditions.
    host_nanos(unsafe { AudioGetCurrentHostTime() })
}

#[derive(Default)]
struct Signal {
    last_buffer: AtomicU64,
    last_voice: AtomicU64,
    dropped: AtomicBool,
}

#[derive(Default)]
struct Clock {
    origin: AtomicU64,
    paused_at: AtomicU64,
}

impl Clock {
    fn elapsed(&self) -> u64 {
        let paused_at = self.paused_at.load(Relaxed);
        let time = if paused_at == 0 { now() } else { paused_at };
        time.saturating_sub(self.origin.load(Relaxed))
    }
}

pub(crate) struct Capture {
    mic: IOProc,
    teams: IOProc,
    mic_device: AudioObject<Device>,
    _aggregate: AppTapAggregateDevice,
    _tap: ProcessTap,
    clock: Arc<Clock>,
    mic_signal: Arc<Signal>,
    mic_muted: Arc<AtomicBool>,
    teams_signal: Arc<Signal>,
    end: Arc<AtomicU32>,
    writer: Option<JoinHandle<Result<()>>>,
}

pub(crate) struct FinishedCapture {
    pub(crate) duration_ms: u64,
    pub(crate) warning: Option<String>,
    pub(crate) writer: JoinHandle<Result<()>>,
}

impl Capture {
    pub(crate) fn start(directory: PathBuf) -> Result<Self> {
        let system = AudioObject::<System>::default();
        let processes: Vec<_> = system
            .processes()?
            .into_iter()
            .filter(|process| {
                process
                    .get_property(PROCESS_BUNDLE_ID)
                    .is_ok_and(|id: String| {
                        id == "com.microsoft.teams2"
                            || id.starts_with("com.microsoft.teams2.")
                            || id == "com.microsoft.teams"
                            || id.starts_with("com.microsoft.teams.")
                    })
            })
            .collect();
        ensure!(
            !processes.is_empty(),
            "No se encontró audio de Teams. Abre la aplicación de escritorio y entra a la llamada antes de grabar."
        );
        let tap = ProcessTap::stereo_mixdown(&processes, "Reuniones — Teams", TapMute::Unmuted)
            .context("No se pudo configurar el audio de Teams")?;
        let aggregate = AppTapAggregateDevice::with_tap(
            "Reuniones",
            &uuid::Uuid::new_v4().to_string(),
            tap.uid(),
        )
        .context("No se pudo preparar la entrada de Teams")?;

        let mic_device = system
            .current_device(Scope::Input)
            .context("No se encontró un micrófono. Revisa la entrada de sonido en macOS.")?;
        let clock = Arc::new(Clock::default());
        let mic_signal = Arc::new(Signal::default());
        let mic_muted = Arc::new(AtomicBool::new(false));
        let teams_signal = Arc::new(Signal::default());
        let (mut mic, mic_input) = input(
            mic_device,
            clock.clone(),
            mic_signal.clone(),
            Some(mic_muted.clone()),
        )?;
        let (mut teams, teams_input) = input(
            aggregate.device(),
            clock.clone(),
            teams_signal.clone(),
            None,
        )?;

        let mic_file = wav(&directory.join(MIC_FILE))?;
        let teams_file = wav(&directory.join(TEAMS_FILE))?;
        clock.origin.store(now(), Relaxed);
        mic.play().context("No se pudo iniciar el micrófono")?;
        teams.play().context(
            "No se pudo iniciar Teams. Revisa el permiso de grabación de audio del sistema.",
        )?;
        let end = Arc::new(AtomicU32::new(u32::MAX));
        let stop_at = end.clone();
        let writer = thread::Builder::new()
            .name("meeting-audio".into())
            .spawn(move || {
                write_tracks(
                    directory,
                    mic_file,
                    teams_file,
                    mic_input,
                    teams_input,
                    stop_at,
                )
            })?;

        Ok(Self {
            mic,
            teams,
            mic_device,
            _aggregate: aggregate,
            _tap: tap,
            clock,
            mic_signal,
            mic_muted,
            teams_signal,
            end,
            writer: Some(writer),
        })
    }

    pub(crate) fn elapsed_ms(&self) -> u64 {
        self.clock.elapsed() / 1_000_000
    }

    pub(crate) fn is_paused(&self) -> bool {
        self.clock.paused_at.load(Relaxed) != 0
    }

    pub(crate) fn is_mic_muted(&self) -> bool {
        self.mic_muted.load(Relaxed)
    }

    pub(crate) fn toggle_mic_mute(&self) {
        self.mic_muted.fetch_xor(true, Relaxed);
    }

    pub(crate) fn toggle_pause(&self) {
        let time = now();
        let paused_at = self.clock.paused_at.load(Relaxed);
        if paused_at == 0 {
            self.clock.paused_at.store(time, Relaxed);
        } else {
            self.clock
                .origin
                .fetch_add(time.saturating_sub(paused_at), Relaxed);
            self.clock.paused_at.store(0, Relaxed);
        }
    }

    pub(crate) fn signals(&self) -> [&'static str; 2] {
        let time = now();
        let mut signals = [&self.mic_signal, &self.teams_signal].map(|signal| {
            if self.is_paused() {
                "En pausa"
            } else if signal.dropped.load(Relaxed) {
                "Audio interrumpido"
            } else if time.saturating_sub(signal.last_buffer.load(Relaxed)) > 2 * SECOND {
                "Sin entrada"
            } else if time.saturating_sub(signal.last_voice.load(Relaxed)) < SECOND {
                "Con señal"
            } else {
                "Sin señal"
            }
        });
        if self.is_mic_muted() {
            signals[0] = "Silenciado";
        }
        if matches!(self.mic_device.get_property(DEVICE_IS_ALIVE), Ok(false)) {
            signals[0] = "Desconectado";
        }
        signals
    }

    pub(crate) fn finish(mut self) -> Result<FinishedCapture> {
        let duration_ms = self.elapsed_ms();
        self.stop();
        let disconnected = [&self.mic_signal, &self.teams_signal]
            .into_iter()
            .zip(["micrófono", "Teams"])
            .filter_map(|(signal, source)| {
                (signal.last_voice.load(Relaxed) == 0 || signal.dropped.load(Relaxed))
                    .then_some(source)
            })
            .collect::<Vec<_>>();
        let warning = (!disconnected.is_empty()).then(|| {
            format!(
                "No se detectó señal o se interrumpió el audio de {}.",
                disconnected.join(" y ")
            )
        });
        Ok(FinishedCapture {
            duration_ms,
            warning,
            writer: self.writer.take().context("La grabación ya terminó")?,
        })
    }

    fn stop(&mut self) {
        if self.end.load(Relaxed) == u32::MAX {
            let elapsed = self.clock.elapsed();
            let _ = self.mic.pause();
            let _ = self.teams.pause();
            let frames = (elapsed as u128 * u128::from(RATE)) / u128::from(SECOND);
            self.end
                .store(frames.min(u128::from(u32::MAX - 1)) as u32, Relaxed);
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

struct AudioBlock {
    offset: u64,
    len: usize,
    samples: [i16; BLOCK_SAMPLES],
}

fn input(
    device: AudioObject<Device>,
    clock: Arc<Clock>,
    signal: Arc<Signal>,
    muted: Option<Arc<AtomicBool>>,
) -> Result<(IOProc, Consumer<AudioBlock>)> {
    let input_rate = device.get_property(DEVICE_NOMINAL_SAMPLE_RATE)?;
    ensure!(
        input_rate.is_finite() && input_rate > 0.0,
        "Frecuencia de audio no válida"
    );
    let (mut producer, consumer) = RingBuffer::new(512);
    let scale = f64::from(RATE) / input_rate;
    let mut next = 0;
    let proc = device.add_io_proc(Scope::Input, move |buffers| {
        let Some(buffer) = buffers.first() else {
            return;
        };
        let time = host_nanos(
            buffer
                .get_input_time()
                .host_time()
                .or_else(|| buffer.get_now_time().host_time())
                .unwrap_or_else(|| {
                    // SAFETY: This Core Audio clock query has no preconditions.
                    unsafe { AudioGetCurrentHostTime() }
                }),
        );
        signal.last_buffer.store(time, Relaxed);
        if clock.paused_at.load(Relaxed) != 0 {
            return;
        }
        let origin = clock.origin.load(Relaxed);
        if time < origin {
            return;
        }

        let frames = buffer.get_frame_count() as usize;
        let offset = ((time - origin) as f64 * f64::from(RATE) / SECOND as f64).round() as u64;
        let start = if offset.abs_diff(next) <= 3 {
            next
        } else {
            offset
        };
        let count = (frames as f64 * scale).round() as usize;
        let is_muted = muted.as_ref().is_some_and(|muted| muted.load(Relaxed));
        next = start + count as u64;
        for begin in (0..count).step_by(BLOCK_SAMPLES) {
            let len = (count - begin).min(BLOCK_SAMPLES);
            let mut block = AudioBlock {
                offset: start + begin as u64,
                len,
                samples: [0; BLOCK_SAMPLES],
            };
            for (index, sample) in block.samples[..len].iter_mut().enumerate() {
                let frame =
                    (((begin + index) as f64 / scale) as usize).min(frames.saturating_sub(1));
                let mut sum = 0.0;
                let mut channels = 0;
                for buffer in buffers.iter() {
                    let width = buffer.get_channels() as usize;
                    if let Some(values) = buffer.data.get(frame * width..(frame + 1) * width) {
                        sum += values.iter().sum::<f32>();
                        channels += width;
                    }
                }
                let pcm = sum / channels.max(1) as f32;
                if pcm.abs() > 0.003 {
                    signal.last_voice.store(time, Relaxed);
                }
                if !is_muted {
                    *sample = (pcm.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
                }
            }
            if producer.push(block).is_err() {
                signal.dropped.store(true, Relaxed);
            }
        }
    })?;
    Ok((proc, consumer))
}

type Writer = WavWriter<BufWriter<File>>;

fn wav(path: &Path) -> Result<Writer> {
    Ok(WavWriter::create(
        path,
        WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        },
    )?)
}

fn drain(writer: &mut Writer, ptr: &mut Consumer<AudioBlock>, end: u32) -> Result<()> {
    while let Ok(block) = ptr.pop() {
        let remaining = u64::from(end).saturating_sub(block.offset) as usize;
        let samples = &block.samples[..block.len.min(remaining)];
        if samples.is_empty() {
            continue;
        }
        while u64::from(writer.duration()) < block.offset {
            writer.write_sample(0_i16)?;
        }
        let overlap = u64::from(writer.duration()).saturating_sub(block.offset) as usize;
        for sample in samples.iter().skip(overlap) {
            writer.write_sample(*sample)?;
        }
    }
    Ok(())
}

fn write_tracks(
    directory: PathBuf,
    mut mic: Writer,
    mut teams: Writer,
    mut mic_input: Consumer<AudioBlock>,
    mut teams_input: Consumer<AudioBlock>,
    end: Arc<AtomicU32>,
) -> Result<()> {
    let length = loop {
        let end = end.load(Relaxed);
        drain(&mut mic, &mut mic_input, end)?;
        drain(&mut teams, &mut teams_input, end)?;
        if end != u32::MAX {
            break end;
        }
        thread::sleep(Duration::from_millis(15));
    };
    for writer in [&mut mic, &mut teams] {
        while writer.duration() < length {
            writer.write_sample(0_i16)?;
        }
    }
    mic.finalize()?;
    teams.finalize()?;
    mix_tracks(&directory)
}

fn mix_tracks(directory: &Path) -> Result<()> {
    let mut mic = WavReader::open(directory.join(MIC_FILE))?;
    let mut teams = WavReader::open(directory.join(TEAMS_FILE))?;
    let mut combined = wav(&directory.join(PLAYBACK_FILE))?;
    let count = mic.duration().max(teams.duration());
    let mut mic = mic.samples::<i16>();
    let mut teams = teams.samples::<i16>();
    for _ in 0..count {
        let a = i32::from(mic.next().transpose()?.unwrap_or_default());
        let b = i32::from(teams.next().transpose()?.unwrap_or_default());
        let mixed = (a + b).clamp(i32::from(i16::MIN), i32::from(i16::MAX));
        combined.write_sample(mixed as i16)?;
    }
    combined.finalize()?;
    Ok(())
}

pub(crate) struct Playback {
    player: Player,
    _output: MixerDeviceSink,
}

impl Playback {
    pub(crate) fn open(directory: &Path) -> Result<Self> {
        let file = File::open(directory.join(PLAYBACK_FILE))
            .context("El audio todavía no está disponible")?;
        let source = Decoder::try_from(file)?;
        let output = DeviceSinkBuilder::open_default_sink()
            .context("No se pudo abrir la salida de audio")?;
        let player = Player::connect_new(output.mixer());
        player.pause();
        player.append(source);
        Ok(Self {
            player,
            _output: output,
        })
    }

    pub(crate) fn player(&self) -> &Player {
        &self.player
    }

    pub(crate) fn position_ms(&self, duration_ms: u64) -> u64 {
        if self.player.empty() {
            duration_ms
        } else {
            (self.player.get_pos().as_millis() as u64).min(duration_ms)
        }
    }

    pub(crate) fn seek(&self, ms: u64) -> Result<()> {
        self.player.try_seek(Duration::from_millis(ms))?;
        Ok(())
    }
}

pub(crate) fn request_microphone() -> Result<bool> {
    use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio};

    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let handler = block2::RcBlock::new(move |granted: objc2::runtime::Bool| {
        let _ = sender.try_send(granted.as_bool());
    });
    // SAFETY: Audio is an AVFoundation media type, and AVFoundation copies the
    // block for its asynchronous response.
    unsafe {
        let media_type =
            AVMediaTypeAudio.context("No se pudo solicitar el permiso de micrófono")?;
        AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &handler);
    }
    Ok(receiver.recv()?)
}
