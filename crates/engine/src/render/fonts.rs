//! Bundled fonts: caption metrics (for placing caption blocks) and the title
//! card / channel banner PNGs the Python engine drew with Pillow.
use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// A caption face measured at size 100 (em = 100 px), like `ImageFont.truetype(path, 100)`.
#[derive(Debug)]
pub struct FontMetrics {
    data: Vec<u8>,
    /// Baseline below the line top (libass: OS/2 winAscent), px at size 100.
    pub ascent: f64,
    /// Line height (winAscent + winDescent), px at size 100.
    pub height: f64,
}

impl FontMetrics {
    fn face(&self) -> Option<ttf_parser::Face<'_>> {
        ttf_parser::Face::parse(&self.data, 0).ok()
    }

    /// Advance width of `text` in px at size 100, as Pillow's `getlength` with
    /// libraqm: HarfBuzz shaping (GPOS kerning), each advance in 1/64 px.
    pub fn get_length(&self, text: &str) -> f64 {
        let Some(face) = rustybuzz::Face::from_slice(&self.data, 0) else { return 0.0 };
        let upem = face.units_per_em() as f64;
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);
        let shaped = rustybuzz::shape(&face, &[], buffer);
        shaped.glyph_positions().iter().map(|p| (p.x_advance as f64 * 6400.0 / upem).round() / 64.0).sum()
    }

    /// Ink (top, bottom) of `text` relative to the baseline, px at size 100
    /// (Pillow's `getbbox(text, anchor="ls")[1]` and `[3]`).
    pub fn ink_top_bottom(&self, text: &str) -> (f64, f64) {
        let Some(face) = self.face() else { return (0.0, 0.0) };
        let scale = 100.0 / face.units_per_em() as f64;
        let (mut y_max, mut y_min) = (f64::MIN, f64::MAX);
        for ch in text.chars() {
            let gid = face.glyph_index(ch).unwrap_or(ttf_parser::GlyphId(0));
            if let Some(b) = face.glyph_bounding_box(gid) {
                y_max = y_max.max(b.y_max as f64 * scale);
                y_min = y_min.min(b.y_min as f64 * scale);
            }
        }
        if y_max == f64::MIN {
            return (0.0, 0.0);
        }
        (-y_max.ceil(), -y_min.floor())
    }
}

/// OS/2 (winAscent, winDescent) as shares of the em, parsed like the Python did.
fn win_metrics(data: &[u8]) -> Option<(f64, f64)> {
    let u16_at = |o: usize| -> Option<u16> { Some(u16::from_be_bytes([*data.get(o)?, *data.get(o + 1)?])) };
    let u32_at = |o: usize| -> Option<u32> {
        Some(u32::from_be_bytes([*data.get(o)?, *data.get(o + 1)?, *data.get(o + 2)?, *data.get(o + 3)?]))
    };
    let count = u16_at(4)? as usize;
    let (mut head, mut os2) = (None, None);
    for i in 0..count {
        let rec = 12 + 16 * i;
        let tag = data.get(rec..rec + 4)?;
        let offset = u32_at(rec + 8)? as usize;
        match tag {
            b"head" => head = Some(offset),
            b"OS/2" => os2 = Some(offset),
            _ => {}
        }
    }
    let units = u16_at(head? + 18)? as f64;
    let ascent = u16_at(os2? + 74)? as f64;
    let descent = u16_at(os2? + 76)? as f64;
    (units > 0.0 && ascent + descent > 0.0).then(|| (ascent / units, descent / units))
}

fn names(face: &ttf_parser::Face<'_>) -> Vec<String> {
    let get = |id: u16| -> Option<String> {
        face.names().into_iter().filter(|n| n.name_id == id).find_map(|n| n.to_string())
    };
    let mut out = Vec::new();
    for (family, sub) in [(16, 17), (1, 2)] {
        if let Some(f) = get(family) {
            let style = get(sub).unwrap_or_else(|| "Regular".into());
            out.push(format!("{f} {style}").to_lowercase());
            if style == "Regular" {
                out.push(f.to_lowercase());
            }
        }
    }
    out
}

fn font_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.extension()
                        .and_then(|e| e.to_str())
                        .map(|e| matches!(e, "ttf" | "otf"))
                        .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

type Cache = Mutex<HashMap<(PathBuf, String), Option<Arc<FontMetrics>>>>;

/// Metrics of a bundled caption face ("Montserrat Black", "Anton"), or None.
/// libass sizes a font so its OS/2 winAscent + winDescent equals the font size.
pub fn caption_font(fonts_dir: Option<&Path>, font_name: &str) -> Option<Arc<FontMetrics>> {
    let dir = fonts_dir?;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let key = (dir.to_path_buf(), font_name.to_lowercase());
    if let Some(hit) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return hit;
    }
    let found = load_caption_font(dir, &key.1).map(Arc::new);
    if let Ok(mut c) = cache.lock() {
        c.insert(key, found.clone());
    }
    found
}

fn load_caption_font(dir: &Path, wanted: &str) -> Option<FontMetrics> {
    for path in font_files(dir) {
        let Ok(data) = std::fs::read(&path) else { continue };
        let (ascent, height) = {
            let Ok(face) = ttf_parser::Face::parse(&data, 0) else { continue };
            if !names(&face).iter().any(|n| n == wanted) {
                continue;
            }
            match win_metrics(&data) {
                Some((a, d)) => (a * 100.0, (a + d) * 100.0),
                None => {
                    let upem = face.units_per_em() as f64;
                    let a = face.ascender() as f64 * 100.0 / upem;
                    let d = -(face.descender() as f64) * 100.0 / upem;
                    (a, a + d)
                }
            }
        };
        if height > 0.0 {
            return Some(FontMetrics { data, ascent, height });
        }
    }
    None
}

