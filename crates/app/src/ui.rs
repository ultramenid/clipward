//! Small pieces shared by every page: the type scale, icons, status dots,
//! progress bars and keyboard hints (DESIGN.md → Typography, Components).
//!
//! Sizes are rems, as in the CSS: Settings → Appearance → Interface scale sets
//! the window's rem size, which scales every size here at once.
use crate::theme::Tokens;
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

/// CSS pixels at the default 16px rem: `u(13.)` is the 13px body size.
pub fn u(css_px: f32) -> Rems {
    rems(css_px / 16.0)
}

/// The type scale from globals.css (size / line height in CSS pixels).
pub trait TypeScale: Styled + Sized {
    fn text_display(self) -> Self {
        self.text_size(u(28.)).line_height(u(32.))
    }
    fn text_title(self) -> Self {
        self.text_size(u(20.)).line_height(u(26.))
    }
    fn text_heading(self) -> Self {
        self.text_size(u(15.)).line_height(u(20.))
    }
    fn text_label(self) -> Self {
        self.text_size(u(13.)).line_height(u(18.))
    }
    fn text_body(self) -> Self {
        self.text_size(u(13.)).line_height(u(20.))
    }
    fn text_caption(self) -> Self {
        self.text_size(u(12.)).line_height(u(18.))
    }
    fn text_meta(self) -> Self {
        self.text_size(u(11.)).line_height(u(16.))
    }
    fn text_micro(self) -> Self {
        self.text_size(u(10.)).line_height(u(14.))
    }
}

impl<T: Styled> TypeScale for T {}

/// A Lucide icon by its file name (`"wand-sparkles"`), any of the bundled set.
pub fn icon(name: &str) -> Icon {
    Icon::default().path(SharedString::from(format!("icons/{name}.svg")))
}

/// The 16px nav/toolbar icon.
pub fn icon_sm(name: &str) -> Icon {
    icon(name).with_size(gpui_kit::component::Size::Small)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Accent,
    Success,
    Warning,
    Danger,
    Idle,
}

impl Tone {
    pub fn color(self, t: &Tokens) -> Hsla {
        match self {
            Tone::Accent => t.accent,
            Tone::Success => t.success,
            Tone::Warning => t.warning,
            Tone::Danger => t.danger,
            Tone::Idle => t.tint(0.35),
        }
    }
}

/// An 8px status dot; `pulse` breathes while something is running.
pub fn status_dot(tone: Tone, pulse: bool, t: &Tokens) -> AnyElement {
    let dot = div().size(u(8.)).rounded_full().bg(tone.color(t));
    if pulse {
        dot.with_animation(
            "status-pulse",
            Animation::new(std::time::Duration::from_millis(1600))
                .repeat()
                .with_easing(pulsating_between(0.45, 1.0)),
            |dot, delta| dot.opacity(delta),
        )
        .into_any_element()
    } else {
        dot.into_any_element()
    }
}

/// A thin accent progress bar; `value` is 0..100.
pub fn progress_bar(value: f64, height: f32, t: &Tokens) -> Div {
    let fraction = (value / 100.0).clamp(0.0, 1.0) as f32;
    div()
        .w_full()
        .h(u(height))
        .rounded_full()
        .bg(t.tint(0.1))
        .overflow_hidden()
        .child(div().h_full().w(relative(fraction)).rounded_full().bg(t.accent))
}

/// A shortcut hint (`⌘1`), set in the UI face.
pub fn kbd(text: impl Into<SharedString>, t: &Tokens) -> Div {
    div().text_meta().text_color(t.ink_tertiary).child(text.into())
}

/// The platform's modifier as it is written in hints.
pub const MOD_KEY: &str = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl+" };

/// A hairline-bordered card with a faint fill (DESIGN.md → Surfaces: `.card`).
pub fn card(t: &Tokens) -> Div {
    div()
        .rounded(u(12.))
        .border_1()
        .border_color(t.stroke)
        .bg(t.tint(0.03))
}

