//! Manual clip mode's editable list (ManualClipList.tsx): start, end and an
//! optional title per clip. Times take seconds, m:ss or h:mm:ss. Enter moves to
//! the next clip (adding one at the end), so a list can be typed without the mouse.
use super::{CreatePage, edit};
use crate::theme::ActiveTokens;
use crate::ui::{Btn, TypeScale, u};
use clipward_core::job_form::{ClipDraft, MAX_MANUAL_CLIPS};
use clipward_core::manual_clips::{
    ManualClipRow, ManualClipsCheck, RowField, format_clip_length, insert_row, list_summary,
    remove_row,
};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// The three fields of one row, by the row's id.
struct RowInputs {
    id: String,
    start: Entity<InputState>,
    end: Entity<InputState>,
    title: Entity<InputState>,
    /// The row number its title placeholder shows ("Clip 2").
    number: usize,
    _subscriptions: Vec<Subscription>,
}

#[derive(Default)]
pub struct ManualState {
    rows: Vec<RowInputs>,
    /// The row last focused (the preview helper's "Set as start/end" target).
    active: Option<String>,
    /// Focus this row's start field once its input exists.
    pending_focus: Option<String>,
}

impl ManualState {
    /// Drop the fields; they're rebuilt from the draft on the next render.
    pub fn reset(&mut self) {
        self.rows.clear();
        self.active = None;
    }
}

#[derive(Clone, Copy)]
enum Field {
    Start,
    End,
    Title,
}

impl CreatePage {
    /// Keep one set of inputs per draft row (rows are added and removed by id).
    pub(super) fn sync_manual_rows(
        &mut self,
        rows: &[ManualClipRow],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let same = rows.len() == self.manual.rows.len()
            && rows
                .iter()
                .zip(&self.manual.rows)
                .all(|(row, inputs)| row.id == inputs.id);
        if !same {
            let mut old: Vec<RowInputs> = std::mem::take(&mut self.manual.rows);
            for row in rows {
                let inputs = match old.iter().position(|inputs| inputs.id == row.id) {
                    Some(at) => old.swap_remove(at),
                    None => self.row_inputs(row, window, cx),
                };
                self.manual.rows.push(inputs);
            }
        }
        // Titles show their row number until typed.
        for (i, inputs) in self.manual.rows.iter_mut().enumerate() {
            if inputs.number != i + 1 {
                inputs.number = i + 1;
                let placeholder = format!("Clip {}", i + 1);
                inputs
                    .title
                    .update(cx, |s, cx| s.set_placeholder(placeholder, window, cx));
            }
        }
        if let Some(id) = self.manual.pending_focus.take() {
            if let Some(inputs) = self.manual.rows.iter().find(|inputs| inputs.id == id) {
                let handle = inputs.start.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
        }
    }

    fn row_inputs(
        &mut self,
        row: &ManualClipRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> RowInputs {
        let mut make = |text: &str, placeholder: &str, field: Field, cx: &mut Context<Self>| {
            let text = SharedString::from(text.to_string());
            let placeholder = SharedString::from(placeholder.to_string());
            let state = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(text)
            });
            let id = row.id.clone();
            let subscription = cx.subscribe_in(
                &state,
                window,
                move |this, state, event: &InputEvent, window, cx| match event {
                    InputEvent::Change => {
                        let value = state.read(cx).value().to_string();
                        let id = id.clone();
                        edit(&this.draft, cx, move |d| {
                            if let Some(row) = d.manual_clips.iter_mut().find(|row| row.id == id) {
                                match field {
                                    Field::Start => row.start = value,
                                    Field::End => row.end = value,
                                    Field::Title => row.title = value,
                                }
                            }
                        });
                    }
                    InputEvent::Focus => this.manual.active = Some(id.clone()),
                    InputEvent::PressEnter {
                        secondary: false, ..
                    } => this.next_row(&id, window, cx),
                    _ => {}
                },
            );
            (state, subscription)
        };
        let (start, a) = make(&row.start, "0:00", Field::Start, cx);
        let (end, b) = make(&row.end, "0:30", Field::End, cx);
        let (title, c) = make(&row.title, "", Field::Title, cx);
        RowInputs {
            id: row.id.clone(),
            start,
            end,
            title,
            number: 0,
            _subscriptions: vec![a, b, c],
        }
    }

