//! The wizard's stepper, step panel, footer and each step's body (JobForm.tsx).
use super::{CreatePage, edit};
use crate::theme::{ActiveTokens, Tokens};
use crate::ui::{self, Btn, Hue, Truncate, TypeScale, u};
use clipward_core::job_form::{
    self, AUTO_DURATION_RANGE, ClipDraft, ClipMode, DURATION_OPTIONS, FORMATS, FormCheck,
    LAYOUT_STYLES, LOGO_POSITION_OPTIONS, LOGO_SIZES, MANUAL_CLIPS_DESCRIPTION, MANUAL_CLIPS_TITLE,
    MAX_BANNER_CHARS, MAX_CLIPS, MAX_LOGOS, MAX_REQUIREMENTS_CHARS, StartedJob,
    VIDEO_SPEED_OPTIONS, WizardStep,
};
use clipward_core::manual_clips::{format_clip_length, format_clip_time};
use clipward_core::shared::{MAX_PARALLEL_JOBS, is_active_job_status};
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub const PROMPT_EXAMPLE: &str = "Example:
- Clips must mention the product name \"Acme\"
- Focus on funny or controversial moments
- 30 to 60 seconds, no swearing
- Hook in the first 3 seconds";

/// Below this window width (CSS px) the stepper shows only the current step's label.
const STEPPER_LABELS_MIN: f32 = 768.;

/// `.eyebrow` label over a group, with an optional aside on the right.
fn group(label: &'static str, aside: Option<String>, t: &Tokens) -> Div {
    div().child(
        div()
            .mb(u(8.))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_caption()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.ink_tertiary)
                    .child(label),
            )
            .when_some(aside, |row, aside| {
                row.child(div().text_caption().text_color(t.ink_tertiary).child(aside))
            }),
    )
}

/// `.tile .tile-hover` / `.tile-selected`: a choosable option.
pub(super) fn option_tile(id: impl Into<ElementId>, selected: bool, t: &Tokens) -> Stateful<Div> {
    let t = *t;
    div()
        .id(id.into())
        .rounded(u(8.))
        .border_1()
        .cursor_pointer()
        .map(|tile| {
            if selected {
                tile.bg(t.accent.opacity(0.1))
                    .border_color(t.accent.opacity(0.7))
                    .text_color(t.ink)
            } else {
                tile.bg(t.tint(0.03))
                    .border_color(t.tint(0.08))
                    .text_color(t.ink_secondary)
                    .hover(move |s| {
                        s.bg(t.tint(0.06))
                            .border_color(t.tint(0.14))
                            .text_color(t.ink)
                    })
            }
        })
}

/// A border of any width (the glyphs use 1.5px).
fn border_w<E: Styled>(mut element: E, width: Pixels) -> E {
    let edges = &mut element.style().border_widths;
    edges.top = Some(width.into());
    edges.right = Some(width.into());
    edges.bottom = Some(width.into());
    edges.left = Some(width.into());
    element
}

/// A radio group the arrow keys, Home and End move through (roving selection).
pub(super) fn radio_group(
    id: &'static str,
    focus: &FocusHandle,
    count: usize,
    current: usize,
    select: impl Fn(usize, &mut Window, &mut App) + 'static,
    t: &Tokens,
) -> Stateful<Div> {
    let ring = t.accent.opacity(0.5);
    div()
        .id(id)
        .track_focus(focus)
        .rounded(u(9.))
        .border_1()
        .border_color(transparent_black())
        .focus_visible(move |s| s.border_color(ring))
        .on_key_down(move |event, window, cx| {
            let next = match event.keystroke.key.as_str() {
                "right" | "down" => (current + 1) % count,
                "left" | "up" => (current + count - 1) % count,
                "home" => 0,
                "end" => count - 1,
                _ => return,
            };
            cx.stop_propagation();
            select(next, window, cx);
        })
}

