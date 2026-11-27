use gpui_kit::base::{SelectableText, StyledExt};
use std::{ops::Range, time::Duration};

use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, Selectable, Sizable, ThemeMode,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    collapsible::Collapsible,
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    input::Input,
    kbd::Kbd,
    scroll::{ScrollableElement, Scrollbar},
    select::Select,
    slider::{Slider, SliderValue},
    switch::Switch,
    tab::{Tab as TabItem, TabBar},
    v_flex,
};
use gpui_kit::{
    AppContext, Context, CursorStyle, Div, ElementId, HighlightStyle, InteractiveElement,
    IntoElement, ListAlignment, ListOffset, ListState, ParentElement, Pixels, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, list, prelude::FluentBuilder, rems,
};
use uuid::Uuid;

use super::{
    AddMarker, Edit, FindTranscript, MoveMeetingDown, MoveMeetingUp, NextTranscriptMatch,
    PlaybackCommand, PreviousTranscriptMatch, PromptKind, Tab, Work, Workspace,
};
use crate::library::{Meeting, Provider, Segment, TranscriptionMode};

const TRANSCRIPT_NOTICE: &str =
    "Transcripción generada con IA. Puede contener errores; consulta el audio para verificarla.";

pub(super) struct TranscriptViewport {
    list: ListState,
    timestamps: bool,
    rem_size: Pixels,
}

impl TranscriptViewport {
    pub(super) fn reveal(&self, paragraph: usize) {
        self.list.scroll_to(ListOffset {
            item_ix: paragraph,
            ..Default::default()
        });
    }
}

pub(super) struct TranscriptMatch {
    pub(super) paragraph: usize,
    range: Range<usize>,
}

#[derive(Clone)]
struct MeetingDrag {
    id: Uuid,
    title: SharedString,
}

impl Render for MeetingDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_48()
            .px_3()
            .py_2()
            .rounded(cx.theme().radius)
            .bg(cx.theme().popover)
            .text_color(cx.theme().popover_foreground)
            .border_1()
            .border_color(cx.theme().border)
            .text_sm()
            .truncate()
            .cursor(CursorStyle::ClosedHand)
            .child(self.title.clone())
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected_meeting();
        v_flex()
            .size_full()
            .text_sm()
            .text_color(cx.theme().foreground)
            .bg(cx.theme().background)
            .track_focus(&self.focus)
            .key_context("Reuniones")
            .on_action(cx.listener(Self::add_marker))
            .on_action(cx.listener(Self::find_transcript))
            .on_action(cx.listener(|this, _: &NextTranscriptMatch, _, cx| {
                this.move_transcript_match(false, cx)
            }))
            .on_action(cx.listener(|this, _: &PreviousTranscriptMatch, _, cx| {
                this.move_transcript_match(true, cx)
            }))
            .child(self.toolbar(cx))
            .when_some(self.notice.clone(), |view, notice| {
                view.child(
                    h_flex()
                        .gap_3()
                        .px_4()
                        .py_2()
                        .bg(cx.theme().muted)
                        .child(div().flex_1().text_sm().child(notice))
                        .child(
                            compact_button("dismiss-notice")
                                .ghost()
                                .label("Cerrar")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.notice = None;
                                    cx.notify();
                                })),
                        ),
                )
            })
            .child(
                h_flex()
                    .items_stretch()
                    .flex_1()
                    .min_h_0()
                    .child(self.sidebar(cx))
                    .child(if self.preferences {
                        self.preferences_view(window, cx).into_any_element()
                    } else if let Some(meeting) = selected {
                        self.detail(meeting, window, cx).into_any_element()
                    } else {
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .p_6()
                            .gap_2()
                            .child(div().text_lg().font_semibold().child("Tus reuniones"))
                            .child(muted("Abre una llamada en Teams y selecciona Grabar.", cx))
                            .into_any_element()
                    }),
            )
    }
}

