//! The live parts of JobProgress.tsx: the ring with its stage, step line and
//! chips, and the stepper. Each is its own entity drawn from the view cache, so
//! the arcade's frames and the stepper's pulse don't redraw the ring, and the
//! ring's elapsed timer doesn't redraw the stepper.
use super::JobsPage;
use super::measured::Measured;
use crate::models::Jobs;
use crate::theme::{ActiveTokens, Tokens};
use crate::ui::{self, Truncate, TypeScale, u};
use clipward_core::format::format_timecode;
use clipward_core::jobs::JobSnapshot;
use clipward_core::jobs_page::{self as logic, cubic_bezier};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use std::time::{Duration, Instant};

/// Tailwind's `ease-out`, which the ring and the step connectors transition with.
fn ease_out() -> impl Fn(f32) -> f32 + Copy {
    cubic_bezier(0.0, 0.0, 0.2, 1.0)
}

const TRANSITION: Duration = Duration::from_millis(700);

/// The ring's `transition-[stroke-dashoffset] duration-700 ease-out`: each new
/// value eases in from wherever the ring was.
struct Tween {
    from: f64,
    to: f64,
    since: Instant,
    /// Keys the animation, so each new value starts a fresh one.
    key: usize,
}

impl Tween {
    fn new(value: f64) -> Self {
        Self { from: value, to: value, since: Instant::now(), key: 0 }
    }

    fn now(&self) -> f64 {
        let p = (self.since.elapsed().as_secs_f32() / TRANSITION.as_secs_f32()).min(1.0);
        self.from + (self.to - self.from) * ease_out()(p) as f64
    }

    fn retarget(&mut self, to: f64) {
        if to != self.to {
            self.from = self.now();
            self.to = to;
            self.since = Instant::now();
            self.key += 1;
        }
    }
}

/// Small hairline chip for a single fact under the progress ring.
fn info_chip(icon: &str, text: String, t: &Tokens) -> Div {
    div()
        .flex()
        .h(u(24.))
        .items_center()
        .gap(u(6.))
        .rounded(u(6.))
        .border_1()
        .border_color(t.tint(0.1))
        .px(u(8.))
        .text_caption()
        .text_color(t.ink_secondary)
        .child(ui::icon(icon).size(u(14.)).text_color(t.ink_tertiary))
        .child(text)
}

