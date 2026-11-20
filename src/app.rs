mod view;

use std::{collections::HashMap, time::Duration};

use anyhow::{Context as _, Result, ensure};
use gpui_kit::component::{
    Theme, ThemeMode, WindowExt,
    button::ButtonVariant,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
};
use gpui_kit::{
    AppContext, Context, Entity, FocusHandle, ParentElement, Subscription, Window, actions,
};
use uuid::Uuid;

use crate::{
    audio::{self, Capture, Playback},
    library::{Library, Marker, Meeting, Provider, Settings},
    processing,
};

actions!(reuniones, [AddMarker]);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Summary,
    Transcript,
    Markers,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Work {
    Saving,
    Transcribing,
    Summarizing,
}

impl Work {
    fn label(self) -> &'static str {
        match self {
            Self::Saving => "Guardando audio…",
            Self::Transcribing => "Transcribiendo con Whisper…",
            Self::Summarizing => "Generando resumen…",
        }
    }
}

#[derive(Clone, Copy)]
enum Edit {
    Meeting(Uuid),
    Marker(Uuid, Uuid),
}

pub(crate) struct Workspace {
    library: Library,
    settings: Settings,
    meetings: Vec<Meeting>,
    selected: Option<Uuid>,
    tab: Tab,
    preferences: bool,
    codex_signed_in: Option<bool>,
    search: Entity<InputState>,
    focus: FocusHandle,
    capture: Option<(Meeting, Capture)>,
    starting: bool,
    playback: Option<(Uuid, Playback)>,
    ticking: bool,
    jobs: HashMap<Uuid, Work>,
    errors: HashMap<Uuid, String>,
    notice: Option<String>,
    downloading: bool,
    transcript_page: usize,
    _search_subscription: Subscription,
}

