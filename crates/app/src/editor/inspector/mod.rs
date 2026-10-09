//! The side panel: brief, inspectors, overlays and captions (TextPanels, OverlayPanels,
//! SoundPanels, ColorPanel and the piece inspector in EditorPage.tsx). See docs/GPUI_PORT.md.
//!
//! Panel order as the React `<aside>`: Campaign brief, the inspector for the selection
//! (text, overlay, PiP, sound or piece; hidden on read-only clips), Overlays, Captions.
//! It follows the playhead only through what it shows from it (a crop piece's zoom and
//! keyframe at the playhead); the caption words follow it in their own cached view
//! (`captions::WordList`), so the panel doesn't re-render every frame while playing.
mod captions;
mod color_picker;
mod layers;
mod piece;

use super::state::{EditorState, Selection};
use crate::theme::{ActiveTokens, Tokens};
use crate::ui::{Btn, TypeScale, u};
use clipward_core::clip_engine::render::project::Project;
use clipward_core::timeline::{self, EditedWord};
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::{Sizable, Size};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::collections::HashMap;
use std::rc::Rc;

type Apply = Rc<dyn Fn(&mut EditorState, f64, &mut Context<EditorState>)>;

/// One slider's state; the range is fixed per state, so a new range gets a new one.
struct Slot {
    state: Entity<SliderState>,
    range: (f32, f32, f32),
    apply: Apply,
    _subscription: Subscription,
}

/// The panel's selects.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Pick {
    Style,
    In,
    Out,
    Transition,
}

impl Pick {
    const ALL: [Pick; 4] = [Pick::Style, Pick::In, Pick::Out, Pick::Transition];

    fn choices(self) -> Vec<Choice> {
        let list: &[(&str, &str)] = match self {
            Pick::Style => {
                return crate::captions::PRESETS
                    .iter()
                    .map(|p| Choice { value: p.id.into(), label: p.name.into() })
                    .collect();
            }
            Pick::In => &[("none", "None"), ("fade", "Fade"), ("pop", "Pop"), ("slide", "Slide up"), ("typewriter", "Typewriter")],
            Pick::Out => &[("none", "None"), ("fade", "Fade"), ("pop", "Pop")],
            Pick::Transition => &piece::TRANSITION_STYLES,
        };
        list.iter().map(|(value, label)| Choice { value: (*value).into(), label: (*label).into() }).collect()
    }
}

/// A select option: the saved value and its label.
#[derive(Clone, Debug, PartialEq)]
struct Choice {
    value: SharedString,
    label: SharedString,
}

impl SearchableListItem for Choice {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &SharedString {
        &self.value
    }

    fn matches(&self, query: &str) -> bool {
        self.label.to_lowercase().contains(&query.to_lowercase())
    }

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div().overflow_hidden().whitespace_nowrap().text_ellipsis().child(self.label.clone())
    }
}

type ChoiceSelect = SelectState<SearchableVec<Choice>>;

pub struct Inspector {
    editor: Entity<EditorState>,
    sliders: HashMap<&'static str, Slot>,
    /// The slider being dragged: its changes are one undo step.
    dragging: Option<&'static str>,
    selects: HashMap<Pick, Entity<ChoiceSelect>>,
    text: Entity<TextareaState>,
    /// The text layer the text field shows.
    text_for: Option<String>,
    title: Entity<InputState>,
    word: Entity<InputState>,
    /// The field being typed in has recorded its undo step: one per focus, not per key.
    typing: bool,
    picker: Entity<color_picker::ColorPicker>,
    picker_open: bool,
    /// A drag on the colour picker is in progress (one undo step).
    picking: bool,
    /// The caption word being edited: (segment, word).
    editing: Option<(usize, usize)>,
    adding_logo: bool,
    words: Rc<Vec<EditedWord>>,
    words_revision: Option<u64>,
    word_list: Entity<captions::WordList>,
    /// What the panel shows of the playhead; it re-renders when this changes.
    followed: Option<(Option<usize>, i64)>,
    _subscriptions: Vec<Subscription>,
}

