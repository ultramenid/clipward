//! The video step's source picker: a drop zone with the link field and Browse,
//! then a preview card for the chosen source (SourcePicker.tsx).
//!
//! Unlike the webview, GPUI hands us the dropped file's real path, so a dropped
//! video is used directly (authorized like a picked file) instead of reopening
//! the file picker. Dropped links can't be read: GPUI only delivers file drops.
use super::{CreatePage, edit};
use crate::state;
use crate::theme::{ActiveTokens, Tokens};
use crate::ui::{self, Btn, Hue, Truncate, TypeScale, u};
use clipward_core::format::{format_timecode, is_url};
use clipward_core::job_form::{ClipDraft, SourceDuration};
use clipward_core::video_source::{
    SourceKind, display_source, is_valid_source_link, is_video_file, local_duration,
    normalize_video_source, source_link_error, twitch_source_error,
};
use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::path::PathBuf;

/// The preview of the chosen source and the picker's errors.
#[derive(Default)]
pub struct SourceState {
    /// The source the preview below belongs to.
    previewed: String,
    thumbnail: Option<PathBuf>,
    duration: Option<f64>,
    /// The still frame couldn't be made: show the kind's icon instead.
    failed: bool,
    probe: Option<Task<()>>,
    pub link_error: Option<String>,
    picker_error: Option<String>,
}

impl CreatePage {
    /// Probe a newly chosen local file: its length (for the chip and the manual
    /// clip checks) and a still frame from 2 s in (past a black first frame).
    pub(super) fn sync_preview(&mut self, source: &str, cx: &mut Context<Self>) {
        if self.source.previewed == source {
            return;
        }
        self.source.previewed = source.to_string();
        self.source.thumbnail = None;
        self.source.duration = None;
        self.source.failed = false;
        self.source.probe = None;
        if source.is_empty() {
            return;
        }
        if is_url(source) {
            self.load_youtube_thumbnail(source, cx);
            return;
        }
        let core = state::core(cx);
        let path = source.to_string();
        let probe = state::run(cx, async move {
            let Some(seconds) = local_duration(&core, &path).await? else {
                return Ok((None, None));
            };
            let thumbnail = core
                .thumbnails_generate(&path, Some((seconds / 2.0).min(2.0)))
                .await?;
            Ok((Some(seconds), thumbnail))
        });
        let source = source.to_string();
        self.source.probe = Some(cx.spawn(async move |this, cx| {
            let (seconds, thumbnail) = probe.await.unwrap_or((None, None));
            let _ = this.update(cx, |this, cx| {
                this.source.failed = thumbnail.is_none();
                this.source.thumbnail = thumbnail;
                this.source.duration = seconds;
                if let Some(seconds) = seconds {
                    edit(&this.draft, cx, |d| {
                        d.source_duration = Some(SourceDuration { source, seconds })
                    });
                }
                cx.notify();
            });
        }));
    }