impl Workspace {
    pub(crate) fn new(
        library: Library,
        settings: Settings,
        meetings: Vec<Meeting>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        match settings.dark {
            Some(dark) => Theme::change(theme_mode(dark), Some(window), cx),
            None => Theme::sync_system_appearance(Some(window), cx),
        }
        let selected = meetings.first().map(|meeting| meeting.id);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Buscar reuniones"));
        let search_subscription = cx.subscribe_in(&search, window, |_, _, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        let weak = cx.weak_entity();
        window.on_window_should_close(cx, move |_, cx| {
            weak.update(cx, |this, cx| {
                if this.capture.is_some() || this.starting {
                    this.notice = Some("Detén la grabación antes de cerrar la ventana.".into());
                } else if this.jobs.values().any(|work| *work == Work::Saving) {
                    this.notice = Some("Espera a que se guarde el audio.".into());
                } else {
                    return true;
                }
                cx.notify();
                false
            })
            .unwrap_or(true)
        });
        Self {
            library,
            settings,
            meetings,
            selected,
            tab: Tab::Summary,
            preferences: false,
            codex_signed_in: None,
            search,
            focus,
            capture: None,
            starting: false,
            playback: None,
            ticking: false,
            jobs: HashMap::new(),
            errors: HashMap::new(),
            notice: None,
            downloading: false,
            transcript_page: 0,
            _search_subscription: search_subscription,
        }
    }

    fn report(&mut self, error: anyhow::Error) {
        self.notice = Some(format!("{error:#}"));
    }

    fn select(&mut self, id: Uuid) {
        if self.selected != Some(id) {
            self.playback = None;
        }
        self.selected = Some(id);
        self.preferences = false;
        self.tab = Tab::Summary;
        self.transcript_page = 0;
        self.notice = None;
    }

    fn record(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.starting || self.capture.is_some() {
            return;
        }
        self.starting = true;
        self.notice = None;
        self.playback = None;
        let permission = cx.background_spawn(async { audio::request_microphone() });
        cx.spawn_in(window, async move |weak, cx| {
            let granted = permission.await;
            let _ = weak.update_in(cx, |this, _, cx| {
                this.starting = false;
                let result = granted.and_then(|granted| {
                    ensure!(granted, "Permite el acceso al micrófono en Configuración del Sistema → Privacidad y seguridad.");
                    let mut meeting = Meeting::new("Reunión de Teams".into());
                    meeting.recording_warning = Some("La grabación se interrumpió antes de finalizar.".into());
                    let directory = this.library.meeting_dir(meeting.id);
                    std::fs::create_dir_all(&directory)?;
                    let result = Capture::start(directory).and_then(|capture| {
                        this.library.save_meeting(&meeting)?;
                        Ok(capture)
                    });
                    match result {
                        Ok(capture) => {
                            this.preferences = false;
                            this.capture = Some((meeting, capture));
                            this.ensure_ticker(cx);
                            Ok(())
                        }
                        Err(error) => {
                            let _ = this.library.delete_meeting(meeting.id);
                            Err(error)
                        }
                    }
                });
                if let Err(error) = result { this.report(error); }
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    fn add_marker(&mut self, _: &AddMarker, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((meeting, capture)) = self.capture.as_mut() {
            let position = capture.elapsed_ms();
            let marker = Marker {
                id: Uuid::new_v4(),
                ms: position,
                label: "Momento importante".into(),
            };
            meeting.markers.push(marker);
            if let Err(error) = self.library.save_meeting(meeting) {
                self.report(error);
            }
        } else if let Some((id, playback)) = self.playback.as_ref() {
            let position = playback.player().get_pos().as_millis() as u64;
            let id = *id;
            let result = self.update_meeting(id, |meeting| {
                meeting.markers.push(Marker {
                    id: Uuid::new_v4(),
                    ms: position,
                    label: "Momento importante".into(),
                });
                meeting.markers.sort_by_key(|marker| marker.ms);
            });
            if let Err(error) = result {
                self.report(error);
            }
        }
        cx.notify();
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        let Some((mut meeting, capture)) = self.capture.take() else {
            return;
        };
        let id = meeting.id;
        let finished = match capture.finish() {
            Ok(finished) => finished,
            Err(error) => {
                self.report(error);
                cx.notify();
                return;
            }
        };
        meeting.duration_ms = finished.duration_ms;
        self.meetings.insert(0, meeting);
        self.select(id);
        self.jobs.insert(id, Work::Saving);
        let task = cx.background_spawn(async move {
            let result = finished
                .writer
                .join()
                .map_err(|_| anyhow::anyhow!("No se pudo guardar el audio"))?;
            Ok::<_, anyhow::Error>((result, finished.warning))
        });
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.jobs.remove(&id);
                let result = result.and_then(|(result, warning)| {
                    result?;
                    this.update_meeting(id, |meeting| meeting.recording_warning = warning)
                });
                match result {
                    Ok(()) if this.library.model_path().is_file() => this.start_transcript(id, cx),
                    Ok(()) => {
                        if this.selected == Some(id) && !this.preferences {
                            this.tab = Tab::Transcript;
                        }
                    }
                    Err(error) => {
                        this.errors.insert(id, format!("{error:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn download(&mut self, cx: &mut Context<Self>) {
        if self.downloading || self.library.model_path().is_file() {
            return;
        }
        self.downloading = true;
        self.notice = None;
        let path = self.library.model_path();
        let selected = self.selected.filter(|id| {
            self.meetings
                .iter()
                .any(|meeting| meeting.id == *id && meeting.transcript.is_none())
        });
        let task = cx.background_spawn(async move { processing::download_model(&path) });
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.downloading = false;
                match result {
                    Ok(()) => {
                        if let Some(id) = selected {
                            this.start_transcript(id, cx);
                        }
                    }
                    Err(error) => this.report(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn start_transcript(&mut self, id: Uuid, cx: &mut Context<Self>) {
        if self.jobs.contains_key(&id) {
            return;
        }
        let directory = self.library.meeting_dir(id);
        let model = self.library.model_path();
        if !model.is_file() || !directory.join(audio::TEAMS_FILE).is_file() {
            return;
        }
        self.jobs.insert(id, Work::Transcribing);
        self.errors.remove(&id);
        let task = cx.background_spawn(async move { processing::transcribe(&directory, &model) });
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.jobs.remove(&id);
                let result = result.and_then(|segments| {
                    this.update_meeting(id, |meeting| {
                        meeting.transcript = Some(segments);
                    })
                });
                match result {
                    Ok(()) => {
                        if this.settings.automatic
                            && let Some(provider) = this.settings.provider
                        {
                            this.start_summary(id, provider, cx);
                        }
                    }
                    Err(error) => {
                        this.errors.insert(id, format!("{error:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn request_summary(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        let Some(provider) = self.settings.provider else {
            self.preferences = true;
            cx.notify();
            return;
        };
        if self.settings.automatic {
            self.start_summary(id, provider, cx);
            return;
        }
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            dialog
                .confirm()
                .title("¿Generar resumen?")
                .description(format!(
                    "Se enviará la transcripción a {}. El audio se queda en tu Mac.",
                    provider.name()
                ))
                .ok_text("Generar resumen")
                .cancel_text("Cancelar")
                .on_ok(move |_, _, cx| {
                    let _ = weak.update(cx, |this, cx| this.start_summary(id, provider, cx));
                    true
                })
        });
    }

    fn start_summary(&mut self, id: Uuid, provider: Provider, cx: &mut Context<Self>) {
        if self.jobs.contains_key(&id) {
            return;
        }
        let Some(segments) = self
            .meetings
            .iter()
            .find(|meeting| meeting.id == id)
            .and_then(|meeting| meeting.transcript.as_ref())
            .filter(|segments| !segments.is_empty())
            .cloned()
        else {
            return;
        };
        self.jobs.insert(id, Work::Summarizing);
        self.errors.remove(&id);
        let task = cx.background_spawn(async move { processing::summarize(provider, &segments) });
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.jobs.remove(&id);
                let result = result.and_then(|summary| {
                    this.update_meeting(id, |meeting| {
                        meeting.summary = Some(summary);
                    })
                });
                if let Err(error) = result {
                    this.errors.insert(id, format!("{error:#}"));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn update_meeting(&mut self, id: Uuid, update: impl FnOnce(&mut Meeting)) -> Result<()> {
        let meeting = self
            .meetings
            .iter_mut()
            .find(|meeting| meeting.id == id)
            .context("No se encontró la reunión")?;
        let mut updated = meeting.clone();
        update(&mut updated);
        self.library.save_meeting(&updated)?;
        *meeting = updated;
        Ok(())
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        self.notice = None;
        if let Err(error) = self.library.save_settings(&self.settings) {
            self.report(error);
        }
        cx.notify();
    }

    fn toggle_preferences(&mut self, cx: &mut Context<Self>) {
        self.preferences = !self.preferences;
        self.notice = None;
        if self.preferences {
            self.check_provider(cx);
        }
        cx.notify();
    }

    fn check_provider(&mut self, cx: &mut Context<Self>) {
        if self.settings.provider != Some(Provider::Codex) {
            return;
        }
        let Some(path) = Provider::Codex.executable() else {
            return;
        };
        let task = cx.background_spawn(async move { processing::codex_signed_in(&path).ok() });
        cx.spawn(async move |weak, cx| {
            let status = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.codex_signed_in = status;
                cx.notify();
            });
        })
        .detach();
    }

    fn set_provider(&mut self, provider: Provider, cx: &mut Context<Self>) {
        if self.settings.provider != Some(provider) {
            self.settings.provider = Some(provider);
            self.settings.automatic = false;
            self.save_settings(cx);
            self.check_provider(cx);
        }
    }

    fn set_automatic(&mut self, enabled: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(provider) = self.settings.provider else {
            return;
        };
        if !enabled {
            self.settings.automatic = false;
            self.save_settings(cx);
            return;
        }
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            dialog.confirm()
                .title("¿Generar resúmenes automáticamente?")
                .description(format!("Después de transcribir cada reunión, la app enviará el texto a {}. El audio se queda en tu Mac.", provider.name()))
                .ok_text("Activar")
                .cancel_text("Cancelar")
                .on_ok(move |_, _, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        if this.settings.provider == Some(provider) {
                            this.settings.automatic = true;
                            this.save_settings(cx);
                        }
                    });
                    true
                })
        });
    }

    fn set_theme(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.settings.dark = Some(dark);
        Theme::change(theme_mode(dark), Some(window), cx);
        self.save_settings(cx);
    }

    fn edit(&self, target: Edit, initial: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(initial));
        let weak = cx.weak_entity();
        let title = match target {
            Edit::Meeting(_) => "Renombrar reunión",
            Edit::Marker(..) => "Cambiar nombre del marcador",
        };
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            let name = input.clone();
            dialog
                .confirm()
                .title(title)
                .child(Input::new(&input))
                .ok_text("Guardar")
                .cancel_text("Cancelar")
                .on_ok(move |_, _, cx| {
                    let name = name.read(cx).value().trim().to_owned();
                    if name.is_empty() {
                        return false;
                    }
                    weak.update(cx, |this, cx| {
                        let id = match target {
                            Edit::Meeting(id) | Edit::Marker(id, _) => id,
                        };
                        let result = this.update_meeting(id, |meeting| match target {
                            Edit::Meeting(_) => meeting.title = name,
                            Edit::Marker(_, marker_id) => {
                                if let Some(marker) =
                                    meeting.markers.iter_mut().find(|m| m.id == marker_id)
                                {
                                    marker.label = name;
                                }
                            }
                        });
                        let success = result.is_ok();
                        if let Err(error) = result {
                            this.report(error);
                        }
                        cx.notify();
                        success
                    })
                    .unwrap_or(false)
                })
        });
    }

    fn edit_summary(&self, id: Uuid, initial: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(6)
                .default_value(initial)
        });
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            let text = input.clone();
            dialog
                .confirm()
                .title("Editar resumen")
                .child(Textarea::new(&input))
                .ok_text("Guardar")
                .cancel_text("Cancelar")
                .on_ok(move |_, _, cx| {
                    let text = text.read(cx).value().trim().to_owned();
                    if text.is_empty() {
                        return false;
                    }
                    weak.update(cx, |this, cx| {
                        let result = this.update_meeting(id, |meeting| {
                            if let Some(summary) = meeting.summary.as_mut() {
                                summary.content.summary = text;
                            }
                        });
                        let saved = result.is_ok();
                        if let Err(error) = result {
                            this.report(error);
                        }
                        cx.notify();
                        saved
                    })
                    .unwrap_or(false)
                })
        });
    }

    fn delete(&self, id: Uuid, title: String, window: &mut Window, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            dialog
                .confirm()
                .title(format!("¿Eliminar «{title}»?"))
                .description("Se eliminarán el audio, la transcripción y todos los marcadores.")
                .ok_text("Eliminar")
                .ok_variant(ButtonVariant::Danger)
                .cancel_text("Cancelar")
                .on_ok(move |_, _, cx| {
                    weak.update(cx, |this, cx| {
                        if this.jobs.contains_key(&id) {
                            return false;
                        }
                        if let Err(error) = this.library.delete_meeting(id) {
                            this.report(error);
                            cx.notify();
                            return true;
                        }
                        this.playback = None;
                        this.meetings.retain(|meeting| meeting.id != id);
                        this.errors.remove(&id);
                        this.selected = this.meetings.first().map(|meeting| meeting.id);
                        this.tab = Tab::Summary;
                        cx.notify();
                        true
                    })
                    .unwrap_or(false)
                })
        });
    }