impl Inspector {
    pub fn new(editor: Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let playhead = editor.read(cx).playhead.clone();
        let text = cx.new(|cx| TextareaState::new(window, cx).rows(3));
        let title = cx.new(|cx| InputState::new(window, cx));
        let word = cx.new(|cx| InputState::new(window, cx));
        let picker = cx.new(|cx| color_picker::ColorPicker::new(clipward_core::inspector::DEFAULT_BACKDROP, window, cx));
        let mut subscriptions = vec![
            cx.observe(&editor, |this, _, cx| {
                this.sync_words(cx);
                cx.notify();
            }),
            cx.observe(&playhead, |this, _, cx| {
                let key = this.follow_key(cx);
                if key != this.followed {
                    this.followed = key;
                    cx.notify();
                }
            }),
            cx.subscribe_in(&text, window, |this, _, event: &InputEvent, window, cx| this.on_text(event, window, cx)),
            cx.subscribe_in(&title, window, |this, _, event: &InputEvent, window, cx| this.on_title(event, window, cx)),
            cx.subscribe_in(&word, window, |this, _, event: &InputEvent, window, cx| this.on_word(event, window, cx)),
            cx.subscribe(&picker, |this, _, event: &color_picker::ColorChanged, cx| this.on_color(event, cx)),
            cx.subscribe(&picker, |this, _, _: &color_picker::ColorReleased, cx| {
                if std::mem::take(&mut this.picking) {
                    this.editor.update(cx, |e, _| e.end_batch());
                }
            }),
        ];
        let panel = cx.entity().downgrade();
        let on_pick: Rc<dyn Fn(&EditedWord, &mut Window, &mut App)> = Rc::new(move |word, window, cx| {
            let _ = panel.update(cx, |this, cx| this.pick_word(word, window, cx));
        });
        let panel = cx.entity().downgrade();
        let word_list = cx.new(|cx| captions::WordList::new(playhead.clone(), panel, on_pick, cx));
        let mut selects = HashMap::new();
        for pick in Pick::ALL {
            let state = cx.new(|cx| SelectState::new(SearchableVec::new(pick.choices()), None, window, cx));
            subscriptions.push(cx.subscribe(&state, move |this, _, event: &SelectEvent<SearchableVec<Choice>>, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    this.on_pick(pick, value.to_string(), cx);
                }
            }));
            selects.insert(pick, state);
        }
        Self {
            editor,
            sliders: HashMap::new(),
            dragging: None,
            selects,
            text,
            text_for: None,
            title,
            word,
            typing: false,
            picker,
            picker_open: false,
            picking: false,
            editing: None,
            adding_logo: false,
            words: Rc::new(Vec::new()),
            words_revision: None,
            word_list,
            followed: None,
            _subscriptions: subscriptions,
        }
    }

    /// Caption words in edit order, recomputed once per edit.
    fn words(&mut self, cx: &App) -> Rc<Vec<EditedWord>> {
        let editor = self.editor.read(cx);
        let revision = editor.history.revision();
        if self.words_revision != Some(revision) {
            self.words = Rc::new(editor.project().map_or_else(Vec::new, |p| timeline::edited_words(&p.video, &p.transcript)));
            self.words_revision = Some(revision);
        }
        self.words.clone()
    }

    /// What a selected crop piece shows of the playhead: the keyframe under it and
    /// the zoom slider's value there.
    fn follow_key(&self, cx: &App) -> Option<(Option<usize>, i64)> {
        let editor = self.editor.read(cx);
        let (Some(Selection::Piece(i)), Some(p)) = (&editor.selection, editor.project()) else { return None };
        let item = p.video.get(*i).filter(|v| v.framing == "crop")?;
        let local = clipward_core::inspector::piece_local(&p.video, *i, editor.time(cx));
        let zoom = clipward_core::timeline::crop_at(item, local).zoom;
        Some((clipward_core::inspector::key_here(item, local), clipward_core::inspector::zoom_to_slider(zoom) as i64))
    }

    // ---- sliders --------------------------------------------------------------------

    /// LabeledSlider: its name above, the track with its reading to the right. A drag
    /// is one undo step. `apply` gets the slider's value.
    #[allow(clippy::too_many_arguments)]
    fn slider(
        &mut self,
        key: &'static str,
        label: &'static str,
        value: f64,
        (min, max, step): (f64, f64, f64),
        display: String,
        apply: impl Fn(&mut EditorState, f64, &mut Context<EditorState>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // A range with no room (a fade on a piece under 0.1 s) shows a still slider.
        let empty = max <= min;
        let range = (min as f32, if empty { (min + step) as f32 } else { max as f32 }, step as f32);
        let apply: Apply = Rc::new(apply);
        match self.sliders.get_mut(key).filter(|s| s.range == range) {
            Some(slot) => {
                slot.apply = apply;
                if self.dragging != Some(key) && slot.state.read(cx).value().start() != value as f32 {
                    slot.state.update(cx, |s, cx| s.set_value(value as f32, window, cx));
                }
            }
            None => {
                // Each bound is checked against the other as it is set (from 0..100), so
                // widen first: a range above 100 sets its max first, one below 0 its min.
                let state = cx.new(|_| {
                    let state = SliderState::new();
                    let state = if range.0 >= 0. { state.max(range.1).min(range.0) } else { state.min(range.0).max(range.1) };
                    state.step(range.2).default_value(value as f32)
                });
                let subscription = cx.subscribe(&state, move |this, _, event: &SliderEvent, cx| this.on_slider(key, event, cx));
                self.sliders.insert(key, Slot { state, range, apply, _subscription: subscription });
            }
        }
        let t = *cx.tokens();
        div()
            .flex()
            .flex_col()
            .gap(u(2.))
            .child(div().text_caption().text_color(t.ink_secondary).child(label))
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(u(12.))
                    .when(empty, |d| d.opacity(0.4))
                    .child(
                        div().flex_1().min_w_0().child(
                            Slider::new(&self.sliders[key].state).disabled(empty).bg(t.tint(0.5)).text_color(t.ink),
                        ),
                    )
                    .child(
                        div()
                            .w(u(40.))
                            .flex_shrink_0()
                            .flex()
                            .justify_end()
                            .whitespace_nowrap()
                            .text_caption()
                            .text_color(t.tint(0.8))
                            .child(display),
                    ),
            )
            .into_any_element()
    }

    fn on_slider(&mut self, key: &'static str, event: &SliderEvent, cx: &mut Context<Self>) {
        let (value, release) = match event {
            SliderEvent::Change(v) => (v.start(), false),
            SliderEvent::Release(v) => (v.start(), true),
        };
        let Some(slot) = self.sliders.get(key) else { return };
        let value = value.clamp(slot.range.0, slot.range.1) as f64;
        let apply = slot.apply.clone();
        let first = self.dragging != Some(key);
        self.dragging = (!release).then_some(key);
        self.editor.update(cx, |e, cx| {
            if first {
                e.begin_batch();
            }
            apply(e, value, cx);
            if release {
                e.end_batch();
            }
        });
    }

    // ---- selects ----------------------------------------------------------------------

    /// A select showing `value`.
    fn select(&self, pick: Pick, value: &str, label: &'static str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.selects[&pick];
        let value = SharedString::from(value.to_string());
        if state.read(cx).selected_value() != Some(&value) {
            state.update(cx, |s, cx| s.set_selected_value(&value, window, cx));
        }
        Select::new(state)
            .with_size(Size::Small)
            .accessibility_label(label)
            .text_caption()
            .into_any_element()
    }

    fn on_pick(&mut self, pick: Pick, value: String, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| match (pick, e.selection.clone()) {
            (Pick::Style | Pick::In | Pick::Out, Some(Selection::Text(id))) => e.edit_texts(
                |texts| {
                    texts
                        .iter()
                        .map(|t| {
                            let mut t = t.clone();
                            if t.id == id {
                                match pick {
                                    Pick::Style => t.preset = value.clone(),
                                    Pick::In => t.animation_in = value.clone(),
                                    _ => t.animation_out = value.clone(),
                                }
                            }
                            t
                        })
                        .collect()
                },
                true,
                cx,
            ),
            (Pick::Transition, Some(Selection::Piece(index))) => piece::set_transition_style(e, index, &value, cx),
            _ => {}
        });
    }

    // ---- text fields ----------------------------------------------------------------

    fn on_text(&mut self, event: &InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            InputEvent::Focus => self.typing = false,
            InputEvent::Change => {
                let Some(Selection::Text(id)) = self.editor.read(cx).selection.clone() else { return };
                let text = self.text.read(cx).value().to_string();
                let text = match clip(&text, clipward_core::clip_engine::render::project::MAX_TEXT_CHARS) {
                    Some(short) => {
                        self.text.update(cx, |input, cx| input.set_value(short.clone(), window, cx));
                        short
                    }
                    None => text,
                };
                let record = !std::mem::replace(&mut self.typing, true);
                self.editor.update(cx, |e, cx| {
                    e.edit_texts(
                        |texts| {
                            texts
                                .iter()
                                .map(|t| if t.id == id { clipward_core::clip_engine::render::project::TextLayer { text: text.clone(), ..t.clone() } } else { t.clone() })
                                .collect()
                        },
                        record,
                        cx,
                    )
                });
            }
            _ => {}
        }
    }

    fn on_title(&mut self, event: &InputEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            InputEvent::Focus => self.typing = false,
            InputEvent::Change => {
                let title = self.title.read(cx).value().to_string();
                let title = match clip(&title, 120) {
                    Some(short) => {
                        self.title.update(cx, |input, cx| input.set_value(short.clone(), window, cx));
                        short
                    }
                    None => title,
                };
                let record = !std::mem::replace(&mut self.typing, true);
                self.editor.update(cx, |e, cx| {
                    e.change(|p| (p.title.as_deref() != Some(&title)).then(|| Project { title: Some(title.clone()), ..p.clone() }), record, cx)
                });
            }
            _ => {}
        }
    }

    /// Keep the text and title fields on the selection (not while typing in them).
    fn sync_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = self.editor.read(cx);
        let (layer, title) = match (&editor.selection, editor.project()) {
            (Some(Selection::Text(id)), Some(p)) => (p.texts.iter().find(|t| &t.id == id).map(|t| (t.id.clone(), t.text.clone())), None),
            (Some(Selection::Overlay(clipward_core::overlays::OverlayKey::Title)), Some(p)) => (None, p.title.clone()),
            _ => (None, None),
        };
        if let Some((id, text)) = layer {
            let switched = self.text_for.as_ref() != Some(&id);
            let focused = self.text.read(cx).focus_handle(cx).is_focused(window);
            if (switched || !focused) && self.text.read(cx).value() != text {
                self.text.update(cx, |input, cx| input.set_value(text, window, cx));
            }
            self.text_for = Some(id);
        } else {
            self.text_for = None;
        }
        if let Some(title) = title {
            let focused = self.title.read(cx).focus_handle(cx).is_focused(window);
            if !focused && self.title.read(cx).value() != title {
                self.title.update(cx, |input, cx| input.set_value(title, window, cx));
            }
        }
    }

    // ---- colour picker ----------------------------------------------------------------

    fn on_color(&mut self, event: &color_picker::ColorChanged, cx: &mut Context<Self>) {
        let Some(color) = clipward_core::inspector::valid_backdrop(&event.color) else { return };
        let start = event.drag && !self.picking;
        self.picking |= event.drag;
        self.editor.update(cx, |e, cx| {
            if start {
                e.begin_batch();
            }
            e.change(|p| (p.backdrop.as_deref() != Some(&color)).then(|| Project { backdrop: Some(color.clone()), ..p.clone() }), true, cx);
        });
    }
}

