//! Drawing a game board as a dot matrix (Arcade.tsx `paint`).
use clipward_core::jobs_page::arcade::{Draw, Game, Look, Sprite};
use gpui_kit::*;
use std::f32::consts::{PI, TAU};

/// One board on screen: the grid it was laid out with and how bright it draws.
pub struct Board<'a> {
    pub bounds: Bounds<Pixels>,
    /// Grid pitch in window pixels.
    pub pitch: f32,
    pub color: Hsla,
    /// The page colour, punched through for the ghosts' eyes.
    pub bg: Hsla,
    pub dim: f32,
    /// Idle band: fade out toward the top, as the CSS mask did.
    pub fade: bool,
    /// Whether to draw the unlit dot matrix under the game.
    pub matrix: bool,
    pub window: &'a mut Window,
}

impl Board<'_> {
    /// Grid size that fits the bounds.
    pub fn grid(bounds: Bounds<Pixels>, pitch: f32) -> (i32, i32) {
        let w = f32::from(bounds.size.width);
        let h = f32::from(bounds.size.height);
        // A pitch fitted to the bounds divides them exactly; don't lose a row to rounding.
        ((w / pitch + 1e-3).floor() as i32, (h / pitch + 1e-3).floor() as i32)
    }

    /// Where grid (0, 0) sits: the leftover margin is split evenly.
    pub fn offset(bounds: Bounds<Pixels>, pitch: f32) -> (f32, f32) {
        let (cols, rows) = Self::grid(bounds, pitch);
        let w = f32::from(bounds.size.width);
        let h = f32::from(bounds.size.height);
        (
            f32::from(bounds.origin.x) + (w - cols as f32 * pitch) / 2.0,
            f32::from(bounds.origin.y) + (h - rows as f32 * pitch) / 2.0,
        )
    }

    /// The unlit grid, then the game.
    pub fn paint(&mut self, game: &dyn Game) {
        if !self.matrix {
            return game.draw(self);
        }
        let (cols, rows) = Self::grid(self.bounds, self.pitch);
        let (ox, oy) = Self::offset(self.bounds, self.pitch);
        let cell = self.pitch - 1.0;
        for y in 0..rows {
            let top = oy + y as f32 * self.pitch;
            let alpha = 0.06 * self.mask(top + cell / 2.0);
            if alpha <= 0.0 {
                continue;
            }
            for x in 0..cols {
                let left = ox + x as f32 * self.pitch;
                self.window.paint_quad(fill(
                    Bounds::new(point(px(left), px(top)), size(px(cell), px(cell))),
                    self.color.opacity(alpha),
                ));
            }
        }
        game.draw(self);
    }

    /// The fade toward the top of the idle band (`linear-gradient(to top, #000 45%, transparent)`).
    fn mask(&self, y: f32) -> f32 {
        if !self.fade {
            return 1.0;
        }
        let h = f32::from(self.bounds.size.height);
        let from_bottom = f32::from(self.bounds.origin.y) + h - y;
        (1.0 - (from_bottom - 0.45 * h) / (0.55 * h)).clamp(0.0, 1.0)
    }

    fn center(&self, x: f32, y: f32) -> (f32, f32) {
        let (ox, oy) = Self::offset(self.bounds, self.pitch);
        let half = (self.pitch - 1.0) / 2.0;
        (ox + x * self.pitch + half, oy + y * self.pitch + half)
    }

    fn color_at(&self, alpha: f32, y: f32) -> Hsla {
        self.color.opacity(alpha * self.dim * self.mask(y))
    }

    fn fill_polygon(&mut self, points: &[(f32, f32)], color: Hsla) {
        let mut path = PathBuilder::fill();
        for (i, (x, y)) in points.iter().enumerate() {
            if i == 0 {
                path.move_to(point(px(*x), px(*y)));
            } else {
                path.line_to(point(px(*x), px(*y)));
            }
        }
        path.close();
        if let Ok(path) = path.build() {
            self.window.paint_path(path, color);
        }
    }

    fn circle(&mut self, x: f32, y: f32, r: f32, color: Hsla) {
        let points: Vec<_> = arc(x, y, r, 0.0, TAU, 24).collect();
        self.fill_polygon(&points, color);
    }
}