/// `.tile`: a raised option with a hover fill.
pub fn tile(t: &Tokens, selected: bool) -> Div {
    div()
        .rounded(u(8.))
        .border_1()
        .border_color(if selected { t.accent } else { t.stroke })
        .bg(if selected { t.selection } else { t.tint(0.03) })
}

/// Conditionally truncate a single line of text.
pub trait Truncate: Styled + Sized + FluentBuilder {
    fn one_line(self) -> Self {
        self.overflow_hidden().whitespace_nowrap().text_ellipsis()
    }
}

impl<T: Styled + FluentBuilder> Truncate for T {}

// ---- Buttons ----------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Variant {
    Primary,
    #[default]
    Secondary,
    Ghost,
    Danger,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BtnSize {
    Sm,
    #[default]
    Md,
    Lg,
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// Dense, flat button (Button.tsx): small radius, hairline borders, tinted hovers.
#[derive(IntoElement)]
pub struct Btn {
    id: ElementId,
    label: Option<SharedString>,
    icon: Option<SharedString>,
    trailing_icon: Option<SharedString>,
    variant: Variant,
    size: BtnSize,
    disabled: bool,
    loading: bool,
    tooltip: Option<SharedString>,
    on_click: Option<ClickHandler>,
}

impl Btn {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: None,
            icon: None,
            trailing_icon: None,
            variant: Variant::default(),
            size: BtnSize::default(),
            disabled: false,
            loading: false,
            tooltip: None,
            on_click: None,
        }
    }
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }
    /// A Lucide icon name; a button with an icon and no label is square.
    pub fn icon(mut self, name: impl Into<SharedString>) -> Self {
        self.icon = Some(name.into());
        self
    }
    pub fn trailing_icon(mut self, name: impl Into<SharedString>) -> Self {
        self.trailing_icon = Some(name.into());
        self
    }
    pub fn variant(mut self, variant: Variant) -> Self {
        self.variant = variant;
        self
    }
    pub fn primary(self) -> Self {
        self.variant(Variant::Primary)
    }
    pub fn ghost(self) -> Self {
        self.variant(Variant::Ghost)
    }
    pub fn danger(self) -> Self {
        self.variant(Variant::Danger)
    }
    pub fn size(mut self, size: BtnSize) -> Self {
        self.size = size;
        self
    }
    pub fn small(self) -> Self {
        self.size(BtnSize::Sm)
    }
    pub fn large(self) -> Self {
        self.size(BtnSize::Lg)
    }
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Btn {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = *cx.global::<Tokens>();
        let inactive = self.disabled || self.loading;
        let icon_only = self.label.is_none();
        let (height, pad, gap) = match self.size {
            BtnSize::Sm => (24., 8., 6.),
            BtnSize::Md => (28., 10., 6.),
            BtnSize::Lg => (32., 12., 8.),
        };
        let (bg, fg, border, hover_bg, hover_fg) = match (self.variant, inactive) {
            (Variant::Primary, false) => (t.accent, t.accent_ink, None, t.accent.opacity(0.88), t.accent_ink),
            (Variant::Primary, true) => (t.accent.opacity(0.3), t.accent_ink.opacity(0.6), None, t.accent.opacity(0.3), t.accent_ink.opacity(0.6)),
            (Variant::Secondary, false) => (transparent_black(), t.tint(0.8), Some(t.tint(0.1)), t.tint(0.1), t.ink),
            (Variant::Secondary, true) => (transparent_black(), t.tint(0.35), Some(t.tint(0.1)), transparent_black(), t.tint(0.35)),
            (Variant::Ghost, false) => (transparent_black(), t.ink_secondary, None, t.tint(0.1), t.ink),
            (Variant::Ghost, true) => (transparent_black(), t.tint(0.3), None, transparent_black(), t.tint(0.3)),
            (Variant::Danger, false) => (transparent_black(), t.danger, Some(t.danger.opacity(0.3)), t.danger.opacity(0.1), t.danger),
            (Variant::Danger, true) => (transparent_black(), t.danger.opacity(0.4), Some(t.danger.opacity(0.12)), transparent_black(), t.danger.opacity(0.4)),
        };
        let icon_size = if self.size == BtnSize::Lg { 16. } else { 14. };
        let tooltip = self.tooltip.clone();
        div()
            .id(self.id)
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .whitespace_nowrap()
            .rounded(u(6.))
            .h(u(height))
            .map(|b| if icon_only { b.w(u(height)) } else { b.px(u(pad)).gap(u(gap)) })
            .map(|b| if self.size == BtnSize::Lg { b.text_body() } else { b.text_caption() })
            .font_weight(FontWeight::MEDIUM)
            .bg(bg)
            .text_color(fg)
            .when_some(border, |b, color| b.border_1().border_color(color))
            .when(!inactive, |b| {
                b.cursor_pointer().hover(move |s| s.bg(hover_bg).text_color(hover_fg))
            })
            .when(self.loading, |b| b.child(spinner(icon_size, fg)))
            .when(!self.loading, |b| {
                b.when_some(self.icon, |b, name| {
                    b.child(icon(&name).size(u(icon_size)).text_color(fg))
                })
            })
            .when_some(self.label, |b, label| b.child(label))
            .when(!self.loading, |b| {
                b.when_some(self.trailing_icon, |b, name| {
                    b.child(icon(&name).size(u(icon_size)).text_color(fg))
                })
            })
            .when_some(tooltip, |b, text| {
                b.tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(text.clone()).build(window, cx)
                })
            })
            .when_some(self.on_click.filter(|_| !inactive), |b, handler| {
                b.on_click(move |event, window, cx| handler(event, window, cx))
            })
    }
}

