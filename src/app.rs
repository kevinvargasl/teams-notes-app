mod theme;
mod view;

use std::{collections::HashMap, time::Duration};

use anyhow::{Context as _, Result, ensure};
use gpui_kit::component::{
    IndexPath, WindowExt,
    button::ButtonVariant,
    input::{Input, InputEvent, InputState, Textarea, TextareaState},
    select::{SearchableVec, SelectEvent, SelectItem, SelectState},
    slider::{SliderEvent, SliderState, SliderValue},
};
use gpui_kit::{
    App, AppContext, Context, Entity, FocusHandle, IntoElement, ParentElement, SharedString,
    Styled, Subscription, Window, actions, div,
};
use uuid::Uuid;

use crate::{
    agent_options::{self, AgentOptions, DEFAULT},
    audio::{self, Capture, Playback},
    library::{Library, Marker, Meeting, Provider, Settings},
    processing,
};

actions!(reuniones, [AddMarker, MoveMeetingUp, MoveMeetingDown]);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Summary,
    Transcript,
    Markers,
}

impl Tab {
    const ALL: [Self; 3] = [Self::Summary, Self::Transcript, Self::Markers];

    fn label(self) -> &'static str {
        match self {
            Self::Summary => "Resumen",
            Self::Transcript => "Transcripción",
            Self::Markers => "Marcadores",
        }
    }
}

#[derive(Clone, Copy)]
enum PlaybackCommand {
    Toggle,
    Jump(u64),
    Seek(u64),
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

#[derive(Clone)]
struct AgentChoice(String);

impl SelectItem for AgentChoice {
    type Value = String;

    fn title(&self) -> SharedString {
        self.0.clone().into()
    }

    fn value(&self) -> &String {
        &self.0
    }

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div().w_full().cursor_pointer().child(self.title())
    }

    fn render_highlighted(
        &self,
        query: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        div()
            .w_full()
            .cursor_pointer()
            .child(self.0.render_highlighted(query, window, cx))
    }
}

pub(crate) struct Workspace {
    library: Library,
    settings: Settings,
    meetings: Vec<Meeting>,
    selected: Option<Uuid>,
    tab: Tab,
    preferences: bool,
    codex_signed_in: Option<bool>,
    claude_signed_in: Option<bool>,
    model_select: Option<Entity<SelectState<SearchableVec<AgentChoice>>>>,
    effort_select: Option<Entity<SelectState<Vec<AgentChoice>>>>,
    agent_subscriptions: Vec<Subscription>,
    options_loading: bool,
    options_revision: u64,
    search: Entity<InputState>,
    focus: FocusHandle,
    capture: Option<(Meeting, Capture)>,
    pending_save: Option<(Uuid, audio::FinishedCapture)>,
    starting: bool,
    playback: Option<(Uuid, Playback)>,
    ticking: bool,
    timeline: Entity<SliderState>,
    scrubbing: bool,
    jobs: HashMap<Uuid, Work>,
    errors: HashMap<Uuid, String>,
    notice: Option<String>,
    downloading: bool,
    transcript_page: usize,
    _subscriptions: [Subscription; 3],
}