/// A progress ring with content centered in it (ProgressRing): round-capped, and
/// its value eased by the caller.
pub(super) fn ring(percent: f64, diameter: f32, stroke: f32, t: &Tokens, inner: impl FnOnce(Div) -> Div) -> Div {
    let fraction = (percent / 100.0).clamp(0.0, 1.0) as f32;
    let accent = t.accent;
    div()
        .relative()
        .flex_shrink_0()
        .size(u(diameter))
        .child(ui::progress_ring(percent, diameter, stroke, t).absolute())
        // strokeLinecap="round": a dot on each end of the arc.
        .when(fraction > 0.0, |d| {
            d.child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| {
                        let width = f32::from(bounds.size.width) / diameter * stroke;
                        let center = bounds.center();
                        let radius = f32::from(bounds.size.width.min(bounds.size.height)) / 2.0 - width / 2.0;
                        let r = px(width / 2.0);
                        for end in [0.0, fraction] {
                            let angle = end * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                            let c = point(center.x + px(radius * angle.cos()), center.y + px(radius * angle.sin()));
                            window.paint_quad(
                                fill(Bounds::new(point(c.x - r, c.y - r), size(r * 2.0, r * 2.0)), accent).corner_radii(r),
                            );
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
        })
        .child(inner(div().absolute().size_full().flex().flex_col().items_center().justify_center()))
}

/// The ring, its stage, the engine's step line and the fact chips.
pub(super) struct ProgressCore {
    job_id: String,
    page: WeakEntity<JobsPage>,
    jobs: Entity<Jobs>,
    revision: Option<u64>,
    ring: Tween,
    pub measured: Measured,
    _subscription: Subscription,
    _clock: Task<()>,
}

impl ProgressCore {
    pub fn new(job_id: String, page: WeakEntity<JobsPage>, cx: &mut Context<Self>) -> Self {
        let jobs = Jobs::global(cx);
        let (revision, percent) = jobs
            .read(cx)
            .jobs
            .get(&job_id)
            .map_or((None, 0.0), |job| (Some(job.revision), job.percent));
        let subscription = cx.observe(&jobs, |this: &mut Self, jobs, cx| {
            let job = jobs.read(cx).jobs.get(&this.job_id);
            let revision = job.map(|job| job.revision);
            if revision != this.revision {
                this.revision = revision;
                if let Some(job) = job {
                    let percent = job.percent;
                    this.ring.retarget(percent);
                }
                cx.notify();
            }
        });
        // The elapsed/waiting chip ticks each second; only this view redraws.
        let clock = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        Self {
            job_id,
            page,
            jobs,
            revision,
            ring: Tween::new(percent),
            measured: Measured::default(),
            _subscription: subscription,
            _clock: clock,
        }
    }

    fn content(&self, job: &JobSnapshot, t: &Tokens) -> Div {
        let queued = job.status == "queued";
        let elapsed = logic::elapsed_ms(logic::timer_start(job), logic::now_ms());
        let pct = logic::whole_percent(job.percent);
        let stage = logic::stage_label(&job.status);
        let (from, to, key) = (self.ring.from, self.ring.to, self.ring.key);
        let t2 = *t;
        let ring = div().with_animation(
            ("ring", key),
            Animation::new(TRANSITION).with_easing(ease_out()),
            move |d, delta| {
                let value = from + (to - from) * delta as f64;
                d.child(ring(value, 160., 6., &t2, |d| {
                    d.child(
                        div()
                            .flex()
                            .items_baseline()
                            .text_display()
                            .text_color(t2.ink)
                            .child(format!("{pct}"))
                            .child(
                                div()
                                    .ml(u(2.))
                                    .text_heading()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(t2.ink_tertiary)
                                    .child("%"),
                            ),
                    )
                    .child(
                        div()
                            .mt(u(6.))
                            .text_caption()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t2.accent_fg)
                            .child(stage),
                    )
                }))
            },
        );
        let speed = job.request.video_speed;
        div()
            .flex()
            .flex_col()
            .items_center()
            .child(ring)
            .child(
                div()
                    .mt(u(16.))
                    .h(u(20.))
                    .max_w_full()
                    .text_center()
                    .text_body()
                    .text_color(t.ink_secondary)
                    .one_line()
                    .child(job.step.clone()),
            )
            .child(
                div()
                    .mt(u(12.))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_center()
                    .gap(u(6.))
                    .when(speed > 1.0, |d| d.child(info_chip("gauge", format!("{speed}× export speed"), t)))
                    .when(job.clips_total > 0, |d| {
                        d.child(info_chip("clapperboard", logic::clips_rendered(job.clips_done, job.clips_total), t))
                    })
                    .child(info_chip(
                        "clock-3",
                        format!("{} {}", format_timecode(elapsed), if queued { "waiting" } else { "elapsed" }),
                        t,
                    )),
            )
    }
}

impl Render for ProgressCore {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.tokens();
        let job = self.jobs.read(cx).jobs.get(&self.job_id).cloned();
        let content = job.map(|job| self.content(&job, &t));
        self.measured.wrap(&self.page, div().children(content))
    }
}

/// Download → Transcribe → (Find moments →) Render, with the current step pulsing.
pub(super) struct Stepper {
    job_id: String,
    page: WeakEntity<JobsPage>,
    jobs: Entity<Jobs>,
    status: Option<String>,
    /// Connectors already full when the view opened stay put; later ones grow in.
    first_done: usize,
    pub measured: Measured,
    _subscription: Subscription,
}