/// `text` cut to `max` characters, when it is longer (the fields' maxLength).
fn clip(text: &str, max: usize) -> Option<String> {
    (text.chars().count() > max).then(|| text.chars().take(max).collect())
}

/// `.card space-y-3 rounded-xl p-3`: one panel section.
fn section(t: &Tokens) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(u(12.))
        .rounded(u(12.))
        .border_1()
        .border_color(t.stroke)
        .bg(t.tint(0.03))
        .p(u(12.))
}

/// A section's title row: the name, and an optional delete button.
fn header(title: impl Into<SharedString>, delete: Option<(&'static str, &'static str)>, editor: &Entity<EditorState>, t: &Tokens) -> Div {
    let editor = editor.clone();
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(u(8.))
        .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_label().text_color(t.ink).child(title.into()))
        .when_some(delete, |d, (id, tooltip)| {
            d.child(
                Btn::new(id)
                    .ghost()
                    .small()
                    .icon("trash")
                    .tooltip(tooltip)
                    .on_click(move |_, _, cx| editor.update(cx, |e, cx| e.remove_selected(cx))),
            )
        })
}

/// A muted hint line.
fn hint(text: impl Into<SharedString>, t: &Tokens) -> Div {
    div().text_caption().text_color(t.ink_tertiary).child(text.into())
}