    fn play(&mut self, id: Uuid, seek: Option<u64>, cx: &mut Context<Self>) {
        let result = (|| -> Result<()> {
            if self
                .playback
                .as_ref()
                .is_none_or(|(active, _)| *active != id)
            {
                self.playback = Some((id, Playback::open(&self.library.meeting_dir(id))?));
            }
            let (_, playback) = self
                .playback
                .as_ref()
                .context("No se pudo abrir la grabación")?;
            let player = playback.player();
            if seek.is_none() && !player.is_paused() && !player.empty() {
                player.pause();
            } else {
                if player.empty() {
                    self.playback = Some((id, Playback::open(&self.library.meeting_dir(id))?));
                }
                let (_, playback) = self
                    .playback
                    .as_ref()
                    .context("No se pudo abrir la grabación")?;
                if let Some(position) = seek {
                    playback.seek(position)?;
                }
                playback.player().play();
            }
            Ok(())
        })();
        match result {
            Ok(()) => self.ensure_ticker(cx),
            Err(error) => self.report(error),
        }
        cx.notify();
    }

    fn ensure_ticker(&mut self, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |weak, cx| {
            loop {
                executor.timer(Duration::from_millis(250)).await;
                let active = weak
                    .update(cx, |this, cx| {
                        let active = this.capture.is_some()
                            || this.playback.as_ref().is_some_and(|(_, playback)| {
                                !playback.player().is_paused() && !playback.player().empty()
                            });
                        this.ticking = active;
                        cx.notify();
                        active
                    })
                    .unwrap_or(false);
                if !active {
                    break;
                }
            }
        })
        .detach();
    }
}

fn theme_mode(dark: bool) -> ThemeMode {
    if dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    }
}
