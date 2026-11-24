use gpui_kit::base::{SelectableText, StyledExt};
use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, Selectable, Sizable, ThemeMode,
    button::{Button, ButtonVariants},
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex,
    input::Input,
    scroll::ScrollableElement,
    select::Select,
    slider::{Slider, SliderValue},
    switch::Switch,
    tab::{Tab as TabItem, TabBar},
    v_flex,
};
use gpui_kit::{
    AppContext, Context, CursorStyle, Div, ElementId, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder,
};
use uuid::Uuid;

use super::{
    AddMarker, Edit, MoveMeetingDown, MoveMeetingUp, PlaybackCommand, Tab, Work, Workspace,
};
use crate::library::{Meeting, Provider};

const SEGMENTS_PER_PAGE: usize = 100;

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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected_meeting();
        v_flex()
            .size_full()
            .text_sm()
            .text_color(cx.theme().foreground)
            .bg(cx.theme().background)
            .track_focus(&self.focus)
            .key_context("Reuniones")
            .on_action(cx.listener(Self::add_marker))
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
                        self.preferences_view(cx).into_any_element()
                    } else if let Some(meeting) = selected {
                        self.detail(meeting, cx).into_any_element()
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

    fn preferences_view(&self, cx: &Context<Self>) -> impl IntoElement {
        let dark = cx.theme().mode == ThemeMode::Dark;
        let settings = v_flex().flex_1().min_w_0().min_h_0().gap_5().p_6()
            .child(div().text_lg().font_semibold().child("Configuración"))
            .child(v_flex().gap_2()
                .child(section("Apariencia"))
                .child(h_flex().gap_2().children([("Claro", false), ("Oscuro", true)].map(|(name, value)| {
                    regular_button(name).outline().selected(dark == value)
                        .when(dark == value, |button| button.bg(cx.theme().transparent).text_color(cx.theme().primary))
                        .label(name)
                        .on_click(cx.listener(move |this, _, window, cx| this.set_theme(value, window, cx)))
                }))))
            .child(v_flex().gap_2()
                .child(section("Resúmenes"))
                .child("Herramienta de IA")
                .child(h_flex().gap_2().children([Provider::Codex, Provider::Copilot, Provider::Claude].map(|provider| {
                    regular_button(provider.name()).outline().selected(self.settings.provider == Some(provider))
                        .when(self.settings.provider == Some(provider), |button| button.bg(cx.theme().transparent).text_color(cx.theme().primary))
                        .label(provider.name())
                        .on_click(cx.listener(move |this, _, window, cx| this.set_provider(provider, window, cx)))
                })))
                .child(muted(match self.settings.provider {
                    Some(Provider::Codex) if Provider::Codex.executable().is_some() && self.codex_signed_in == Some(true) =>
                        "Codex · Listo".into(),
                    Some(Provider::Codex) if Provider::Codex.executable().is_some() && self.codex_signed_in == Some(false) =>
                        "Codex · Inicia sesión con tu cuenta de ChatGPT en la terminal.".into(),
                    Some(Provider::Claude) if Provider::Claude.executable().is_some() && self.claude_signed_in == Some(true) =>
                        "Claude Code · Listo".into(),
                    Some(Provider::Claude) if Provider::Claude.executable().is_some() && self.claude_signed_in == Some(false) =>
                        "Claude Code · Inicia sesión en la terminal con claude auth login.".into(),
                    Some(provider) if provider.executable().is_some() =>
                        format!("{} · CLI instalada. Usa la sesión iniciada en la terminal.", provider.name()),
                    Some(provider) => format!("{} · CLI no instalada.", provider.name()),
                    None => "Elige la herramienta que usas en la terminal.".into(),
                }, cx))
                .when_some(self.model_select.as_ref().zip(self.effort_select.as_ref()), |view, (model, effort)| {
                    view.child(h_flex().items_start().gap_4()
                        .child(v_flex().flex_1().min_w_0().gap_2().child("Modelo")
                            .child(Select::new(model).cursor_pointer().w_full().disabled(self.options_loading)))
                        .child(v_flex().flex_1().min_w_0().gap_2().child("Esfuerzo de razonamiento")
                            .child(Select::new(effort).cursor_pointer().w_full().disabled(self.options_loading))))
                })
                .when(self.options_loading, |view| view.child(muted("Cargando modelos y niveles de esfuerzo…", cx)))
                .child(Switch::new("automatic-summary").cursor_pointer()
                    .label("Generar resúmenes automáticamente")
                    .checked(self.settings.automatic)
                    .disabled(self.settings.provider.is_none())
                    .on_change(cx.listener(|this, enabled, window, cx| this.set_automatic(*enabled, window, cx))))
                .child(Switch::new("automatic-markers").cursor_pointer()
                    .label("Generar marcadores automáticamente")
                    .checked(self.settings.automatic_markers)
                    .disabled(self.settings.provider.is_none())
                    .on_change(cx.listener(|this, enabled, window, cx| this.set_automatic_markers(*enabled, window, cx))))
                .child(muted("El proveedor recibe la transcripción. El audio se queda en tu Mac.", cx)))
            .child(v_flex().gap_2()
                .child(section("Transcripción"))
                .child(muted("Whisper transcribe en español en tu Mac.", cx))
                .child(self.whisper_settings(cx)))
            .child(v_flex().gap_2()
                .child(section("Grabación"))
                .child(muted("Se usa la entrada predeterminada de macOS. Silenciarte en Teams no detiene la grabación del micrófono.", cx))
                .child(muted("Añadir marcador durante la grabación · ⌘⇧M", cx)))
            .child(v_flex().gap_2()
                .child(section("Archivos"))
                .child(h_flex().gap_3()
                    .child(regular_button("open-recordings-folder")
                        .icon(Icon::default().path("icons/folder-open.svg"))
                        .label("Abrir en Finder")
                        .on_click(cx.listener(|this, _, _, cx| this.open_recordings_folder(cx))))
                    .child(muted("Grabaciones y transcripciones guardadas en tu Mac.", cx).flex_1().min_w_0())));
        settings.id("settings-scroll").overflow_y_scrollbar()
    }

    fn whisper_settings(&self, cx: &Context<Self>) -> impl IntoElement {
        if self.library.model_path().is_file() {
            muted("Modelo de Whisper instalado.", cx).into_any_element()
        } else {
            regular_button("download-model")
                .label(if self.downloading {
                    "Descargando Whisper…"
                } else {
                    "Descargar Whisper (547 MiB)"
                })
                .disabled(self.downloading)
                .on_click(cx.listener(|this, _, _, cx| this.download(cx)))
                .into_any_element()
        }
    }

    fn detail(&self, meeting: &Meeting, cx: &Context<Self>) -> impl IntoElement {
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
                TabBar::new("meeting-tabs")
                    .underline()
                    .px_4()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .children(
                        Tab::ALL.map(|tab| TabItem::new().label(tab.label()).cursor_pointer()),
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
                Tab::Summary => self.summary(meeting, cx).into_any_element(),
                Tab::Transcript => self.transcript(meeting, cx).into_any_element(),
                Tab::Markers => self.markers(meeting, cx).into_any_element(),
            })
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

    fn summary(&self, meeting: &Meeting, cx: &Context<Self>) -> impl IntoElement {
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
                    content = content.child(section(heading));
                    for item in items {
                        content = content.child(self.summary_item(
                            meeting,
                            item.text.clone(),
                            &item.segment_ids,
                            cx,
                        ));
                    }
                }
            }
            if !summary.content.actions.is_empty() {
                content = content.child(section("Tareas"));
                for item in &summary.content.actions {
                    let mut text = item.text.clone();
                    if let Some(owner) = &item.owner {
                        text.push_str(&format!(" · {owner}"));
                    }
                    if let Some(deadline) = &item.deadline {
                        text.push_str(&format!(" · {deadline}"));
                    }
                    content =
                        content.child(self.summary_item(meeting, text, &item.segment_ids, cx));
                }
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
        text: String,
        ids: &[usize],
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let links = h_flex()
            .flex_wrap()
            .gap_2()
            .children(ids.iter().filter_map(|id| {
                let segment = meeting.transcript.as_ref()?.get(id.checked_sub(1)?)?;
                Some(self.jump(
                    meeting.id,
                    format!("reference-{}", segment.id),
                    segment.start_ms,
                    cx,
                ))
            }));
        v_flex()
            .id(SharedString::from(format!("summary-item-{text}-{ids:?}")))
            .gap_1()
            .child(selectable_text("summary-item-text", text))
            .child(links)
    }

    fn transcript(&self, meeting: &Meeting, cx: &Context<Self>) -> impl IntoElement {
        let mut content = pane(SharedString::from(format!(
            "transcript-{}-{}",
            meeting.id, self.transcript_page
        )));
        if let Some(segments) = &meeting.transcript {
            if segments.is_empty() {
                content = content.child(muted("Whisper no detectó diálogo en esta grabación.", cx));
            } else {
                let pages = segments.len().div_ceil(SEGMENTS_PER_PAGE);
                let page = self.transcript_page.min(pages - 1);
                if pages > 1 {
                    content = content.child(
                        h_flex()
                            .gap_2()
                            .child(
                                compact_button("prev-segments")
                                    .label("Anterior")
                                    .disabled(page == 0)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.transcript_page =
                                            this.transcript_page.saturating_sub(1);
                                        cx.notify();
                                    })),
                            )
                            .child(muted(format!("Página {} de {pages}", page + 1), cx))
                            .child(
                                compact_button("next-segments")
                                    .label("Siguiente")
                                    .disabled(page + 1 == pages)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.transcript_page += 1;
                                        cx.notify();
                                    })),
                            ),
                    );
                }
                content = content.children(
                    segments
                        .iter()
                        .skip(page * SEGMENTS_PER_PAGE)
                        .take(SEGMENTS_PER_PAGE)
                        .map(|segment| {
                            h_flex()
                                .items_start()
                                .gap_3()
                                .py_2()
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .child(self.jump(
                                    meeting.id,
                                    format!("segment-{}", segment.id),
                                    segment.start_ms,
                                    cx,
                                ))
                                .child(
                                    v_flex()
                                        .min_w_0()
                                        .flex_1()
                                        .gap_1()
                                        .child(muted(segment.source.label(), cx))
                                        .child(selectable_text(
                                            SharedString::from(format!(
                                                "transcript-text-{}-{}",
                                                meeting.id, segment.id
                                            )),
                                            segment.text.clone(),
                                        )),
                                )
                        }),
                );
            }
        } else if !self.jobs.contains_key(&meeting.id) {
            if self.library.model_path().is_file() {
                let id = meeting.id;
                content = content.child(
                    regular_button("transcribe-meeting")
                        .label("Transcribir")
                        .on_click(cx.listener(move |this, _, _, cx| this.start_transcript(id, cx))),
                );
            } else {
                content = content
                    .child(muted(
                        "Descarga el modelo para transcribir esta reunión.",
                        cx,
                    ))
                    .child(self.whisper_settings(cx));
            }
        }
        content.overflow_y_scrollbar()
    }

    fn markers(&self, meeting: &Meeting, cx: &Context<Self>) -> impl IntoElement {
        let id = meeting.id;
        let can_mark = self.capture.is_none()
            && !self.starting
            && meeting.duration_ms > 0
            && self.jobs.get(&id) != Some(&Work::Saving);
        pane("markers")
            .child(
                h_flex()
                    .gap_2()
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
                h_flex()
                    .gap_2()
                    .child(self.jump(id, format!("marker-{marker_id}"), marker.ms, cx))
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