/// A small label above a control.
fn label(text: impl Into<SharedString>, t: &Tokens) -> Div {
    div().text_caption().text_color(t.ink_secondary).child(text.into())
}

impl Render for Inspector {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.tokens();
        self.sync_fields(window, cx);
        self.followed = self.follow_key(cx);
        let editor = self.editor.read(cx);
        let editable = editor.editable();
        let has_project = editor.project().is_some();
        let selection = editor.selection.clone();
        let brief = editor.opened.as_ref().and_then(|o| o.requirements.clone());

        let inspector = match selection.filter(|_| editable && has_project) {
            Some(Selection::Text(id)) => self.text_inspector(&id, window, cx),
            Some(Selection::Overlay(key)) => self.overlay_inspector(key, window, cx),
            Some(Selection::Pip(id)) => self.pip_inspector(&id, window, cx),
            Some(Selection::Sound(id)) => self.sound_inspector(&id, window, cx),
            Some(Selection::Piece(index)) => self.piece_inspector(index, window, cx),
            None => None,
        };
        let overlays = has_project.then(|| self.overlays_panel(cx));
        let captions = has_project.then(|| self.captions_panel(window, cx));

        div()
            .id("inspector")
            // Mounted as a cached view at 288px by the full height: fill it and scroll.
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(u(12.))
                    .child(
                        section(&t)
                            .gap(u(6.))
                            .child(div().text_label().text_color(t.ink).child("Campaign brief"))
                            .child(
                                div()
                                    .text_caption()
                                    .text_color(if brief.is_some() { t.ink_secondary } else { t.ink_tertiary })
                                    .child(brief.unwrap_or_else(|| "No brief for this run.".into())),
                            ),
                    )
                    .children(inspector)
                    .children(overlays)
                    .children(captions),
            )
    }
}