impl Workspace {
    fn toolbar(&self, cx: &Context<Self>) -> impl IntoElement {
        let toolbar = h_flex()
            .min_h_12()
            .flex_wrap()
            .gap_3()
            .px_4()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border);
        if let Some((meeting, active)) = self.capture.as_ref() {
            let [mic, teams] = active.signals();
            toolbar
                .child(div().font_semibold().child(if active.is_paused() {
                    "En pausa"
                } else {
                    "Grabando"
                }))
                .child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_sm()
                        .child(timestamp(active.elapsed_ms())),
                )
                .child(muted(format!("Micrófono · {mic}"), cx))
                .child(muted(format!("Teams · {teams}"), cx))
                .child(div().flex_1())
                .child(
                    compact_button("marker-record")
                        .label(format!("Añadir marcador ({})", meeting.markers.len()))
                        .tooltip("Añadir marcador · ⌘⇧M")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.add_marker(&AddMarker, window, cx)
                        })),
                )
                .child(
                    icon_button(
                        "mute-record-mic",
                        if active.is_mic_muted() {
                            "icons/mic-off.svg"
                        } else {
                            "icons/mic.svg"
                        },
                        if active.is_mic_muted() {
                            "Activar micrófono en la grabación"
                        } else {
                            "Silenciar micrófono en la grabación"
                        },
                    )
                    .ghost()
                    .selected(active.is_mic_muted())
                    .when(active.is_mic_muted(), |button| {
                        button.text_color(cx.theme().button_danger)
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some((_, capture)) = this.capture.as_ref() {
                            capture.toggle_mic_mute();
                        }
                        cx.notify();
                    })),
                )
                .child(
                    if active.is_paused() {
                        icon_button("pause-record", "icons/play.svg", "Reanudar grabación")
                    } else {
                        icon_button("pause-record", "icons/pause.svg", "Pausar grabación")
                    }
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some((_, capture)) = this.capture.as_ref() {
                            capture.toggle_pause();
                        }
                        cx.notify();
                    })),
                )
                .child(
                    icon_button("stop-record", "icons/square.svg", "Detener grabación")
                        .danger()
                        .on_click(cx.listener(|this, _, window, cx| this.finish(window, cx))),
                )
        } else {
            toolbar
                .child(div().font_semibold().child("Reuniones"))
                .child(div().flex_1())
                .child(
                    Button::new("start-record")
                        .cursor_pointer()
                        .large()
                        .size_10()
                        .p_0()
                        .ghost()
                        .icon(
                            Icon::default()
                                .data(br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" stroke-width="2"/><circle cx="12" cy="12" r="5" fill="currentColor"/></svg>"#)
                                .text_color(cx.theme().button_danger),
                        )
                        .accessibility_label("Grabar")
                        .tooltip("Grabar")
                        .loading(self.starting)
                        .disabled(
                            self.starting || self.jobs.values().any(|job| *job == Work::Saving),
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.record(window, cx))),
                )
        }
    }

    fn sidebar(&self, cx: &Context<Self>) -> impl IntoElement {
        let query = self.search.read(cx).value().trim().to_lowercase();
        let mut rows =
            self.meetings
                .iter()
                .filter(|meeting| {
                    query.is_empty()
                        || meeting.title.to_lowercase().contains(&query)
                        || meeting.transcript.as_ref().is_some_and(|segments| {
                            segments
                                .iter()
                                .any(|segment| segment.text.to_lowercase().contains(&query))
                        })
                })
                .enumerate()
                .map(|(index, meeting)| {
                    let id = meeting.id;
                    let selected = !self.preferences && self.selected == Some(id);
                    let button =
                        regular_button(SharedString::from(format!("meeting-{id}")))
                            .w_full()
                            .h_auto()
                            .px_2()
                            .py_0()
                            .pb_2()
                            .ghost()
                            .selected(selected)
                            .when(selected, |button| {
                                button
                                    .bg(cx.theme().transparent)
                                    .text_color(cx.theme().primary)
                            })
                            .accessibility_label(meeting.title.clone())
                            .child(
                                v_flex()
                                    .w_full()
                                    .min_w_0()
                                    .gap_0()
                                    .text_center()
                                    .child(h_flex().h_7().justify_center().child(
                                        div().min_w_0().truncate().child(meeting.title.clone()),
                                    ))
                                    .child(
                                        muted(
                                            format!(
                                                "{} · {}",
                                                date_label(meeting.created),
                                                timestamp(meeting.duration_ms)
                                            ),
                                            cx,
                                        )
                                        .text_xs(),
                                    ),
                            )
                            .tooltip(meeting.title.clone())
                            .when(query.is_empty(), |button| {
                                button.drag_over::<MeetingDrag>(|style, _, _, cx| {
                                    style.bg(cx.theme().muted)
                                })
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.select(id, window, cx);
                                cx.notify();
                            }));
                    v_flex()
                        .id(SharedString::from(format!("meeting-row-{id}")))
                        .key_context("MeetingRow")
                        .on_action(cx.listener(move |this, _: &MoveMeetingUp, _, cx| {
                            this.move_meeting(id, -1, cx)
                        }))
                        .on_action(cx.listener(move |this, _: &MoveMeetingDown, _, cx| {
                            this.move_meeting(id, 1, cx)
                        }))
                        .gap_0()
                        .py_1()
                        .when(index > 0, |row| {
                            row.border_t_1().border_color(cx.theme().border)
                        })
                        .child(button)
                        .when(query.is_empty(), |row| {
                            row.on_drag(
                                MeetingDrag {
                                    id,
                                    title: meeting.title.clone().into(),
                                },
                                |drag, _, window, cx| {
                                    window.defer(cx, |window, cx| {
                                        cx.set_active_drag_cursor_style(
                                            CursorStyle::ClosedHand,
                                            window,
                                        );
                                    });
                                    cx.new(|_| drag.clone())
                                },
                            )
                            .on_drop(cx.listener(
                                move |this, drag: &MeetingDrag, _, cx| {
                                    this.reorder_meeting(drag.id, id, cx);
                                },
                            ))
                        })
                })
                .peekable();
        let empty_message = if self.meetings.is_empty() {
            "Todavía no hay reuniones."
        } else {
            "No se encontraron reuniones."
        };
        let no_results = rows.peek().is_none();
        v_flex()
            .w_56()
            .flex_shrink_0()
            .min_h_0()
            .gap_3()
            .p_3()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                Input::new(&self.search)
                    .h_9()
                    .aria_label("Buscar reuniones")
                    .cleanable(true),
            )
            .child(
                v_flex()
                    .id("meeting-library")
                    .min_h_0()
                    .flex_1()
                    .children(rows)
                    .when(no_results, |view| view.child(muted(empty_message, cx)))
                    .overflow_y_scrollbar(),
            )
            .child(
                regular_button("settings")
                    .ghost()
                    .selected(self.preferences)
                    .when(self.preferences, |button| {
                        button
                            .bg(cx.theme().transparent)
                            .text_color(cx.theme().primary)
                    })
                    .icon(Icon::default().path("icons/settings.svg"))
                    .label("Configuración")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.toggle_preferences(window, cx)),
                    ),
            )
    }

    fn preferences_view(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let dark = cx.theme().mode == ThemeMode::Dark;
        let provider = self.settings.provider;
        let installed = self.provider_installed;
        let ready = installed
            && match provider {
                Some(Provider::Codex) => self.codex_signed_in == Some(true),
                Some(Provider::Claude) => self.claude_signed_in == Some(true),
                _ => false,
            };
        let status = match provider {
            Some(provider) if !installed => format!("{} · CLI no instalada", provider.name()),
            Some(provider) if ready => format!("{} · Listo", provider.name()),
            Some(Provider::Codex) if self.codex_signed_in == Some(false) => {
                "Inicia sesión en la terminal con codex login.".into()
            }
            Some(Provider::Claude) if self.claude_signed_in == Some(false) => {
                "Inicia sesión en la terminal con claude auth login.".into()
            }
            Some(_) => "Usa la sesión iniciada en la terminal.".into(),
            None => "Elige la herramienta que usas en la terminal.".into(),
        };
        let content = v_flex().w_full().max_w(rems(44.)).mx_auto().gap_5()
            .child(div().text_xl().font_semibold().child("Configuración"))
            .child(h_flex().gap_4()
                .child(div().flex_1().child(settings_heading("Apariencia")))
                .child(h_flex().gap_2().children([
                    ("Claro", false, "icons/sun.svg"),
                    ("Oscuro", true, "icons/moon.svg"),
                ].map(|(name, value, icon)| {
                    regular_button(name).outline().w_24().px_2()
                        .icon(Icon::default().path(icon))
                        .selected(dark == value)
                        .when(dark == value, |button| button
                            .bg(cx.theme().transparent).text_color(cx.theme().primary)
                            .border_color(cx.theme().primary))
                        .label(name)
                        .on_click(cx.listener(move |this, _, window, cx| this.set_theme(value, window, cx)))
                }))))
            .child(settings_group("Inteligencia artificial", cx)
                .child(v_flex().gap_2()
                    .child(section("Herramienta de IA"))
                    .child(h_flex().flex_wrap().gap_2().children([Provider::Codex, Provider::Copilot, Provider::Claude].map(|provider| {
                        let selected = self.settings.provider == Some(provider);
                        let logo: &'static [u8] = match provider {
                            Provider::Codex => include_bytes!("../../assets/brands/openai.svg"),
                            Provider::Copilot => include_bytes!("../../assets/brands/github-copilot.svg"),
                            Provider::Claude => include_bytes!("../../assets/brands/claude.svg"),
                        };
                        regular_button(provider.name()).outline().flex_1().min_w_32().px_2()
                            .icon(Icon::default().data(logo))
                            .selected(selected)
                            .when(selected, |button| button
                                .bg(cx.theme().transparent).text_color(cx.theme().primary)
                                .border_color(cx.theme().primary))
                            .label(provider.name())
                            .on_click(cx.listener(move |this, _, window, cx| this.set_provider(provider, window, cx)))
                    })))
                    .child(h_flex().gap_2()
                        .when(ready, |row| row.child(Icon::default().path("icons/check.svg")
                            .size_3().text_color(cx.theme().muted_foreground)))
                        .child(muted(status, cx).text_xs())))
                .when_some(self.model_select.as_ref().zip(self.effort_select.as_ref()), |view, (model, effort)| {
                    view.child(h_flex().items_start().gap_4()
                        .child(v_flex().flex_1().min_w_0().gap_2().child(section("Modelo"))
                            .child(Select::new(model).accessibility_label("Modelo")
                                .cursor_pointer().w_full().disabled(self.options_loading)))
                        .child(v_flex().flex_1().min_w_0().gap_2().child(section("Esfuerzo"))
                            .child(Select::new(effort).accessibility_label("Esfuerzo de razonamiento")
                                .cursor_pointer().w_full().disabled(self.options_loading))))
                })
                .when(self.options_loading, |view| view.child(muted("Cargando modelos y niveles de esfuerzo…", cx).text_xs()))
                .child(v_flex().gap_3().pt_2()
                    .child(section("Instrucciones de generación"))
                    .children([
                        (PromptKind::Summary, "Resumen", self.settings.summary_prompt.is_some()),
                        (PromptKind::Markers, "Marcadores", self.settings.markers_prompt.is_some()),
                    ].map(|(kind, title, custom)| {
                        h_flex().gap_4()
                            .child(v_flex().flex_1().min_w_0().gap_1()
                                .child(div().font_medium().child(title))
                                .child(muted(if custom { "Personalizadas" } else { "Predeterminadas" }, cx).text_xs()))
                            .child(compact_button(match kind {
                                PromptKind::Summary => "edit-summary-prompt",
                                PromptKind::Markers => "edit-markers-prompt",
                            }).ghost()
                                .icon(Icon::default().path("icons/pencil.svg"))
                                .label("Editar…")
                                .on_click(cx.listener(move |this, _, window, cx| this.edit_prompt(kind, window, cx))))
                    })))
                .child(v_flex().gap_1().pt_2()
                    .child(section("Al terminar una reunión"))
                    .child(settings_toggle("Resúmenes automáticos", "Decisiones, tareas y preguntas abiertas.",
                        Switch::new("automatic-summary").cursor_pointer()
                            .checked(self.settings.automatic)
                            .disabled(provider.is_none())
                            .on_change(cx.listener(|this, enabled, window, cx| this.set_automatic(*enabled, window, cx))), cx))
                    .child(settings_toggle("Marcadores automáticos", "Momentos importantes para volver a escuchar.",
                        Switch::new("automatic-markers").cursor_pointer()
                            .checked(self.settings.automatic_markers)
                            .disabled(provider.is_none())
                            .on_change(cx.listener(|this, enabled, window, cx| this.set_automatic_markers(*enabled, window, cx))), cx)))
                .child(muted("El proveedor recibe la transcripción. El audio se queda en tu Mac.", cx).text_xs()))
            .child(settings_group("Transcripción", cx)
                .child(h_flex().items_start().flex_wrap().gap_4()
                    .child(v_flex().flex_1().min_w_0().gap_1()
                        .child(section("Español · Whisper"))
                        .child(muted("Se procesa en tu Mac, sin enviar el audio.", cx).text_xs()))
                    .child(if self.library.transcription_ready(self.settings.vad_enabled()) {
                        h_flex().gap_2().py_1()
                            .child(Icon::default().path("icons/check.svg").size_4()
                                .text_color(cx.theme().muted_foreground))
                            .child(muted("Listo para transcribir", cx)).into_any_element()
                    } else {
                        self.whisper_settings(cx).into_any_element()
                    }))
                .child(settings_toggle("Detección de voz (VAD)",
                    "Omite silencios antes de transcribir. Puede hacer que la transcripción tarde más.",
                    Switch::new("transcription-vad").cursor_pointer()
                        .checked(self.settings.vad_enabled())
                        .on_change(cx.listener(|this, enabled, _, cx| this.set_vad_enabled(*enabled, cx))), cx))
                .child(v_flex().gap_2()
                    .child(section("Modo de transcripción"))
                    .child(h_flex().flex_wrap().gap_2().children([
                        ("Greedy", TranscriptionMode::Greedy),
                        ("Beam Search", TranscriptionMode::BeamSearch),
                    ].map(|(label, mode)| {
                        let selected = self.settings.transcription_mode == mode;
                        regular_button(label).outline()
                            .label(label)
                            .selected(selected)
                            .when(selected, |button| button
                                .bg(cx.theme().transparent).text_color(cx.theme().primary)
                                .border_color(cx.theme().primary))
                            .on_click(cx.listener(move |this, _, _, cx| this.set_transcription_mode(mode, cx)))
                    })))
                    .child(muted("Greedy prioriza la velocidad. Beam Search explora más alternativas y puede tardar más.", cx).text_xs())
                    .child(muted("El modo y la detección de voz se aplican a las próximas transcripciones y regeneraciones.", cx).text_xs())))
            .child(settings_group("Grabación y archivos", cx)
                .child(v_flex().gap_1()
                    .child(section("Micrófono"))
                    .child(muted("Se usa la entrada predeterminada de macOS. Silenciarte en Teams no detiene la grabación de tu voz.", cx).text_xs()))
                .child(h_flex().gap_4()
                    .child(div().flex_1().child("Añadir marcador durante la grabación"))
                    .when_some(Kbd::binding_for_action(&AddMarker, Some("Reuniones"), window), |row, shortcut| row.child(shortcut)))
                .child(h_flex().flex_wrap().gap_3()
                    .child(regular_button("open-recordings-folder")
                        .icon(Icon::default().path("icons/folder-open.svg"))
                        .label("Abrir en Finder")
                        .on_click(cx.listener(|this, _, _, cx| this.open_recordings_folder(cx))))
                    .child(muted("Grabaciones y transcripciones guardadas en tu Mac.", cx)
                        .flex_1().min_w_0().text_xs())));
        v_flex()
            .id("settings-scroll")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(div().w_full().p_6().child(content))
            .overflow_y_scrollbar()
    }

    fn whisper_settings(&self, cx: &Context<Self>) -> impl IntoElement {
        if self
            .library
            .transcription_ready(self.settings.vad_enabled())
        {
            muted("Listo para transcribir.", cx).into_any_element()
        } else {
            regular_button("download-model")
                .label(if self.downloading {
                    "Descargando modelos…"
                } else if self.library.model_path().is_file() {
                    "Descargar detección de voz"
                } else if self.settings.vad_enabled() {
                    "Descargar modelos (548 MiB)"
                } else {
                    "Descargar Whisper (547 MiB)"
                })
                .disabled(self.downloading)
                .on_click(cx.listener(|this, _, _, cx| this.download(cx)))
                .into_any_element()
        }
    }

    fn detail(
        &self,
        meeting: &Meeting,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = meeting.id;
        let title = meeting.title.clone();
        let delete_title = title.clone();
        let work = self.jobs.get(&id).copied();
        v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .gap_2()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .text_lg()
                                            .font_semibold()
                                            .child(title.clone()),
                                    )
                                    .child(
                                        icon_button(
                                            "rename-meeting",
                                            "icons/pencil.svg",
                                            "Renombrar…",
                                        )
                                        .ghost()
                                        .text_color(cx.theme().muted_foreground)
                                        .flex_shrink_0()
                                        .on_click(
                                            cx.listener(move |this, _, window, cx| {
                                                this.edit(
                                                    Edit::Meeting(id),
                                                    title.clone(),
                                                    window,
                                                    cx,
                                                )
                                            }),
                                        ),
                                    ),
                            )
                            .child(div().flex_1())
                            .child(
                                icon_button("delete-meeting", "icons/trash.svg", "Eliminar…")
                                    .ghost()
                                    .flex_shrink_0()
                                    .disabled(work.is_some())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.delete(id, delete_title.clone(), window, cx)
                                    })),
                            ),
                    )
                    .when_some(work, |content, job| content.child(muted(job.label(), cx)))
                    .when_some(self.errors.get(&id).cloned(), |content, error| {
                        content.child(div().text_sm().text_color(cx.theme().danger).child(error))
                    })
                    .when(work != Some(Work::Saving), |content| {
                        content.when_some(meeting.recording_warning.clone(), |content, warning| {
                            content.child(muted(warning, cx))
                        })
                    })
                    .child(self.player(meeting, cx)),
            )
            .child(
                v_flex()
                    .w_full()
                    .max_w(rems(56.))
                    .mx_auto()
                    .flex_1()
                    .min_h_0()
                    .child(
                        TabBar::new("meeting-tabs")
                            .underline()
                            .px_4()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .children(
                                Tab::ALL
                                    .map(|tab| TabItem::new().label(tab.label()).cursor_pointer()),
                            )
                            .selected_index(
                                Tab::ALL
                                    .iter()
                                    .position(|tab| *tab == self.tab)
                                    .unwrap_or_default(),
                            )
                            .on_click(cx.listener(|this, index: &usize, _, cx| {
                                if let Some(tab) = Tab::ALL.get(*index) {
                                    this.tab = *tab;
                                    cx.notify();
                                }
                            })),
                    )
                    .child(match self.tab {
                        Tab::Summary => self.summary(meeting, window, cx).into_any_element(),
                        Tab::Transcript => self.transcript(meeting, window, cx).into_any_element(),
                        Tab::Markers => self.markers(meeting, cx).into_any_element(),
                    }),
            )
    }

    fn player(&self, meeting: &Meeting, cx: &Context<Self>) -> impl IntoElement {
        let id = meeting.id;
        let playback = self.playback.as_ref().filter(|(active, _)| *active == id);
        let playing = playback.is_some_and(|(_, playback)| {
            let player = playback.player();
            !player.is_paused() && !player.empty()
        });
        let duration = meeting.duration_ms;
        let position = match self.timeline.read(cx).value() {
            SliderValue::Single(value) if self.scrubbing => {
                (f64::from(value) * duration as f64 / 1000.).round() as u64
            }
            _ => playback.map_or(0, |(_, playback)| playback.position_ms(duration)),
        };
        let disabled = duration == 0
            || self.capture.is_some()
            || self.starting
            || self.jobs.get(&id) == Some(&Work::Saving);
        v_flex()
            .gap_2()
            .child(
                Slider::new(&self.timeline)
                    .cursor_pointer()
                    .w_full()
                    .disabled(disabled)
                    .accessibility_label("Posición de reproducción"),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        if playing {
                            icon_button("play", "icons/pause.svg", "Pausar audio")
                        } else {
                            icon_button("play", "icons/play.svg", "Reproducir audio")
                        }
                        .disabled(disabled)
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.play(id, PlaybackCommand::Toggle, window, cx)
                            },
                        )),
                    )
                    .child(
                        compact_button("back-15")
                            .ghost()
                            .label("−15 s")
                            .tooltip("Retroceder 15 segundos")
                            .disabled(disabled || position == 0)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.play(
                                    id,
                                    PlaybackCommand::Seek(position.saturating_sub(15_000)),
                                    window,
                                    cx,
                                )
                            })),
                    )
                    .child(
                        compact_button("forward-15")
                            .ghost()
                            .label("+15 s")
                            .tooltip("Avanzar 15 segundos")
                            .disabled(disabled || position >= duration)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.play(
                                    id,
                                    PlaybackCommand::Seek(position.saturating_add(15_000)),
                                    window,
                                    cx,
                                )
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} / {}", timestamp(position), timestamp(duration))),
                    ),
            )
    }

    fn summary(
        &self,
        meeting: &Meeting,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = meeting.id;
        let work = self.jobs.get(&id);
        let empty_transcript = meeting.transcript.as_ref().is_some_and(Vec::is_empty);
        let mut content = pane("summary");
        if let Some(summary) = meeting.summary.as_ref() {
            let text = summary.content.summary.clone();
            content = content
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().child(muted(
                            format!("Generado con {}", summary.provider.name()),
                            cx,
                        )))
                        .child(
                            icon_button("edit-summary", "icons/pencil.svg", "Editar resumen…")
                                .ghost()
                                .disabled(work.is_some())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.edit_summary(id, text.clone(), window, cx)
                                })),
                        )
                        .child(
                            icon_button(
                                "regenerate-summary",
                                "icons/refresh-cw.svg",
                                "Volver a generar…",
                            )
                            .ghost()
                            .disabled(
                                work.is_some() || meeting.transcript.is_none() || empty_transcript,
                            )
                            .on_click(cx.listener(
                                move |this, _, window, cx| this.request_summary(id, window, cx),
                            )),
                        ),
                )
                .child(selectable_text(
                    SharedString::from(format!("summary-text-{id}")),
                    summary.content.summary.clone(),
                ));
            for (heading, items) in [
                ("Decisiones", &summary.content.decisions),
                ("Preguntas abiertas", &summary.content.questions),
            ] {
                if !items.is_empty() {
                    let mut rows = v_flex().gap_6();
                    for item in items {
                        rows = rows.child(self.summary_item(
                            meeting,
                            heading,
                            item.text.clone(),
                            &item.segment_ids,
                            window,
                            cx,
                        ));
                    }
                    content = content.child(summary_section(heading, rows, cx));
                }
            }
            if !summary.content.actions.is_empty() {
                let mut rows = v_flex().gap_6();
                for item in &summary.content.actions {
                    let mut text = item.text.clone();
                    if let Some(owner) = &item.owner {
                        text.push_str(&format!(" · {owner}"));
                    }
                    if let Some(deadline) = &item.deadline {
                        text.push_str(&format!(" · {deadline}"));
                    }
                    rows = rows.child(self.summary_item(
                        meeting,
                        "Tareas",
                        text,
                        &item.segment_ids,
                        window,
                        cx,
                    ));
                }
                content = content.child(summary_section("Tareas", rows, cx));
            }
        } else if empty_transcript {
            content = content.child(muted("Whisper no detectó diálogo en esta grabación.", cx));
        } else if meeting.transcript.is_none() {
            content = content.child(muted(
                "El resumen estará disponible cuando termine la transcripción.",
                cx,
            ));
        }
        if meeting.summary.is_none()
            && meeting.transcript.is_some()
            && !empty_transcript
            && work.is_none()
        {
            let label = if self.settings.provider.is_none() {
                "Elegir herramienta de IA"
            } else if self.errors.contains_key(&id) {
                "Reintentar resumen…"
            } else if self.settings.automatic {
                "Generar resumen"
            } else {
                "Generar resumen…"
            };
            content = content.child(regular_button("generate-summary").label(label).on_click(
                cx.listener(move |this, _, window, cx| this.request_summary(id, window, cx)),
            ));
        }
        content.overflow_y_scrollbar()
    }

    fn summary_item(
        &self,
        meeting: &Meeting,
        section: &str,
        text: String,
        ids: &[usize],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui_kit::Stateful<Div> {
        let key = SharedString::from(format!(
            "summary-item-{}-{section}-{text}-{ids:?}",
            meeting.id
        ));
        let expanded = window.use_keyed_state(
            SharedString::from(format!("{key}-references")),
            cx,
            |_, _| false,
        );
        let open = *expanded.read(cx);
        let segments: Vec<_> = ids
            .iter()
            .filter_map(|id| meeting.transcript.as_ref()?.get(id.checked_sub(1)?))
            .collect();
        let reference = |segment: &Segment| {
            self.jump(
                meeting.id,
                format!("reference-{}", segment.id),
                segment.start_ms,
                cx,
            )
            .ghost()
            .font_normal()
            .font_family(cx.theme().mono_font_family.clone())
            .text_color(cx.theme().muted_foreground)
            .accessibility_label(format!(
                "Escuchar referencia en {}",
                timestamp(segment.start_ms)
            ))
            .tooltip("Escuchar referencia")
        };
        let mut controls = h_flex().flex_wrap().gap_2();
        if let Some(first) = segments.first() {
            controls = controls.child(reference(first));
        }
        if segments.len() > 1 {
            let count = segments.len() - 1;
            controls = controls.child(
                compact_button("toggle-references")
                    .ghost()
                    .font_normal()
                    .text_color(cx.theme().muted_foreground)
                    .label(if open {
                        "Ocultar referencias".into()
                    } else if count == 1 {
                        "+1 referencia".into()
                    } else {
                        format!("+{count} referencias")
                    })
                    .selected(open)
                    .when(open, |button| {
                        button
                            .bg(cx.theme().transparent)
                            .text_color(cx.theme().primary)
                    })
                    .on_click(move |_, _, cx| {
                        expanded.update(cx, |open, cx| {
                            *open = !*open;
                            cx.notify();
                        })
                    }),
            );
        }
        v_flex()
            .id(key)
            .gap_1()
            .child(selectable_text("summary-item-text", text))
            .when(!segments.is_empty(), |view| {
                view.child(
                    Collapsible::new()
                        .open(open)
                        .gap_1()
                        .child(controls)
                        .content(
                            h_flex().flex_wrap().gap_2().children(
                                segments.iter().skip(1).map(|segment| reference(segment)),
                            ),
                        ),
                )
            })
    }

    fn transcript(
        &self,
        meeting: &Meeting,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = meeting.id;
        if let Some(segments) = &meeting.transcript
            && !segments.is_empty()
        {
            let mut start = 0;
            let ranges: Vec<_> = transcript_paragraphs(segments)
                .map(|paragraph| {
                    let range = start..start + paragraph.len();
                    start = range.end;
                    range
                })
                .collect();
            let count = ranges.len();
            let timestamps = self.transcript_timestamps;
            let rem_size = window.rem_size();
            let list_state = self.transcript_viewports.update(cx, |viewports, _| {
                let viewport = viewports.entry(id).or_insert_with(|| {
                    let viewport = TranscriptViewport {
                        list: ListState::new(count, ListAlignment::Top, rem_size * 12.),
                        timestamps,
                        rem_size,
                    };
                    if self.transcript_search_open
                        && let Some(found) =
                            self.transcript_matches.get(self.transcript_match_index)
                    {
                        viewport.reveal(found.paragraph);
                    }
                    viewport
                });
                if viewport.list.item_count() != count {
                    viewport.list.reset(count);
                } else if viewport.timestamps != timestamps || viewport.rem_size != rem_size {
                    viewport.list.remeasure();
                }
                viewport.timestamps = timestamps;
                viewport.rem_size = rem_size;
                viewport.list.clone()
            });
            let scroll_to_top = list_state.clone();
            let id = meeting.id;
            return v_flex()
                .flex_1()
                .min_h_0()
                .child(
                    h_flex()
                        .items_start()
                        .px_4()
                        .py_3()
                        .gap_3()
                        .child(muted(TRANSCRIPT_NOTICE, cx).text_xs().flex_1().min_w_0())
                        .child(
                            h_flex()
                                .gap_3()
                                .flex_shrink_0()
                                .child(
                                    Checkbox::new("transcript-timestamps")
                                        .label("Mostrar tiempos")
                                        .checked(timestamps)
                                        .cursor_pointer()
                                        .on_click(cx.listener(|this, checked, _, cx| {
                                            this.transcript_timestamps = *checked;
                                            cx.notify();
                                        })),
                                )
                                .child(self.regenerate_transcript_button(id, cx))
                                .child(
                                    icon_button(
                                        "find-transcript",
                                        "icons/search.svg",
                                        "Buscar en la transcripción · ⌘F",
                                    )
                                    .ghost()
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.find_transcript(&FindTranscript, window, cx)
                                        },
                                    )),
                                )
                                .child(
                                    icon_button(
                                        "transcript-top",
                                        "icons/arrow-up-to-line.svg",
                                        "Volver arriba",
                                    )
                                    .ghost()
                                    .text_color(cx.theme().muted_foreground)
                                    .on_click(cx.listener(
                                        move |_, _, _, cx| {
                                            scroll_to_top.scroll_to(ListOffset::default());
                                            cx.notify();
                                        },
                                    )),
                                ),
                        ),
                )
                .when(self.transcript_search_open, |view| {
                    view.child(self.transcript_search_bar(cx))
                })
                .child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .child(
                            list(
                                list_state.clone(),
                                cx.processor(move |this, index: usize, _, cx| {
                                    let paragraph = this
                                        .meetings
                                        .iter()
                                        .find(|meeting| meeting.id == id)
                                        .and_then(|meeting| meeting.transcript.as_ref())
                                        .and_then(|segments| segments.get(ranges[index].clone()));
                                    match paragraph {
                                        Some(paragraph) => this
                                            .transcript_paragraph(id, index, paragraph, cx)
                                            .into_any_element(),
                                        None => div().into_any_element(),
                                    }
                                }),
                            )
                            .size_full()
                            .p_4(),
                        )
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .child(Scrollbar::vertical(&list_state).viewport_from_layout()),
                        ),
                )
                .into_any_element();
        }
        let mut content = pane(SharedString::from(format!("transcript-{}", meeting.id)));
        if meeting.transcript.is_some() {
            content = content
                .child(
                    h_flex()
                        .gap_3()
                        .child(muted(TRANSCRIPT_NOTICE, cx).text_xs().flex_1().min_w_0())
                        .child(self.regenerate_transcript_button(id, cx)),
                )
                .child(muted("Whisper no detectó diálogo en esta grabación.", cx));
        } else if !self.jobs.contains_key(&meeting.id) {
            if self
                .library
                .transcription_ready(self.settings.vad_enabled())
            {
                let id = meeting.id;
                content = content.child(
                    regular_button("transcribe-meeting")
                        .label("Transcribir")
                        .on_click(cx.listener(move |this, _, _, cx| this.start_transcript(id, cx))),
                );
            } else {
                content = content
                    .child(muted(
                        "Descarga los modelos para transcribir esta reunión.",
                        cx,
                    ))
                    .child(self.whisper_settings(cx));
            }
        }
        content.overflow_y_scrollbar().into_any_element()
    }

    fn regenerate_transcript_button(&self, id: Uuid, cx: &Context<Self>) -> Button {
        icon_button(
            "regenerate-transcript",
            "icons/refresh-cw.svg",
            "Volver a generar la transcripción…",
        )
        .ghost()
        .disabled(
            self.jobs.contains_key(&id)
                || !self
                    .library
                    .transcription_ready(self.settings.vad_enabled()),
        )
        .on_click(cx.listener(move |this, _, window, cx| this.request_transcript(id, window, cx)))
    }

    fn transcript_search_bar(&self, cx: &Context<Self>) -> Div {
        let count = self.transcript_matches.len();
        let counter = if self.transcript_search.read(cx).value().trim().is_empty() {
            String::new()
        } else if count == 0 {
            "Sin coincidencias".into()
        } else {
            format!("{} de {count}", self.transcript_match_index + 1)
        };
        h_flex()
            .key_context("TranscriptSearch")
            .on_action(
                cx.listener(|this, _: &gpui_kit::component::input::Escape, window, cx| {
                    this.close_transcript_search(window, cx)
                }),
            )
            .px_4()
            .pb_3()
            .gap_2()
            .child(
                Input::new(&self.transcript_search)
                    .aria_label("Buscar en la transcripción")
                    .cleanable(true)
                    .flex_1()
                    .min_w_0(),
            )
            .child(muted(counter, cx).text_xs().flex_shrink_0())
            .child(
                icon_button(
                    "previous-transcript-match",
                    "icons/chevron-up.svg",
                    "Coincidencia anterior · ⇧Enter",
                )
                .ghost()
                .disabled(count == 0)
                .on_click(cx.listener(|this, _, _, cx| this.move_transcript_match(true, cx))),
            )
            .child(
                icon_button(
                    "next-transcript-match",
                    "icons/chevron-down.svg",
                    "Coincidencia siguiente · Enter",
                )
                .ghost()
                .disabled(count == 0)
                .on_click(cx.listener(|this, _, _, cx| this.move_transcript_match(false, cx))),
            )
            .child(
                icon_button(
                    "close-transcript-search",
                    "icons/x.svg",
                    "Cerrar búsqueda · Esc",
                )
                .ghost()
                .on_click(
                    cx.listener(|this, _, window, cx| this.close_transcript_search(window, cx)),
                ),
            )
    }

    fn transcript_paragraph(
        &self,
        id: Uuid,
        index: usize,
        paragraph: &[Segment],
        cx: &Context<Self>,
    ) -> Div {
        let first = &paragraph[0];
        let text = paragraph_text(paragraph);
        let highlights = self
            .transcript_matches
            .iter()
            .enumerate()
            .filter(|(_, found)| self.transcript_search_open && found.paragraph == index)
            .map(|(position, found)| {
                (
                    found.range.clone(),
                    HighlightStyle {
                        background_color: Some(cx.theme().primary.opacity(
                            if position == self.transcript_match_index {
                                1.
                            } else {
                                0.2
                            },
                        )),
                        color: Some(if position == self.transcript_match_index {
                            cx.theme().primary_foreground
                        } else {
                            cx.theme().foreground
                        }),
                        ..Default::default()
                    },
                )
            });
        v_flex()
            .w_full()
            .gap_1()
            .pb_6()
            .when(self.transcript_timestamps, |content| {
                content.child(
                    self.jump(id, format!("paragraph-{}", first.id), first.start_ms, cx)
                        .ghost()
                        .self_start()
                        .px_0()
                        .font_normal()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(cx.theme().primary)
                        .accessibility_label(format!(
                            "Reproducir desde {}",
                            timestamp(first.start_ms)
                        ))
                        .tooltip("Reproducir desde este párrafo"),
                )
            })
            .child(
                div()
                    .cursor_text()
                    .child(
                        SelectableText::new(
                            SharedString::from(format!("transcript-text-{id}-{}", first.id)),
                            text,
                        )
                        .highlights(highlights),
                    )
                    .line_height(rems(1.5)),
            )
    }

    fn markers(&self, meeting: &Meeting, cx: &Context<Self>) -> impl IntoElement {
        let id = meeting.id;
        let can_mark = self.capture.is_none()
            && !self.starting
            && meeting.duration_ms > 0
            && self.jobs.get(&id) != Some(&Work::Saving);
        let count = meeting.markers.len();
        pane("markers")
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        muted(
                            if count == 1 {
                                "1 marcador".into()
                            } else {
                                format!("{count} marcadores")
                            },
                            cx,
                        )
                        .flex_1(),
                    )
                    .child(
                        icon_button(
                            "mark-playback",
                            "icons/bookmark-plus.svg",
                            "Marcar posición",
                        )
                        .ghost()
                        .disabled(!can_mark)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.add_marker(&AddMarker, window, cx)
                        })),
                    )
                    .child(
                        icon_button(
                            "generate-markers",
                            "icons/refresh-cw.svg",
                            "Generar marcadores…",
                        )
                        .ghost()
                        .disabled(
                            self.jobs.contains_key(&id)
                                || !meeting
                                    .transcript
                                    .as_ref()
                                    .is_some_and(|segments| !segments.is_empty()),
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| this.request_markers(id, window, cx),
                        )),
                    ),
            )
            .when(
                !meeting
                    .transcript
                    .as_ref()
                    .is_some_and(|segments| !segments.is_empty()),
                |view| {
                    view.child(muted(
                        "Genera la transcripción para crear marcadores con IA.",
                        cx,
                    ))
                },
            )
            .when(meeting.markers.is_empty(), |view| {
                view.child(muted(
                    "Los marcadores que añadas al grabar o escuchar aparecerán aquí.",
                    cx,
                ))
            })
            .children(meeting.markers.iter().map(|marker| {
                let marker_id = marker.id;
                let label = marker.label.clone();
                let delete_label = label.clone();
                h_flex()
                    .id(SharedString::from(format!("marker-row-{marker_id}")))
                    .gap_2()
                    .rounded(cx.theme().radius)
                    .hover(|row| row.bg(cx.theme().muted))
                    .child(
                        self.jump(id, format!("marker-{marker_id}"), marker.ms, cx)
                            .flex_shrink_0(),
                    )
                    .child(div().min_w_0().truncate().child(label.clone()))
                    .child(
                        icon_button(
                            SharedString::from(format!("edit-{marker_id}")),
                            "icons/pencil.svg",
                            "Renombrar marcador…",
                        )
                        .ghost()
                        .text_color(cx.theme().muted_foreground)
                        .flex_shrink_0()
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.edit(Edit::Marker(id, marker_id), label.clone(), window, cx)
                            },
                        )),
                    )
                    .child(
                        icon_button(
                            SharedString::from(format!("delete-marker-{marker_id}")),
                            "icons/trash.svg",
                            "Eliminar marcador…",
                        )
                        .ghost()
                        .text_color(cx.theme().muted_foreground)
                        .flex_shrink_0()
                        .disabled(self.jobs.get(&id) == Some(&Work::GeneratingMarkers))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.delete_marker(id, marker_id, delete_label.clone(), window, cx)
                            },
                        )),
                    )
            }))
            .overflow_y_scrollbar()
    }

    fn jump(&self, id: Uuid, key: String, position: u64, cx: &Context<Self>) -> Button {
        compact_button(SharedString::from(key))
            .outline()
            .label(timestamp(position))
            .disabled(
                self.capture.is_some()
                    || self.starting
                    || self.jobs.get(&id) == Some(&Work::Saving),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.play(id, PlaybackCommand::Jump(position), window, cx)
            }))
    }
}