impl CreatePage {
    pub(super) fn render_stepper(
        &self,
        check: &FormCheck,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = *cx.tokens();
        let scale = f32::from(window.rem_size()) / 16.0;
        let narrow = f32::from(window.viewport_size().width) / scale < STEPPER_LABELS_MIN;
        let reachable = if check.has_source {
            check.steps.len() - 1
        } else {
            0
        };
        let last = check.steps.len() - 1;
        div()
            .flex()
            .items_center()
            .gap(u(4.))
            .children(check.steps.iter().enumerate().map(|(i, step)| {
                let active = i == check.index;
                let done = i < check.index;
                let enabled = i <= reachable;
                let id = step.id;
                let circle = div()
                    .flex()
                    .flex_shrink_0()
                    .size(u(16.))
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .text_micro()
                    .map(|c| {
                        if active {
                            c.bg(t.accent)
                                .text_color(t.accent_ink)
                                .child((i + 1).to_string())
                        } else if done {
                            c.bg(t.tint(0.7))
                                .child(ui::icon("check").size(u(10.)).text_color(t.bg))
                        } else {
                            c.border_1()
                                .border_color(t.tint(0.2))
                                .child((i + 1).to_string())
                        }
                    });
                let button = div()
                    .id(("step", i))
                    .flex()
                    .h(u(28.))
                    .min_w_0()
                    .items_center()
                    .gap(u(6.))
                    .rounded(u(6.))
                    .pl(u(6.))
                    .pr(u(8.))
                    .text_caption()
                    .map(|b| {
                        if active {
                            b.bg(t.selection).text_color(t.ink)
                        } else if enabled {
                            b.text_color(t.ink_secondary)
                                .cursor_pointer()
                                .hover(|s| s.bg(t.tint(0.05)).text_color(t.ink))
                        } else {
                            b.text_color(t.tint(0.25))
                        }
                    })
                    .when(enabled, |b| {
                        b.on_click(cx.listener(move |this, _, _, cx| this.go_to(id, cx)))
                    })
                    .child(circle)
                    .when(active || !narrow, |b| {
                        b.child(div().one_line().child(step.label))
                    });
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(u(4.))
                    .when(i < last, |item| item.flex_1())
                    .child(button)
                    .when(i < last, |item| {
                        item.child(div().h(px(1.)).min_w(u(12.)).flex_1().bg(if done {
                            t.tint(0.3)
                        } else {
                            t.stroke
                        }))
                    })
            }))
    }