/// The spinning loader icon.
pub fn spinner(size: f32, color: Hsla) -> AnyElement {
    icon("loader-circle")
        .size(u(size))
        .text_color(color)
        .with_animation(
            "spin",
            Animation::new(std::time::Duration::from_millis(900)).repeat(),
            |icon, delta| icon.rotate(percentage(delta)),
        )
        .into_any_element()
}

// ---- Surfaces ---------------------------------------------------------------

/// Page column inside the scroll area, under the title bar (Page.tsx).
pub fn page(max_width: f32) -> Div {
    div()
        .mx_auto()
        .w_full()
        .max_w(u(max_width))
        .px(u(32.))
        .pt(u(28.))
        .pb(u(48.))
}

pub const PAGE_FOCUS: f32 = 720.;
pub const PAGE_NARROW: f32 = 880.;
pub const PAGE_DEFAULT: f32 = 1280.;
pub const PAGE_WIDE: f32 = 1680.;

/// Title, description and actions at the top of a page (PageHeader.tsx).
pub fn page_header(
    title: impl Into<SharedString>,
    description: Option<SharedString>,
    actions: Option<AnyElement>,
    t: &Tokens,
) -> Div {
    div()
        .flex()
        .flex_wrap()
        .items_end()
        .justify_between()
        .gap_x(u(16.))
        .gap_y(u(8.))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .child(div().text_title().text_color(t.ink).child(title.into()))
                .when_some(description, |d, text| {
                    d.child(
                        div()
                            .mt(u(4.))
                            .max_w(u(560.))
                            .text_caption()
                            .text_color(t.ink_secondary)
                            .child(text),
                    )
                }),
        )
        .when_some(actions, |d, actions| {
            d.child(div().flex().flex_shrink_0().items_center().gap(u(6.)).child(actions))
        })
}

/// Card: the standard surface for a group of related content (Panel.tsx).
pub fn panel(t: &Tokens) -> Div {
    card(t).p(u(16.))
}