    /// A YouTube link's thumbnail, downloaded by the core (other links have none).
    fn load_youtube_thumbnail(&mut self, source: &str, cx: &mut Context<Self>) {
        let Some(id) = clipward_core::format::youtube_id(source) else {
            return;
        };
        let core = state::core(cx);
        let fetch = state::run(cx, async move { core.youtube_thumbnail(&id).await });
        self.source.probe = Some(cx.spawn(async move |this, cx| {
            let thumbnail = fetch.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.source.thumbnail = thumbnail;
                cx.notify();
            });
        }));
    }

    fn set_source(&mut self, source: String, cx: &mut Context<Self>) {
        self.source.picker_error = None;
        self.source.link_error = None;
        edit(&self.draft, cx, |d| d.source = source);
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let picked = crate::shell::pick_video(cx).await;
            let _ = this.update(cx, |this, cx| match picked {
                Ok(Some(path)) => this.set_source(path, cx),
                Ok(None) => {}
                Err(_) => {
                    this.source.picker_error =
                        Some("Could not open the file picker. Please try again.".into());
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Use a typed or pasted link once it checks out.
    pub(super) fn submit_link(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let link = text.trim();
        if link.is_empty() {
            return;
        }
        if let Some(error) = source_link_error(link) {
            self.source.link_error = Some(error.into());
            cx.notify();
            return;
        }
        let source = normalize_video_source(link);
        self.inputs
            .link
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.set_source(source, cx);
    }

    fn drop_files(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        if self.starting {
            return;
        }
        let Some(path) = paths.paths().first() else {
            return;
        };
        if !is_video_file(path) {
            self.source.link_error =
                Some("Drop a video file (MP4, MOV, MKV, WebM, AVI, M4V or FLV).".into());
            cx.notify();
            return;
        }
        match state::core(cx).authorize_picked(path) {
            Ok(path) => self.set_source(path, cx),
            Err(_) => {
                self.source.link_error =
                    Some("Could not open that video. Try another file or use Browse.".into());
                cx.notify();
            }
        }
    }

    pub(super) fn render_source_picker(
        &mut self,
        draft: &ClipDraft,
        disabled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = *cx.tokens();
        let picker = if draft.source.is_empty() {
            self.render_drop_zone(disabled, window, cx)
                .into_any_element()
        } else {
            self.render_preview(&draft.source, disabled, cx)
                .into_any_element()
        };
        div()
            .child(picker)
            .when_some(self.source.picker_error.clone(), |d, error| {
                d.child(
                    div()
                        .mt(u(10.))
                        .px(u(4.))
                        .text_caption()
                        .text_color(t.danger)
                        .child(error),
                )
            })
            .into_any_element()
    }

    fn render_drop_zone(
        &mut self,
        disabled: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = *cx.tokens();
        let typed = !self.inputs.link.read(cx).value().trim().is_empty();
        let this = cx.entity().downgrade();
        let link = Input::new(&self.inputs.link)
            .text_size(u(13.))
            .large()
            .disabled(disabled)
            .prefix(ui::icon("link-2").size(u(14.)).text_color(t.ink_tertiary))
            .when(self.source.link_error.is_some(), |i| {
                i.border_color(t.danger.opacity(0.6))
            })
            // A pasted link that checks out (or a Twitch page to explain) is used at once.
            .on_paste(move |item, window, cx| {
                let Some(text) = item.text() else {
                    return false;
                };
                if !is_valid_source_link(&text) && twitch_source_error(&text).is_none() {
                    return false;
                }
                let _ = this.update(cx, |this, cx| this.submit_link(&text, window, cx));
                true
            })
            .when(typed, |input| {
                input.suffix(
                    Btn::new("use-link")
                        .small()
                        .primary()
                        .label("Use link")
                        .disabled(disabled)
                        .on_click(self.on_click(cx, |this, window, cx| {
                            let text = this.inputs.link.read(cx).value().to_string();
                            this.submit_link(&text, window, cx);
                        })),
                )
            });
        let accent = t.accent;
        div()
            .id("drop-zone")
            .group("drop-zone")
            .relative()
            .rounded(u(12.))
            .border_1()
            .border_dashed()
            .border_color(t.tint(0.15))
            .bg(t.tint(0.02))
            .p(u(16.))
            .when(disabled, |d| d.opacity(0.5))
            .drag_over::<ExternalPaths>(move |s, _, _, _| {
                s.border_color(accent.opacity(0.7)).bg(accent.opacity(0.06))
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.drop_files(paths, cx)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(12.))
                    .child(ui::icon_tile("cloud-upload", Hue::Accent, 32., &t))
                    .child(
                        div()
                            .min_w_0()
                            .text_label()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.ink)
                            .child(
                                div()
                                    .one_line()
                                    .group_drag_over::<ExternalPaths>("drop-zone", |s| s.hidden())
                                    .child("Drop a video file or paste a link"),
                            )
                            .child(
                                div()
                                    .hidden()
                                    .one_line()
                                    .group_drag_over::<ExternalPaths>("drop-zone", |s| s.block())
                                    .child("Drop to use this video"),
                            ),
                    ),
            )
            .child(
                div()
                    .mt(u(12.))
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .child(div().min_w_0().flex_1().child(link))
                    .child(div().text_caption().text_color(t.ink_tertiary).child("or"))
                    .child(
                        Btn::new("browse")
                            .large()
                            .icon("folder-open")
                            .label("Browse")
                            .disabled(disabled)
                            .on_click(self.on_click(cx, |this, _, cx| this.browse(cx))),
                    ),
            )
            .when_some(self.source.link_error.clone(), |d, error| {
                d.child(
                    div()
                        .mt(u(8.))
                        .px(u(4.))
                        .text_caption()
                        .text_color(t.danger)
                        .child(error),
                )
            })
    }

    fn render_preview(
        &self,
        source: &str,
        disabled: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = *cx.tokens();
        let kind = SourceKind::of(source);
        let icon_name = match kind {
            SourceKind::Twitch => "tv-minimal-play",
            SourceKind::YouTube(_) => "square-play",
            SourceKind::Link => "link-2",
            SourceKind::Local => "file-video-camera",
        };
        let frame = match &self.source.thumbnail {
            Some(path) if !self.source.failed => img(path.clone())
                .size_full()
                .object_fit(ObjectFit::Cover)
                .into_any_element(),
            _ => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(t.tint(0.05))
                .child(ui::icon(icon_name).size(u(20.)).text_color(t.ink_secondary))
                .into_any_element(),
        };
        let title = display_source(source);
        ui::card(&t)
            .flex()
            .items_center()
            .gap(u(12.))
            .p(u(8.))
            .pr(u(12.))
            .child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .h(u(72.))
                    .w(u(128.))
                    .overflow_hidden()
                    .rounded(u(8.))
                    .border_1()
                    .border_color(t.stroke)
                    .bg(black().opacity(0.4))
                    .child(frame)
                    .when_some(self.source.duration, |d, seconds| {
                        d.child(
                            div()
                                .absolute()
                                .bottom(u(4.))
                                .right(u(4.))
                                .rounded(u(4.))
                                .px(u(4.))
                                .text_micro()
                                .text_color(white().opacity(0.92))
                                .bg(black().opacity(0.5))
                                .border_1()
                                .border_color(white().opacity(0.12))
                                .child(format_timecode(seconds * 1000.0)),
                        )
                    }),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .child(div().one_line().text_label().text_color(t.ink).child(title))
                    .child(
                        div()
                            .mt(u(6.))
                            .flex()
                            .min_w_0()
                            .items_center()
                            .gap(u(8.))
                            .child(kind_badge(icon_name, kind.label(), &t))
                            .child(
                                div()
                                    .one_line()
                                    .text_caption()
                                    .text_color(t.ink_secondary)
                                    .child(if kind == SourceKind::Twitch {
                                        "Public, completed videos only"
                                    } else {
                                        "Ready to clip"
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(u(6.))
                    .child(
                        Btn::new("replace")
                            .small()
                            .label("Replace")
                            .disabled(disabled)
                            .on_click(self.on_click(cx, |this, _, cx| this.browse(cx))),
                    )
                    .child(
                        Btn::new("remove-video")
                            .ghost()
                            .small()
                            .icon("x")
                            .tooltip("Remove video")
                            .disabled(disabled)
                            .on_click(self.on_click(cx, |this, window, cx| {
                                this.source.picker_error = None;
                                edit(&this.draft, cx, |d| d.clear_source());
                                for state in [&this.inputs.trim_start, &this.inputs.trim_end] {
                                    state.update(cx, |s, cx| s.set_value("", window, cx));
                                }
                            })),
                    ),
            )
    }
}

/// Badge.tsx with a leading icon.
fn kind_badge(icon: &str, label: &'static str, t: &Tokens) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(u(4.))
        .min_h(u(18.))
        .px(u(6.))
        .rounded(u(4.))
        .border_1()
        .border_color(t.tint(0.1))
        .bg(t.tint(0.05))
        .text_meta()
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.ink_secondary)
        .child(ui::icon(icon).size(u(12.)).text_color(t.ink_secondary))
        .child(label)
}