    pub(super) fn render_step_panel(
        &mut self,
        draft: &ClipDraft,
        check: &FormCheck,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = *cx.tokens();
        let info = check.steps[check.index];
        let manual = draft.clip_mode == ClipMode::Manual;
        let (title, description) = if check.step == WizardStep::Clips && manual {
            (MANUAL_CLIPS_TITLE, MANUAL_CLIPS_DESCRIPTION)
        } else {
            (info.title, info.description)
        };
        let body = match check.step {
            WizardStep::Video => self.render_video_step(draft, check, window, cx),
            WizardStep::Format => self.render_format_step(draft, cx),
            WizardStep::Clips => self.render_clips_step(draft, check, cx),
            WizardStep::Captions => self.render_captions_step(draft, cx),
            WizardStep::Branding => self.render_branding_step(draft, cx),
            WizardStep::Review => self.render_review_step(draft, check, cx),
        };
        ui::card(&t)
            .child(
                div()
                    .border_b_1()
                    .border_color(t.stroke)
                    .px(u(16.))
                    .py(u(12.))
                    .child(
                        div()
                            .text_label()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.ink)
                            .child(title),
                    )
                    .child(
                        div()
                            .mt(u(2.))
                            .text_caption()
                            .text_color(t.ink_secondary)
                            .child(description),
                    ),
            )
            .child(
                div()
                    .p(u(16.))
                    .when_some(check.source_error, |d, error| {
                        d.child(
                            div()
                                .mb(u(12.))
                                .text_caption()
                                .text_color(t.danger)
                                .child(error),
                        )
                    })
                    .child(body),
            )
    }

    /// Back, the status line and Next / Generate, pinned under the step card.
    pub(super) fn render_footer(
        &self,
        check: &FormCheck,
        blocked: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = *cx.tokens();
        let status = job_form::status_line(check, blocked, ui::MOD_KEY);
        let next = check.next().filter(|_| check.step != WizardStep::Review);
        let generate = self.on_click(cx, |this, _, cx| this.submit(cx));
        div()
            .flex()
            .items_center()
            .gap(u(12.))
            .py(u(12.))
            .border_t_1()
            .border_color(t.stroke)
            .bg(t.bg.opacity(t.glass_strip))
            .child(match check.back() {
                Some(back) => Btn::new("back")
                    .ghost()
                    .icon("arrow-left")
                    .label("Back")
                    .on_click(self.on_click(cx, move |this, _, cx| this.go_to(back.id, cx)))
                    .into_any_element(),
                None => div().into_any_element(),
            })
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .one_line()
                    .text_center()
                    .text_meta()
                    .text_color(t.ink_secondary)
                    .child(status),
            )
            .child(match next {
                Some(next) => div()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .when(check.step != WizardStep::Video, |row| {
                        row.child(
                            Btn::new("generate-now")
                                .ghost()
                                .label("Generate now")
                                .disabled(!check.can_submit)
                                .loading(self.starting)
                                .on_click(generate),
                        )
                    })
                    .child(
                        Btn::new("next")
                            .primary()
                            .label(format!("Next: {}", next.label))
                            .trailing_icon("arrow-right")
                            .disabled(!check.step_valid)
                            .on_click(
                                self.on_click(cx, move |this, _, cx| this.go_to(next.id, cx)),
                            ),
                    )
                    .into_any_element(),
                None => Btn::new("generate")
                    .primary()
                    .large()
                    .icon("sparkles")
                    .label("Generate clips")
                    .disabled(!check.can_submit)
                    .loading(self.starting)
                    .on_click(generate)
                    .into_any_element(),
            })
    }

    fn render_video_step(
        &mut self,
        draft: &ClipDraft,
        check: &FormCheck,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = *cx.tokens();
        let source = self.render_source_picker(draft, self.starting, window, cx);
        let column = div().flex().flex_col().gap(u(12.)).child(source);
        if draft.clip_mode == ClipMode::Manual {
            return column
                .child(div().text_caption().text_color(t.ink_secondary).child(
                    "You’re choosing the moments in the Clips step, so each clip sets its own start and end.",
                ))
                .into_any_element();
        }
        let d = self.draft.clone();
        let trim_error = check.trim.error;
        let mono = |state| {
            Input::new(state)
                .text_size(u(13.))
                .font_family(crate::MONO_FONT)
                .when(trim_error.is_some(), |i| {
                    i.border_color(t.danger.opacity(0.6))
                })
        };
        column
            .child(ui::setting_row(
                "Clip only part of the video",
                Some("Set a start and end time. Leave either empty for an open range.".into()),
                ui::switch(
                    "trim",
                    draft.trim_open,
                    false,
                    move |on, _, cx| edit(&d, cx, |d| d.trim_open = on),
                    &t,
                ),
                false,
                &t,
            ))
            .when(draft.trim_open, |column| {
                column.child(
                    div()
                        .child(
                            div()
                                .flex()
                                .gap(u(8.))
                                .child(div().flex_1().child(mono(&self.inputs.trim_start)))
                                .child(div().flex_1().child(mono(&self.inputs.trim_end))),
                        )
                        .child(
                            div()
                                .mt(u(6.))
                                .text_caption()
                                .text_color(if trim_error.is_some() {
                                    t.danger
                                } else {
                                    t.ink_secondary
                                })
                                .child(trim_error.unwrap_or("Use seconds (90) or mm:ss (1:30).")),
                        ),
                )
            })
            .into_any_element()
    }

    /// The AI's prompt, under "Let AI decide". The planner treats every line as a hard rule.
    fn render_prompt(&self, draft: &ClipDraft, t: &Tokens) -> Div {
        let length = draft.requirements.encode_utf16().count();
        div()
            .mt(u(8.))
            .flex()
            .flex_col()
            .gap(u(8.))
            .child(Textarea::new(&self.inputs.prompt).text_size(u(13.)))
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(u(12.))
                    .text_caption()
                    .child(div().max_w(u(470.)).text_color(t.ink_secondary).child(
                        "Tell the AI what you want, or paste a campaign’s rules. Leave empty for the most viral moments.",
                    ))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(if length > MAX_REQUIREMENTS_CHARS { t.danger } else { t.ink_secondary })
                            .child(format!("{length}/{MAX_REQUIREMENTS_CHARS}")),
                    ),
            )
    }

    fn render_format_step(&self, draft: &ClipDraft, cx: &mut Context<Self>) -> AnyElement {
        let t = *cx.tokens();
        let manual = draft.clip_mode == ClipMode::Manual;

        let format_index = FORMATS
            .iter()
            .position(|(id, _, _)| *id == draft.aspect_ratio)
            .unwrap_or(0);
        let d = self.draft.clone();
        let formats = radio_group(
            "formats",
            &self.radios.format,
            FORMATS.len(),
            format_index,
            move |i, _, cx| edit(&d, cx, |d| d.aspect_ratio = FORMATS[i].0.into()),
            &t,
        )
        .grid()
        .grid_cols(2)
        .gap(u(8.))
        .children(FORMATS.iter().enumerate().map(|(i, (id, label, hint))| {
            let selected = i == format_index;
            let (w, h) = if *id == "9:16" {
                (12., 21.)
            } else {
                (24., 14.)
            };
            let d = self.draft.clone();
            let focus = self.radios.format.clone();
            option_tile(("format", i), selected, &t)
                .flex()
                .items_center()
                .gap(u(12.))
                .px(u(12.))
                .py(u(10.))
                .on_click(move |_, window, cx| {
                    window.focus(&focus, cx);
                    edit(&d, cx, |d| d.aspect_ratio = (*id).into());
                })
                .child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .w(u(28.))
                        .h(u(24.))
                        .items_center()
                        .justify_center()
                        .child(
                            border_w(div(), px(1.5))
                                .w(u(w))
                                .h(u(h))
                                .rounded(u(3.))
                                .border_color(if selected { t.accent } else { t.tint(0.35) })
                                .bg(if selected {
                                    t.accent.opacity(0.25)
                                } else {
                                    t.tint(0.05)
                                }),
                        ),
                )
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
                                .child(*label)
                                .child(div().text_meta().text_color(t.ink_secondary).child(*id)),
                        )
                        .child(
                            div()
                                .one_line()
                                .text_caption()
                                .text_color(t.ink_secondary)
                                .child(*hint),
                        ),
                )
        }));

        let framing = (draft.aspect_ratio == "9:16").then(|| {
            let style_index = LAYOUT_STYLES
                .iter()
                .position(|(id, _, _)| *id == draft.layout_style)
                .unwrap_or(0);
            let d = self.draft.clone();
            let tiles = radio_group(
                "framing",
                &self.radios.framing,
                LAYOUT_STYLES.len(),
                style_index,
                move |i, _, cx| edit(&d, cx, |d| d.layout_style = LAYOUT_STYLES[i].0.into()),
                &t,
            )
            .grid()
            .grid_cols(3)
            .gap(u(8.))
            .children(LAYOUT_STYLES.iter().enumerate().map(|(i, (id, label, hint))| {
                let selected = i == style_index;
                let d = self.draft.clone();
                let focus = self.radios.framing.clone();
                option_tile(("framing", i), selected, &t)
                    .flex()
                    .items_center()
                    .gap(u(10.))
                    .px(u(12.))
                    .py(u(10.))
                    .on_click(move |_, window, cx| {
                        window.focus(&focus, cx);
                        edit(&d, cx, |d| d.layout_style = (*id).into());
                    })
                    .child(framing_glyph(id, selected, &t))
                    .child(
                        div()
                            .min_w_0()
                            .child(div().text_label().child(*label))
                            .child(div().one_line().text_caption().text_color(t.ink_secondary).child(*hint)),
                    )
            }));
            let d = self.draft.clone();
            group("Framing", None, &t)
                .child(tiles)
                .when(draft.layout_style == "auto" && manual, |g| {
                    g.child(div().mt(u(8.)).text_caption().text_color(t.ink_secondary).child(
                        "Smart framing runs on this computer. The AI vision check is off when you choose the moments.",
                    ))
                })
                .when(draft.layout_style == "auto" && !manual, |g| {
                    g.child(div().mt(u(8.)).child(ui::setting_row(
                        "Check tricky shots with AI vision",
                        Some("Checks uncertain shots with your API-key provider. CLI providers skip this.".into()),
                        ui::switch(
                            "vision",
                            draft.layout_vision,
                            false,
                            move |on, _, cx| edit(&d, cx, |d| d.layout_vision = on),
                            &t,
                        ),
                        false,
                        &t,
                    )))
                })
        });

        let d = self.draft.clone();
        let pacing = group("Pacing", None, &t).child(ui::setting_row(
            "Cut dead air",
            Some("Trims long pauses and filler words, keeps demos.".into()),
            ui::switch(
                "pacing",
                draft.pacing == "tight",
                false,
                move |on, _, cx| {
                    edit(&d, cx, |d| {
                        d.pacing = if on { "tight" } else { "natural" }.into()
                    })
                },
                &t,
            ),
            false,
            &t,
        ));

        let speed_index = VIDEO_SPEED_OPTIONS
            .iter()
            .position(|s| *s == draft.video_speed)
            .unwrap_or(0);
        let d = self.draft.clone();
        let speeds = radio_group(
            "speeds",
            &self.radios.speed,
            VIDEO_SPEED_OPTIONS.len(),
            speed_index,
            move |i, _, cx| edit(&d, cx, |d| d.video_speed = VIDEO_SPEED_OPTIONS[i]),
            &t,
        )
        .grid()
        .grid_cols(6)
        .gap(u(6.))
        .children(VIDEO_SPEED_OPTIONS.iter().enumerate().map(|(i, speed)| {
            let speed = *speed;
            let d = self.draft.clone();
            let focus = self.radios.speed.clone();
            option_tile(("speed", i), i == speed_index, &t)
                .px(u(8.))
                .py(u(8.))
                .flex()
                .flex_col()
                .items_center()
                .on_click(move |_, window, cx| {
                    window.focus(&focus, cx);
                    edit(&d, cx, |d| d.video_speed = speed);
                })
                .child(div().text_label().child(job_form::speed_label(speed)))
                .child(
                    div()
                        .text_meta()
                        .text_color(t.ink_secondary)
                        .child(job_form::speed_hint(speed)),
                )
        }));

        div()
            .flex()
            .flex_col()
            .gap(u(16.))
            .child(group("Format", None, &t).child(formats))
            .children(framing)
            .child(pacing)
            .child(
                group("Video speed", Some("All clips in this job".into()), &t)
                    .child(speeds)
                    .child(div().mt(u(8.)).text_caption().text_color(t.ink_secondary).child(
                        "Speeds up every exported clip, keeping voice pitch natural and captions in sync. Faster clips are shorter.",
                    )),
            )
            .into_any_element()
    }

    fn render_clips_step(
        &mut self,
        draft: &ClipDraft,
        check: &FormCheck,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = *cx.tokens();
        let manual = draft.clip_mode == ClipMode::Manual;
        let d = self.draft.clone();
        let mode_picker = div()
            .flex()
            .flex_col()
            .items_start()
            .gap(u(6.))
            .child(ui::segmented(
                "clip-mode",
                &draft.clip_mode,
                vec![
                    (ClipMode::Ai, "AI picks the moments".into(), None),
                    (ClipMode::Manual, "I’ll choose the moments".into(), None),
                ],
                false,
                move |mode, _, cx| {
                    let mode = *mode;
                    edit(&d, cx, |d| d.clip_mode = mode)
                },
                &t,
            ))
            .child(div().text_caption().text_color(t.ink_secondary).child(if manual {
                "No AI provider needed. The prompt, clip length and count, and AI vision are skipped."
            } else {
                "Your AI provider finds the moments that fit the lengths below."
            }));
        if manual {
            return div()
                .flex()
                .flex_col()
                .gap(u(16.))
                .child(mode_picker)
                .child(self.render_manual_list(draft, &check.manual_check, cx))
                .into_any_element();
        }

        let auto = draft.durations.is_empty();
        let d = self.draft.clone();
        let length_tile = |id: usize, range: &'static str, label: &'static str, selected: bool| {
            option_tile(("length", id), selected, &t)
                .px(u(4.))
                .py(u(6.))
                .flex()
                .flex_col()
                .items_center()
                .min_w_0()
                .child(
                    div()
                        .text_caption()
                        .font_weight(FontWeight::MEDIUM)
                        .child(range),
                )
                .child(
                    div()
                        .w_full()
                        .text_center()
                        .one_line()
                        .text_meta()
                        .text_color(t.ink_secondary)
                        .child(label),
                )
        };
        let lengths = div()
            .grid()
            .grid_cols(8)
            .gap(u(6.))
            .child(
                length_tile(0, AUTO_DURATION_RANGE, "Auto", auto)
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(
                            "The AI picks each clip's length to fit its moment",
                        )
                        .build(window, cx)
                    })
                    .on_click(move |_, _, cx| edit(&d, cx, |d| d.durations.clear())),
            )
            .children(
                DURATION_OPTIONS
                    .iter()
                    .enumerate()
                    .map(|(i, (id, label, range))| {
                        let d = self.draft.clone();
                        let id = *id;
                        length_tile(i + 1, range, label, draft.durations.iter().any(|x| x == id))
                            .on_click(move |_, _, cx| {
                                edit(&d, cx, |d| {
                                    if let Some(at) = d.durations.iter().position(|x| x == id) {
                                        d.durations.remove(at);
                                    } else {
                                        d.durations.push(id.into());
                                    }
                                })
                            })
                    }),
            );
        let aside = if auto {
            "Auto".to_string()
        } else {
            format!("{} selected", draft.durations.len())
        };

        let d = self.draft.clone();
        let count = draft.max_clips;
        let step_by = |delta: i64| {
            self.on_click(cx, move |this, window, cx| {
                let next = (this.draft.read(cx).value.max_clips as i64 + delta)
                    .clamp(1, MAX_CLIPS as i64) as u32;
                edit(&this.draft, cx, |d| d.max_clips = next);
                this.inputs
                    .max_clips
                    .update(cx, |s, cx| s.set_value(next.to_string(), window, cx));
            })
        };
        let stepper = div()
            .mt(u(8.))
            .flex()
            .max_w(u(320.))
            .items_center()
            .gap(u(8.))
            .child(
                Btn::new("fewer")
                    .icon("minus")
                    .tooltip("Fewer clips")
                    .disabled(count <= 1)
                    .on_click(step_by(-1)),
            )
            .child(
                div().flex_1().child(
                    Input::new(&self.inputs.max_clips)
                        .text_size(u(13.))
                        .font_family(crate::MONO_FONT)
                        .text_center(),
                ),
            )
            .child(
                Btn::new("more")
                    .icon("plus")
                    .tooltip("More clips")
                    .disabled(count >= MAX_CLIPS)
                    .on_click(step_by(1)),
            );

        div()
            .flex()
            .flex_col()
            .gap(u(16.))
            .child(mode_picker)
            .child(
                group("Clip length", Some(aside), &t)
                    .child(lengths)
                    .when_some(job_form::speed_length_note(draft.video_speed), |g, note| {
                        g.child(
                            div()
                                .mt(u(8.))
                                .text_caption()
                                .text_color(t.ink_secondary)
                                .child(note),
                        )
                    }),
            )
            .child(
                group("Number of clips", None, &t)
                    .child(ui::setting_row(
                        "Let AI decide",
                        Some("Tell it what you want; it cuts every moment that fits.".into()),
                        ui::switch(
                            "auto-count",
                            draft.auto_clip_count,
                            false,
                            move |on, _, cx| edit(&d, cx, |d| d.auto_clip_count = on),
                            &t,
                        ),
                        false,
                        &t,
                    ))
                    .map(|g| {
                        if draft.auto_clip_count {
                            g.child(self.render_prompt(draft, &t))
                        } else {
                            g.child(stepper)
                        }
                    }),
            )
            .into_any_element()
    }

    fn render_captions_step(&self, draft: &ClipDraft, cx: &mut Context<Self>) -> AnyElement {
        let t = *cx.tokens();
        let d = self.draft.clone();
        div()
            .flex()
            .flex_col()
            .gap(u(12.))
            .child(ui::setting_row(
                "Burn in captions",
                Some("Turn off for clean clips without on-screen text.".into()),
                ui::switch(
                    "captions",
                    draft.include_captions,
                    false,
                    move |on, _, cx| edit(&d, cx, |d| d.include_captions = on),
                    &t,
                ),
                false,
                &t,
            ))
            .child(
                div()
                    .when(!draft.include_captions, |d| d.opacity(0.35))
                    .child(self.render_caption_picker(draft, cx)),
            )
            .into_any_element()
    }

    /// Channel link and logos stamped on every clip.
    fn render_branding_step(&self, draft: &ClipDraft, cx: &mut Context<Self>) -> AnyElement {
        let t = *cx.tokens();
        let too_long = clipward_core::format::js_trim(&draft.banner_url)
            .chars()
            .count()
            > MAX_BANNER_CHARS;
        let logos = draft.logos.iter().enumerate().map(|(i, logo)| {
            let name = clipward_core::format::basename(&logo.path).to_string();
            let d = self.draft.clone();
            let position = ui::segmented(
                ("logo-position", i),
                &LOGO_POSITION_OPTIONS
                    .iter()
                    .map(|(v, _)| *v)
                    .find(|v| *v == logo.position)
                    .unwrap_or("top-right"),
                LOGO_POSITION_OPTIONS
                    .iter()
                    .map(|(v, l)| (*v, SharedString::from(*l), None))
                    .collect(),
                true,
                move |value, _, cx| {
                    let value = value.to_string();
                    edit(&d, cx, |d| d.logos[i].position = value)
                },
                &t,
            );
            let d = self.draft.clone();
            let size = ui::segmented(
                ("logo-size", i),
                &logo.size,
                LOGO_SIZES
                    .iter()
                    .map(|(v, l)| (*v, SharedString::from(*l), None))
                    .collect(),
                true,
                move |value, _, cx| {
                    let value = *value;
                    edit(&d, cx, |d| d.logos[i].size = value)
                },
                &t,
            );
            let d = self.draft.clone();
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(u(8.))
                .px(u(8.))
                .py(u(8.))
                .when(i > 0, |row| row.border_t_1().border_color(t.stroke))
                .child(
                    img(std::path::PathBuf::from(&logo.path))
                        .flex_shrink_0()
                        .size(u(32.))
                        .rounded(u(4.))
                        .object_fit(ObjectFit::Contain),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .one_line()
                        .text_body()
                        .text_color(t.ink)
                        .child(name.clone()),
                )
                .child(position)
                .child(size)
                .child(
                    Btn::new(("remove-logo", i))
                        .ghost()
                        .small()
                        .icon("x")
                        .tooltip(format!("Remove {name}"))
                        .on_click(move |_, _, cx| {
                            edit(&d, cx, |d| {
                                d.logos.remove(i);
                            })
                        }),
                )
        });
        let has_logos = !draft.logos.is_empty();
        let link_help = if too_long {
            format!("Use at most {MAX_BANNER_CHARS} characters.")
        } else {
            "Shown in capitals at the bottom of every clip. Leave empty for none.".into()
        };
        let logo_help = self.logo_error.clone().unwrap_or_else(|| {
            format!("PNG, JPEG or WebP, up to {MAX_LOGOS}. Use a transparent PNG for a clean edge.")
        });
        div()
            .flex()
            .flex_col()
            .gap(u(16.))
            .child(
                group("Channel link", None, &t)
                    .child(
                        Input::new(&self.inputs.banner)
                            .text_size(u(13.))
                            .prefix(ui::icon("link-2").size(u(14.)).text_color(t.ink_tertiary))
                            .when(too_long, |i| i.border_color(t.danger.opacity(0.6))),
                    )
                    .child(
                        div()
                            .mt(u(6.))
                            .text_caption()
                            .text_color(if too_long { t.danger } else { t.ink_secondary })
                            .child(link_help),
                    ),
            )
            .child(
                group(
                    "Logos",
                    Some(format!("{} of {MAX_LOGOS}", draft.logos.len())),
                    &t,
                )
                .when(has_logos, |g| {
                    g.child(
                        div()
                            .mb(u(8.))
                            .rounded(u(8.))
                            .border_1()
                            .border_color(t.tint(0.1))
                            .bg(t.tint(0.03))
                            .children(logos),
                    )
                })
                .child(
                    div().flex().child(
                        Btn::new("add-logos")
                            .icon("image-plus")
                            .label(if has_logos { "Add more" } else { "Add logos" })
                            .disabled(draft.logos.len() >= MAX_LOGOS)
                            .on_click(self.on_click(cx, |this, _, cx| this.add_logos(cx))),
                    ),
                )
                .child(
                    div()
                        .mt(u(6.))
                        .text_caption()
                        .text_color(if self.logo_error.is_some() {
                            t.danger
                        } else {
                            t.ink_secondary
                        })
                        .child(logo_help),
                ),
            )
            .into_any_element()
    }

    fn add_logos(&mut self, cx: &mut Context<Self>) {
        self.logo_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let picked = crate::shell::pick_logos(cx).await;
            let _ = this.update(cx, |this, cx| {
                match picked {
                    Ok(paths) => edit(&this.draft, cx, |d| {
                        d.logos = job_form::add_logos(&d.logos, &paths)
                    }),
                    Err(_) => this.logo_error = Some("Could not add the logo.".into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_review_step(
        &self,
        draft: &ClipDraft,
        check: &FormCheck,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = *cx.tokens();
        let manual = draft.clip_mode == ClipMode::Manual;
        let (provider, model) = self
            .settings
            .read(cx)
            .value
            .as_ref()
            .map(|s| {
                // "sonnet · Low effort" (Antigravity's effort is already in its model id).
                let effort = clipward_core::clip_engine::llm::effort_levels(&s.ai_provider)
                    .contains(&s.ai_effort.as_str())
                    .then(|| format!("{} effort", clipward_core::provider_catalog::effort_label(&s.ai_effort)));
                let model = [Some(s.ai_model.clone()).filter(|m| !m.is_empty()), effort];
                (s.ai_provider.clone(), model.into_iter().flatten().collect::<Vec<_>>().join(" · "))
            })
            .unwrap_or_default();
        let (active, running) = {
            let jobs = self.jobs.read(cx);
            let active = jobs
                .jobs
                .values()
                .filter(|j| is_active_job_status(&j.status));
            let running = active.clone().filter(|j| j.status != "queued").count();
            (active.count(), running)
        };
        let provider_label = clipward_core::provider_catalog::provider_label(&provider);
        let context = job_form::ReviewContext {
            caption_name: crate::captions::preset_name(&draft.caption_preset),
            provider_label: &provider_label,
            model: &model,
        };
        let rows = job_form::review_rows(draft, check, &context);
        let note = job_form::queue_note(running, active, manual, &provider);
        div()
            .flex()
            .flex_col()
            .gap(u(12.))
            .child(
                div()
                    .rounded(u(8.))
                    .overflow_hidden()
                    .border_1()
                    .border_color(t.tint(0.1))
                    .bg(t.tint(0.03))
                    .children(rows.into_iter().enumerate().map(|(i, row)| {
                        let step = row.step;
                        div()
                            .flex()
                            .items_center()
                            .gap(u(12.))
                            .py(u(6.))
                            .pl(u(12.))
                            .pr(u(6.))
                            .when(i > 0, |r| r.border_t_1().border_color(t.stroke))
                            .child(
                                div()
                                    .w(u(80.))
                                    .flex_shrink_0()
                                    .text_caption()
                                    .text_color(t.ink_secondary)
                                    .child(row.label),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .one_line()
                                    .text_body()
                                    .text_color(t.ink)
                                    .child(row.value),
                            )
                            .child(
                                Btn::new(("edit", i))
                                    .ghost()
                                    .small()
                                    .label("Edit")
                                    .tooltip(format!("Edit {}", row.label.to_lowercase()))
                                    .on_click(
                                        self.on_click(cx, move |this, _, cx| this.go_to(step, cx)),
                                    ),
                            )
                    })),
            )
            .when(manual, |col| col.child(manual_summary(draft, check, &t)))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(u(8.))
                    .text_caption()
                    .text_color(t.ink_secondary)
                    .child(
                        div()
                            .mt(px(1.))
                            .child(ui::icon("clock-3").size(u(14.)).text_color(t.ink_secondary)),
                    )
                    .child(note),
            )
            .into_any_element()
    }

    pub(super) fn render_started(
        &self,
        started: &StartedJob,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = *cx.tokens();
        let job_id = started.job_id.clone();
        ui::card(&t)
            .flex()
            .flex_col()
            .items_center()
            .px(u(20.))
            .py(u(32.))
            .child(ui::icon_tile("circle-check-big", Hue::Success, 40., &t))
            .child(
                div()
                    .mt(u(12.))
                    .text_heading()
                    .text_color(t.ink)
                    .child(if started.queued { "Queued" } else { "Clipping started" }),
            )
            .child(
                div()
                    .mt(u(4.))
                    .max_w(u(448.))
                    .one_line()
                    .text_body()
                    .text_color(t.ink_secondary)
                    .child(clipward_core::format::source_label(&started.source)),
            )
            .child(
                div()
                    .mt(u(4.))
                    .max_w(u(448.))
                    .text_center()
                    .text_caption()
                    .text_color(t.ink_secondary)
                    .child(if started.queued {
                        format!("Up to {MAX_PARALLEL_JOBS} jobs run at once. This one starts as soon as a slot frees up.")
                    } else {
                        "It keeps running while you queue more videos or use the rest of Clipward.".into()
                    }),
            )
            .child(
                div()
                    .mt(u(16.))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_center()
                    .gap(u(6.))
                    .child(
                        Btn::new("another")
                            .primary()
                            .icon("plus")
                            .label("Clip another video")
                            .on_click(self.on_click(cx, |this, window, cx| this.start_another(window, cx))),
                    )
                    .child(
                        Btn::new("view-job")
                            .icon("list-video")
                            .label("View job")
                            .on_click(self.on_click(cx, move |this, window, cx| {
                                this.view_job(job_id.clone(), window, cx)
                            })),
                    ),
            )
    }
}

/// Tiny 9:16 diagram of a framing style: split panels, full bleed, or letterbox.
fn framing_glyph(style: &str, selected: bool, t: &Tokens) -> Div {
    let fill = if selected {
        t.accent.opacity(0.8)
    } else {
        t.tint(0.35)
    };
    let block = || div().rounded(px(1.)).bg(fill);
    border_w(div(), px(1.5))
        .flex()
        .flex_col()
        .flex_shrink_0()
        .gap(px(1.))
        .w(u(14.))
        .h(u(24.))
        .p(px(1.))
        .overflow_hidden()
        .rounded(u(3.))
        .border_color(if selected { t.accent } else { t.tint(0.35) })
        .map(|glyph| match style {
            "auto" => glyph
                .child(block().flex_1())
                .child(block().flex_1().opacity(0.6)),
            "fill" => glyph.child(block().flex_1()),
            _ => glyph.justify_center().child(block().h(u(6.))),
        })
}

/// The typed clips, in order, as Review lists them.
fn manual_summary(draft: &ClipDraft, check: &FormCheck, t: &Tokens) -> impl IntoElement {
    let rows = draft
        .manual_clips
        .iter()
        .zip(&check.manual_check.rows)
        .enumerate()
        .map(|(i, (row, rc))| {
            let title = match clipward_core::format::js_trim(&row.title) {
                "" => format!("Clip {}", i + 1),
                title => title.to_string(),
            };
            let range = if rc.error.is_some() {
                "–".to_string()
            } else {
                format!(
                    "{} – {}",
                    format_clip_time(rc.start.unwrap_or(0.0)),
                    format_clip_time(rc.end.unwrap_or(0.0))
                )
            };
            let length = match (&rc.error, rc.length) {
                (None, Some(length)) => format_clip_length(length),
                _ => String::new(),
            };
            div()
                .flex()
                .items_center()
                .gap(u(12.))
                .py(u(6.))
                .px(u(12.))
                .when(i > 0, |r| r.border_t_1().border_color(t.stroke))
                .child(
                    div()
                        .w(u(20.))
                        .flex_shrink_0()
                        .text_right()
                        .text_caption()
                        .text_color(t.ink_tertiary)
                        .child((i + 1).to_string()),
                )
                .child(
                    div()
                        .w(u(128.))
                        .flex_shrink_0()
                        .font_family(crate::MONO_FONT)
                        .text_caption()
                        .text_color(t.ink)
                        .child(range),
                )
                .child(
                    div()
                        .w(u(56.))
                        .flex_shrink_0()
                        .text_right()
                        .text_caption()
                        .text_color(t.ink_secondary)
                        .child(length),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .one_line()
                        .text_body()
                        .text_color(if rc.error.is_some() { t.danger } else { t.ink })
                        .child(rc.error.clone().unwrap_or(title)),
                )
        });
    div()
        .child(
            div()
                .mb(u(6.))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_caption()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.ink_tertiary)
                        .child("Clips to cut"),
                )
                .when(!check.manual_check.valid, |r| {
                    r.child(
                        div()
                            .text_caption()
                            .text_color(t.danger)
                            .child("Some clips need fixing"),
                    )
                }),
        )
        .child(
            div()
                .id("manual-summary")
                .max_h(u(256.))
                .overflow_y_scroll()
                .rounded(u(8.))
                .border_1()
                .border_color(t.tint(0.1))
                .bg(t.tint(0.03))
                .children(rows),
        )
}
