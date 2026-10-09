//! Small pieces the Settings page, ProviderSetup and WhisperModels share:
//! option tiles, eyebrows, labelled fields, the refresh button and select items.
use crate::theme::Tokens;
use crate::ui::{TypeScale, u};
use gpui_kit::component::searchable_list::SearchableListItem;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

/// `.eyebrow`: a small group label.
pub fn eyebrow(text: impl Into<SharedString>, t: &Tokens) -> Div {
    div()
        .text_caption()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.ink_tertiary)
        .child(text.into())
}

/// `.tile .tile-hover` (and `.tile-selected`): a choosable option.
pub fn option_tile(id: impl Into<ElementId>, selected: bool, t: &Tokens) -> Stateful<Div> {
    let t = *t;
    div()
        .id(id.into())
        .rounded(u(8.))
        .border_1()
        .map(|tile| {
            if selected {
                tile.bg(t.accent.opacity(0.1)).border_color(t.accent.opacity(0.7))
            } else {
                tile.bg(t.tint(0.03))
                    .border_color(t.tint(0.08))
                    .hover(move |s| s.bg(t.tint(0.06)).border_color(t.tint(0.14)))
            }
        })
}

/// Field.tsx: label (with an optional aside on the right), control, hint.
pub fn field(
    label: impl IntoElement,
    aside: Option<AnyElement>,
    control: impl IntoElement,
    hint: Option<AnyElement>,
    t: &Tokens,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(u(6.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(u(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(8.))
                        .text_label()
                        .text_color(t.ink)
                        .child(label),
                )
                .children(aside),
        )
        .child(control)
        .when_some(hint, |d, hint| {
            d.child(div().text_caption().text_color(t.ink_secondary).child(hint))
        })
}

/// An inline error under a control (`role="alert"`).
pub fn error_text(message: impl Into<SharedString>, t: &Tokens) -> Div {
    div().text_caption().text_color(t.danger).child(message.into())
}

/// RefreshIcon: turns while `spinning`, in whole turns.
pub fn refresh_icon(spinning: bool, color: Hsla) -> AnyElement {
    let icon = crate::ui::icon("refresh-cw").size(u(14.)).text_color(color);
    if spinning {
        icon.with_animation(
            "refresh-spin",
            Animation::new(std::time::Duration::from_millis(700)).repeat(),
            |icon, delta| icon.rotate(percentage(delta)),
        )
        .into_any_element()
    } else {
        icon.into_any_element()
    }
}

/// A small button (Button.tsx `size="sm"`) whose icon is the RefreshIcon;
/// disabled while it spins. `ghost` or the secondary look.
pub fn refresh_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    spinning: bool,
    ghost: bool,
    t: &Tokens,
) -> Stateful<Div> {
    let t = *t;
    let fg = match (ghost, spinning) {
        (true, false) => t.ink_secondary,
        (true, true) => t.tint(0.3),
        (false, false) => t.tint(0.8),
        (false, true) => t.tint(0.35),
    };
    div()
        .id(id.into())
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(u(6.))
        .h(u(24.))
        .px(u(8.))
        .rounded(u(6.))
        .text_caption()
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .text_color(fg)
        .when(!ghost, |b| b.border_1().border_color(t.tint(0.1)))
        .when(!spinning, |b| {
            b.cursor_pointer().hover(move |s| s.bg(t.tint(0.1)).text_color(t.ink))
        })
        .child(refresh_icon(spinning, fg))
        .child(label.into())
}

/// Grid columns for option tiles: 3 from Tailwind's `sm` (640px window) up, else 2.
pub fn tile_columns(window: &Window) -> u16 {
    let scale = f32::from(window.rem_size()) / 16.0;
    if f32::from(window.viewport_size().width) / scale >= 640.0 { 3 } else { 2 }
}

/// A Select option: the saved value, its label, and a muted detail.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub value: SharedString,
    pub label: SharedString,
    pub detail: Option<SharedString>,
}

impl Choice {
    pub fn new(value: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self { value: value.into(), label: label.into(), detail: None }
    }
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
        let query = query.to_lowercase();
        format!("{} {}", self.label, self.value).to_lowercase().contains(&query)
    }

    fn render(&self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *cx.global::<Tokens>();
        div()
            .flex()
            .min_w_0()
            .items_center()
            .gap(u(8.))
            .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(self.label.clone()))
            .when_some(self.detail.clone(), |row, detail| {
                row.child(
                    div()
                        .flex_shrink_0()
                        .text_meta()
                        .font_family(crate::MONO_FONT)
                        .text_color(t.ink_tertiary)
                        .child(detail),
                )
            })
    }
}
