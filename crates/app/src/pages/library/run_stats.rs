//! The run at a glance (RunStats.tsx): clips, processing time, API cost, created.
//! Cells fade in one after another and their numbers count up, easing out, once;
//! GPUI's animations render their last frame when the OS asks for reduced motion.
use crate::theme::Tokens;
use crate::ui::{self, Hue, Truncate, TypeScale, u};
use clipward_core::format::{cost_parts, format_duration, format_timecode, format_usd};
use clipward_core::job_view::{ApiCosts, JobOutput, is_manual_run};
use clipward_core::library::{count_up, created_labels};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::ops::Range;
use std::time::Duration;

const STAGGER_MS: f32 = 70.;
const COUNT_MS: f32 = 700.;
const FADE_MS: f32 = 200.;

/// Tabular figures, so counting numbers don't jitter.
pub fn tnum() -> FontFeatures {
    FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]))
}

/// A line with some parts in the stronger `Num` colour (text-content/80).
struct Sub {
    text: String,
    nums: Vec<Range<usize>>,
}

impl Sub {
    fn new() -> Self {
        Self {
            text: String::new(),
            nums: Vec::new(),
        }
    }
    fn text(mut self, part: &str) -> Self {
        self.text.push_str(part);
        self
    }
    fn num(mut self, part: &str) -> Self {
        let start = self.text.len();
        self.text.push_str(part);
        self.nums.push(start..self.text.len());
        self
    }
    fn render(self, t: &Tokens) -> StyledText {
        let style = HighlightStyle {
            color: Some(t.tint(0.8)),
            ..Default::default()
        };
        StyledText::new(self.text)
            .with_highlights(self.nums.into_iter().map(|range| (range, style)))
    }
}

/// `value` is what shows while counting, from 0 to 1 through its count; `delay` and
/// the count are in ms from the stats' first frame.
fn cell(
    job_id: &str,
    index: usize,
    icon: &str,
    label: &'static str,
    value: impl Fn(f32) -> String + 'static,
    sub: Option<StyledText>,
    t: &Tokens,
) -> AnyElement {
    let delay = index as f32 * STAGGER_MS;
    let total = delay + COUNT_MS;
    let t = *t;
    div()
        .flex()
        .min_w_0()
        .items_start()
        .gap(u(12.))
        .px(u(16.))
        .py(u(14.))
        .child(ui::icon_tile(icon, Hue::Neutral, 24., &t))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .text_caption()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.ink_tertiary)
                        .child(label),
                )
                .child(
                    div()
                        .id(ElementId::NamedInteger(
                            format!("stat-value-{job_id}").into(),
                            index as u64,
                        ))
                        .mt(u(4.))
                        .text_title()
                        .font_features(tnum())
                        .text_color(t.ink)
                        .one_line()
                        .with_animation(
                            ElementId::NamedInteger(
                                format!("stat-count-{job_id}").into(),
                                index as u64,
                            ),
                            Animation::new(Duration::from_millis(total as u64)),
                            move |d, delta| {
                                let elapsed = delta * total;
                                let progress =
                                    count_up(1.0, elapsed as f64, delay as f64, COUNT_MS as f64);
                                d.child(value(progress as f32))
                            },
                        ),
                )
                .when_some(sub, |d, sub| {
                    d.child(
                        div()
                            .mt(u(4.))
                            .text_caption()
                            .text_color(t.ink_secondary)
                            .one_line()
                            .child(sub),
                    )
                }),
        )
        .with_animation(
            ElementId::NamedInteger(format!("stat-fade-{job_id}").into(), index as u64),
            Animation::new(Duration::from_millis((delay + FADE_MS) as u64)),
            move |d, delta| {
                let elapsed = delta * (delay + FADE_MS);
                let p = ((elapsed - delay) / FADE_MS).clamp(0., 1.);
                let p = 1. - (1. - p).powi(4);
                d.opacity(p).relative().top(px(4. * (1. - p)))
            },
        )
        .into_any_element()
}

/// The 2×2 (4 across from 960px) stats card. `wide` is whether the window is.
pub fn run_stats(
    output: &JobOutput,
    costs: Option<&ApiCosts>,
    video_speed: Option<f64>,
    wide: bool,
    t: &Tokens,
) -> AnyElement {
    let job = output.job_id.as_str();
    let clips = output.total_clips as f64;
    let manual = is_manual_run(output);
    let top_score = output
        .clips
        .iter()
        .map(|c| c.virality_score)
        .fold(0., f64::max)
        * 10.;
    let processing_ms = output.processing_time_seconds * 1000.;

    let clip_sub = (clips > 0.).then(|| {
        let sub = if manual {
            Sub::new().text("Your picks")
        } else {
            Sub::new()
                .text("Top score ")
                .num(&format!("{top_score:.1}"))
        };
        match video_speed {
            Some(speed) => sub.text(" · ").num(&format!("{speed}×")).text(" speed"),
            None => sub,
        }
        .render(t)
    });
    let processing_sub = Sub::new()
        .num(&format_timecode(
            output.source_video_duration_seconds * 1000.,
        ))
        .text(" source")
        .render(t);
    let cost_value = costs.map(|c| (c.total_estimated_cost_usd, c.cost_incomplete == Some(true)));
    let cost_sub = match costs {
        Some(costs) => cost_parts(costs, manual).map(|s| Sub::new().text(&s).render(t)),
        None => Some(Sub::new().text("Not recorded for this run").render(t)),
    };
    let created = output.created_at.as_deref().and_then(created_labels);

    let cells = vec![
        cell(
            job,
            0,
            "clapperboard",
            "Clips",
            move |p| format!("{}", (clips * p as f64).round()),
            clip_sub,
            t,
        ),
        cell(
            job,
            1,
            "timer",
            "Processing",
            move |p| format_duration(processing_ms * p as f64),
            Some(processing_sub),
            t,
        ),
        cell(
            job,
            2,
            "circle-dollar-sign",
            "API cost",
            move |p| match cost_value {
                None => "—".into(),
                Some((total, incomplete)) => {
                    let prefix = if incomplete { "≥ " } else { "" };
                    let shown = if p >= 1. {
                        format_usd(total)
                    } else {
                        format!("${:.2}", total * p as f64)
                    };
                    format!("{prefix}{shown}")
                }
            },
            cost_sub,
            t,
        ),
        cell(
            job,
            3,
            "calendar-clock",
            "Created",
            {
                let time = created
                    .as_ref()
                    .map_or("—".to_string(), |(time, _)| time.clone());
                move |_| time.clone()
            },
            created.map(|(_, day)| Sub::new().text(&day).render(t)),
            t,
        ),
    ];
    let columns = if wide { 4 } else { 2 };
    ui::card(t)
        .grid()
        .grid_cols(columns)
        .overflow_hidden()
        .children(cells.into_iter().enumerate().map(|(i, cell)| {
            let right = if wide { i < 3 } else { i % 2 == 0 };
            let bottom = !wide && i < 2;
            div()
                .min_w_0()
                .border_color(t.stroke)
                .when(right, |d| d.border_r_1())
                .when(bottom, |d| d.border_b_1())
                .child(cell)
        }))
        .into_any_element()
}
