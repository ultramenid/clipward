//! The custom accent picker (ColorSwatches.tsx ColorPicker): a saturation and
//! brightness square, a hue strip and a hex field. Drag either, or focus one and
//! use the arrow keys. Ported from MonoCode's ColorPickerPopover (MIT, see
//! THIRD_PARTY_NOTICES.md).
use crate::theme::ActiveTokens;
use crate::ui::{TypeScale, u};
use clipward_core::color::{Hsv, hex_to_hsv, hsv_to_hex, normalize_hex};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable, Size};
use gpui_kit::*;
use std::cell::Cell;
use std::rc::Rc;

/// A new colour, `#rrggbb`.
pub struct ColorChanged(pub String);

pub struct ColorPicker {
    hsv: Hsv,
    hex: Entity<InputState>,
    square_focus: FocusHandle,
    strip_focus: FocusHandle,
    _subscription: Subscription,
}

impl EventEmitter<ColorChanged> for ColorPicker {}

const STEP: f64 = 2.0;

impl ColorPicker {
    pub fn new(value: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let hsv = hex_to_hsv(value);
        let preview = hsv_to_hex(hsv.h, hsv.s, hsv.v);
        let hex = cx.new(|cx| InputState::new(window, cx).default_value(preview));
        let subscription = cx.subscribe_in(&hex, window, |this, input, event: &InputEvent, window, cx| match event {
            InputEvent::Change => {
                let raw = input.read(cx).value().trim().to_string();
                let digits = raw.strip_prefix('#').unwrap_or(&raw);
                if digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_hexdigit()) {
                    let color = normalize_hex(&format!("#{digits}"));
                    this.hsv = hex_to_hsv(&color);
                    cx.emit(ColorChanged(color));
                    cx.notify();
                }
            }
            // Leaving the field drops an unfinished draft.
            InputEvent::Blur => this.show_hex(window, cx),
            _ => {}
        });
        Self {
            hsv,
            hex,
            square_focus: cx.focus_handle().tab_index(0).tab_stop(true),
            strip_focus: cx.focus_handle().tab_index(0).tab_stop(true),
            _subscription: subscription,
        }
    }

    /// Follow a colour chosen elsewhere; the same colour keeps its hue (grey has none).
    pub fn set_color(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let hex = normalize_hex(value);
        if hsv_to_hex(self.hsv.h, self.hsv.s, self.hsv.v) != hex {
            self.hsv = hex_to_hsv(&hex);
            self.show_hex(window, cx);
            cx.notify();
        }
    }

    fn show_hex(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let preview = hsv_to_hex(self.hsv.h, self.hsv.s, self.hsv.v);
        if self.hex.read(cx).value() != preview {
            self.hex.update(cx, |input, cx| input.set_value(preview, window, cx));
        }
    }

    fn apply(&mut self, hsv: Hsv, window: &mut Window, cx: &mut Context<Self>) {
        self.hsv = hsv;
        self.show_hex(window, cx);
        cx.emit(ColorChanged(hsv_to_hex(hsv.h, hsv.s, hsv.v)));
        cx.notify();
    }

    fn on_square_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let (ds, dv) = match event.keystroke.key.as_str() {
            "left" => (-STEP, 0.0),
            "right" => (STEP, 0.0),
            "up" => (0.0, STEP),
            "down" => (0.0, -STEP),
            _ => return,
        };
        let hsv = Hsv {
            s: (self.hsv.s + ds).clamp(0.0, 100.0),
            v: (self.hsv.v + dv).clamp(0.0, 100.0),
            ..self.hsv
        };
        self.apply(hsv, window, cx);
        cx.stop_propagation();
    }

    fn on_strip_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let dh = match event.keystroke.key.as_str() {
            "left" => -STEP,
            "right" => STEP,
            _ => return,
        };
        let hsv = Hsv { h: (self.hsv.h + dh).clamp(0.0, 360.0), ..self.hsv };
        self.apply(hsv, window, cx);
        cx.stop_propagation();
    }
}

