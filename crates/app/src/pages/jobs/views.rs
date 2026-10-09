//! One job in detail (JobProgress, JobFailure in JobProgress.tsx; JobCancelled in
//! JobsPage.tsx), and a finished run's clips.
use super::arcade::Arcade;
use super::progress::{ProgressCore, Stepper};
use super::{JobsPage, Progress, tooltip};
use crate::theme::Tokens;
use crate::ui::{self, Btn, Hue, Truncate, TypeScale, u};
use clipward_core::format::source_label;
use clipward_core::job_view::JobOutput;
use clipward_core::jobs::JobSnapshot;
use clipward_core::jobs_page as logic;
use clipward_core::shared::{ISSUES_URL, is_active_job_status};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

impl JobsPage {
    /// "← All jobs" above a job or a run (BackLink).
    pub(super) fn back_link(&self, t: &Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        div().flex().child(
            div()
                .id("back")
                .ml(u(-8.))
                .flex()
                .h(u(24.))
                .flex_none()
                .items_center()
                .gap(u(4.))
                .rounded(u(6.))
                .pl(u(6.))
                .pr(u(8.))
                .text_caption()
                .text_color(t.ink_secondary)
                .cursor_pointer()
                .hover(|b| b.bg(t.tint(0.1)).text_color(t.ink))
                .on_click(cx.listener(|this, _, _, cx| this.back(cx)))
                .child(ui::icon("arrow-left").size(u(14.)))
                .child("All jobs"),
        )
    }

    /// The error callout above a job's page (`<Page width="focus" className="pb-0">`).
    fn focused_callout(&self, t: &Tokens, cx: &mut Context<Self>) -> Option<Div> {
        self.error_callout(t, cx).map(|callout| ui::page(ui::PAGE_FOCUS).pb_0().child(callout))
    }

    pub(super) fn render_focused(&mut self, job: JobSnapshot, window: &mut Window, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        if is_active_job_status(&job.status) {
            return self.job_progress(&job, window, t, cx);
        }
        let failed = job.status == "failed";
        let source = job.request.video_url.clone();
        let retry = cx.listener({
            let job = job.clone();
            move |this, _, _, cx| this.run_again(&job, cx)
        });
        let run_again = Btn::new("run-again").primary().icon("rotate-ccw").label("Run again").on_click(retry);
        let title = |text: &'static str| div().text_title().text_color(t.ink).child(text);
        let source_line = div()
            .id("source")
            .mt(u(4.))
            .text_body()
            .text_color(t.ink_secondary)
            .one_line()
            .tooltip(tooltip(source.clone()))
            .child(source_label(&source));
        let callout = self.focused_callout(t, cx);
        let back = div().mb(u(12.)).child(self.back_link(t, cx));
        let page = if !failed {
            ui::page(ui::PAGE_FOCUS)
                .child(back)
                .child(title("Job cancelled"))
                .child(source_line)
                .child(div().mt(u(16.)).flex().child(run_again))
        } else {
            let error = job
                .error
                .clone()
                .filter(|e| !e.is_empty())
                .unwrap_or_else(|| "The clipping engine stopped without an error message.".into());
            let code = job.failure_code.as_deref().filter(|c| !c.is_empty()).map(|code| logic::failure_code_line(code, job.http_status));
            let hint = job.error_hint.clone().filter(|h| !h.is_empty());
            ui::page(ui::PAGE_FOCUS)
                .child(back)
                .child(div().flex().child(ui::icon_tile("triangle-alert", Hue::Danger, 32., t)))
                .child(title("Clipping failed").mt(u(12.)))
                .child(source_line)
                .child(
                    ui::card(t)
                        .mt(u(20.))
                        .overflow_hidden()
                        .child(
                            div()
                                .id("failure-error")
                                .max_h(u(288.))
                                .overflow_y_scroll()
                                .border_b_1()
                                .border_color(t.stroke)
                                .bg(t.danger.opacity(0.04))
                                .px(u(16.))
                                .py(u(12.))
                                .font_family(crate::MONO_FONT)
                                .text_caption()
                                .text_color(t.danger)
                                .child(error),
                        )
                        .when_some(hint, |card, hint| {
                            card.child(
                                div()
                                    // `last:border-b-0`
                                    .when(code.is_some(), |d| d.border_b_1().border_color(t.stroke))
                                    .px(u(16.))
                                    .py(u(12.))
                                    .text_body()
                                    .text_color(t.ink.opacity(0.7))
                                    .child(hint),
                            )
                        })
                        .when_some(code, |card, code| {
                            card.child(
                                div()
                                    .px(u(16.))
                                    .py(u(10.))
                                    .font_family(crate::MONO_FONT)
                                    .text_meta()
                                    .text_color(t.ink_tertiary)
                                    .child(code),
                            )
                        }),
                )
                .child(
                    div()
                        .mt(u(16.))
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(u(6.))
                        .child(run_again)
                        .child(Btn::new("show-logs").icon("scroll-text").label("Show logs").on_click(|_, _, cx| {
                            crate::shell::show_log_file(cx);
                        }))
                        .child(Btn::new("report").ghost().icon("github").label("Report an issue").on_click(|_, _, cx| {
                            let _ = crate::shell::open_url(ISSUES_URL, cx);
                        })),
                )
        };
        div().children(callout).child(page).into_any_element()
    }

