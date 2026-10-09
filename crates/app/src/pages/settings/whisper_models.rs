//! Offline transcription models (WhisperModels.tsx). The selected one must be
//! downloaded before a job can start. Downloads run on the core's runtime and feed
//! `models::Settings`, so they carry on (and keep reporting) after the page closes.
//!
//! `compact` (the Create page's setup card) shows only the selected model and no
//! delete button.
use crate::models::Settings;
use crate::state;
use crate::theme::ActiveTokens;
use crate::ui::{self, Btn, TypeScale, Truncate, u};
use clipward_core::provider_catalog::{WHISPER_MODELS, format_size_mb};
use clipward_core::whisper::WhisperModelStatus;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use super::widgets::{error_text, option_tile};

pub struct WhisperModels {
    compact: bool,
    settings: Entity<Settings>,
    error: Option<SharedString>,
    _observe: Subscription,
}

impl WhisperModels {
    pub fn new(compact: bool, cx: &mut Context<Self>) -> Self {
        let settings = Settings::global(cx);
        let observe = cx.observe(&settings, |_, _, cx| cx.notify());
        Self { compact, settings, error: None, _observe: observe }
    }

    fn download(&mut self, id: &'static str, cx: &mut Context<Self>) {
        self.error = None;
        let core = state::core(cx);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<WhisperModelStatus>();
        let download = state::run(cx, async move {
            core.whisper_download(id, move |status| {
                let _ = tx.send(status.clone());
            })
            .await
        });
        // Show it started at once; the first progress report comes ~250 ms in.
        self.settings.update(cx, |settings, cx| {
            if !settings.whisper_models.iter().any(|m| m.id == id && m.received_bytes.is_some()) {
                settings.set_whisper_status(
                    WhisperModelStatus { id: id.into(), downloaded: false, received_bytes: Some(0), total_bytes: None },
                    cx,
                );
            }
        });
        let feed = self.settings.downgrade();
        cx.spawn(async move |_, cx| {
            while let Some(status) = rx.recv().await {
                if feed.update(cx, |settings, cx| settings.set_whisper_status(status, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let settings = self.settings.clone();
        cx.spawn(async move |this, cx| {
            let result = download.await;
            settings.update(cx, |settings, cx| settings.reload_whisper_models(cx));
            if let Err(message) = result {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(message.into());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn select(&mut self, id: &'static str, cx: &mut Context<Self>) {
        let saved = self.settings.update(cx, |s, cx| s.save(|e| e.whisper_model = id.into(), cx));
        if !saved {
            self.error = Some(self.save_error(cx, "Could not save"));
        }
        let downloaded = self.settings.read(cx).whisper_models.iter().any(|m| m.id == id && m.downloaded);
        if !downloaded {
            self.download(id, cx);
        }
        cx.notify();
    }

    fn save_error(&self, cx: &App, fallback: &str) -> SharedString {
        self.settings.read(cx).save_error.clone().unwrap_or_else(|| fallback.into()).into()
    }
}

impl Render for WhisperModels {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.tokens();
        let settings = self.settings.read(cx);
        let selected_id = settings.value.as_ref().map(|s| s.whisper_model.clone()).unwrap_or_default();
        let statuses = settings.whisper_models.clone();
        let compact = self.compact;
        let rows = WHISPER_MODELS
            .iter()
            .enumerate()
            .filter(|(_, model)| !compact || model.id == selected_id)
            .map(|(ix, model)| {
                let id = model.id;
                let status = statuses.iter().find(|s| s.id == id);
                let selected = selected_id == id;
                let received = status.and_then(|s| s.received_bytes);
                let percent = match (received, status.and_then(|s| s.total_bytes)) {
                    (Some(got), Some(total)) if total > 0 => got as f64 / total as f64 * 100.0,
                    _ => 0.0,
                };
                let action = if received.is_some() {
                    Btn::new(("whisper-cancel", ix))
                        .ghost()
                        .small()
                        .icon("x")
                        .label("Cancel")
                        .on_click(move |_, _, cx| state::core(cx).whisper_cancel(id))
                        .into_any_element()
                } else if status.is_some_and(|s| s.downloaded) {
                    div()
                        .flex()
                        .items_center()
                        .gap(u(4.))
                        .child(div().text_meta().text_color(t.success).child("Downloaded"))
                        .when(!compact, |d| {
                            d.child(
                                Btn::new(("whisper-delete", ix))
                                    .ghost()
                                    .small()
                                    .icon("trash")
                                    .tooltip(format!("Delete {}", model.label))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if let Err(message) = state::core(cx).whisper_delete(id) {
                                            this.error = Some(message.into());
                                        }
                                        this.settings.update(cx, |s, cx| s.reload_whisper_models(cx));
                                    })),
                            )
                        })
                        .into_any_element()
                } else {
                    Btn::new(("whisper-download", ix))
                        .variant(if selected { ui::Variant::Primary } else { ui::Variant::Secondary })
                        .small()
                        .icon("download")
                        .label("Download")
                        .on_click(cx.listener(move |this, _, _, cx| this.download(id, cx)))
                        .into_any_element()
                };
                option_tile(("whisper", ix), selected, &t)
                    .px(u(12.))
                    .py(u(10.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(u(12.))
                            .child(
                                div()
                                    .id(("whisper-pick", ix))
                                    .flex()
                                    .flex_1()
                                    .min_w_0()
                                    .items_center()
                                    .gap(u(10.))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
                                    .child(radio_dot(selected, &t))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_baseline()
                                                    .gap(u(4.))
                                                    .text_label()
                                                    .text_color(t.ink)
                                                    .child(model.label)
                                                    .child(
                                                        div()
                                                            .text_meta()
                                                            .text_color(t.ink_secondary)
                                                            .child(format_size_mb(model.size_mb as f64)),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .text_caption()
                                                    .text_color(t.ink_secondary)
                                                    .one_line()
                                                    .child(model.note),
                                            ),
                                    ),
                            )
                            .child(action),
                    )
                    .when_some(received, |tile, got| {
                        tile.child(
                            div()
                                .mt(u(8.))
                                .flex()
                                .items_center()
                                .gap(u(8.))
                                .child(div().flex_1().child(ui::progress_bar(percent, 4., &t)))
                                .child(
                                    div()
                                        .w(u(96.))
                                        .flex_shrink_0()
                                        .flex()
                                        .justify_end()
                                        .whitespace_nowrap()
                                        .text_meta()
                                        .text_color(t.ink_secondary)
                                        .child(format!(
                                            "{} / {}",
                                            format_size_mb(got as f64 / 1e6),
                                            format_size_mb(model.size_mb as f64)
                                        )),
                                ),
                        )
                    })
            })
            .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .gap(u(8.))
            .child(div().flex().flex_col().gap(u(6.)).children(rows))
            .when_some(self.error.clone(), |d, message| d.child(error_text(message, &t)))
    }
}

/// The round radio mark: filled accent with a check when chosen.
fn radio_dot(selected: bool, t: &crate::theme::Tokens) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(u(16.))
        .rounded_full()
        .border_1()
        .map(|d| {
            if selected {
                d.border_color(t.accent).bg(t.accent).child(
                    ui::icon("check").size(u(10.)).text_color(t.accent_ink),
                )
            } else {
                d.border_color(t.tint(0.3))
            }
        })
}
