//! The piece inspector (the "Selected piece" section of EditorPage.tsx's `<aside>`)
//! with ColorPanel.tsx: framing, split panels, the whole-frame background, crop zoom
//! and keyframes, colour, speed, the transition in, volume, fades and detaching audio.
use super::{Inspector, Pick, hint, label, section};
use crate::editor::state::{EditorState, TRANSITIONS};
use crate::theme::{ActiveTokens, Tokens};
use crate::ui::{self, Btn, TypeScale, u};
use clipward_core::clip_engine::render::project::{
    Crop, MAX_CROP_ZOOM, PIECE_SPEED_RANGE, Project, VideoItem, split_panels,
};
use clipward_core::inspector::{
    self as logic, ColorSlider, LOOKS, backdrop_mode, framing_choice, key_here, piece_local,
};
use clipward_core::timeline::{crop_at, item_length, update_item};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

pub(super) use clipward_core::inspector::TRANSITION_STYLES;

/// Change piece `index` (one undo step, or the drag's).
fn edit_piece(e: &mut EditorState, index: usize, patch: impl FnOnce(&mut VideoItem), cx: &mut Context<EditorState>) {
    e.edit_video(|v| update_item(v, index, patch), true, cx);
}

/// The transition style select picked `style` for piece `index`.
pub(super) fn set_transition_style(e: &mut EditorState, index: usize, style: &str, cx: &mut Context<EditorState>) {
    edit_piece(e, index, |item| logic::set_transition_style(item, style), cx);
}

/// The playhead as ms into piece `index`.
fn local_time(e: &EditorState, index: usize, cx: &App) -> f64 {
    let time = e.time(cx);
    e.project().map_or(0., |p| piece_local(&p.video, index, time))
}

/// A caption-sized label with a control below it, 4px apart (the TS's `-mb-2`).
fn labelled(text: impl Into<SharedString>, control: impl IntoElement, t: &Tokens) -> Div {
    div().flex().flex_col().gap(u(4.)).child(label(text, t)).child(div().flex().child(control))
}

