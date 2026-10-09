//! TextPanels.tsx CaptionsPanel: captions on or off, their style, and the words:
//! click one to fix, delete or colour it.
//!
//! The word list follows the playhead in its own view (`WordList`), drawn cached at
//! a measured height: while playing only it re-renders, and only when the active
//! word changes, never the rest of the panel.
use super::{Inspector, clip, hint, section};
use crate::captions::{PRESETS, SCENE, color, sample};
use crate::editor::state::Playhead;
use crate::theme::{ActiveTokens, Tokens};
use crate::ui::{self, Btn, Truncate, TypeScale, u};
use clipward_core::clip_engine::render::project::Project;
use clipward_core::timeline::{EditedWord, is_emphasized, set_word, toggle_emphasis};
use gpui_kit::component::input::{Input, InputEvent};
use gpui_kit::component::{Sizable, Size};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::cell::Cell;
use std::rc::Rc;

/// The word editor's maxLength.
const MAX_WORD_CHARS: usize = 64;
/// `max-h-56`: the word list scrolls past this, CSS px.
const LIST_MAX: f32 = 224.;

// ---- word list ---------------------------------------------------------------------

type OnPick = Rc<dyn Fn(&EditedWord, &mut Window, &mut App)>;

/// The clickable caption words (WordList in TextPanels.tsx).
pub(super) struct WordList {
    words: Rc<Vec<EditedWord>>,
    emphasis: Vec<String>,
    editing: Option<(usize, usize)>,
    /// The word under the playhead.
    active: Option<usize>,
    playhead: Entity<Playhead>,
    on_pick: OnPick,
    /// The list's height, CSS px, as last measured; the panel lays the cached view out at it.
    pub(super) height: Rc<Cell<f32>>,
    /// Told when the height changes.
    panel: WeakEntity<Inspector>,
    scroll: ScrollHandle,
    _subscription: Subscription,
}

fn active_word(words: &[EditedWord], time: f64) -> Option<usize> {
    words.iter().position(|w| time >= w.start as f64 && time < w.end)
}

impl WordList {
    pub(super) fn new(playhead: Entity<Playhead>, panel: WeakEntity<Inspector>, on_pick: OnPick, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&playhead, |this, playhead, cx| {
            let active = active_word(&this.words, playhead.read(cx).time);
            if active != this.active {
                this.active = active;
                cx.notify();
            }
        });
        Self {
            words: Rc::new(Vec::new()),
            emphasis: Vec::new(),
            editing: None,
            active: None,
            playhead,
            on_pick,
            height: Rc::new(Cell::new(0.)),
            panel,
            scroll: ScrollHandle::new(),
            _subscription: subscription,
        }
    }

    /// New words, key words or word being edited; re-renders only on a change.
    pub(super) fn set(&mut self, words: Rc<Vec<EditedWord>>, emphasis: &[String], editing: Option<(usize, usize)>, cx: &mut Context<Self>) {
        let same = Rc::ptr_eq(&words, &self.words) && emphasis == self.emphasis.as_slice() && editing == self.editing;
        if !same {
            self.active = active_word(&words, self.playhead.read(cx).time);
            self.words = words;
            self.emphasis = emphasis.to_vec();
            self.editing = editing;
            cx.notify();
        }
    }
}