impl Stepper {
    pub fn new(job_id: String, page: WeakEntity<JobsPage>, cx: &mut Context<Self>) -> Self {
        let jobs = Jobs::global(cx);
        let job = jobs.read(cx).jobs.get(&job_id);
        let status = job.map(|job| job.status.clone());
        let first_done = job.map_or(0, |job| {
            let steps = logic::steps_for(job.request.manual_clips.is_some());
            logic::step_index(&steps, &job.status).unwrap_or(0)
        });
        let subscription = cx.observe(&jobs, |this: &mut Self, jobs, cx| {
            let status = jobs.read(cx).jobs.get(&this.job_id).map(|job| job.status.clone());
            if status != this.status {
                this.status = status;
                cx.notify();
            }
        });
        Self { job_id, page, jobs, status, first_done, measured: Measured::default(), _subscription: subscription }
    }

    fn content(&self, job: &JobSnapshot, t: &Tokens) -> Div {
        let steps = logic::steps_for(job.request.manual_clips.is_some());
        let current = logic::step_index(&steps, &job.status);
        let accent = t.accent;
        let pulse = cubic_bezier(0.16, 1.0, 0.3, 1.0);
        div().flex().items_start().children(steps.iter().enumerate().map(|(i, step)| {
            let done = current.is_some_and(|c| c > i);
            let active = current == Some(i);
            let last = i == steps.len() - 1;
            let badge = div()
                .flex()
                .size(u(32.))
                .items_center()
                .justify_center()
                .rounded_full()
                .border_1()
                .map(|b| {
                    if done {
                        b.border_color(t.ink.opacity(0.7)).bg(t.ink.opacity(0.7))
                    } else if active {
                        b.border_color(t.accent.opacity(0.7)).bg(t.accent.opacity(0.15))
                    } else {
                        b.border_color(t.tint(0.15))
                    }
                })
                .child(if done {
                    ui::icon("check").size(u(14.)).text_color(t.bg)
                } else {
                    ui::icon(step.icon).size(u(14.)).text_color(if active { t.accent_fg } else { t.ink_tertiary })
                });
            // animate-pulse-ring: an accent halo that spreads to 8px and fades.
            let badge = if active {
                badge
                    .with_animation(
                        ("pulse-ring", i),
                        Animation::new(Duration::from_millis(1800)).repeat().with_easing(pulse),
                        move |b, delta| {
                            b.shadow(vec![BoxShadow {
                                color: accent.opacity(0.45 * (1.0 - delta)),
                                offset: point(px(0.), px(0.)),
                                blur_radius: px(0.),
                                spread_radius: px(8. * delta),
                                inset: false,
                            }])
                        },
                    )
                    .into_any_element()
            } else {
                badge.into_any_element()
            };
            let fill = div().h_full().rounded_full().bg(t.accent);
            let connector = if done && i >= self.first_done {
                fill.with_animation(("connector", i), Animation::new(TRANSITION).with_easing(ease_out()), |d, delta| {
                    d.w(relative(delta))
                })
                .into_any_element()
            } else {
                fill.w(relative(if done { 1. } else { 0. })).into_any_element()
            };
            div()
                .flex()
                .items_start()
                .when(!last, |d| d.flex_1())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .w(u(84.))
                        .items_center()
                        .gap(u(8.))
                        .child(badge)
                        .child(
                            div()
                                .text_center()
                                .text_meta()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if active {
                                    t.ink
                                } else if done {
                                    t.ink_secondary
                                } else {
                                    t.ink_tertiary
                                })
                                .child(step.label),
                        ),
                )
                .when(!last, |d| {
                    d.child(
                        div()
                            .mt(u(15.))
                            .h(u(2.))
                            .flex_1()
                            .rounded_full()
                            .overflow_hidden()
                            .bg(t.tint(0.1))
                            .child(connector),
                    )
                })
        }))
    }
}

impl Render for Stepper {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.tokens();
        let job = self.jobs.read(cx).jobs.get(&self.job_id).cloned();
        let content = job.map(|job| self.content(&job, &t));
        self.measured.wrap(&self.page, div().children(content))
    }
}