/// Title/banner face: Montserrat Black, then ExtraBold, then Liberation Sans Bold.
pub fn overlay_font_path(fonts_dir: Option<&Path>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = fonts_dir {
        candidates.push(dir.join("Montserrat-Black.ttf"));
        candidates.push(dir.join("Montserrat-ExtraBold.ttf"));
    }
    candidates.push(PathBuf::from("/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf"));
    candidates.into_iter().find(|p| p.is_file())
}

/// RGBA canvas with Pillow-style drawing (colors blend into all four channels).
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pixels: Vec<[f32; 4]>,
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height, pixels: vec![[0.0; 4]; (width as usize) * (height as usize)] }
    }

    fn blend(&mut self, x: i64, y: i64, color: [u8; 4], coverage: f32) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 || coverage <= 0.0 {
            return;
        }
        let a = coverage.min(1.0);
        let px = &mut self.pixels[y as usize * self.width as usize + x as usize];
        for c in 0..4 {
            px[c] = color[c] as f32 * a + px[c] * (1.0 - a);
        }
    }

    /// Filled rounded rectangle over the whole canvas (Pillow `rounded_rectangle`).
    pub fn rounded_rect(&mut self, radius: f64, color: [u8; 4]) {
        let (w, h) = (self.width as f64, self.height as f64);
        let r = radius.max(0.0).min(w / 2.0).min(h / 2.0);
        for y in 0..self.height {
            for x in 0..self.width {
                let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
                let cx = if px < r { r } else if px > w - r { w - r } else { px };
                let cy = if py < r { r } else if py > h - r { h - r } else { py };
                if (px - cx).powi(2) + (py - cy).powi(2) <= r * r {
                    self.blend(x as i64, y as i64, color, 1.0);
                }
            }
        }
    }

    pub fn encode_png(&self) -> std::io::Result<Vec<u8>> {
        let mut out = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut out, self.width, self.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
            let bytes: Vec<u8> = self
                .pixels
                .iter()
                .flat_map(|p| p.map(|c| c.round().clamp(0.0, 255.0) as u8))
                .collect();
            writer.write_image_data(&bytes).map_err(std::io::Error::other)?;
        }
        Ok(out)
    }
}

/// A font at a pixel size (em = `size` px), laid out Pillow-style with anchor "la":
/// y = 0 is the ascender line.
pub struct TextFont {
    font: FontArc,
    size: f32,
}

impl TextFont {
    pub fn load(path: &Path, size: f64) -> Option<Self> {
        let data = std::fs::read(path).ok()?;
        let font = FontArc::try_from_vec(data).ok()?;
        Some(Self { font, size: size as f32 })
    }

    fn scaled(&self) -> ab_glyph::PxScaleFont<&FontArc> {
        let upem = self.font.units_per_em().unwrap_or(1000.0);
        let height = self.font.height_unscaled();
        self.font.as_scaled(PxScale::from(self.size * height / upem))
    }

    fn layout(&self, text: &str) -> Vec<ab_glyph::Glyph> {
        let sf = self.scaled();
        let ascent = sf.ascent().round();
        let mut x = 0.0f32;
        let mut prev = None;
        let mut glyphs = Vec::new();
        for ch in text.chars() {
            let id = sf.glyph_id(ch);
            if let Some(p) = prev {
                x += sf.kern(p, id);
            }
            glyphs.push(id.with_scale_and_position(sf.scale(), ab_glyph::point(x, ascent)));
            x += sf.h_advance(id).round();
            prev = Some(id);
        }
        glyphs
    }

    /// Pillow `textbbox((0, 0), text)`: (x0, y0, x1, y1), advance-inclusive width.
    pub fn bbox(&self, text: &str) -> (i64, i64, i64, i64) {
        let glyphs = self.layout(text);
        let sf = self.scaled();
        let advance_end = glyphs.last().map(|g| g.position.x + sf.h_advance(g.id).round()).unwrap_or(0.0);
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for g in glyphs {
            if let Some(o) = self.font.outline_glyph(g) {
                let b = o.px_bounds();
                x0 = x0.min(b.min.x);
                y0 = y0.min(b.min.y);
                x1 = x1.max(b.max.x);
                y1 = y1.max(b.max.y);
            }
        }
        if x0 == f32::MAX {
            return (0, 0, advance_end.round() as i64, sf.ascent().round() as i64);
        }
        (x0.floor().min(0.0) as i64, y0.floor() as i64, x1.max(advance_end).ceil() as i64, y1.ceil() as i64)
    }

    pub fn draw(&self, canvas: &mut Canvas, x: i64, y: i64, text: &str, color: [u8; 4]) {
        for g in self.layout(text) {
            if let Some(o) = self.font.outline_glyph(g) {
                let b = o.px_bounds();
                o.draw(|gx, gy, c| {
                    canvas.blend(x + b.min.x as i64 + gx as i64, y + b.min.y as i64 + gy as i64, color, c);
                });
            }
        }
    }
}