    /// Enter: on to the next clip's start, or a new clip after the last one.
    fn next_row(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.manual.rows.iter().position(|inputs| inputs.id == id);
        match at.and_then(|at| self.manual.rows.get(at + 1)) {
            Some(next) => {
                let handle = next.start.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            None => self.add_row(None, false, cx),
        }
    }

    /// Add a blank row at the end, or a copy of row `after` right below it.
    fn add_row(&mut self, after: Option<usize>, copy: bool, cx: &mut Context<Self>) {
        let mut rows = self.draft.read(cx).value.manual_clips.clone();
        let source = after.filter(|_| copy).and_then(|i| rows.get(i).cloned());
        if let Some(id) = insert_row(&mut rows, after, source.as_ref()) {
            self.manual.active = Some(id.clone());
            self.manual.pending_focus = Some(id);
            edit(&self.draft, cx, |d| d.manual_clips = rows);
        }
    }

    fn remove_manual_row(&mut self, index: usize, cx: &mut Context<Self>) {
        let mut rows = self.draft.read(cx).value.manual_clips.clone();
        self.manual.pending_focus = Some(remove_row(&mut rows, index));
        edit(&self.draft, cx, |d| d.manual_clips = rows);
    }

    pub(super) fn render_manual_list(
        &mut self,
        draft: &ClipDraft,
        check: &ManualClipsCheck,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = *cx.tokens();
        let full = draft.manual_clips.len() >= MAX_MANUAL_CLIPS;
        let header = |text: &'static str| div().text_meta().text_color(t.ink_tertiary).child(text);
        let rows = draft
            .manual_clips
            .iter()
            .zip(&check.rows)
            .zip(&self.manual.rows)
            .enumerate()
            .map(|(i, ((row, rc), inputs))| {
                let touched = [&row.start, &row.end, &row.title]
                    .iter()
                    .any(|f| !f.trim().is_empty());
                let error = rc.error.clone().filter(|_| touched);
                let invalid = |field: RowField| error.is_some() && rc.field == Some(field);
                let field = |state: &Entity<InputState>, bad: bool, mono: bool| {
                    Input::new(state)
                        .text_size(u(13.))
                        .when(mono, |i| i.font_family(crate::MONO_FONT))
                        .when(bad, |i| i.border_color(t.danger.opacity(0.6)))
                };
                let n = i + 1;
                let length = match rc.length {
                    Some(length) if length > 0.0 => format_clip_length(length),
                    _ => "–".into(),
                };
                div()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(u(8.))
                            .child(
                                div()
                                    .w(u(20.))
                                    .flex_shrink_0()
                                    .text_right()
                                    .text_caption()
                                    .text_color(t.ink_tertiary)
                                    .child(n.to_string()),
                            )
                            .child(div().w(u(88.)).flex_shrink_0().child(field(
                                &inputs.start,
                                invalid(RowField::Start),
                                true,
                            )))
                            .child(div().w(u(88.)).flex_shrink_0().child(field(
                                &inputs.end,
                                invalid(RowField::End),
                                true,
                            )))
                            .child(
                                div()
                                    .w(u(56.))
                                    .flex_shrink_0()
                                    .text_right()
                                    .text_caption()
                                    .text_color(t.ink_secondary)
                                    .child(length),
                            )
                            .child(div().min_w_0().flex_1().child(field(
                                &inputs.title,
                                invalid(RowField::Title),
                                false,
                            )))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .child(
                                        Btn::new(("duplicate", i))
                                            .ghost()
                                            .small()
                                            .icon("copy")
                                            .tooltip("Duplicate")
                                            .disabled(full)
                                            .on_click(self.on_click(cx, move |this, _, cx| {
                                                this.add_row(Some(i), true, cx)
                                            })),
                                    )
                                    .child(
                                        Btn::new(("remove", i))
                                            .ghost()
                                            .small()
                                            .icon("x")
                                            .tooltip("Remove")
                                            .on_click(self.on_click(cx, move |this, _, cx| {
                                                this.remove_manual_row(i, cx)
                                            })),
                                    ),
                            ),
                    )
                    .when_some(error.clone(), |d, error| {
                        d.child(
                            div()
                                .mt(u(4.))
                                .pl(u(28.))
                                .text_caption()
                                .text_color(t.danger)
                                .child(error),
                        )
                    })
            })
            .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .gap(u(12.))
            .child(preview_helper())
            .child(
                div()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(u(8.))
                            .px(px(2.))
                            .pb(u(6.))
                            .child(header("#").w(u(20.)).flex_shrink_0().text_right())
                            .child(header("Start").w(u(88.)).flex_shrink_0())
                            .child(header("End").w(u(88.)).flex_shrink_0())
                            .child(header("Length").w(u(56.)).flex_shrink_0().text_right())
                            .child(header("Title").flex_1())
                            .child(div().w(u(48.))),
                    )
                    .child(div().flex().flex_col().gap(u(6.)).children(rows)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(u(8.))
                    .child(
                        Btn::new("add-clip")
                            .icon("plus")
                            .label("Add clip")
                            .disabled(full)
                            .on_click(self.on_click(cx, |this, _, cx| this.add_row(None, false, cx))),
                    )
                    .child(
                        div()
                            .text_caption()
                            .text_color(t.ink_secondary)
                            .child(list_summary(draft.manual_clips.len(), check)),
                    ),
            )
            .child(div().text_caption().text_color(t.ink_secondary).child(format!(
                "Seconds (90), m:ss (1:30) or h:mm:ss. Each clip runs 3 seconds to 15 minutes; overlaps are fine. Enter moves to the next clip.{}",
                if draft.known_duration().is_none() {
                    " Times are checked against the video once it downloads."
                } else {
                    ""
                }
            )))
    }
}

// TODO(video): preview helper — a small player for local files: scrub to a moment,
// then "Set as start" / "Set as end" for the active clip (ManualState::active).
fn preview_helper() -> impl IntoElement {
    div()
}