/// A panel's title row: optional icon tile, title, description, actions.
pub fn panel_header(
    icon_tile: Option<AnyElement>,
    title: impl Into<SharedString>,
    description: Option<SharedString>,
    action: Option<AnyElement>,
    t: &Tokens,
) -> Div {
    div()
        .flex()
        .items_start()
        .justify_between()
        .gap(u(12.))
        .child(
            div()
                .flex()
                .min_w_0()
                .items_start()
                .gap(u(12.))
                .children(icon_tile)
                .child(
                    div()
                        .min_w_0()
                        .child(div().text_label().text_color(t.ink).child(title.into()))
                        .when_some(description, |d, text| {
                            d.child(
                                div()
                                    .mt(u(2.))
                                    .max_w(u(560.))
                                    .text_caption()
                                    .text_color(t.ink_secondary)
                                    .child(text),
                            )
                        }),
                ),
        )
        .when_some(action, |d, action| {
            d.child(div().flex().flex_shrink_0().items_center().gap(u(8.)).child(action))
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Hue {
    #[default]
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
}

impl Hue {
    /// Text colour, fill and border for a tinted chip in this hue.
    fn colors(self, t: &Tokens) -> (Hsla, Hsla, Hsla) {
        let base = match self {
            Hue::Neutral => return (t.tint(0.7), t.tint(0.05), t.tint(0.1)),
            Hue::Accent => (t.accent_fg, t.accent),
            Hue::Success => (t.success, t.success),
            Hue::Warning => (t.warning, t.warning),
            Hue::Danger => (t.danger, t.danger),
        };
        (base.0, base.1.opacity(0.1), base.1.opacity(0.25))
    }
}

/// A small tinted square holding an icon (IconTile.tsx); `size` is 24/32/40/48.
pub fn icon_tile(name: &str, hue: Hue, size: f32, t: &Tokens) -> AnyElement {
    let (fg, bg, border) = hue.colors(t);
    let (radius, glyph) = match size as u32 {
        0..=24 => (6., 14.),
        25..=32 => (8., 16.),
        33..=40 => (8., 18.),
        _ => (12., 20.),
    };
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(u(size))
        .rounded(u(radius))
        .bg(bg)
        .border_1()
        .border_color(border)
        .child(icon(name).size(u(glyph)).text_color(fg))
        .into_any_element()
}

/// A compact status label: tinted fill, hairline border, `text-meta` (Badge.tsx).
pub fn badge(text: impl Into<SharedString>, hue: Hue, t: &Tokens) -> Div {
    let (fg, bg, border) = match hue {
        Hue::Neutral => (t.ink_secondary, t.tint(0.05), t.tint(0.1)),
        _ => hue.colors(t),
    };
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(u(4.))
        .min_h(u(18.))
        .px(u(6.))
        .rounded(u(4.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_meta()
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg)
        .child(text.into())
}

/// A labelled setting: title and description on the left, control on the right.
pub fn setting_row(
    title: impl Into<SharedString>,
    description: Option<SharedString>,
    control: impl IntoElement,
    bare: bool,
    t: &Tokens,
) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(u(16.))
        .when(!bare, |row| {
            row.rounded(u(8.))
                .border_1()
                .border_color(t.stroke)
                .bg(t.tint(0.03))
                .px(u(12.))
                .py(u(10.))
        })
        .child(
            div()
                .min_w_0()
                .child(div().text_label().text_color(t.ink).child(title.into()))
                .when_some(description, |d, text| {
                    d.child(
                        div()
                            .mt(u(2.))
                            .max_w(u(520.))
                            .text_caption()
                            .text_color(t.ink_secondary)
                            .child(text),
                    )
                }),
        )
        .child(control)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalloutTone {
    Danger,
    Warning,
    Success,
    Info,
}

/// Tinted strip with a hairline border for errors, warnings and confirmations.
pub fn callout(
    tone: CalloutTone,
    title: Option<SharedString>,
    body: Option<AnyElement>,
    action: Option<AnyElement>,
    on_dismiss: Option<ClickHandler>,
    t: &Tokens,
) -> Div {
    let (border, bg, icon_color, icon_name) = match tone {
        CalloutTone::Danger => (t.danger.opacity(0.25), t.danger.opacity(0.06), t.danger, "triangle-alert"),
        CalloutTone::Warning => (t.warning.opacity(0.25), t.warning.opacity(0.06), t.warning, "triangle-alert"),
        CalloutTone::Success => (t.success.opacity(0.25), t.success.opacity(0.06), t.success, "check"),
        CalloutTone::Info => (t.tint(0.1), t.tint(0.03), t.ink_secondary, "info"),
    };
    let has_title = title.is_some();
    div()
        .flex()
        .items_start()
        .gap(u(8.))
        .rounded(u(8.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .px(u(12.))
        .py(u(8.))
        .child(div().mt(u(2.)).flex_shrink_0().child(icon(icon_name).size(u(14.)).text_color(icon_color)))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .when_some(title, |d, title| d.child(div().text_label().text_color(t.ink).child(title)))
                .when_some(body, |d, body| {
                    d.child(
                        div()
                            .text_caption()
                            .when(has_title, |d| d.mt(u(2.)).text_color(t.ink_secondary))
                            .when(!has_title, |d| d.text_color(t.ink))
                            .child(body),
                    )
                }),
        )
        .when_some(action, |d, action| {
            d.child(div().flex().flex_shrink_0().items_center().gap(u(8.)).child(action))
        })
        .when_some(on_dismiss, |d, handler| {
            d.child(
                Btn::new("dismiss")
                    .ghost()
                    .small()
                    .icon("x")
                    .tooltip("Dismiss")
                    .on_click(handler),
            )
        })
}

/// Centered placeholder for an empty list (EmptyState.tsx).
pub fn empty_state(
    icon_name: &str,
    title: impl Into<SharedString>,
    description: Option<SharedString>,
    action: Option<AnyElement>,
    t: &Tokens,
) -> Div {
    card(t)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .px(u(24.))
        .py(u(40.))
        .child(div().mb(u(12.)).child(icon_tile(icon_name, Hue::Neutral, 32., t)))
        .child(div().text_label().text_color(t.ink).child(title.into()))
        .when_some(description, |d, text| {
            d.child(
                div()
                    .mt(u(4.))
                    .max_w(u(360.))
                    .text_center()
                    .text_caption()
                    .text_color(t.ink_secondary)
                    .child(text),
            )
        })
        .when_some(action, |d, action| d.child(div().mt(u(16.)).child(action)))
}

/// Loading placeholder: a faint flat block that pulses.
pub fn skeleton(t: &Tokens) -> AnyElement {
    div()
        .rounded(u(6.))
        .bg(t.tint(0.05))
        .size_full()
        .with_animation(
            "skeleton",
            Animation::new(std::time::Duration::from_millis(2000))
                .repeat()
                .with_easing(pulsating_between(0.5, 1.0)),
            |d, delta| d.opacity(delta),
        )
        .into_any_element()
}

/// Single choice in a hairline track; the chosen segment gets the selection fill.
/// Options are `(value, label, disabled reason)`.
pub fn segmented<T: Clone + PartialEq + 'static>(
    id: impl Into<ElementId>,
    value: &T,
    options: Vec<(T, SharedString, Option<SharedString>)>,
    small: bool,
    on_change: impl Fn(&T, &mut Window, &mut App) + 'static,
    t: &Tokens,
) -> Stateful<Div> {
    let on_change = std::rc::Rc::new(on_change);
    let t = *t;
    div()
        .id(id.into())
        .flex()
        .items_center()
        .gap(px(2.))
        .rounded(u(6.))
        .border_1()
        .border_color(t.tint(0.1))
        .p(px(2.))
        .children(options.into_iter().enumerate().map(|(index, (option, label, disabled))| {
            let selected = &option == value;
            let on_change = on_change.clone();
            div()
                .id(index)
                .rounded(px(4.))
                .map(|s| if small { s.h(u(20.)).px(u(8.)).text_meta() } else { s.h(u(24.)).px(u(10.)).text_caption() })
                .flex()
                .items_center()
                .map(|s| match (&disabled, selected) {
                    (Some(_), _) => s.text_color(t.tint(0.25)),
                    (None, true) => s.bg(t.selection).text_color(t.ink),
                    (None, false) => s
                        .text_color(t.ink_secondary)
                        .cursor_pointer()
                        .hover(|s| s.text_color(t.ink)),
                })
                .when_some(disabled.clone(), |s, reason| {
                    s.tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(reason.clone()).build(window, cx)
                    })
                })
                .when(disabled.is_none(), |s| {
                    s.on_click(move |_, window, cx| on_change(&option, window, cx))
                })
                .child(label)
        }))
}

/// A two-state switch (28×16 track), as Switch.tsx.
pub fn switch(
    id: impl Into<ElementId>,
    on: bool,
    disabled: bool,
    on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
    t: &Tokens,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex_shrink_0()
        .w(u(28.))
        .h(u(16.))
        .rounded_full()
        .p(px(2.))
        .bg(if on { t.accent } else { t.tint(0.18) })
        .when(disabled, |s| s.opacity(0.4))
        .when(!disabled, |s| {
            s.cursor_pointer().on_click(move |_, window, cx| on_toggle(!on, window, cx))
        })
        .child(
            div()
                .size(u(12.))
                .rounded_full()
                .bg(if on { t.accent_ink } else { t.ink })
                .when(on, |k| k.ml(u(12.))),
        )
}

/// A checkbox (Checkbox.tsx): `None` is the indeterminate state.
pub fn checkbox(
    id: impl Into<ElementId>,
    checked: Option<bool>,
    on_toggle: impl Fn(bool, &mut Window, &mut App) + 'static,
    t: &Tokens,
) -> Stateful<Div> {
    let filled = checked != Some(false);
    div()
        .id(id.into())
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(u(16.))
        .rounded(u(4.))
        .border_1()
        .border_color(if filled { t.accent } else { t.tint(0.3) })
        .bg(if filled { t.accent } else { transparent_black() })
        .cursor_pointer()
        .on_click(move |_, window, cx| on_toggle(checked != Some(true), window, cx))
        .when(filled, |b| {
            b.child(
                icon(if checked.is_none() { "minus" } else { "check" })
                    .size(u(12.))
                    .text_color(t.accent_ink),
            )
        })
}

/// Circular progress with a solid accent stroke (ProgressRing); `value` is 0..100.
pub fn progress_ring(value: f64, size: f32, stroke: f32, t: &Tokens) -> Canvas<()> {
    let fraction = (value / 100.0).clamp(0.0, 1.0) as f32;
    let track = t.tint(0.1);
    let accent = t.accent;
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let scale = bounds.size.width / u(size).to_pixels(window.rem_size());
            let stroke = px(stroke) * scale;
            let center = bounds.center();
            let radius = bounds.size.width.min(bounds.size.height) / 2.0 - stroke / 2.0;
            let arc = |from: f32, to: f32| {
                let mut path = PathBuilder::stroke(stroke);
                let steps = ((to - from) * 96.0).ceil().max(1.0) as usize;
                for i in 0..=steps {
                    let angle = (from + (to - from) * i as f32 / steps as f32)
                        * std::f32::consts::TAU
                        - std::f32::consts::FRAC_PI_2;
                    let p = point(center.x + radius * angle.cos(), center.y + radius * angle.sin());
                    if i == 0 {
                        path.move_to(p);
                    } else {
                        path.line_to(p);
                    }
                }
                path.build()
            };
            if let Ok(path) = arc(0.0, 1.0) {
                window.paint_path(path, track);
            }
            if fraction > 0.0 {
                if let Ok(path) = arc(0.0, fraction) {
                    window.paint_path(path, accent);
                }
            }
        },
    )
    .size(u(size))
}

/// Quiet link back to the list a run was opened from (ClipList.tsx BackLink).
pub fn back_link(
    label: SharedString,
    t: &Tokens,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id("back-link")
        .ml(u(-8.))
        .flex()
        .items_center()
        .gap(u(4.))
        .h(u(24.))
        .pl(u(6.))
        .pr(u(8.))
        .rounded(u(6.))
        .text_caption()
        .text_color(t.ink_secondary)
        .cursor_pointer()
        .hover(|s| s.bg(t.tint(0.1)).text_color(t.ink))
        .on_click(on_click)
        .child(icon("arrow-left").size(u(14.)))
        .child(label)
}
