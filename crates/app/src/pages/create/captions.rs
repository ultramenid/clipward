//! The caption style picker (CaptionPresetPicker.tsx): 13 looks with live samples.
//!
//! The samples mirror the engine's presets closely enough to choose by: typeface
//! (the engine's own bundled fonts, loaded at first use), colours, stroke, shadow,
//! glow, pill, plate and karaoke sweep. GPUI text has no text-shadow, so each
//! effect is drawn as copies of the line behind the fill: the stroke as 16 copies
//! in a ring, a drop shadow as an offset copy (a soft one as a few faint copies),
//! a halo or glow as faint copies in widening rings.
use super::{
    CreatePage, edit,
    steps::{option_tile, radio_group},
};
use crate::captions::{PRESETS, SCENE, color, sample};
use crate::theme::{ActiveTokens, Tokens};
use crate::ui::{self, Truncate, TypeScale, u};
use clipward_core::job_form::ClipDraft;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

const TILE_MIN: f32 = 104.;
const TILE_GAP: f32 = 8.;

impl CreatePage {
    pub(super) fn render_caption_picker(
        &self,
        draft: &ClipDraft,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t: Tokens = *cx.tokens();
        let disabled = !draft.include_captions;
        let width = self.caption_width.get();
        let columns = if width > 0. {
            (((width + TILE_GAP) / (TILE_MIN + TILE_GAP)).floor() as u16).max(1)
        } else {
            7
        };
        let current = PRESETS
            .iter()
            .position(|p| p.id == draft.caption_preset)
            .unwrap_or(0);
        let d = self.draft.clone();
        let measured = self.caption_width.clone();
        let page = cx.entity().downgrade();
        radio_group(
            "caption-presets",
            &self.radios.caption,
            PRESETS.len(),
            current,
            move |i, _, cx| {
                if !disabled {
                    edit(&d, cx, |d| d.caption_preset = PRESETS[i].id.into())
                }
            },
            &t,
        )
        .relative()
        .grid()
        .grid_cols(columns)
        .gap(u(TILE_GAP))
        // Measure the grid for its auto-fill columns (CSS repeat(auto-fill, minmax(104px, 1fr))).
        .child(
            canvas(
                move |bounds, window, cx| {
                    let css = f32::from(bounds.size.width) / (f32::from(window.rem_size()) / 16.);
                    if (css - measured.get()).abs() > 0.5 {
                        measured.set(css);
                        let _ = page.update(cx, |_, cx| cx.notify());
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .children(PRESETS.iter().enumerate().map(|(i, preset)| {
            let selected = i == current;
            let d = self.draft.clone();
            let focus = self.radios.caption.clone();
            let description = preset.description;
            option_tile(("caption", i), selected, &t)
                .p(u(4.))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(description).build(window, cx)
                })
                .when(!disabled, |tile| {
                    tile.on_click(move |_, window, cx| {
                        window.focus(&focus, cx);
                        edit(&d, cx, |d| d.caption_preset = PRESETS[i].id.into());
                    })
                })
                .child(
                    div()
                        .relative()
                        .flex()
                        .h(u(60.))
                        .items_end()
                        .justify_center()
                        .overflow_hidden()
                        .rounded(u(6.))
                        .px(u(6.))
                        .pb(u(10.))
                        .bg(color(SCENE))
                        .child(sample(preset))
                        .when(selected, |scene| {
                            scene.child(
                                div()
                                    .absolute()
                                    .top(u(6.))
                                    .right(u(6.))
                                    .flex()
                                    .size(u(16.))
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(t.accent)
                                    .child(ui::icon("check").size(u(10.)).text_color(t.accent_ink)),
                            )
                        }),
                )
                .child(
                    div()
                        .px(u(4.))
                        .pb(u(2.))
                        .pt(u(6.))
                        .one_line()
                        .text_caption()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if selected { t.ink } else { t.tint(0.7) })
                        .child(preset.name),
                )
        }))
    }
}