fn paragraph_text(segments: &[Segment]) -> String {
    segments
        .iter()
        .map(|segment| segment.text.trim())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn transcript_matches(segments: &[Segment], query: &str) -> Vec<TranscriptMatch> {
    let query: Vec<_> = query.trim().chars().collect();
    if query.is_empty() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    for (paragraph, segments) in transcript_paragraphs(segments).enumerate() {
        let text = paragraph_text(segments);
        let chars: Vec<_> = text.char_indices().collect();
        let mut start = 0;
        while start + query.len() <= chars.len() {
            if query
                .iter()
                .zip(&chars[start..start + query.len()])
                .all(|(needle, (_, character))| needle.to_lowercase().eq(character.to_lowercase()))
            {
                let (end, character) = chars[start + query.len() - 1];
                matches.push(TranscriptMatch {
                    paragraph,
                    range: chars[start].0..end + character.len_utf8(),
                });
                start += query.len();
            } else {
                start += 1;
            }
        }
    }
    matches
}

fn transcript_paragraphs(segments: &[Segment]) -> impl Iterator<Item = &[Segment]> {
    let mut remaining = segments;
    std::iter::from_fn(move || {
        let first = remaining.first()?;
        let mut count = 0;
        let mut length = 0;
        let mut end_ms = first.end_ms;
        let mut sentence_ended = false;
        for segment in remaining {
            // Keep nearby fragments together, breaking at pauses or readable sentence boundaries.
            if count > 0
                && (segment.start_ms > end_ms.saturating_add(2_000)
                    || segment.start_ms.saturating_sub(first.start_ms) >= 30_000
                    || length >= 600
                    || (length >= 240 && sentence_ended))
            {
                break;
            }
            let text = segment.text.trim();
            length += text.chars().count() + 1;
            end_ms = end_ms.max(segment.end_ms);
            sentence_ended = text.ends_with(['.', '!', '?', '…']);
            count += 1;
        }
        let (paragraph, rest) = remaining.split_at(count);
        remaining = rest;
        Some(paragraph)
    })
}

fn selectable_text(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Div {
    div().cursor_text().child(SelectableText::new(id, text))
}

pub(super) fn confirmation_footer(
    label: &'static str,
    variant: gpui_kit::component::button::ButtonVariant,
) -> DialogFooter {
    DialogFooter::new()
        .child(DialogClose::new().trigger(|button| button.label("Cancelar").cursor_pointer()))
        .child(
            DialogAction::new().child(
                Button::new("confirm")
                    .label(label)
                    .with_variant(variant)
                    .cursor_pointer(),
            ),
        )
}

fn regular_button(id: impl Into<ElementId>) -> Button {
    Button::new(id)
        .cursor_pointer()
        .h_9()
        .px_4()
        .text_sm()
        .font_medium()
}

fn compact_button(id: impl Into<ElementId>) -> Button {
    Button::new(id)
        .cursor_pointer()
        .small()
        .h_7()
        .px_2p5()
        .text_xs()
        .font_medium()
}

fn icon_button(id: impl Into<ElementId>, icon: &'static str, label: &'static str) -> Button {
    Button::new(id)
        .cursor_pointer()
        .small()
        .size_7()
        .p_0()
        .icon(Icon::default().path(icon))
        .accessibility_label(label)
        .tooltip(label)
}

fn pane(id: impl Into<gpui_kit::ElementId>) -> gpui_kit::Stateful<Div> {
    v_flex().id(id).flex_1().min_h_0().gap_3().p_4()
}

fn summary_section(title: &'static str, rows: Div, cx: &Context<Workspace>) -> Div {
    let icon = match title {
        "Decisiones" => "icons/circle-check.svg",
        "Tareas" => "icons/list-todo.svg",
        _ => "icons/circle-question-mark.svg",
    };
    v_flex()
        .pt_4()
        .gap_4()
        .child(
            h_flex()
                .gap_2()
                .pb_2()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(
                    Icon::default()
                        .path(icon)
                        .size_5()
                        .text_color(cx.theme().primary),
                )
                .child(section(title).text_base().text_color(cx.theme().foreground)),
        )
        .child(rows)
}

fn section(text: impl Into<SharedString>) -> Div {
    div().text_sm().font_semibold().child(text.into())
}

fn muted(text: impl Into<SharedString>, cx: &Context<Workspace>) -> Div {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

fn timestamp(ms: u64) -> String {
    let secs = Duration::from_millis(ms).as_secs();
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

fn date_label(secs: u64) -> String {
    let date = time::OffsetDateTime::from_unix_timestamp(secs as i64)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    let offset = time::UtcOffset::local_offset_at(date).unwrap_or(time::UtcOffset::UTC);
    let date = date.to_offset(offset);
    format!(
        "{:02}/{:02}/{} {:02}:{:02}",
        date.day(),
        u8::from(date.month()),
        date.year(),
        date.hour(),
        date.minute()
    )
}

fn settings_heading(text: &'static str) -> Div {
    div().text_base().font_semibold().child(text)
}

fn settings_group(title: &'static str, cx: &Context<Workspace>) -> Div {
    v_flex()
        .gap_3()
        .pt_5()
        .border_t_1()
        .border_color(cx.theme().border)
        .child(settings_heading(title))
}

fn settings_toggle(
    title: &'static str,
    description: &'static str,
    switch: Switch,
    cx: &Context<Workspace>,
) -> Div {
    h_flex()
        .items_start()
        .gap_4()
        .py_2()
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(div().font_medium().child(title))
                .child(muted(description, cx).text_xs()),
        )
        .child(switch.accessibility_label(title).flex_shrink_0())
}

#[cfg(test)]
mod tests {
    use super::transcript_matches;
    use crate::library::{AudioSource, Segment};

    fn segment(text: &str, start_ms: u64) -> Segment {
        Segment {
            id: 1,
            start_ms,
            end_ms: start_ms + 1_000,
            source: AudioSource::Teams,
            text: text.into(),
        }
    }

    #[test]
    fn transcript_search_preserves_utf8_ranges_and_counts_repeated_matches() {
        let text = "İndice: REUNIÓN, reunión y reunión.";
        let matches = transcript_matches(&[segment(text, 0)], "REUNIÓN");
        assert_eq!(
            matches
                .iter()
                .map(|found| &text[found.range.clone()])
                .collect::<Vec<_>>(),
            ["REUNIÓN", "reunión", "reunión"]
        );
    }

    #[test]
    fn transcript_search_uses_the_displayed_paragraph_text_across_segments() {
        let segments = [
            segment("  revisar el ", 0),
            segment(" presupuesto ", 1_000),
            segment("presupuesto", 50_000),
        ];
        let matches = transcript_matches(&segments, "el presupuesto");
        assert_eq!(
            matches
                .iter()
                .map(|found| (found.paragraph, found.range.clone()))
                .collect::<Vec<_>>(),
            [(0, 8..22)]
        );
    }

    #[test]
    fn transcript_search_treats_special_characters_literally_and_ignores_empty_queries() {
        let segments = [segment("Versión (1.0): [pendiente]", 0)];
        assert_eq!(
            [" ", "ausente", "(1.0)", "[pendiente]"]
                .map(|query| transcript_matches(&segments, query).len()),
            [0, 0, 1, 1]
        );
    }
}
