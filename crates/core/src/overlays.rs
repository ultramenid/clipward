//! The title, banner and logos as editor layers (ported from the React renderer's
//! `overlays.ts`; see docs/GPUI_PORT.md). Each sits where the renderer puts it until the
//! user moves, resizes or times it; then it gets an OverlayBox of its own.
use crate::timeline::{js_round, Rect};
use clip_engine::render::project::{OverlayBox, OverlayPreview, Project};
use clip_engine::render::service::LOGO_MARGIN;
use std::collections::HashMap;
use std::path::PathBuf;

/// Which overlay: the title, the banner, or logo N (TS `'title' | 'banner' | 'logo:N'`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OverlayKey {
    Title,
    Banner,
    Logo(usize),
}

impl OverlayKey {
    /// The logo's index in `Project::logos`, for a logo.
    pub fn logo_index(self) -> Option<usize> {
        match self {
            OverlayKey::Logo(i) => Some(i),
            _ => None,
        }
    }
}

/// An overlay as the player, timeline and inspector show it.
#[derive(Debug, Clone, PartialEq)]
pub struct OverlayView {
    pub key: OverlayKey,
    pub label: String,
    /// Where it is now: its own box, or the automatic place over the whole clip.
    pub box_: OverlayBox,
    pub placed: bool,
    /// Image file (engine preview for title and banner, the run's copy for a logo).
    pub path: PathBuf,
    /// Canvas px at the box's size.
    pub width: f64,
    pub height: f64,
}

/// Width / height of each logo image, once loaded (by run file name).
pub type LogoAspects = HashMap<String, f64>;

/// Engine even(): logo widths are rounded to even pixels.
fn even(v: f64) -> f64 {
    js_round(v / 2.0) * 2.0
}

/// Where a logo sits on its own: its corner, LOGO_MARGIN in from the edges. `x`, `y` are
/// its centre as fractions of the canvas, `w`, `h` its size in canvas px. Panics when
/// `index` is not a logo.
pub fn auto_logo_centre(project: &Project, index: usize, aspect: f64) -> Rect {
    let logo = &project.logos[index];
    let (width, height) = (project.width as f64, project.height as f64);
    let w = even(width * logo.size);
    let h = w / aspect;
    let margin = js_round(width * LOGO_MARGIN);
    let right = logo.position.ends_with("right");
    let bottom = logo.position.starts_with("bottom");
    Rect {
        x: (if right {
            width - margin - w / 2.0
        } else {
            margin + w / 2.0
        }) / width,
        y: (if bottom {
            height - margin - h / 2.0
        } else {
            margin + h / 2.0
        }) / height,
        w,
        h,
    }
}

/// Every overlay the clip has, for the player, timeline and inspector: the previews'
/// title and banner, then each logo whose image size is known. `total` is the edit's
/// length, ms; `run_dir` holds the logo files.
pub fn overlay_views(
    project: &Project,
    previews: &[OverlayPreview],
    aspects: &LogoAspects,
    total: i64,
    run_dir: &str,
) -> Vec<OverlayView> {
    let whole = |x: f64, y: f64| OverlayBox {
        x,
        y,
        scale: 1.0,
        start_ms: 0,
        end_ms: total,
    };
    let mut views = Vec::new();
    for preview in previews {
        let title = preview.key == "title";
        let own = if title {
            project.title_box
        } else {
            project.banner_box
        };
        let box_ = own.unwrap_or_else(|| whole(preview.auto_x, preview.auto_y));
        views.push(OverlayView {
            key: if title {
                OverlayKey::Title
            } else {
                OverlayKey::Banner
            },
            label: if title {
                format!("Title: {}", project.title.as_deref().unwrap_or(""))
            } else {
                "Channel banner".into()
            },
            box_,
            placed: own.is_some(),
            path: preview.path.clone(),
            width: preview.width as f64 * box_.scale,
            height: preview.height as f64 * box_.scale,
        });
    }
    let separator = if run_dir.contains('\\') { '\\' } else { '/' };
    for (i, logo) in project.logos.iter().enumerate() {
        // Not loaded yet (or unreadable: JS skipped 0 and NaN too).
        let Some(&aspect) = aspects
            .get(&logo.path)
            .filter(|a| **a != 0.0 && !a.is_nan())
        else {
            continue;
        };
        let auto = auto_logo_centre(project, i, aspect);
        views.push(OverlayView {
            key: OverlayKey::Logo(i),
            label: format!("Logo {}", i + 1),
            box_: logo.placed.unwrap_or_else(|| whole(auto.x, auto.y)),
            placed: logo.placed.is_some(),
            path: PathBuf::from(format!("{run_dir}{separator}{}", logo.path)),
            width: auto.w,
            height: auto.h,
        });
    }
    views
}