impl Workspace {
    pub(crate) fn new(
        library: Library,
        settings: Settings,
        mut meetings: Vec<Meeting>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        theme::apply(settings.dark, window, cx);
        let ranks: HashMap<_, _> = settings
            .meeting_order
            .iter()
            .enumerate()
            .map(|(index, id)| (*id, index + 1))
            .collect();
        meetings.sort_by_key(|meeting| ranks.get(&meeting.id).copied().unwrap_or(0));
        let selected = meetings.first().map(|meeting| meeting.id);
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Buscar reuniones"));
        let search_subscription = cx.subscribe_in(&search, window, |_, _, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let timeline = cx.new(|_| SliderState::new().max(1000.).step(1.));
        let timeline_subscription = cx.subscribe_in(
            &timeline,
            window,
            |this, _, event, window, cx| match event {
                SliderEvent::Change(_) => {
                    this.scrubbing = true;
                    cx.notify();
                }
                SliderEvent::Release(SliderValue::Single(value)) => {
                    this.scrubbing = false;
                    if let Some(meeting) = this.selected_meeting() {
                        let target =
                            (f64::from(*value) * meeting.duration_ms as f64 / 1000.).round() as u64;
                        this.play(meeting.id, PlaybackCommand::Seek(target), window, cx);
                    }
                }
                SliderEvent::Release(_) => this.scrubbing = false,
            },
        );
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
        let quit_subscription = cx.on_app_quit(|this, _| {
            // GPUI waits only 200 ms for quit futures. Complete the writer and
            // metadata synchronously before returning the shutdown future.
            if let Err(error) = this.stop_capture() {
                eprintln!("No se pudo detener la grabación al salir: {error:#}");
            }
            if let Some((_, Err(error))) = this.complete_save() {
                eprintln!("No se pudo guardar la grabación al salir: {error:#}");
            }
            async {}
        });
        Self {
            library,
            settings,
            meetings,
            selected,
            tab: Tab::Summary,
            preferences: false,
            codex_signed_in: None,
            claude_signed_in: None,
            model_select: None,
            effort_select: None,
            agent_subscriptions: Vec::new(),
            options_loading: false,
            options_revision: 0,
            search,
            focus,
            capture: None,
            pending_save: None,
            starting: false,
            playback: None,
            ticking: false,
            timeline,
            scrubbing: false,
            jobs: HashMap::new(),
            errors: HashMap::new(),
            notice: None,
            downloading: false,
            transcript_page: 0,
            _subscriptions: [
                search_subscription,
                timeline_subscription,
                quit_subscription,
            ],
        }
    }

    fn report(&mut self, error: anyhow::Error) {
        self.notice = Some(format!("{error:#}"));
    }

    fn selected_meeting(&self) -> Option<&Meeting> {
        self.meetings
            .iter()
            .find(|meeting| Some(meeting.id) == self.selected)
    }

    fn reorder_meeting(&mut self, dragged: Uuid, target: Uuid, cx: &mut Context<Self>) {
        if !self.search.read(cx).value().trim().is_empty() {
            return;
        }
        let Some(source) = self
            .meetings
            .iter()
            .position(|meeting| meeting.id == dragged)
        else {
            return;
        };
        let Some(destination) = self
            .meetings
            .iter()
            .position(|meeting| meeting.id == target)
        else {
            return;
        };
        if source == destination {
            return;
        }
        let mut order: Vec<_> = self.meetings.iter().map(|meeting| meeting.id).collect();
        let id = order.remove(source);
        order.insert(destination, id);
        let previous = std::mem::replace(&mut self.settings.meeting_order, order);
        if let Err(error) = self.library.save_settings(&self.settings) {
            self.settings.meeting_order = previous;
            self.report(error);
        } else {
            let meeting = self.meetings.remove(source);
            self.meetings.insert(destination, meeting);
        }
        cx.notify();
    }

    fn move_meeting(&mut self, id: Uuid, offset: isize, cx: &mut Context<Self>) {
        let target = self
            .meetings
            .iter()
            .position(|meeting| meeting.id == id)
            .and_then(|index| index.checked_add_signed(offset))
            .and_then(|index| self.meetings.get(index))
            .map(|meeting| meeting.id);
        if let Some(target) = target {
            self.reorder_meeting(id, target, cx);
        }
    }

    fn select(&mut self, id: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        self.preferences = false;
        if self.selected == Some(id) {
            return;
        }
        self.playback = None;
        self.reset_timeline(window, cx);
        self.selected = Some(id);
        self.tab = Tab::Summary;
        self.transcript_page = 0;
        self.notice = None;
    }

    fn record(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.starting || self.capture.is_some() || self.pending_save.is_some() {
            return;
        }
        self.starting = true;
        self.notice = None;
        self.playback = None;
        let permission = cx.background_spawn(async { audio::request_microphone() });
        cx.spawn_in(window, async move |weak, cx| {
            let granted = permission.await;
            let _ = weak.update_in(cx, |this, window, cx| {
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
                            this.reset_timeline(window, cx);
                            this.ensure_ticker(window, cx);
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
            let position = playback.position_ms(
                self.selected_meeting()
                    .map_or(0, |meeting| meeting.duration_ms),
            );
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

    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.stop_capture() {
            Ok(Some(id)) => {
                self.select(id, window, cx);
                self.ensure_ticker(window, cx);
            }
            Ok(None) => return,
            Err(error) => self.report(error),
        }
        cx.notify();
    }

    fn stop_capture(&mut self) -> Result<Option<Uuid>> {
        let Some((mut meeting, capture)) = self.capture.take() else {
            return Ok(None);
        };
        let id = meeting.id;
        let finished = capture.finish()?;
        meeting.duration_ms = finished.duration_ms;
        self.meetings.insert(0, meeting);
        self.jobs.insert(id, Work::Saving);
        self.pending_save = Some((id, finished));
        Ok(Some(id))
    }

    fn complete_save(&mut self) -> Option<(Uuid, Result<()>)> {
        let (id, finished) = self.pending_save.take()?;
        let result = (|| {
            finished
                .writer
                .join()
                .map_err(|_| anyhow::anyhow!("No se pudo guardar el audio"))??;
            self.update_meeting(id, |meeting| meeting.recording_warning = finished.warning)
        })();
        self.jobs.remove(&id);
        Some((id, result))
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
        let regenerating = self
            .meetings
            .iter()
            .any(|meeting| meeting.id == id && meeting.summary.is_some());
        if self.settings.automatic && !regenerating {
            self.start_summary(id, provider, cx);
            return;
        }
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let weak = weak.clone();
            dialog
                .confirm()
                .title(if regenerating { "¿Volver a generar el resumen?" } else { "¿Generar resumen?" })
                .description(if regenerating {
                    format!("Se enviará la transcripción a {} y se reemplazará el resumen actual, incluidas tus ediciones.", provider.name())
                } else {
                    format!("Se enviará la transcripción a {}. El audio se queda en tu Mac.", provider.name())
                })
                .ok_text(if regenerating { "Volver a generar" } else { "Generar resumen" })
                .footer(view::confirmation_footer(
                    if regenerating { "Volver a generar" } else { "Generar resumen" },
                    ButtonVariant::Primary,
                ))
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
        let preferences = self.settings.agent(provider).clone();
        let task =
            cx.background_spawn(
                async move { processing::summarize(provider, &preferences, &segments) },
            );
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

    fn open_recordings_folder(&mut self, cx: &mut Context<Self>) {
        let directory = self.library.meetings_dir();
        let task = cx.background_spawn(async move {
            let status = std::process::Command::new("/usr/bin/open")
                .arg(directory)
                .status()
                .context("No se pudo abrir la carpeta en Finder")?;
            ensure!(
                status.success(),
                "Finder no pudo abrir la carpeta de grabaciones"
            );
            Ok::<_, anyhow::Error>(())
        });
        cx.spawn(async move |weak, cx| {
            if let Err(error) = task.await {
                let _ = weak.update(cx, |this, cx| {
                    this.report(error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn toggle_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.preferences = !self.preferences;
        self.notice = None;
        if self.preferences {
            self.check_provider(cx);
            self.refresh_agent_options(window, cx);
        }
        cx.notify();
    }

    fn check_provider(&mut self, cx: &mut Context<Self>) {
        let Some(provider @ (Provider::Codex | Provider::Claude)) = self.settings.provider else {
            return;
        };
        let Some(path) = provider.executable() else {
            return;
        };
        let task = cx.background_spawn(async move {
            match provider {
                Provider::Codex => processing::codex_signed_in(&path),
                Provider::Claude => processing::claude_signed_in(&path),
                Provider::Copilot => return None,
            }
            .ok()
        });
        cx.spawn(async move |weak, cx| {
            let status = task.await;
            let _ = weak.update(cx, |this, cx| {
                match provider {
                    Provider::Codex => this.codex_signed_in = status,
                    Provider::Claude => this.claude_signed_in = status,
                    Provider::Copilot => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn set_provider(&mut self, provider: Provider, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.provider != Some(provider) {
            self.settings.provider = Some(provider);
            self.settings.automatic = false;
            self.save_settings(cx);
            self.check_provider(cx);
            self.refresh_agent_options(window, cx);
        }
    }

    fn refresh_agent_options(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(provider) = self.settings.provider else {
            return;
        };
        self.options_revision += 1;
        let revision = self.options_revision;
        self.install_agent_selects(
            provider,
            AgentOptions {
                models: Vec::new(),
                efforts: Vec::new(),
            },
            window,
            cx,
        );
        self.options_loading = true;
        let preferences = self.settings.agent(provider).clone();
        let task =
            cx.background_spawn(async move { agent_options::discover(provider, &preferences) });
        cx.spawn_in(window, async move |weak, cx| {
            let result = task.await;
            let _ = weak.update_in(cx, |this, window, cx| {
                if this.options_revision != revision {
                    return;
                }
                this.options_loading = false;
                match result {
                    Ok(options) => this.install_agent_selects(provider, options, window, cx),
                    Err(error) => this.report(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn install_agent_selects(
        &mut self,
        provider: Provider,
        mut options: AgentOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let preferences = self.settings.agent(provider);
        let selected_model = preferences.model.clone().unwrap_or_else(|| DEFAULT.into());
        let selected_effort = preferences.effort.clone().unwrap_or_else(|| DEFAULT.into());
        options.models.retain(|model| model != DEFAULT);
        options.efforts.retain(|effort| effort != DEFAULT);
        options.models.insert(0, DEFAULT.into());
        options.efforts.insert(0, DEFAULT.into());
        // Preserve saved choices while the catalog is loading or temporarily unavailable.
        if !options.models.contains(&selected_model) {
            options.models.push(selected_model.clone());
        }
        if !options.efforts.contains(&selected_effort) {
            options.efforts.push(selected_effort.clone());
        }
        let model_index = options
            .models
            .iter()
            .position(|model| *model == selected_model)
            .unwrap_or_default();
        let effort_index = options
            .efforts
            .iter()
            .position(|effort| *effort == selected_effort)
            .unwrap_or_default();
        let model_items = SearchableVec::new(
            options
                .models
                .into_iter()
                .map(AgentChoice)
                .collect::<Vec<_>>(),
        );
        let model = if let Some(model) = &self.model_select {
            model.update(cx, |state, cx| {
                state.set_items(model_items, window, cx);
                state.set_selected_index(Some(IndexPath::new(model_index)), window, cx);
            });
            model.clone()
        } else {
            cx.new(|cx| {
                SelectState::new(model_items, Some(IndexPath::new(model_index)), window, cx)
                    .searchable(true)
            })
        };
        let effort_items = options
            .efforts
            .into_iter()
            .map(AgentChoice)
            .collect::<Vec<_>>();
        let effort = if let Some(effort) = &self.effort_select {
            effort.update(cx, |state, cx| {
                state.set_items(effort_items, window, cx);
                state.set_selected_index(Some(IndexPath::new(effort_index)), window, cx);
            });
            effort.clone()
        } else {
            cx.new(|cx| {
                SelectState::new(effort_items, Some(IndexPath::new(effort_index)), window, cx)
            })
        };
        self.agent_subscriptions = vec![
            cx.subscribe_in(&model, window, move |this, _, event, window, cx| {
                if this.settings.provider != Some(provider) {
                    return;
                }
                if let SelectEvent::Confirm(Some(value)) = event {
                    let value = (value != DEFAULT).then(|| value.clone());
                    let preferences = this.settings.agent_mut(provider);
                    if preferences.model == value {
                        return;
                    }
                    preferences.model = value;
                    preferences.effort = None;
                    this.save_settings(cx);
                    this.refresh_agent_options(window, cx);
                }
            }),
            cx.subscribe_in(&effort, window, move |this, _, event, _, cx| {
                if this.settings.provider != Some(provider) {
                    return;
                }
                if let SelectEvent::Confirm(Some(value)) = event {
                    this.settings.agent_mut(provider).effort =
                        (value != DEFAULT).then(|| value.clone());
                    this.save_settings(cx);
                }
            }),
        ];
        self.model_select = Some(model);
        self.effort_select = Some(effort);
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
                .footer(view::confirmation_footer("Activar", ButtonVariant::Primary))
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
        theme::apply(Some(dark), window, cx);
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
                .footer(view::confirmation_footer("Guardar", ButtonVariant::Primary))
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
                .footer(view::confirmation_footer("Guardar", ButtonVariant::Primary))
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
                .footer(view::confirmation_footer("Eliminar", ButtonVariant::Danger))
                .cancel_text("Cancelar")
                .on_ok(move |_, window, cx| {
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
                        this.reset_timeline(window, cx);
                        this.meetings.retain(|meeting| meeting.id != id);
                        this.errors.remove(&id);
                        this.selected = this.meetings.first().map(|meeting| meeting.id);
                        this.tab = Tab::Summary;
                        this.transcript_page = 0;
                        cx.notify();
                        true
                    })
                    .unwrap_or(false)
                })
        });
    }

    fn play(
        &mut self,
        id: Uuid,
        command: PlaybackCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.capture.is_some() || self.starting || self.jobs.get(&id) == Some(&Work::Saving) {
            return;
        }
        let duration = self
            .selected_meeting()
            .filter(|meeting| meeting.id == id)
            .map_or(0, |meeting| meeting.duration_ms);
        if duration == 0 {
            return;
        }

        let result = (|| -> Result<()> {
            if self
                .playback
                .as_ref()
                .is_none_or(|(active, playback)| *active != id || playback.player().empty())
            {
                self.playback = Some((id, Playback::open(&self.library.meeting_dir(id))?));
            }
            let (_, playback) = self
                .playback
                .as_ref()
                .context("No se pudo abrir la grabación")?;
            let player = playback.player();
            match command {
                PlaybackCommand::Toggle if !player.is_paused() => player.pause(),
                PlaybackCommand::Toggle => player.play(),
                PlaybackCommand::Jump(position) | PlaybackCommand::Seek(position) => {
                    let position = position.min(duration.saturating_sub(1));
                    playback.seek(position)?;
                    if matches!(command, PlaybackCommand::Jump(_)) {
                        player.play();
                    }
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.ensure_ticker(window, cx);
                self.sync_timeline(window, cx);
            }
            Err(error) => {
                self.report(error);
                self.sync_timeline(window, cx);
            }
        }
        cx.notify();
    }

    fn reset_timeline(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.scrubbing = false;
        self.timeline
            .update(cx, |slider, cx| slider.set_value(0., window, cx));
    }

    fn sync_timeline(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.scrubbing {
            return;
        }
        let value = self
            .playback
            .as_ref()
            .zip(self.selected_meeting())
            .filter(|((id, _), meeting)| *id == meeting.id && meeting.duration_ms != 0)
            .map_or(0., |((_, playback), meeting)| {
                let position = playback.position_ms(meeting.duration_ms);
                (1000. * position as f64 / meeting.duration_ms as f64) as f32
            });
        self.timeline.update(cx, |slider, cx| {
            if slider.value() != SliderValue::Single(value) {
                slider.set_value(value, window, cx);
            }
        });
    }

    fn ensure_ticker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        let executor = cx.background_executor().clone();
        cx.spawn_in(window, async move |weak, cx| {
            loop {
                executor.timer(Duration::from_millis(250)).await;
                let active = weak
                    .update_in(cx, |this, window, cx| {
                        if this
                            .pending_save
                            .as_ref()
                            .is_some_and(|(_, save)| save.writer.is_finished())
                            && let Some((id, result)) = this.complete_save()
                        {
                            match result {
                                Ok(()) if this.library.model_path().is_file() => {
                                    this.start_transcript(id, cx)
                                }
                                Ok(()) => {
                                    if this.selected == Some(id) && !this.preferences {
                                        this.tab = Tab::Transcript;
                                    }
                                }
                                Err(error) => {
                                    this.errors.insert(id, format!("{error:#}"));
                                }
                            }
                        }
                        this.sync_timeline(window, cx);
                        let active = this.capture.is_some()
                            || this.pending_save.is_some()
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
