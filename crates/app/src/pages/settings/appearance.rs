//! Settings → Appearance (SettingsPage.tsx AppearanceSettings, from MonoCode's
//! appearance page, MIT): theme and accent, the tint of the neutral chrome, window
//! glass, and layout. Every change shows at once: `Settings::set_appearance` saves
//! and re-themes the window. Slider and picker drags update their value on every
//! move but save at most every 100 ms (and on release), so dragging stays smooth.
use super::SettingsPage;
use super::widgets::more_toggle;
use crate::theme::Tokens;
use crate::ui::{self, Btn, Hue, TypeScale, u};
use clipward_core::appearance::{
    self as look, ACCENT_COLOR_LABELS, ACCENT_COLOR_PRESETS, Appearance, ThemePreference,
};
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::time::{Duration, Instant};

/// Longest gap between saves while a slider or the picker is dragged.
const DRAG_SAVE_EVERY: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliderKind {
    Hue,
    Saturation,
    DarkLightness,
    SidebarOpacity,
    BlurRadius,
    UiScale,
}

impl SliderKind {
    pub const ALL: [SliderKind; 6] = [
        SliderKind::Hue,
        SliderKind::Saturation,
        SliderKind::DarkLightness,
        SliderKind::SidebarOpacity,
        SliderKind::BlurRadius,
        SliderKind::UiScale,
    ];

    /// (min, max, step) as the slider shows them: opacity and scale in percent.
    fn range(self) -> (f32, f32, f32) {
        let percent = |r: look::Range| ((r.min * 100.0).round() as f32, (r.max * 100.0).round() as f32);
        match self {
            SliderKind::Hue => (look::HUE.min as f32, look::HUE.max as f32, 1.),
            SliderKind::Saturation => (look::SATURATION.min as f32, look::SATURATION.max as f32, 1.),
            SliderKind::DarkLightness => (look::DARK_LIGHTNESS.min as f32, look::DARK_LIGHTNESS.max as f32, 1.),
            SliderKind::SidebarOpacity => {
                let (min, max) = percent(look::SIDEBAR_OPACITY);
                (min, max, 1.)
            }
            SliderKind::BlurRadius => (look::BLUR_RADIUS.min as f32, look::BLUR_RADIUS.max as f32, 1.),
            SliderKind::UiScale => {
                let (min, max) = percent(look::UI_SCALE);
                (min, max, 10.)
            }
        }
    }

    fn value(self, a: &Appearance) -> f32 {
        let v = match self {
            SliderKind::Hue => a.hue,
            SliderKind::Saturation => a.saturation,
            SliderKind::DarkLightness => a.dark_lightness,
            SliderKind::SidebarOpacity => a.sidebar_opacity * 100.0,
            SliderKind::BlurRadius => a.blur_radius,
            SliderKind::UiScale => a.ui_scale * 100.0,
        };
        v.round() as f32
    }

    fn set(self, a: &mut Appearance, value: f32) {
        let v = value.round() as f64;
        match self {
            SliderKind::Hue => a.hue = v,
            SliderKind::Saturation => a.saturation = v,
            SliderKind::DarkLightness => a.dark_lightness = v,
            SliderKind::SidebarOpacity => a.sidebar_opacity = v / 100.0,
            SliderKind::BlurRadius => a.blur_radius = v,
            SliderKind::UiScale => a.ui_scale = v / 100.0,
        }
    }

    fn display(self, a: &Appearance) -> String {
        let v = self.value(a);
        match self {
            SliderKind::Hue => format!("{v}°"),
            SliderKind::BlurRadius => format!("{v}"),
            _ => format!("{v}%"),
        }
    }

    pub fn state(self, a: &Appearance) -> SliderState {
        let (min, max, step) = self.range();
        SliderState::new().min(min).max(max).step(step).default_value(self.value(a))
    }
}

/// Clipward's own accent in each scheme (globals.css `--accent`).
fn default_accent(light: bool) -> Hsla {
    let v = if light { 46. / 255. } else { 235. / 255. };
    Rgba { r: v, g: v, b: v, a: 1.0 }.into()
}

impl SettingsPage {
    /// The appearance on screen: a drag in progress, else the saved one.
    pub(super) fn appearance(&self, cx: &App) -> Appearance {
        self.appearance_draft.clone().unwrap_or_else(|| self.settings.read(cx).appearance())
    }