/// Dragging over an element's box, reported as 0–1 fractions of it. Pressing
/// starts the drag; it follows the pointer anywhere until release.
fn drag_area(
    focus: FocusHandle,
    on_move: impl Fn(f32, f32, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let on_move = Rc::new(on_move);
    let dragging = Rc::new(Cell::new(false));
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |bounds, hitbox, window, _| {
            let fraction = move |p: Point<Pixels>| {
                (
                    ((p.x - bounds.left()) / bounds.size.width).clamp(0.0, 1.0),
                    ((p.y - bounds.top()) / bounds.size.height).clamp(0.0, 1.0),
                )
            };
            window.set_cursor_style(CursorStyle::PointingHand, &hitbox);
            {
                let (on_move, dragging) = (on_move.clone(), dragging.clone());
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble && event.button == MouseButton::Left && hitbox.is_hovered(window) {
                        dragging.set(true);
                        focus.focus(window, cx);
                        let (x, y) = fraction(event.position);
                        on_move(x, y, window, cx);
                        cx.stop_propagation();
                    }
                });
            }
            {
                let (on_move, dragging) = (on_move.clone(), dragging.clone());
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && dragging.get() {
                        if event.pressed_button == Some(MouseButton::Left) {
                            let (x, y) = fraction(event.position);
                            on_move(x, y, window, cx);
                        } else {
                            dragging.set(false);
                        }
                    }
                });
            }
            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, _| {
                if phase == DispatchPhase::Capture {
                    dragging.set(false);
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

fn white() -> Hsla {
    hsla(0.0, 0.0, 1.0, 1.0)
}

fn color(hex: &str) -> Hsla {
    crate::theme::hex(hex).unwrap_or(white())
}

impl Render for ColorPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.tokens();
        let hsv = self.hsv;
        let preview = color(&hsv_to_hex(hsv.h, hsv.s, hsv.v));
        let hue_color = color(&hsv_to_hex(hsv.h, 100.0, 100.0));
        let ring = |focused: bool| if focused { t.accent } else { transparent_black() };
        let handle = |fill: Hsla| {
            div()
                .absolute()
                .size(u(14.))
                .ml(u(-7.))
                .mt(u(-7.))
                .rounded_full()
                .border_2()
                .border_color(white())
                .shadow_md()
                .bg(fill)
        };
        let weak = cx.weak_entity();
        let square = div()
            .id("sv-square")
            .track_focus(&self.square_focus)
            .on_key_down(cx.listener(Self::on_square_key))
            .relative()
            .h(u(112.))
            .w_full()
            .rounded(u(6.))
            .border_1()
            .border_color(ring(self.square_focus.is_focused(window)))
            .bg(linear_gradient(90., linear_color_stop(white(), 0.), linear_color_stop(hue_color, 1.)))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .rounded(u(6.))
                    .bg(linear_gradient(0., linear_color_stop(black(), 0.), linear_color_stop(black().opacity(0.), 1.))),
            )
            .child(
                handle(preview)
                    .left(relative((hsv.s / 100.0) as f32))
                    .top(relative(1.0 - (hsv.v / 100.0) as f32)),
            )
            .child(drag_area(self.square_focus.clone(), {
                let weak = weak.clone();
                move |x, y, window, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        let hsv = Hsv { s: x as f64 * 100.0, v: (1.0 - y as f64) * 100.0, ..this.hsv };
                        this.apply(hsv, window, cx);
                    });
                }
            }));
        let hues = ["#ff0000", "#ffff00", "#00ff00", "#00ffff", "#0000ff", "#ff00ff", "#ff0000"];
        let strip = div()
            .id("hue-strip")
            .track_focus(&self.strip_focus)
            .on_key_down(cx.listener(Self::on_strip_key))
            .relative()
            .h(u(12.))
            .w_full()
            .rounded_full()
            .border_1()
            .border_color(ring(self.strip_focus.is_focused(window)))
            .child(
                div().size_full().rounded_full().overflow_hidden().flex().children(hues.windows(2).map(|pair| {
                    div()
                        .flex_1()
                        .h_full()
                        .bg(linear_gradient(90., linear_color_stop(color(pair[0]), 0.), linear_color_stop(color(pair[1]), 1.)))
                })),
            )
            .child(handle(hue_color).left(relative((hsv.h / 360.0) as f32)).top(relative(0.5)))
            .child(drag_area(self.strip_focus.clone(), move |x, _, window, cx| {
                let _ = weak.update(cx, |this, cx| {
                    let hsv = Hsv { h: x as f64 * 360.0, ..this.hsv };
                    this.apply(hsv, window, cx);
                });
            }));
        div()
            .flex()
            .flex_col()
            .gap(u(8.))
            .child(square)
            .child(strip)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .child(
                        div()
                            .flex_shrink_0()
                            .size(u(28.))
                            .rounded(u(6.))
                            .border_1()
                            .border_color(t.tint(0.1))
                            .bg(preview),
                    )
                    .child(
                        div().flex_1().min_w_0().child(
                            Input::new(&self.hex).with_size(Size::Small).font_family(crate::MONO_FONT).text_caption(),
                        ),
                    ),
            )
    }
}