/// The project with an overlay's box set (None puts it back in its automatic place).
pub fn with_overlay_box(project: &Project, key: OverlayKey, box_: Option<OverlayBox>) -> Project {
    let mut next = project.clone();
    match key {
        OverlayKey::Title => next.title_box = box_,
        OverlayKey::Banner => next.banner_box = box_,
        OverlayKey::Logo(index) => {
            if let Some(logo) = next.logos.get_mut(index) {
                logo.placed = box_;
            }
        }
    }
    next
}

/// The project without an overlay.
pub fn without_overlay(project: &Project, key: OverlayKey) -> Project {
    let mut next = project.clone();
    match key {
        OverlayKey::Title => {
            next.title = None;
            next.title_box = None;
        }
        OverlayKey::Banner => {
            next.banner_platform = None;
            next.banner_channel_url = None;
            next.banner_box = None;
        }
        OverlayKey::Logo(index) => {
            if index < next.logos.len() {
                next.logos.remove(index);
            }
        }
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn project() -> Project {
        serde_json::from_value(json!({
            "version": 1, "source_width": 1920, "source_height": 1080, "width": 1080, "height": 1920,
            "fps": "30", "aspect_ratio": "9:16", "video": [], "shots": [], "face_samples": [], "transcript": [],
            "captions": true, "caption_preset": "pop", "emphasis_words": [], "speed": 1.0,
            "title": "Hi", "title_box": null, "banner_box": null, "banner_platform": "youtube", "banner_channel_url": "x",
            "logos": [{ "path": "logo_0.png", "position": "bottom-right", "size": 0.2, "placed": null }]
        }))
        .unwrap()
    }

    fn previews() -> Vec<OverlayPreview> {
        vec![OverlayPreview {
            key: "title".into(),
            path: "/p/title.png".into(),
            width: 300,
            height: 80,
            auto_x: 0.5,
            auto_y: 0.1,
        }]
    }

    #[test]
    fn overlays_sit_in_their_automatic_place_until_the_user_places_them() {
        let project = project();
        let aspects = LogoAspects::from([("logo_0.png".to_string(), 2.0)]);
        let views = overlay_views(&project, &previews(), &aspects, 5000, "/lib/run");
        let got: Vec<_> = views
            .iter()
            .map(|v| (v.key, v.placed, v.box_.start_ms, v.box_.end_ms))
            .collect();
        assert_eq!(
            got,
            [
                (OverlayKey::Title, false, 0, 5000),
                (OverlayKey::Logo(0), false, 0, 5000)
            ]
        );
        assert_eq!(views[0].label, "Title: Hi");
        // Logo: 216 px wide, 108 tall, 43 px in from the bottom-right corner.
        let logo = &views[1];
        assert_eq!(
            (logo.width, logo.height, logo.path.clone()),
            (216.0, 108.0, PathBuf::from("/lib/run/logo_0.png"))
        );
        assert_eq!(js_round(logo.box_.x * 1080.0), (1080 - 43 - 108) as f64);
        assert_eq!(js_round(logo.box_.y * 1920.0), (1920 - 43 - 54) as f64);
        // Placed and scaled: its own box, drawn at its size.
        let placed = with_overlay_box(
            &project,
            OverlayKey::Title,
            Some(OverlayBox {
                x: 0.5,
                y: 0.5,
                scale: 2.0,
                start_ms: 100,
                end_ms: 900,
            }),
        );
        let views = overlay_views(&placed, &previews(), &LogoAspects::new(), 5000, "/lib/run");
        assert_eq!(
            (views[0].placed, views[0].width, views[0].box_.start_ms),
            (true, 600.0, 100)
        );
        assert_eq!(
            with_overlay_box(&placed, OverlayKey::Logo(0), Some(views[0].box_)).logos[0]
                .placed
                .unwrap()
                .end_ms,
            900
        );
        assert_eq!(without_overlay(&placed, OverlayKey::Title).title, None);
        assert_eq!(without_overlay(&placed, OverlayKey::Logo(0)).logos.len(), 0);
        assert_eq!(
            without_overlay(&placed, OverlayKey::Banner).banner_platform,
            None
        );
    }

    #[test]
    fn windows_run_folders_join_with_a_backslash() {
        let aspects = LogoAspects::from([("logo_0.png".to_string(), 2.0)]);
        let views = overlay_views(&project(), &[], &aspects, 5000, r"C:\lib\run");
        assert_eq!(views[0].path, PathBuf::from(r"C:\lib\run\logo_0.png"));
        // A zero aspect is not loaded yet: no view.
        let zero = LogoAspects::from([("logo_0.png".to_string(), 0.0)]);
        assert!(overlay_views(&project(), &[], &zero, 5000, "/r").is_empty());
    }
}