    /// The live views for `job`, made once per job on show.
    fn progress_views(&mut self, job_id: &str, window: &mut Window, cx: &mut Context<Self>) -> (Entity<ProgressCore>, Entity<Stepper>, Entity<Arcade>) {
        if let Some(p) = self.progress.as_ref().filter(|p| p.job_id == job_id) {
            return (p.core.clone(), p.stepper.clone(), p.arcade.clone());
        }
        let page = cx.weak_entity();
        let core = cx.new(|cx| ProgressCore::new(job_id.to_string(), page.clone(), cx));
        let stepper = cx.new(|cx| Stepper::new(job_id.to_string(), page, cx));
        let arcade = cx.new(|cx| Arcade::new(job_id.to_string(), window, cx));
        self.progress = Some(Progress {
            job_id: job_id.to_string(),
            core: core.clone(),
            stepper: stepper.clone(),
            arcade: arcade.clone(),
        });
        (core, stepper, arcade)
    }

    /// JobProgress: the page around the cached ring, stepper and arcade.
    fn job_progress(&mut self, job: &JobSnapshot, window: &mut Window, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let (core, stepper, arcade) = self.progress_views(&job.id, window, cx);
        let queued = job.status == "queued";
        let source = job.request.video_url.clone();
        let id = job.id.clone();
        let core_size = core.read(cx).measured.clone();
        let stepper_size = stepper.read(cx).measured.clone();
        let playing = arcade.read(cx).playing();
        let card = ui::card(t)
            .mt(u(20.))
            .overflow_hidden()
            .px(u(20.))
            .pb(u(20.))
            .pt(u(24.))
            .child(core_size.view(&core))
            .child(div().mt(u(24.)).child(stepper_size.view(&stepper)));
        div()
            .flex()
            .flex_col()
            .min_h_full()
            .children(self.focused_callout(t, cx))
            .child(
                ui::page(ui::PAGE_FOCUS)
                    .child(div().mb(u(8.)).child(self.back_link(t, cx)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(
                                div()
                                    .text_caption()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.accent_fg)
                                    .child(if queued { "Waiting in the queue" } else { "Generating clips" }),
                            )
                            .child(
                                div()
                                    .id("progress-source")
                                    .mt(u(4.))
                                    .max_w(u(560.))
                                    .text_title()
                                    .text_color(t.ink)
                                    .one_line()
                                    .tooltip(tooltip(source.clone()))
                                    .child(source_label(&source)),
                            ),
                    )
                    .child(card)
                    .child(
                        div()
                            .mt(u(16.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(u(16.))
                            .px(u(4.))
                            // max-w-[60ch] of the caption face.
                            .child(div().max_w(u(432.)).text_caption().text_color(t.ink_secondary).child(if queued {
                                "Starts automatically when a running job finishes."
                            } else {
                                "Long videos can take a while. Keep using Clipward or queue more videos; progress shows in the sidebar."
                            }))
                            .child(
                                Btn::new("cancel-focused")
                                    .label(if queued { "Remove from queue" } else { "Cancel" })
                                    .on_click(cx.listener(move |this, _, _, cx| this.cancel(&id, cx))),
                            ),
                    ),
            )
            // Full-bleed band along the bottom, under the page column. Drawn from the
            // cache: only its own frames redraw it.
            .child(div().mt_auto().w_full().child(if playing {
                arcade.into_any_element()
            } else {
                arcade
                    .cached(StyleRefinement::default().w_full().h(u(224.)).flex_shrink_0())
                    .into_any_element()
            }))
            .into_any_element()
    }

    /// A finished run's clips: the Library's clip list, which opens the editor itself.
    #[cfg(feature = "page-library")]
    pub(super) fn render_clips(&mut self, output_dir: String, output: JobOutput, window: &mut Window, _: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        use crate::pages::library::clip_list::{ClipList, ClipListBack};
        let list = match &self.clips {
            Some((dir, list, _)) if *dir == output_dir => list.clone(),
            _ => {
                let list = cx.new(|cx| ClipList::new(output_dir.clone(), output, "All jobs", window, cx));
                let back = cx.subscribe(&list, |this, _, _: &ClipListBack, cx| this.back(cx));
                self.clips = Some((output_dir, list.clone(), back));
                list
            }
        };
        list.into_any_element()
    }

    /// Without the Library page built in, a stand-in: the run's title and its folder.
    #[cfg(not(feature = "page-library"))]
    pub(super) fn render_clips(&mut self, output_dir: String, output: JobOutput, _: &mut Window, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        ui::page(ui::PAGE_DEFAULT)
            .child(self.back_link(t, cx))
            .child(div().mt(u(12.)).text_title().child(output.source_video_title))
            .child(
                div()
                    .mt(u(12.))
                    .flex()
                    .child(Btn::new("open-run-folder").icon("folder-open").label("Open folder").on_click(
                        cx.listener(move |this, _, _, cx| this.open_folder(&output_dir, cx)),
                    )),
            )
            .into_any_element()
    }
}