impl Inspector {
    pub(super) fn piece_inspector(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let editor = self.editor.read(cx);
        let project = editor.project()?;
        let piece = project.video.get(index)?.clone();
        let backdrop = project.backdrop.clone();
        let local = local_time(editor, index, cx);
        let t = *cx.tokens();

        // Framing: an added video has no face data, so it shows whole or cropped.
        let added = piece.media.as_ref().is_some_and(|m| !m.is_empty());
        let mut framings: Vec<(&'static str, &'static str)> = Vec::new();
        if !added {
            framings.push(("auto", "Smart"));
        }
        framings.extend([("fit", "Whole"), ("crop", "Crop")]);
        if !added {
            framings.push(("split", "Split"));
        }
        let framing = {
            let editor = self.editor.clone();
            ui::segmented(
                "piece-framing",
                &framing_choice(&piece.framing).to_string(),
                framings.into_iter().map(|(v, l)| (v.to_string(), l.into(), None)).collect(),
                true,
                move |choice, _, cx| {
                    let choice = choice.clone();
                    editor.update(cx, |e, cx| edit_piece(e, index, |item| logic::set_framing(item, &choice), cx));
                },
                &t,
            )
        };

        let split = split_panels(&piece.framing).is_some().then(|| {
            let editor = self.editor.clone();
            let panels = ui::segmented(
                "piece-panels",
                &piece.framing,
                [("split", "2"), ("split3", "3"), ("split4", "4")]
                    .into_iter()
                    .map(|(v, l)| (v.to_string(), l.into(), None))
                    .collect(),
                true,
                move |framing, _, cx| {
                    let framing = framing.clone();
                    editor.update(cx, |e, cx| edit_piece(e, index, |item| item.framing = framing, cx));
                },
                &t,
            );
            div()
                .flex()
                .flex_col()
                .gap(u(12.))
                .child(div().flex().items_center().gap(u(8.)).child(label("Panels", &t)).child(panels))
                .child(hint(
                    "The people on screen longest in this piece, left to right, one per panel. With fewer than two, it shows the whole frame.",
                    &t,
                ))
        });

        let background = self.backdrop_controls(backdrop.as_deref(), window, cx);
        let crop = (piece.framing == "crop").then(|| self.crop_controls(index, &piece, local, window, cx));
        let color = self.color_panel(index, piece.color, window, cx);

        let (speed_min, speed_max) = PIECE_SPEED_RANGE;
        let speed = self.slider(
            "piece-speed",
            "Speed",
            logic::speed_to_slider(piece.speed),
            (logic::speed_to_slider(speed_min), logic::speed_to_slider(speed_max), 1.),
            logic::speed_label(piece.speed),
            // Fades can't outlast the piece at its new speed; keyframes stay on their frames.
            move |e, v, cx| edit_piece(e, index, |item| *item = logic::with_speed(item, logic::slider_to_speed(v)), cx),
            window,
            cx,
        );

        let transition_style = (index > 0).then(|| self.select(Pick::Transition, logic::transition_style(&piece), "Transition style", window, cx));
        let transition_length = {
            let editor = self.editor.clone();
            ui::segmented(
                "piece-transition",
                &logic::transition_choice(piece.transition_ms),
                TRANSITIONS.iter().map(|&ms| (ms, logic::transition_label(ms).into(), None)).collect(),
                true,
                // fit_transitions shortens one that doesn't fit the pieces.
                move |ms, _, cx| {
                    let ms = *ms;
                    editor.update(cx, |e, cx| edit_piece(e, index, |item| item.transition_ms = ms, cx));
                },
                &t,
            )
        };

        let volume = self.slider(
            "piece-volume",
            "Volume",
            clipward_core::timeline::js_round(piece.volume * 100.),
            (0., 200., 5.),
            logic::volume_label(piece.volume),
            move |e, v, cx| edit_piece(e, index, |item| item.volume = v / 100., cx),
            window,
            cx,
        );
        let fades = self.fade_sliders(
            ("piece-fade-in", "piece-fade-out"),
            (piece.fade_in_ms, piece.fade_out_ms),
            item_length(&piece),
            move |e, fade_in, fade_out, cx| {
                edit_piece(
                    e,
                    index,
                    |item| {
                        item.fade_in_ms = fade_in.unwrap_or(item.fade_in_ms);
                        item.fade_out_ms = fade_out.unwrap_or(item.fade_out_ms);
                    },
                    cx,
                )
            },
            window,
            cx,
        );
        let detach = {
            let editor = self.editor.clone();
            Btn::new("piece-detach")
                .small()
                .icon("square-split-vertical")
                .label("Detach audio")
                .tooltip("Move this piece's sound to the sound track, to move or trim it on its own")
                .disabled(piece.volume == 0.)
                .on_click(move |_, _, cx| editor.update(cx, |e, cx| e.detach(index, cx)))
        };

        Some(
            section(&t)
                .child(div().text_label().text_color(t.ink).child(format!("Piece {}", index + 1)))
                .child(labelled("Framing", framing, &t))
                .children(split)
                .child(labelled("Whole-frame background (whole clip)", background, &t))
                .children(crop)
                .child(color)
                .child(speed)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(u(4.))
                        .child(label(if index == 0 { "Fade in from black" } else { "Transition from the previous piece" }, &t))
                        .when_some(transition_style, |d, select| d.child(div().pb(u(8.)).child(select)))
                        .child(div().flex().child(transition_length)),
                )
                .child(volume)
                .children(fades)
                .child(div().flex().child(detach))
                .into_any_element(),
        )
    }

    /// Blur / Black / Colour for the whole clip, with a swatch that opens the picker.
    fn backdrop_controls(&mut self, backdrop: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = *cx.tokens();
        let mode = backdrop_mode(backdrop);
        let editor = self.editor.clone();
        let modes = ui::segmented(
            "piece-backdrop",
            &mode,
            [("blur", "Blur"), ("black", "Black"), ("colour", "Colour")]
                .into_iter()
                .map(|(v, l)| (v, l.into(), None))
                .collect(),
            true,
            move |mode, _, cx| {
                let mode = *mode;
                editor.update(cx, |e, cx| {
                    e.change(
                        |p| {
                            let backdrop = logic::set_backdrop_mode(p.backdrop.as_deref(), mode);
                            (backdrop != p.backdrop).then(|| Project { backdrop, ..p.clone() })
                        },
                        true,
                        cx,
                    )
                });
            },
            &t,
        );
        let colour = (mode == "colour").then(|| backdrop.unwrap_or(logic::DEFAULT_BACKDROP).to_string());
        if colour.is_none() {
            self.picker_open = false;
        }
        if let Some(colour) = &colour
            && !self.picking
        {
            self.picker.update(cx, |p, cx| p.set_color(colour, window, cx));
        }
        let swatch = colour.as_ref().map(|colour| {
            let fill = crate::theme::hex(colour).unwrap_or(t.ink);
            div()
                .id("backdrop-swatch")
                .h(u(28.))
                .w(u(36.))
                .flex_shrink_0()
                .rounded(u(4.))
                .border_1()
                .border_color(if self.picker_open { t.accent } else { t.tint(0.1) })
                .p(u(3.))
                .cursor_pointer()
                .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Background colour").build(window, cx))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.picker_open = !this.picker_open;
                    cx.notify();
                }))
                .child(div().size_full().rounded(u(2.)).bg(fill))
        });
        div()
            .flex()
            .flex_col()
            .gap(u(8.))
            .child(div().flex().items_center().gap(u(8.)).child(modes).children(swatch))
            .when(colour.is_some() && self.picker_open, |d| d.child(self.picker.clone()))
    }

    /// Crop zoom, keyframe buttons and the hint. With keyframes, the zoom sets the one
    /// at the playhead.
    fn crop_controls(&mut self, index: usize, piece: &VideoItem, local: f64, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = *cx.tokens();
        let now = crop_at(piece, local);
        let here = key_here(piece, local).is_some();
        let zoom = self.slider(
            "crop-zoom",
            "Zoom",
            logic::zoom_to_slider(now.zoom),
            (10., MAX_CROP_ZOOM * 10., 1.),
            logic::zoom_label(now.zoom),
            move |e, v, cx| {
                let local = local_time(e, index, cx);
                edit_piece(
                    e,
                    index,
                    |item| {
                        let crop = Crop { zoom: v / 10., ..crop_at(item, local) };
                        logic::set_crop_here(item, local, crop);
                    },
                    cx,
                )
            },
            window,
            cx,
        );
        let keyframe = {
            let editor = self.editor.clone();
            Btn::new("crop-keyframe")
                .small()
                .icon("diamond")
                .label(if here { "Remove keyframe" } else { "Add keyframe" })
                .tooltip("Keyframe the crop here; with two or more, it zooms and pans between them")
                .on_click(move |_, _, cx| {
                    editor.update(cx, |e, cx| {
                        let local = local_time(e, index, cx);
                        edit_piece(e, index, |item| item.keyframes = logic::toggle_crop_key(item, local), cx)
                    })
                })
        };
        let clear = (!piece.keyframes.is_empty()).then(|| {
            let editor = self.editor.clone();
            Btn::new("crop-clear").ghost().small().label("Clear").on_click(move |_, _, cx| {
                editor.update(cx, |e, cx| {
                    let local = local_time(e, index, cx);
                    edit_piece(e, index, |item| logic::clear_crop_keys(item, local), cx)
                })
            })
        });
        div()
            .flex()
            .flex_col()
            .gap(u(12.))
            .child(zoom)
            .child(div().flex().flex_wrap().gap(u(4.)).child(keyframe).children(clear))
            .child(hint(logic::crop_hint(piece.keyframes.len()), &t))
    }

    /// ColorPanel.tsx: a piece's colour, picked from a look and then tuned.
    fn color_panel(&mut self, index: usize, color: Option<clipward_core::clip_engine::render::project::ColorAdjust>, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = *cx.tokens();
        let now = color.unwrap_or(logic::AS_SHOT);
        let looks: Vec<_> = LOOKS.iter().enumerate().map(|(i, &(name, look))| {
            let pressed = logic::look_pressed(color, look);
            let editor = self.editor.clone();
            div()
                .id(("look", i))
                .rounded(u(6.))
                .border_1()
                .px(u(8.))
                .py(u(2.))
                .text_caption()
                .cursor_pointer()
                .map(|d| {
                    if pressed {
                        d.border_color(t.accent).bg(t.selection).text_color(t.ink)
                    } else {
                        d.border_color(t.stroke).text_color(t.ink_secondary).hover(|s| s.bg(t.tint(0.1)))
                    }
                })
                .on_click(move |_, _, cx| {
                    editor.update(cx, |e, cx| edit_piece(e, index, |item| item.color = logic::pick_look(look), cx))
                })
                .child(name)
        }).collect();
        let keys = ["color-brightness", "color-contrast", "color-saturation", "color-temperature"];
        let sliders: Vec<AnyElement> = ColorSlider::ALL
            .into_iter()
            .zip(keys)
            .map(|(slider, key)| {
                self.slider(
                    key,
                    slider.label(),
                    slider.value(&now),
                    slider.range(),
                    slider.display(&now),
                    move |e, v, cx| edit_piece(e, index, |item| item.color = slider.apply(item.color, v), cx),
                    window,
                    cx,
                )
            })
            .collect();
        let editor = self.editor.clone();
        div()
            .flex()
            .flex_col()
            .gap(u(8.))
            .child(label("Color", &t))
            .child(div().flex().flex_wrap().gap(u(4.)).children(looks))
            .children(sliders)
            .child(
                div().flex().child(
                    Btn::new("color-apply-all")
                        .ghost()
                        .small()
                        .icon("palette")
                        .label("Apply to all pieces")
                        .tooltip("Give every piece this colour")
                        .on_click(move |_, _, cx| {
                            editor.update(cx, |e, cx| {
                                e.edit_video(|v| v.iter().map(|p| VideoItem { color, ..p.clone() }).collect(), true, cx)
                            })
                        }),
                ),
            )
    }
}
