use gpui_kit::base::StyledExt;
use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme, Disableable, Sizable, ThemeMode,
    button::{Button, ButtonVariants},
    h_flex,
    input::Input,
    scroll::ScrollableElement,
    switch::Switch,
    v_flex,
};
use gpui_kit::{
    Context, Div, InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled,
    Window, div, prelude::FluentBuilder,
};
use uuid::Uuid;

use super::{AddMarker, Edit, Tab, Work, Workspace};
use crate::library::{Meeting, Provider};

const SEGMENTS_PER_PAGE: usize = 100;

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.meetings.iter().find(|m| Some(m.id) == self.selected);
        v_flex()
            .size_full()
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
                            Button::new("dismiss-notice")
                                .small()
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
                    Button::new("marker-record")
                        .small()
                        .label(format!("Marcar ({})", meeting.markers.len()))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.add_marker(&AddMarker, window, cx)
                        })),
                )
                .child(
                    Button::new("pause-record")
                        .small()
                        .label(if active.is_paused() {
                            "Reanudar"
                        } else {
                            "Pausar"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some((_, capture)) = this.capture.as_ref() {
                                capture.toggle_pause();
                            }
                            cx.notify();
                        })),
                )
                .child(
                    Button::new("stop-record")
                        .small()
                        .danger()
                        .label("Detener")
                        .on_click(cx.listener(|this, _, _, cx| this.finish(cx))),
                )
        } else {
            toolbar
                .child(div().font_semibold().child("Reuniones"))
                .child(div().flex_1())
                .child(
                    Button::new("start-record")
                        .label(if self.starting {
                            "Preparando…"
                        } else {
                            "Grabar"
                        })
                        .disabled(
                            self.starting || self.jobs.values().any(|job| *job == Work::Saving),
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.record(window, cx))),
                )
        }
    }

    fn sidebar(&self, cx: &Context<Self>) -> impl IntoElement {
        let query = self.search.read(cx).value().trim().to_lowercase();
        let rows = self
            .meetings
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
            .map(|meeting| {
                let id = meeting.id;
                let selected = !self.preferences && self.selected == Some(id);
                let button = Button::new(SharedString::from(format!("meeting-{id}")))
                    .w_full()
                    .label(meeting.title.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select(id);
                        cx.notify();
                    }));
                v_flex()
                    .gap_1()
                    .pb_2()
                    .child(if selected {
                        button.outline()
                    } else {
                        button.ghost()
                    })
                    .child(div().px_2().child(muted(
                        format!(
                            "{} · {}",
                            date_label(meeting.created),
                            timestamp(meeting.duration_ms)
                        ),
                        cx,
                    )))
            });
        v_flex()
            .w_56()
            .flex_shrink_0()
            .min_h_0()
            .gap_3()
            .p_3()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().border)
            .child(Input::new(&self.search).cleanable(true))
            .child(
                v_flex()
                    .id("meeting-library")
                    .min_h_0()
                    .flex_1()
                    .children(rows)
                    .when(self.meetings.is_empty(), |view| {
                        view.child(muted("Todavía no hay reuniones.", cx))
                    })
                    .overflow_y_scrollbar(),
            )
            .child(
                Button::new("settings")
                    .ghost()
                    .label("Configuración")
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_preferences(cx))),
            )
    }

    fn preferences_view(&self, cx: &Context<Self>) -> impl IntoElement {
        let dark = cx.theme().mode == ThemeMode::Dark;
        let settings = v_flex().flex_1().min_w_0().min_h_0().gap_5().p_6()
            .child(div().text_lg().font_semibold().child("Configuración"))
            .child(v_flex().gap_2()
                .child(section("Apariencia"))
                .child(h_flex().gap_2().children([("Claro", false), ("Oscuro", true)].map(|(name, value)| {
                    let button = Button::new(name).label(name)
                        .on_click(cx.listener(move |this, _, window, cx| this.set_theme(value, window, cx)));
                    if dark == value { button.outline() } else { button.ghost() }
                }))))
            .child(v_flex().gap_2()
                .child(section("Resúmenes"))
                .child("Herramienta de IA")
                .child(h_flex().gap_2().children([Provider::Codex, Provider::Copilot].map(|provider| {
                    let button = Button::new(provider.name()).label(provider.name())
                        .on_click(cx.listener(move |this, _, _, cx| this.set_provider(provider, cx)));
                    if self.settings.provider == Some(provider) { button.outline() } else { button.ghost() }
                })))
                .child(muted(match self.settings.provider {
                    Some(Provider::Codex) if Provider::Codex.executable().is_some() && self.codex_signed_in == Some(true) =>
                        "Codex · Listo".into(),
                    Some(Provider::Codex) if Provider::Codex.executable().is_some() && self.codex_signed_in == Some(false) =>
                        "Codex · Inicia sesión con tu cuenta de ChatGPT en la terminal.".into(),
                    Some(provider) if provider.executable().is_some() =>
                        format!("{} · CLI instalada. Usa la sesión iniciada en la terminal.", provider.name()),
                    Some(provider) => format!("{} · CLI no instalada.", provider.name()),
                    None => "Elige la herramienta que usas en la terminal.".into(),
                }, cx))
                .child(Switch::new("automatic-summary")
                    .label("Generar resúmenes automáticamente")
                    .checked(self.settings.automatic)
                    .disabled(self.settings.provider.is_none())
                    .on_change(cx.listener(|this, enabled, window, cx| this.set_automatic(*enabled, window, cx))))
                .child(muted("El proveedor recibe la transcripción. El audio se queda en tu Mac.", cx)))
            .child(v_flex().gap_2()
                .child(section("Transcripción"))
                .child(muted("Whisper transcribe en español en tu Mac.", cx))
                .child(self.whisper_settings(cx)))
            .child(v_flex().gap_2()
                .child(section("Grabación"))
                .child(muted("Se usa la entrada predeterminada de macOS. Silenciarte en Teams no detiene la grabación del micrófono.", cx))
                .child(muted("Añadir marcador durante la grabación · ⌘⇧M", cx)));
        settings.id("settings-scroll").overflow_y_scrollbar()
    }

    fn whisper_settings(&self, cx: &Context<Self>) -> impl IntoElement {
        if self.library.model_path().is_file() {
            muted("Modelo de Whisper instalado.", cx).into_any_element()
        } else {
            Button::new("download-model")
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
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .text_lg()
                                    .font_semibold()
                                    .child(title.clone()),
                            )
                            .child(
                                Button::new("rename-meeting")
                                    .small()
                                    .ghost()
                                    .label("Renombrar…")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.edit(Edit::Meeting(id), title.clone(), window, cx)
                                    })),
                            )
                            .child(
                                Button::new("delete-meeting")
                                    .small()
                                    .ghost()
                                    .label("Eliminar…")
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
                h_flex()
                    .gap_2()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .children(
                        [
                            ("Resumen", Tab::Summary),
                            ("Transcripción", Tab::Transcript),
                            ("Marcadores", Tab::Markers),
                        ]
                        .map(|(name, tab)| {
                            let button = Button::new(name).label(name).small().on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.tab = tab;
                                    cx.notify();
                                }),
                            );
                            if self.tab == tab {
                                button.outline()
                            } else {
                                button.ghost()
                            }
                        }),
                    ),
            )
            .child(match self.tab {
                Tab::Summary => self.summary(meeting, cx).into_any_element(),
                Tab::Transcript => self.transcript(meeting, cx).into_any_element(),
                Tab::Markers => self.markers(meeting, cx).into_any_element(),
            })
    }

    fn player(&self, meeting: &Meeting, cx: &Context<Self>) -> impl IntoElement {
        let id = meeting.id;
        let playback = self
            .playback
            .as_ref()
            .filter(|(id, _)| *id == meeting.id)
            .map(|(_, player)| player.player());
        let playing = playback.is_some_and(|p| !p.is_paused() && !p.empty());
        let position = playback.map_or(0, |p| p.get_pos().as_millis() as u64);
        let disabled =
            self.capture.is_some() || self.starting || self.jobs.get(&id) == Some(&Work::Saving);
        h_flex()
            .gap_2()
            .flex_wrap()
            .child(
                Button::new("play")
                    .small()
                    .label(if playing {
                        "Pausar audio"
                    } else {
                        "Reproducir"
                    })
                    .disabled(disabled)
                    .on_click(cx.listener(move |this, _, _, cx| this.play(id, None, cx))),
            )
            .child(
                Button::new("back-15")
                    .small()
                    .ghost()
                    .label("−15 s")
                    .disabled(disabled || position == 0)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.play(id, Some(position.saturating_sub(15_000)), cx)
                    })),
            )
            .child(
                Button::new("forward-15")
                    .small()
                    .ghost()
                    .label("+15 s")
                    .disabled(disabled || position >= meeting.duration_ms)
                    .on_click(cx.listener({
                        let target = position.saturating_add(15_000).min(meeting.duration_ms);
                        move |this, _, _, cx| this.play(id, Some(target), cx)
                    })),
            )
            .child(
                div()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{} / {}",
                        timestamp(position),
                        timestamp(meeting.duration_ms)
                    )),
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
                            Button::new("edit-summary")
                                .small()
                                .ghost()
                                .label("Editar resumen…")
                                .disabled(work.is_some())
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.edit_summary(id, text.clone(), window, cx)
                                })),
                        ),
                )
                .child(div().child(summary.content.summary.clone()));
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
        if meeting.transcript.is_some() && !empty_transcript && work.is_none() {
            let label = if self.settings.provider.is_none() {
                "Elegir herramienta de IA"
            } else if meeting.summary.is_some() {
                "Volver a generar…"
            } else if self.errors.contains_key(&id) {
                "Reintentar resumen…"
            } else if self.settings.automatic {
                "Generar resumen"
            } else {
                "Generar resumen…"
            };
            content = content.child(Button::new("generate-summary").label(label).on_click(
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
            .child(text)
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
                                Button::new("prev-segments")
                                    .small()
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
                                Button::new("next-segments")
                                    .small()
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
                                        .child(div().child(segment.text.clone())),
                                )
                        }),
                );
            }
        } else if !self.jobs.contains_key(&meeting.id) {
            if self.library.model_path().is_file() {
                let id = meeting.id;
                content = content.child(
                    Button::new("transcribe-meeting")
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
            && self
                .playback
                .as_ref()
                .is_some_and(|(playing, _)| *playing == id);
        pane("markers")
            .child(
                Button::new("mark-playback")
                    .label("Marcar posición")
                    .disabled(!can_mark)
                    .on_click(
                        cx.listener(|this, _, window, cx| this.add_marker(&AddMarker, window, cx)),
                    ),
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
                    .child(div().min_w_0().flex_1().child(label.clone()))
                    .child(
                        Button::new(SharedString::from(format!("edit-{marker_id}")))
                            .small()
                            .ghost()
                            .label("Renombrar…")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.edit(Edit::Marker(id, marker_id), label.clone(), window, cx)
                            })),
                    )
            }))
            .overflow_y_scrollbar()
    }

    fn jump(&self, id: Uuid, key: String, position: u64, cx: &Context<Self>) -> Button {
        Button::new(SharedString::from(key))
            .small()
            .outline()
            .label(timestamp(position))
            .disabled(
                self.capture.is_some()
                    || self.starting
                    || self.jobs.get(&id) == Some(&Work::Saving),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.play(id, Some(position), cx)))
    }
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
    format!(
        "{:02}/{:02}/{}",
        date.day(),
        u8::from(date.month()),
        date.year()
    )
}
