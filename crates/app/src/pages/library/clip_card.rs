//! One clip in a clip list (ClipCard.tsx): thumbnail, chips, hover actions,
//! selection, and the badges for unexported edits and render fallbacks.
use super::clip_list::ClipList;
use super::run_stats::tnum;
use super::thumbnails::Thumb;
use crate::shell;
use crate::theme::Tokens;
use crate::ui::{self, Truncate, TypeScale, u};
use clipward_core::format::format_timecode;
use clipward_core::job_view::ClipArtifact;
use clipward_core::library::{clip_file_path, clip_title, fallback_label, layout_label};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

pub(super) struct CardProps {
    pub clip: ClipArtifact,
    pub vertical: bool,
    pub top_pick: bool,
    /// Off for manual runs, whose clips have no virality score.
    pub show_score: bool,
    pub selected: bool,
    pub selecting: bool,
}

/// A label floating over footage: always dark (`.media-chip`).
pub(super) fn media_chip() -> Div {
    div()
        .flex()
        .items_center()
        .text_color(white().opacity(0.92))
        .bg(black().opacity(0.5))
        .border_1()
        .border_color(white().opacity(0.12))
}

pub(super) fn tip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |window, cx| Tooltip::new(text.clone()).build(window, cx)
}

/// The clip's picture. On hover the React card played the clip muted and looped
/// over the thumbnail.
fn hover_preview(_file_path: &str, _hovering: bool, thumbnail: AnyElement) -> AnyElement {
    // TODO(video): hover preview — while `_hovering`, play `_file_path` muted and
    // looping over the thumbnail with the native player; falls back to the thumbnail.
    thumbnail
}

fn warning_badge(
    id: &str,
    icon: &str,
    text: &'static str,
    tooltip: &'static str,
    t: &Tokens,
) -> Stateful<Div> {
    div()
        .id(SharedString::from(id.to_string()))
        .mt(u(6.))
        .self_start()
        .flex()
        .items_center()
        .gap(u(4.))
        .min_h(u(18.))
        .px(u(6.))
        .rounded(u(4.))
        .border_1()
        .border_color(t.warning.opacity(0.25))
        .bg(t.warning.opacity(0.1))
        .text_meta()
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.warning)
        .child(ui::icon(icon).size(u(12.)))
        .child(text)
        .tooltip(tip(tooltip))
}