/// Points along a circle from angle `from` to `to` (canvas angles: y points down).
fn arc(cx: f32, cy: f32, r: f32, from: f32, to: f32, steps: usize) -> impl Iterator<Item = (f32, f32)> {
    (0..=steps).map(move |i| {
        let a = from + (to - from) * i as f32 / steps as f32;
        (cx + r * a.cos(), cy + r * a.sin())
    })
}

impl Draw for Board<'_> {
    fn cell(&mut self, x: f32, y: f32, alpha: f32, size_factor: f32) {
        let (cx, cy) = self.center(x, y);
        let s = (self.pitch - 1.0) * size_factor;
        let color = self.color_at(alpha, cy);
        self.window.paint_quad(fill(
            Bounds::new(point(px(cx - s / 2.0), px(cy - s / 2.0)), size(px(s), px(s))),
            color,
        ));
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, alpha: f32) {
        let (ox, oy) = Self::offset(self.bounds, self.pitch);
        // Half a pixel back, so a cell's middle lines up with `cell`'s.
        let (left, top) = (ox + x * self.pitch - 0.5, oy + y * self.pitch - 0.5);
        let (w, h) = (w * self.pitch, h * self.pitch);
        let color = self.color_at(alpha, top + h / 2.0);
        self.window.paint_quad(fill(Bounds::new(point(px(left), px(top)), size(px(w), px(h))), color));
    }

    fn disc(&mut self, x: f32, y: f32, r: f32, alpha: f32) {
        let (cx, cy) = self.center(x, y);
        let color = self.color_at(alpha, cy);
        self.circle(cx, cy, r * self.pitch, color);
    }

    fn pac(&mut self, x: f32, y: f32, r: f32, dx: f32, dy: f32, mouth: f32, alpha: f32) {
        if mouth >= PI - 0.01 {
            return;
        }
        let (cx, cy) = self.center(x, y);
        let a = dy.atan2(dx);
        let mut points = vec![(cx, cy)];
        points.extend(arc(cx, cy, r * self.pitch, a + mouth, a - mouth + TAU, 32));
        let color = self.color_at(alpha, cy);
        self.fill_polygon(&points, color);
    }

    fn ghost(&mut self, g: &Sprite) {
        let (cx, cy) = self.center(g.x, g.y);
        let r = g.r * self.pitch;
        let ink = |a: f32| self.color_at(a, cy);
        // Body, then the face: eye whites and pupils, or a scared ghost's eyes and wavy mouth.
        let (body, whites, pupils, scared) = match g.look {
            Look::Body => (Some(ink(g.alpha)), self.bg, ink(g.alpha), false),
            Look::Scared { flash: false } => (Some(ink(0.3)), ink(0.9), ink(0.9), true),
            Look::Scared { flash: true } => (Some(ink(0.9)), self.bg, self.bg, true),
            Look::Eyes => (None, ink(0.85), self.bg, false),
        };
        let top = cy - r * 0.15;
        if let Some(body) = body {
            let bottom = cy + r * 0.85;
            let mut points: Vec<(f32, f32)> = arc(cx, top, r, PI, TAU, 16).collect();
            points.push((cx + r, bottom));
            for i in 1..=6 {
                let up = (i + usize::from(g.feet)) % 2 == 1;
                points.push((cx + r - i as f32 * r / 3.0, if up { bottom - r * 0.3 } else { bottom }));
            }
            self.fill_polygon(&points, body);
        }
        if scared {
            let s = r * 0.2;
            for ex in [cx - r * 0.35, cx + r * 0.35] {
                self.window.paint_quad(fill(
                    Bounds::new(point(px(ex - s / 2.0), px(top - s / 2.0)), size(px(s), px(s))),
                    pupils,
                ));
            }
            let s = r * 0.14;
            for i in 0..7 {
                let x = cx - r * 0.6 + i as f32 * r * 0.2;
                let y = cy + r * 0.38 - if i % 2 == 1 { r * 0.14 } else { 0.0 };
                self.window.paint_quad(fill(
                    Bounds::new(point(px(x - s / 2.0), px(y - s / 2.0)), size(px(s), px(s))),
                    whites,
                ));
            }
            return;
        }
        let (lx, ly) = g.dir;
        for side in [-1.0, 1.0] {
            let (ex, ey) = (cx + side * r * 0.38 + lx * r * 0.1, top + ly * r * 0.1);
            self.circle(ex, ey, r * 0.27, whites);
            self.circle(ex + lx * r * 0.13, ey + ly * r * 0.13, r * 0.13, pupils);
        }
    }
}