    /// Show `next` and save it; `throttle` (drags) saves at most every 100 ms.
    pub(super) fn change_appearance(&mut self, next: Appearance, throttle: bool, cx: &mut Context<Self>) {
        self.appearance_draft = Some(next);
        let wait = self
            .appearance_saved_at
            .map_or(Duration::ZERO, |at| DRAG_SAVE_EVERY.saturating_sub(at.elapsed()));
        if !throttle || wait.is_zero() {
            self.flush_appearance(cx);
            return;
        }
        if self.appearance_flush.is_none() {
            self.appearance_flush = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                let _ = this.update(cx, |this, cx| this.flush_appearance(cx));
            }));
        }
        cx.notify();
    }

    fn flush_appearance(&mut self, cx: &mut Context<Self>) {
        self.appearance_flush = None;
        let Some(next) = self.appearance_draft.take() else { return };
        self.appearance_saved_at = Some(Instant::now());
        let ok = self.settings.update(cx, |s, cx| s.set_appearance(next, cx));
        self.saved_result(ok, "Could not save appearance settings", cx);
    }

    pub(super) fn on_slider(&mut self, kind: SliderKind, event: &SliderEvent, cx: &mut Context<Self>) {
        let (value, release) = match event {
            SliderEvent::Change(value) => (value.start(), false),
            SliderEvent::Release(value) => (value.start(), true),
        };
        let mut next = self.appearance(cx);
        kind.set(&mut next, value);
        if release || next != self.appearance(cx) {
            self.change_appearance(next, !release, cx);
        }
    }

    /// Follow appearance changed elsewhere (Reset, ⌘= / ⌘- / ⌘0) unless mid-drag.
    pub(super) fn sync_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.appearance_draft.is_some() {
            return;
        }
        let saved = self.settings.read(cx).appearance();
        for (kind, slider) in &self.sliders {
            let value = kind.value(&saved);
            if slider.read(cx).value().start() != value {
                slider.update(cx, |slider, cx| slider.set_value(value, window, cx));
            }
        }
        if let Some(color) = saved.accent_color.clone() {
            self.picker.update(cx, |picker, cx| picker.set_color(&color, window, cx));
        }
    }

    fn reset_appearance(&mut self, cx: &mut Context<Self>) {
        self.change_appearance(Appearance::default(), false, cx);
        let s = self.saved(cx);
        if s.window_glass_supported && !s.window_glass {
            let ok = self.settings.update(cx, |s, cx| s.set_window_glass(true, cx));
            self.saved_result(ok, "Could not change window transparency", cx);
        }
    }

    fn set_accent(&mut self, color: Option<String>, close: bool, window: &mut Window, cx: &mut Context<Self>) {
        if close {
            self.accent_open = false;
        }
        if let Some(color) = &color {
            self.picker.update(cx, |picker, cx| picker.set_color(color, window, cx));
        }
        let mut next = self.appearance(cx);
        next.accent_color = color;
        self.change_appearance(next, false, cx);
    }

    fn slider(&self, kind: SliderKind, disabled: bool, a: &Appearance, t: &Tokens) -> AnyElement {
        let state = &self.sliders.iter().find(|(k, _)| *k == kind).expect("every slider kind has a state").1;
        div()
            .w(u(224.))
            .max_w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(u(12.))
            .when(disabled, |d| d.opacity(0.4))
            .child(
                div().flex_1().min_w_0().child(
                    Slider::new(state)
                        .disabled(disabled)
                        .bg(t.tint(0.5))
                        .text_color(t.ink),
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
                    .child(kind.display(a)),
            )
            .into_any_element()
    }

    fn accent_picker(&self, a: &Appearance, light: bool, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let t = *t;
        let value = a.accent_color.clone();
        let preset = value.as_deref().map(|v| ACCENT_COLOR_PRESETS.iter().position(|p| *p == v));
        // None: Default; Some(Some(i)): preset i; Some(None): a custom colour.
        let custom = match preset {
            Some(None) => value.clone(),
            _ => None,
        };
        let selected_index = match preset {
            None => Some(0),
            Some(Some(i)) => Some(i + 1),
            Some(None) => None,
        };
        let colors = std::iter::once(default_accent(light))
            .chain(ACCENT_COLOR_PRESETS.iter().map(|hex| crate::theme::hex(hex).unwrap_or(default_accent(light))));
        let labels = std::iter::once("Default").chain(ACCENT_COLOR_LABELS);
        let ring = move |selected: bool, inner: Div| {
            div()
                .flex()
                .items_center()
                .justify_center()
                .size(u(24.))
                .rounded_full()
                .child(if selected {
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(u(22.))
                        .rounded_full()
                        .border_2()
                        .border_color(t.tint(0.8))
                        .child(inner)
                } else {
                    inner
                })
        };
        let open = self.accent_open;
        let swatches = div()
            .flex()
            .items_center()
            .gap(u(4.))
            .children(colors.zip(labels).enumerate().map(|(i, (color, label))| {
                let selected = custom.is_none() && selected_index == Some(i);
                div()
                    .id(("accent", i))
                    .cursor_pointer()
                    .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(label).build(window, cx))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let color = (i > 0).then(|| ACCENT_COLOR_PRESETS[i - 1].to_string());
                        this.set_accent(color, true, window, cx);
                    }))
                    .child(ring(selected, div().size(u(16.)).rounded_full().bg(color)))
            }))
            .child(
                div()
                    .id("accent-custom")
                    .cursor_pointer()
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Custom color").build(window, cx))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.accent_open = !this.accent_open;
                        if this.accent_open {
                            this.accent_focus.focus(window, cx);
                        }
                        cx.notify();
                    }))
                    .child(ring(
                        custom.is_some() || open,
                        match custom.as_deref().and_then(crate::theme::hex) {
                            Some(color) => div().size(u(16.)).rounded_full().bg(color),
                            // The conic rainbow, as a sweep of its ends, with the pipette.
                            None => div()
                                .size(u(16.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(linear_gradient(
                                    135.,
                                    linear_color_stop(hsla(0.0, 1.0, 0.5, 1.0), 0.),
                                    linear_color_stop(hsla(0.6, 1.0, 0.5, 1.0), 1.),
                                ))
                                .child(ui::icon("pipette").size(u(10.)).text_color(white())),
                        },
                    )),
            );
        let bounds = self.swatch_bounds.clone();
        div()
            .relative()
            .flex_shrink_0()
            .on_children_prepainted(move |children, _, _| {
                if let Some(row) = children.first() {
                    *bounds.borrow_mut() = *row;
                }
            })
            .child(swatches)
            .when(open, |root| {
                root.child(
                    deferred(
                        div()
                            .id("accent-popover")
                            .track_focus(&self.accent_focus)
                            .occlude()
                            .absolute()
                            .right_0()
                            .top(u(32.))
                            .w(u(248.))
                            .p(u(8.))
                            .rounded(u(12.))
                            .border_1()
                            .border_color(t.tint(0.1))
                            .bg(t.elevated)
                            .shadow_lg()
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                                if event.keystroke.key == "escape" {
                                    this.accent_open = false;
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            }))
                            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                // The swatch row toggles and picks on its own.
                                if !this.swatch_bounds.borrow().contains(&event.position) {
                                    this.accent_open = false;
                                    cx.notify();
                                }
                            }))
                            .child(self.picker.clone()),
                    )
                    .with_priority(1),
                )
            })
            .into_any_element()
    }

    pub(super) fn appearance_panel(&self, window: &Window, t: &Tokens, cx: &mut Context<Self>) -> AnyElement {
        let a = self.appearance(cx);
        let s = self.saved(cx);
        let light = !t.dark;
        let glass_on = s.window_glass_supported && s.window_glass;
        let glass_off = !glass_on || light;
        let _ = window;
        let translucency_note = if !s.window_glass_supported {
            "Window transparency is available on macOS and Windows."
        } else if light {
            "Light mode always uses an opaque window, so these are off. Your dark-mode values are preserved."
        } else if !glass_on {
            "Turn on window transparency to adjust these. Your values are preserved."
        } else {
            "How much of the desktop shows through Clipward. Blur costs more to composite the higher it goes."
        };
        let group = |title: &'static str, description: Option<&'static str>, rows: Vec<AnyElement>| {
            div()
                .mt(u(20.))
                .child(super::widgets::eyebrow(title, t))
                .when_some(description, |d, text| {
                    d.child(div().mt(u(2.)).max_w(u(560.)).text_caption().text_color(t.ink_secondary).child(text))
                })
                .child(div().mt(u(8.)).flex().flex_col().gap(u(6.)).children(rows))
        };
        let weak = cx.weak_entity();
        let toggle = move |set: fn(&mut Appearance, bool)| {
            let weak = weak.clone();
            move |on: bool, _: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| {
                    let mut next = this.appearance(cx);
                    set(&mut next, on);
                    this.change_appearance(next, false, cx);
                });
            }
        };
        let weak = cx.weak_entity();
        let theme = ui::segmented(
            "theme",
            &a.theme,
            vec![
                (ThemePreference::System, "System".into(), None),
                (ThemePreference::Dark, "Dark".into(), None),
                (ThemePreference::Light, "Light".into(), None),
            ],
            false,
            move |theme, _, cx| {
                let theme = *theme;
                let _ = weak.update(cx, |this, cx| {
                    let mut next = this.appearance(cx);
                    next.theme = theme;
                    this.change_appearance(next, false, cx);
                });
            },
            t,
        );
        let weak = cx.weak_entity();
        let glass_switch = ui::switch(
            "window-glass",
            glass_on,
            !s.window_glass_supported,
            move |on, _, cx| {
                let _ = weak.update(cx, |this, cx| {
                    let ok = this.settings.update(cx, |s, cx| s.set_window_glass(on, cx));
                    this.saved_result(ok, "Could not change window transparency", cx);
                });
            },
            t,
        );
        let row = |title: &'static str, description: String, control: AnyElement| {
            ui::setting_row(title, Some(description.into()), control, false, t).into_any_element()
        };
        let mod_key = ui::MOD_KEY;
        ui::panel(t)
            .child(ui::panel_header(
                Some(ui::icon_tile("app-window", Hue::Neutral, 24., t)),
                "Look",
                Some("How Clipward looks on this computer.".into()),
                Some(
                    Btn::new("reset-appearance")
                        .small()
                        .ghost()
                        .icon("rotate-ccw")
                        .label("Reset")
                        .on_click(cx.listener(|this, _, _, cx| this.reset_appearance(cx)))
                        .into_any_element(),
                ),
                t,
            ))
            .child(div().mt(u(16.)).flex().flex_col().gap(u(6.)).children([
                row("Theme", "System matches your computer.".into(), theme.into_any_element()),
                row("Accent color", "Buttons, selections and progress.".into(), self.accent_picker(&a, light, t, cx)),
                row(
                    "Text size",
                    format!("Make everything bigger or smaller. You can also use {mod_key}= and {mod_key}-."),
                    self.slider(SliderKind::UiScale, false, &a, t),
                ),
            ]))
            .child(group(
                "Translucency",
                Some(translucency_note),
                vec![
                    row(
                        "Window transparency",
                        if s.window_glass_supported {
                            "The desktop shows softly through the sidebar, and the pages with main pane glass, in dark mode. Video, cards and fields stay solid."
                        } else {
                            "Available on macOS and Windows."
                        }
                        .into(),
                        glass_switch.into_any_element(),
                    ),
                    row(
                        "Sidebar opacity",
                        "How solid the sidebar is over the glass. With main pane glass, the pages too.".into(),
                        self.slider(SliderKind::SidebarOpacity, glass_off, &a, t),
                    ),
                    row(
                        "Blur radius",
                        if s.window_blur_supported || !s.window_glass_supported {
                            "Background blur behind the window."
                        } else {
                            "Windows draws Mica and acrylic with a fixed blur."
                        }
                        .into(),
                        self.slider(SliderKind::BlurRadius, glass_off || !s.window_blur_supported, &a, t),
                    ),
                    row(
                        "Main pane glass",
                        "Extend the translucent treatment to the pages beside the sidebar.".into(),
                        ui::switch("main-pane-glass", a.main_pane_glass, glass_off, toggle(|a, on| a.main_pane_glass = on), t)
                            .into_any_element(),
                    ),
                ],
            ))
            .child(
                div().mt(u(12.)).flex().child(more_toggle("appearance-more", self.appearance_more).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.appearance_more = !this.appearance_more;
                        cx.notify();
                    },
                ))),
            )
            .when(self.appearance_more, |panel| panel.child(group(
                "Color",
                Some("Hue and saturation tint every surface. Lightness only moves the dark theme."),
                vec![
                    row("Hue", "Base hue for tinted surfaces.".into(), self.slider(SliderKind::Hue, false, &a, t)),
                    row(
                        "Saturation",
                        "How strongly the hue tints the interface. Zero keeps it neutral.".into(),
                        self.slider(SliderKind::Saturation, false, &a, t),
                    ),
                    row(
                        "Dark-mode lightness",
                        if light {
                            "This only affects dark mode. Your dark-mode value is preserved."
                        } else {
                            "Base brightness of the dark theme. Lower values are darker; zero is true black."
                        }
                        .into(),
                        self.slider(SliderKind::DarkLightness, light, &a, t),
                    ),
                ],
            ))
            .child(group(
                "Layout",
                None,
                vec![
                    row(
                        "Collapsed rail",
                        format!(
                            "Show the sidebar as a compact icon rail. {mod_key}\\ toggles it. Narrow windows always use the rail."
                        ),
                        ui::switch("collapsed-rail", a.collapsed_rail, false, toggle(|a, on| a.collapsed_rail = on), t)
                            .into_any_element(),
                    ),
                ],
            )))
            .into_any_element()
    }
}