impl ClipList {
    pub(super) fn render_card(
        &self,
        p: CardProps,
        t: &Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = *t;
        let clip = &p.clip;
        let index = clip.clip_index;
        let file_path = clip_file_path(&clip.s3_url).to_string();
        let title: SharedString = clip_title(clip).into();
        let thumb = self.thumbnails.read(cx).get(&file_path).cloned();
        let aspect = match &thumb {
            Some(Thumb::Ready { aspect, .. }) => *aspect,
            _ => None,
        };
        let clip_vertical = aspect.map_or(p.vertical, |a| a < 1.);
        let layout = clip_vertical
            .then(|| layout_label(&clip.layout_type))
            .flatten();
        let hovering = self.hovered == Some(index);
        let group: SharedString = format!("clip-card-{index}").into();
        let id = |name: &str| ElementId::NamedInteger(SharedString::from(name.to_string()), index);

        let picture = match &thumb {
            Some(Thumb::Ready { path, .. }) => hover_preview(
                &file_path,
                hovering,
                img(path.clone())
                    .absolute()
                    .inset_0()
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .with_fallback(move || unavailable(&t))
                    .into_any_element(),
            ),
            Some(Thumb::Failed) => unavailable(&t),
            _ => div()
                .absolute()
                .inset_0()
                .child(ui::skeleton(&t))
                .into_any_element(),
        };

        let open_path = file_path.clone();
        let selecting = p.selecting;
        let click_layer = div()
            .id(id("clip-play"))
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                if selecting {
                    this.toggle(index, cx);
                } else {
                    this.open_clip(index, &open_path, cx);
                }
            }))
            .when(!selecting, |d| {
                d.child(
                    media_chip()
                        .justify_center()
                        .size(u(40.))
                        .rounded_full()
                        .opacity(0.)
                        .group_hover(group.clone(), |s| s.opacity(1.))
                        .child(ui::icon("play").size(u(16.)).ml(u(2.))),
                )
            });

        let check = div()
            .absolute()
            .left(u(8.))
            .top(u(8.))
            .opacity(if selecting || p.selected { 1. } else { 0. })
            .group_hover(group.clone(), |s| s.opacity(1.))
            .child(
                div()
                    .id(id("clip-check"))
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(u(16.))
                    .rounded(u(4.))
                    .border_1()
                    .cursor_pointer()
                    .map(|d| {
                        if p.selected {
                            d.border_color(t.accent)
                                .bg(t.accent)
                                .child(ui::icon("check").size(u(10.)).text_color(t.accent_ink))
                        } else {
                            d.border_color(white().opacity(0.6))
                                .bg(black().opacity(0.4))
                                .hover(|s| s.border_color(white().opacity(0.9)))
                        }
                    })
                    .tooltip(tip(format!("Select {title}")))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle(index, cx);
                    })),
            );

        let chips = div()
            .absolute()
            .right(u(8.))
            .top(u(8.))
            .flex()
            .items_center()
            .gap(u(4.))
            .when(p.top_pick, |d| {
                d.child(
                    div()
                        .h(u(18.))
                        .flex()
                        .items_center()
                        .px(u(6.))
                        .rounded(u(4.))
                        .bg(t.accent)
                        .text_meta()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.accent_ink)
                        .child("Top pick"),
                )
            })
            .when(p.show_score, |d| {
                d.child(
                    media_chip()
                        .id(id("clip-score"))
                        .h(u(18.))
                        .gap(u(4.))
                        .px(u(6.))
                        .rounded(u(4.))
                        .text_meta()
                        .font_weight(FontWeight::MEDIUM)
                        .font_features(tnum())
                        .child(
                            ui::icon("trending-up")
                                .size(u(12.))
                                .text_color(t.brand_gold),
                        )
                        .child(format!("{:.1}", clip.virality_score * 10.))
                        .tooltip(tip("Virality score")),
                )
            });

        let duration = media_chip()
            .absolute()
            .left(u(8.))
            .bottom(u(8.))
            .px(u(4.))
            .py(px(1.))
            .rounded(u(4.))
            .text_meta()
            .font_weight(FontWeight::MEDIUM)
            .font_features(tnum())
            .child(format_timecode(clip.duration_ms));

        let action = |name: &str, icon: &str, label: SharedString| {
            media_chip()
                .id(id(name))
                .justify_center()
                .size(u(24.))
                .rounded(u(6.))
                .cursor_pointer()
                .hover(|s| s.bg(black().opacity(0.7)).text_color(white()))
                .child(ui::icon(icon).size(u(14.)))
                .tooltip(tip(label))
        };
        let reveal_path = file_path.clone();
        let editable = clip.editable;
        let actions =
            (!selecting).then(|| {
                div()
                    .absolute()
                    .right(u(8.))
                    .bottom(u(8.))
                    .flex()
                    .items_center()
                    .gap(u(4.))
                    .opacity(0.)
                    .group_hover(group.clone(), |s| s.opacity(1.))
                    .when(editable, |d| {
                        d.child(action("clip-edit", "scissors", "Edit".into()).on_click(
                            cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.open_editor(index, window, cx);
                            }),
                        ))
                    })
                    .child(
                        action(
                            "clip-reveal",
                            "folder-open",
                            if cfg!(target_os = "macos") {
                                "Show in Finder"
                            } else {
                                "Show in folder"
                            }
                            .into(),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.reveal_clip(index, &reveal_path, cx);
                        })),
                    )
            });

        let media = div()
            .id(id("clip-media"))
            .relative()
            .w_full()
            .aspect_ratio(if clip_vertical { 9. / 16. } else { 16. / 9. })
            .overflow_hidden()
            .rounded(u(8.))
            .bg(black().opacity(0.4))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let next = if *hovered {
                    Some(index)
                } else {
                    this.hovered.filter(|h| *h != index)
                };
                if this.hovered != next {
                    this.hovered = next;
                    cx.notify();
                }
            }))
            .child(picture)
            .child(click_layer)
            .child(check)
            .child(chips)
            .child(duration)
            .children(actions);

        let unexported = clip.unexported;
        let fallback = fallback_label(clip.render_fallback.as_deref());
        let action_error = self.action_errors.get(&index).cloned();
        let span = format!(
            "{} – {}",
            format_timecode(clip.start_time_ms),
            format_timecode(clip.end_time_ms)
        );
        let info = div()
            .flex()
            .flex_col()
            .px(u(6.))
            .pb(u(4.))
            .pt(u(8.))
            .child(
                div()
                    .id(id("clip-title"))
                    .line_clamp(2)
                    .text_ellipsis()
                    .text_label()
                    .text_color(t.ink)
                    .child(title.clone())
                    .tooltip(tip(title.clone())),
            )
            .child(
                div()
                    .mt(u(2.))
                    .flex()
                    .min_w_0()
                    .overflow_hidden()
                    .text_meta()
                    .text_color(t.ink_secondary)
                    .child(div().id(id("clip-span")).flex_shrink_0().font_features(tnum()).child(span).tooltip(tip("Position in the source video")))
                    .when_some(layout, |d, layout| {
                        d.child(
                            div()
                                .id(id("clip-layout"))
                                .min_w_0()
                                .one_line()
                                .child(format!("\u{a0}· {layout}"))
                                .tooltip(tip("How this clip was framed")),
                        )
                    }),
            )
            .when(unexported, |d| {
                d.child(warning_badge(
                    &format!("unexported-{index}"),
                    "pencil-line",
                    "Edited, not exported",
                    "This clip has edits saved in the editor that aren't in the video file yet. Open it and press Export.",
                    &t,
                ))
            })
            .when_some(fallback, |d, label| {
                d.child(warning_badge(
                    &format!("fallback-{index}"),
                    "triangle-alert",
                    label,
                    "Smart framing failed to render this clip, so it used the classic whole-frame layout. The log has the details.",
                    &t,
                ))
            })
            .when_some(action_error, |d, error| {
                d.child(div().mt(u(6.)).text_caption().text_color(t.danger).child(error))
            });

        div()
            .id(id("clip-card"))
            .group(group)
            .relative()
            .flex()
            .flex_col()
            .min_w_0()
            .rounded(u(12.))
            .p(u(4.))
            .border_1()
            .map(|d| {
                if p.selected {
                    d.border_color(t.accent.opacity(0.8))
                        .bg(t.accent.opacity(0.06))
                } else {
                    d.border_color(t.stroke)
                        .bg(t.tint(0.03))
                        .hover(|s| s.border_color(t.tint(0.15)).bg(t.tint(0.05)))
                }
            })
            .child(media)
            .child(info)
            .into_any_element()
    }

    fn open_clip(&mut self, index: u64, path: &str, cx: &mut Context<Self>) {
        self.action_errors.remove(&index);
        match shell::open_path(path, cx) {
            Ok(true) => {}
            Ok(false) => drop(
                self.action_errors
                    .insert(index, "Clip file is no longer available.".into()),
            ),
            Err(_) => drop(
                self.action_errors
                    .insert(index, "Could not open this clip.".into()),
            ),
        }
        cx.notify();
    }

    fn reveal_clip(&mut self, index: u64, path: &str, cx: &mut Context<Self>) {
        self.action_errors.remove(&index);
        match shell::reveal(path, cx) {
            Ok(true) => {}
            Ok(false) => drop(
                self.action_errors
                    .insert(index, "Clip file is no longer available.".into()),
            ),
            Err(_) => drop(
                self.action_errors
                    .insert(index, "Could not show this clip in its folder.".into()),
            ),
        }
        cx.notify();
    }
}

fn unavailable(t: &Tokens) -> AnyElement {
    div()
        .absolute()
        .inset_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(u(8.))
        .text_color(t.ink_tertiary)
        .child(ui::icon("image-off").size(u(20.)))
        .child(div().text_caption().one_line().child("Preview unavailable"))
        .into_any_element()
}