impl Render for WordList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.tokens();
        let height = self.height.clone();
        let panel = self.panel.clone();
        let words = self.words.iter().enumerate().map(|(i, w)| {
            let chosen = self.editing == Some((w.seg, w.word));
            let key = !chosen && is_emphasized(&self.emphasis, &w.text);
            let on_pick = self.on_pick.clone();
            let word = w.clone();
            div()
                .id(("caption-word", i))
                .rounded(u(4.))
                .px(u(4.))
                .py(px(1.))
                .text_caption()
                .cursor_pointer()
                .map(|d| {
                    if chosen {
                        d.bg(t.accent).text_color(t.accent_ink)
                    } else if Some(i) == self.active {
                        d.bg(t.tint(0.15)).text_color(t.ink)
                    } else {
                        d.text_color(t.ink_secondary).hover(|s| s.bg(t.tint(0.1)))
                    }
                })
                .when(key, |d| d.font_weight(FontWeight::SEMIBOLD).text_color(t.success))
                .on_click(move |_, window, cx| on_pick(&word, window, cx))
                .child(w.text.clone())
        });
        div()
            .id("caption-words")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_wrap()
                    .gap(u(4.))
                    .children(words)
                    // Measure the words: the panel lays this view out at their height.
                    .child(
                        canvas(
                            move |bounds, window, cx| {
                                let css = f32::from(bounds.size.height) / (f32::from(window.rem_size()) / 16.);
                                let next = css.min(LIST_MAX).ceil();
                                if next != height.get() {
                                    height.set(next);
                                    let _ = panel.update(cx, |_, cx| cx.notify());
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    ),
            )
    }
}

// ---- caption style tiles -------------------------------------------------------------


