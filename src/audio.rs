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
    pub(crate) fn start(directory: PathBuf, noise_reduction: bool) -> Result<Self> {
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

        let mic_file = TrackWriter::new(wav(&directory.join(MIC_FILE))?, noise_reduction);
        let teams_file = TrackWriter::new(wav(&directory.join(TEAMS_FILE))?, false);
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
    muted: bool,
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
                muted: is_muted,
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

const NOISE_FRAME: usize = nnnoiseless::DenoiseState::FRAME_SIZE / 3;

struct NoiseFilter {
    state: Box<nnnoiseless::DenoiseState<'static>>,
    input: [f32; nnnoiseless::DenoiseState::FRAME_SIZE],
    input_len: usize,
    last_sample: f32,
    muted: [bool; NOISE_FRAME],
    previous_muted: [bool; NOISE_FRAME],
    previous_len: usize,
}

impl NoiseFilter {
    fn new() -> Self {
        Self {
            state: nnnoiseless::DenoiseState::new(),
            input: [0.; nnnoiseless::DenoiseState::FRAME_SIZE],
            input_len: 0,
            last_sample: 0.,
            muted: [false; NOISE_FRAME],
            previous_muted: [false; NOISE_FRAME],
            previous_len: 0,
        }
    }

    fn push(&mut self, sample: i16, muted: bool, writer: &mut Writer) -> Result<()> {
        let sample = f32::from(sample);
        // RNNoise uses 48 kHz; interpolate the recording's 16 kHz PCM to its frame rate.
        for step in 0..3 {
            self.input[self.input_len * 3 + step] =
                self.last_sample + (sample - self.last_sample) * (step + 1) as f32 / 3.;
        }
        self.last_sample = sample;
        self.muted[self.input_len] = muted;
        self.input_len += 1;
        if self.input_len == NOISE_FRAME {
            self.process(writer)?;
        }
        Ok(())
    }

    fn process(&mut self, writer: &mut Writer) -> Result<()> {
        let mut output = [0.; nnnoiseless::DenoiseState::FRAME_SIZE];
        self.state.process_frame(&mut output, &self.input);
        // The output belongs to the preceding frame. Keep its mute mask and omit warm-up.
        for (index, muted) in self.previous_muted[..self.previous_len].iter().enumerate() {
            let sample = if *muted {
                0
            } else {
                let group = &output[index * 3..index * 3 + 3];
                (group.iter().sum::<f32>() / 3.)
                    .round()
                    .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16
            };
            writer.write_sample(sample)?;
        }
        self.previous_muted = self.muted;
        self.previous_len = self.input_len;
        self.input_len = 0;
        self.input.fill(0.);
        Ok(())
    }

    fn finish(&mut self, writer: &mut Writer) -> Result<()> {
        if self.input_len > 0 {
            self.process(writer)?;
        }
        if self.previous_len > 0 {
            self.process(writer)?;
        }
        Ok(())
    }
}

struct TrackWriter {
    writer: Writer,
    filter: Option<NoiseFilter>,
    position: u64,
}

impl TrackWriter {
    fn new(writer: Writer, noise_reduction: bool) -> Self {
        Self {
            writer,
            filter: noise_reduction.then(NoiseFilter::new),
            position: 0,
        }
    }

    fn write_sample(&mut self, sample: i16, muted: bool) -> Result<()> {
        let sample = if muted { 0 } else { sample };
        if let Some(filter) = &mut self.filter {
            filter.push(sample, muted, &mut self.writer)?;
        } else {
            self.writer.write_sample(sample)?;
        }
        self.position += 1;
        Ok(())
    }

    fn finalize(mut self) -> Result<()> {
        if let Some(filter) = &mut self.filter {
            filter.finish(&mut self.writer)?;
        }
        self.writer.finalize()?;
        Ok(())
    }
}

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

fn drain(writer: &mut TrackWriter, ptr: &mut Consumer<AudioBlock>, end: u32) -> Result<()> {
    while let Ok(block) = ptr.pop() {
        let remaining = u64::from(end).saturating_sub(block.offset) as usize;
        let samples = &block.samples[..block.len.min(remaining)];
        if samples.is_empty() {
            continue;
        }
        while writer.position < block.offset {
            writer.write_sample(0, true)?;
        }
        let overlap = writer.position.saturating_sub(block.offset) as usize;
        for sample in samples.iter().skip(overlap) {
            writer.write_sample(*sample, block.muted)?;
        }
    }
    Ok(())
}

fn write_tracks(
    directory: PathBuf,
    mut mic: TrackWriter,
    mut teams: TrackWriter,
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
        while writer.position < u64::from(length) {
            writer.write_sample(0, true)?;
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

#[cfg(test)]
mod tests {
    use super::{
        AudioBlock, BLOCK_SAMPLES, MIC_FILE, RATE, TEAMS_FILE, TrackWriter, drain, wav,
        write_tracks,
    };
    use std::sync::{Arc, atomic::AtomicU32};

    #[test]
    fn noise_filter_preserves_sample_counts_including_partial_final_frames() {
        let directory = tempfile::tempdir().unwrap();
        for length in [0, 1, 159, 160, 161, 480, 512, 1001] {
            let path = directory.path().join(format!("{length}.wav"));
            let mut track = TrackWriter::new(wav(&path).unwrap(), true);
            for index in 0..length {
                track
                    .write_sample((index % 100) as i16 * 100, false)
                    .unwrap();
            }
            track.finalize().unwrap();
            assert_eq!(hound::WavReader::open(path).unwrap().duration(), length);
        }
    }

    #[test]
    fn filtered_microphone_stays_silent_when_muted_and_teams_remains_unchanged() {
        let directory = tempfile::tempdir().unwrap();
        let (mut mic_tx, mic_rx) = rtrb::RingBuffer::new(16);
        let (mut teams_tx, teams_rx) = rtrb::RingBuffer::new(16);
        for index in 0..6 {
            let muted = (2..4).contains(&index);
            mic_tx
                .push(AudioBlock {
                    offset: index * BLOCK_SAMPLES as u64,
                    len: BLOCK_SAMPLES,
                    samples: [8000; BLOCK_SAMPLES],
                    muted,
                })
                .ok()
                .unwrap();
            teams_tx
                .push(AudioBlock {
                    offset: index * BLOCK_SAMPLES as u64,
                    len: BLOCK_SAMPLES,
                    samples: [1200; BLOCK_SAMPLES],
                    muted: false,
                })
                .ok()
                .unwrap();
        }
        write_tracks(
            directory.path().to_owned(),
            TrackWriter::new(wav(&directory.path().join(MIC_FILE)).unwrap(), true),
            TrackWriter::new(wav(&directory.path().join(TEAMS_FILE)).unwrap(), false),
            mic_rx,
            teams_rx,
            Arc::new(AtomicU32::new((6 * BLOCK_SAMPLES) as u32)),
        )
        .unwrap();
        let mic: Vec<_> = hound::WavReader::open(directory.path().join(MIC_FILE))
            .unwrap()
            .samples::<i16>()
            .map(Result::unwrap)
            .collect();
        let teams: Vec<_> = hound::WavReader::open(directory.path().join(TEAMS_FILE))
            .unwrap()
            .samples::<i16>()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            (mic.len(), teams.len()),
            (6 * BLOCK_SAMPLES, 6 * BLOCK_SAMPLES)
        );
        assert!(
            mic[2 * BLOCK_SAMPLES..4 * BLOCK_SAMPLES]
                .iter()
                .all(|sample| *sample == 0)
        );
        assert!(teams.iter().all(|sample| *sample == 1200));
    }

    #[test]
    fn filtered_track_preserves_timestamp_gaps_and_discards_overlap() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mic.wav");
        let mut track = TrackWriter::new(wav(&path).unwrap(), true);
        let (mut tx, mut rx) = rtrb::RingBuffer::new(4);
        for offset in [0, 400, 1500] {
            tx.push(AudioBlock {
                offset,
                len: BLOCK_SAMPLES,
                samples: [9000; BLOCK_SAMPLES],
                muted: false,
            })
            .ok()
            .unwrap();
        }
        drain(&mut track, &mut rx, 1801).unwrap();
        assert_eq!(track.position, 1801);
        track.finalize().unwrap();
        let samples: Vec<_> = hound::WavReader::open(path)
            .unwrap()
            .samples::<i16>()
            .map(Result::unwrap)
            .collect();
        assert_eq!(samples.len(), 1801);
        assert!(samples[912..1500].iter().all(|sample| *sample == 0));
    }

    #[test]
    fn noise_filter_attenuates_stationary_noise() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("noise.wav");
        let mut track = TrackWriter::new(wav(&path).unwrap(), true);
        let mut seed = 7_u32;
        let mut input_power = 0_f64;
        for index in 0..RATE * 2 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let sample = ((seed >> 16) as i16) / 8;
            track.write_sample(sample, false).unwrap();
            if index >= RATE {
                input_power += f64::from(sample).powi(2);
            }
        }
        track.finalize().unwrap();
        let output_power: f64 = hound::WavReader::open(path)
            .unwrap()
            .samples::<i16>()
            .skip(RATE as usize)
            .map(|sample| f64::from(sample.unwrap()).powi(2))
            .sum();
        assert!(
            output_power < input_power * 0.5,
            "noise power ratio: {}",
            output_power / input_power
        );
    }
}
