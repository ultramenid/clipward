//! Layout data shared by the analyzer (layout/) and the renderer (render/).
//! Mirrors layout_analyzer.py. All boxes are normalized 0-1 of the source frame.
use serde::{Deserialize, Serialize};

pub const TALKING_HEAD: &str = "talking_head";
pub const TWO_SHOT: &str = "two_shot";
pub const SCREEN_CAM: &str = "screen_cam";
pub const SCREEN: &str = "screen";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub struct Box {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Box {
    pub fn cx(&self) -> f64 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f64 {
        self.y + self.h / 2.0
    }
    pub fn area(&self) -> f64 {
        self.w * self.h
    }
    pub fn clamp(&self) -> Box {
        let w = self.w.clamp(0.0, 1.0);
        let h = self.h.clamp(0.0, 1.0);
        Box { x: self.x.clamp(0.0, 1.0 - w), y: self.y.clamp(0.0, 1.0 - h), w, h }
    }
    pub fn contains(&self, px: f64, py: f64) -> bool {
        self.x <= px && px <= self.x + self.w && self.y <= py && py <= self.y + self.h
    }
    pub fn to_list(&self) -> [f64; 4] {
        let r = |v: f64| (v * 10000.0).round() / 10000.0;
        [r(self.x), r(self.y), r(self.w), r(self.h)]
    }
}

/// How to frame one shot. Times are ms from the start of the render window.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShotLayout {
    pub start_ms: i64,
    pub end_ms: i64,
    pub layout: String,
    /// heuristic | vision | style
    pub source: String,
    pub detected_layout: Option<String>,
    /// talking_head: (t_ms from shot start, cx, cy)
    pub focus_path: Vec<(i64, f64, f64)>,
    /// two_shot: left to right
    pub people: Vec<Box>,
    pub screen_box: Option<Box>,
    pub screen_focus: Option<Box>,
    pub cam_box: Option<Box>,
    pub cam_face: Option<Box>,
}

impl ShotLayout {
    /// The shape saved into result.json metrics (see src/shared/job-output.ts safeShot).
    pub fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "start_ms": self.start_ms,
            "end_ms": self.end_ms,
            "layout": self.layout,
            "source": self.source,
            "screen_box": self.screen_box.map(|b| b.to_list()),
            "screen_focus": self.screen_focus.map(|b| b.to_list()),
            "cam_box": self.cam_box.map(|b| b.to_list()),
            "people": self.people.iter().map(|p| p.to_list()).collect::<Vec<_>>(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClipLayoutPlan {
    pub shots: Vec<ShotLayout>,
    pub source_width: u32,
    pub source_height: u32,
    pub vision_cost_usd: f64,
    /// Every analyzed frame's faces as (t_ms in window time, boxes). Captions avoid them.
    pub face_samples: Vec<(i64, Vec<Box>)>,
}

impl ClipLayoutPlan {
    pub fn dominant_layout(&self) -> &str {
        let mut totals: Vec<(&str, i64)> = Vec::new();
        for s in &self.shots {
            match totals.iter_mut().find(|(l, _)| *l == s.layout) {
                Some(t) => t.1 += s.end_ms - s.start_ms,
                None => totals.push((&s.layout, s.end_ms - s.start_ms)),
            }
        }
        totals.iter().max_by_key(|t| t.1).map(|t| t.0).unwrap_or(SCREEN)
    }
    pub fn is_letterbox_only(&self) -> bool {
        self.shots.iter().all(|s| s.layout == SCREEN)
    }
}