/// CaptionPresetPicker: the 13 looks as tiles, two to a row in the 288px panel.
fn preset_picker(value: &str, disabled: bool, editor: &Entity<crate::editor::state::EditorState>, t: &Tokens) -> Div {
    let t = *t;
    div().grid().grid_cols(2).gap(u(8.)).children(PRESETS.iter().enumerate().map(|(i, preset)| {
        let selected = preset.id == value;
        let editor = editor.clone();
        let description = preset.description;
        div()
            .id(("caption-preset", i))
            .rounded(u(8.))
            .border_1()
            .p(u(4.))
            .map(|tile| {
                if selected {
                    tile.bg(t.accent.opacity(0.1)).border_color(t.accent.opacity(0.7))
                } else {
                    tile.bg(t.tint(0.03)).border_color(t.tint(0.08))
                }
            })
            .when(disabled, |tile| tile.opacity(0.5))
            .when(!disabled && !selected, |tile| tile.hover(move |s| s.bg(t.tint(0.06)).border_color(t.tint(0.14))))
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(description).build(window, cx))
            .when(!disabled, |tile| {
                tile.cursor_pointer().on_click(move |_, _, cx| {
                    editor.update(cx, |e, cx| {
                        e.change(
                            |p| (p.caption_preset != PRESETS[i].id).then(|| Project { caption_preset: PRESETS[i].id.into(), ..p.clone() }),
                            true,
                            cx,
                        )
                    })
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

// ---- the panel -----------------------------------------------------------------------

impl Inspector {
    /// Push the words, key words and the word being edited to the list.
    pub(super) fn sync_words(&mut self, cx: &mut Context<Self>) {
        let words = self.words(cx);
        let emphasis = self.editor.read(cx).project().map(|p| p.emphasis_words.clone()).unwrap_or_default();
        let editing = self.editing;
        self.word_list.update(cx, |list, cx| list.set(words, &emphasis, editing, cx));
    }

    /// A click on a word: seek to it, and (editable) open it in the word editor.
    pub(super) fn pick_word(&mut self, w: &EditedWord, window: &mut Window, cx: &mut Context<Self>) {
        let editable = self.editor.read(cx).editable();
        self.editor.update(cx, |e, cx| e.seek_paused(w.start as f64, cx));
        if !editable {
            return;
        }
        self.editing = Some((w.seg, w.word));
        let text = w.text.clone();
        self.word.update(cx, |input, cx| {
            input.set_value(text, window, cx);
            input.focus(window, cx);
        });
        self.sync_words(cx);
        cx.notify();
    }

    /// The word being edited, as the transcript has it.
    fn current_word(&self, cx: &App) -> Option<String> {
        let (seg, word) = self.editing?;
        Some(self.editor.read(cx).project()?.transcript.get(seg)?.words.get(word)?.word.clone())
    }

    fn close_word(&mut self, cx: &mut Context<Self>) {
        self.editing = None;
        self.sync_words(cx);
        cx.notify();
    }

    /// Rewrite the word being edited; empty text removes it.
    fn commit_word(&mut self, text: String, cx: &mut Context<Self>) {
        let Some((seg, word)) = self.editing else { return };
        self.editor.update(cx, |e, cx| {
            e.change(
                |p| {
                    let transcript = set_word(&p.transcript, seg, word, &text);
                    (transcript != p.transcript).then(|| Project { transcript, ..p.clone() })
                },
                true,
                cx,
            )
        });
        self.close_word(cx);
    }

    pub(super) fn on_word(&mut self, event: &InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_none() {
            return;
        }
        let draft = self.word.read(cx).value().to_string();
        match event {
            InputEvent::Change => {
                if let Some(short) = clip(&draft, MAX_WORD_CHARS) {
                    self.word.update(cx, |input, cx| input.set_value(short, window, cx));
                }
            }
            InputEvent::PressEnter { .. } => self.commit_word(draft, cx),
            // Leaving the field keeps a real change (not an emptied field: Remove does that).
            InputEvent::Blur => {
                if !draft.trim().is_empty() && self.current_word(cx).is_some_and(|w| w != draft) {
                    self.commit_word(draft, cx);
                }
            }
            _ => {}
        }
    }

    pub(super) fn captions_panel(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = *cx.tokens();
        let editor = self.editor.read(cx);
        let editable = editor.editable();
        let Some(project) = editor.project() else { return div().into_any_element() };
        let (on, preset) = (project.captions, project.caption_preset.clone());
        let emphasized = |word: &str| is_emphasized(&project.emphasis_words, word);
        let current = self.current_word(cx).map(|w| {
            let key = emphasized(&w);
            (w, key)
        });
        let has_words = !self.words(cx).is_empty();

        let switch = {
            let editor = self.editor.clone();
            ui::switch(
                "captions-on",
                on,
                !editable,
                move |captions, _, cx| {
                    editor.update(cx, |e, cx| e.change(|p| (p.captions != captions).then(|| Project { captions, ..p.clone() }), true, cx))
                },
                &t,
            )
        };
        let list = has_words.then(|| {
            let height = self.word_list.read(cx).height.get();
            div().w_full().child(self.word_list.clone().cached(StyleRefinement::default().w_full().h(u(height))))
        });
        let word_editor = current.filter(|_| on).map(|(word, key)| {
            let star = self.editor.clone();
            let remove = cx.entity().downgrade();
            div()
                .flex()
                .flex_col()
                .gap(u(8.))
                .border_t_1()
                .border_color(t.stroke)
                .pt(u(12.))
                .child(
                    div()
                        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                            if event.keystroke.key == "escape" {
                                this.close_word(cx);
                                cx.stop_propagation();
                            }
                        }))
                        .child(Input::new(&self.word).with_size(Size::Small).aria_label("Caption word").text_caption()),
                )
                .child(
                    div()
                        .flex()
                        .gap(u(4.))
                        .child(
                            Btn::new("caption-key-word")
                                .ghost()
                                .small()
                                .icon(if key { "star-fill" } else { "star" })
                                .label("Key word")
                                .tooltip("Colour this word wherever it's said (styles with key word colours)")
                                .on_click(move |_, _, cx| {
                                    let word = word.clone();
                                    star.update(cx, |e, cx| {
                                        e.change(
                                            |p| Some(Project { emphasis_words: toggle_emphasis(&p.emphasis_words, &word), ..p.clone() }),
                                            true,
                                            cx,
                                        )
                                    })
                                }),
                        )
                        .child(Btn::new("caption-remove-word").ghost().small().icon("trash").label("Remove").on_click(
                            move |_, _, cx| {
                                let _ = remove.update(cx, |this, cx| this.commit_word(String::new(), cx));
                            },
                        )),
                )
        });

        section(&t)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_label().text_color(t.ink).child("Captions"))
                    .child(switch),
            )
            .when(on, |d| {
                d.child(preset_picker(&preset, !editable, &self.editor, &t))
                    .map(|d| match list {
                        Some(list) => d.child(list),
                        None => d.child(hint("No speech in this edit.", &t)),
                    })
                    .children(word_editor)
            })
            .into_any_element()
    }
}
