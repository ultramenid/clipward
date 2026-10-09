//! Pure logic behind the editor's side panel (ColorPanel.tsx, SoundPanels.tsx and the
//! piece inspector in EditorPage.tsx): looks, speed, fades, framing and the backdrop.
use crate::format::to_fixed;
use crate::timeline::{KEY_SNAP_MS, crop_at, item_length, js_round, placements, set_crop_key};
use clip_engine::render::layout_renderer::is_backdrop_color;
use clip_engine::render::project::{ColorAdjust, Crop, CropKey, TRANSITION_EFFECTS, VideoItem, split_panels};

/// The picture as shot: no colour change.
pub const AS_SHOT: ColorAdjust = ColorAdjust { brightness: 0.0, contrast: 1.0, saturation: 1.0, temperature: 6500.0 };

const fn look(brightness: f64, contrast: f64, saturation: f64, temperature: f64) -> ColorAdjust {
    ColorAdjust { brightness, contrast, saturation, temperature }
}

/// Ready-made looks: each is a starting point the sliders then tune.
pub const LOOKS: [(&str, ColorAdjust); 7] = [
    ("None", AS_SHOT),
    ("Vivid", look(0.02, 1.15, 1.4, 6500.0)),
    ("Warm", look(0.02, 1.05, 1.1, 4800.0)),
    ("Cool", look(0.0, 1.05, 0.95, 8800.0)),
    ("B&W", look(0.0, 1.2, 0.0, 6500.0)),
    ("Vintage", look(0.04, 0.85, 0.7, 5200.0)),
    ("Fade", look(0.06, 0.75, 0.85, 6500.0)),
];

/// A piece's colour after picking a look (None is the picture as shot).
pub fn pick_look(look: ColorAdjust) -> Option<ColorAdjust> {
    (look != AS_SHOT).then_some(look)
}

/// A piece's colour with one value tuned; back to None when it ends up as shot.
pub fn tune_color(color: Option<ColorAdjust>, patch: impl FnOnce(&mut ColorAdjust)) -> Option<ColorAdjust> {
    let mut next = color.unwrap_or(AS_SHOT);
    patch(&mut next);
    pick_look(next)
}

/// The temperature slider's reading.
pub fn temperature_label(kelvin: f64) -> &'static str {
    if kelvin < 6500.0 {
        "Warmer"
    } else if kelvin > 6500.0 {
        "Cooler"
    } else {
        "As shot"
    }
}

/// The piece at a new speed: fades can't outlast it, and keyframes stay on their
/// frames, which now play sooner or later.
pub fn with_speed(item: &VideoItem, speed: f64) -> VideoItem {
    let len = js_round((item.src_out_ms - item.src_in_ms) as f64 / speed) as i64;
    VideoItem {
        speed,
        fade_in_ms: item.fade_in_ms.min(len),
        fade_out_ms: item.fade_out_ms.min(len),
        keyframes: item
            .keyframes
            .iter()
            .map(|k| crate::clip_engine::render::project::CropKey {
                t_ms: js_round(k.t_ms as f64 * item.speed / speed) as i64,
                ..*k
            })
            .collect(),
        ..item.clone()
    }
}

/// Longest fade the sliders offer, ms.
pub const MAX_FADE_MS: i64 = 5000;

/// The fade sliders' top for something `len` ms long, in whole tenths of a second.
pub fn fade_max(len: i64) -> i64 {
    MAX_FADE_MS.min(len.div_euclid(100) * 100)
}

/// A fade slider's reading: "Off" or seconds to one decimal.
pub fn fade_label(ms: i64) -> String {
    if ms == 0 { "Off".into() } else { format!("{}s", to_fixed(ms as f64 / 1000.0, 1)) }
}

/// A volume slider's reading (`volume` 1 is 100%).
pub fn volume_label(volume: f64) -> String {
    match js_round(volume * 100.0) as i64 {
        0 => "Muted".into(),
        pct => format!("{pct}%"),
    }
}

/// The framing segment a piece shows: any split is "split".
pub fn framing_choice(framing: &str) -> &str {
    if split_panels(framing).is_some() { "split" } else { framing }
}

/// Pick a framing segment: split keeps the piece's panel count when it already is one,
/// and crop starts centred and unzoomed.
pub fn set_framing(item: &mut VideoItem, choice: &str) {
    if !(choice == "split" && split_panels(&item.framing).is_some()) {
        item.framing = choice.into();
    }
    if choice == "crop" && item.crop.is_none() {
        item.crop = Some(Crop { x: 0.5, y: 0.5, zoom: 1.0 });
    }
}

/// The whole-frame background a project uses: "blur", "black" or "colour".
pub fn backdrop_mode(backdrop: Option<&str>) -> &'static str {
    match backdrop {
        None => "blur",
        Some(c) if c.eq_ignore_ascii_case("#000000") => "black",
        Some(_) => "colour",
    }
}

/// Default solid background when "Colour" is picked first.
pub const DEFAULT_BACKDROP: &str = "#101828";

/// The backdrop after picking a mode; "colour" keeps a colour already chosen.
pub fn set_backdrop_mode(backdrop: Option<&str>, mode: &str) -> Option<String> {
    match mode {
        "blur" => None,
        "black" => Some("#000000".into()),
        _ => Some(match backdrop {
            Some(c) if c != "#000000" => c.into(),
            _ => DEFAULT_BACKDROP.into(),
        }),
    }
}

/// A picked background colour, if the engine accepts it.
pub fn valid_backdrop(color: &str) -> Option<String> {
    is_backdrop_color(color).then(|| color.to_string())
}

// ---- speed, crop zoom, transitions ------------------------------------------------

/// The speed slider moves in twentieths (0.05×): its value for a speed, and back.
pub fn speed_to_slider(speed: f64) -> f64 {
    js_round(speed * 20.0)
}

pub fn slider_to_speed(value: f64) -> f64 {
    value / 20.0
}

/// The speed slider's reading, as JS prints the number ("1.5×", "2×").
pub fn speed_label(speed: f64) -> String {
    format!("{speed}×")
}

/// The crop zoom slider moves in tenths; its reading has one decimal ("1.5×").
pub fn zoom_to_slider(zoom: f64) -> f64 {
    js_round(zoom * 10.0)
}

pub fn zoom_label(zoom: f64) -> String {
    format!("{}×", to_fixed(zoom, 1))
}

/// Edited time `time` as ms into piece `index`, kept inside the piece.
pub fn piece_local(video: &[VideoItem], index: usize, time: f64) -> f64 {
    let Some(item) = video.get(index) else { return 0.0 };
    let start = placements(video).get(index).map_or(0, |p| p.start);
    (time - start as f64).max(0.0).min(item_length(item) as f64)
}

/// The keyframe at `local` ms into a piece (within KEY_SNAP_MS), if any.
pub fn key_here(item: &VideoItem, local: f64) -> Option<usize> {
    item.keyframes.iter().position(|k| ((k.t_ms as f64) - local).abs() < KEY_SNAP_MS as f64)
}

/// Set the crop shown at `local`: with keyframes, the keyframe there; else the fixed crop.
pub fn set_crop_here(item: &mut VideoItem, local: f64, crop: Crop) {
    if item.keyframes.is_empty() {
        item.crop = Some(crop);
    } else {
        item.keyframes = set_crop_key(&item.keyframes, local, crop);
    }
}

/// "Add keyframe" / "Remove keyframe": drop the one at `local`, or key the crop there.
pub fn toggle_crop_key(item: &VideoItem, local: f64) -> Vec<CropKey> {
    match key_here(item, local) {
        Some(i) => item.keyframes.iter().enumerate().filter(|&(j, _)| j != i).map(|(_, k)| *k).collect(),
        None => set_crop_key(&item.keyframes, local, crop_at(item, local)),
    }
}

/// "Clear": keep the crop shown at `local` as the fixed crop, without keyframes.
pub fn clear_crop_keys(item: &mut VideoItem, local: f64) {
    item.crop = Some(crop_at(item, local));
    item.keyframes.clear();
}

/// The hint under the crop controls.
pub fn crop_hint(keyframes: usize) -> String {
    if keyframes > 1 {
        format!("Zooms and pans through {keyframes} keyframes. Move the playhead to one to change it.")
    } else {
        "Drag the video to move the crop. Add keyframes at two moments to zoom or pan between them.".into()
    }
}

/// Transition styles a piece offers, by value and label: a dip to black, then the
/// engine's blends (TRANSITION_EFFECTS in project.ts order).
pub const TRANSITION_STYLES: [(&str, &str); 11] = [
    ("dip", "Dip to black"),
    ("fade", "Dissolve"),
    ("slideleft", "Slide"),
    ("slideup", "Slide up"),
    ("coverleft", "Cover"),
    ("wipeleft", "Wipe"),
    ("smoothleft", "Smooth wipe"),
    ("zoomin", "Zoom"),
    ("circleopen", "Circle"),
    ("radial", "Clock"),
    ("fadewhite", "Flash"),
];

/// Transition lengths offered between pieces, ms.
pub const TRANSITIONS: [i64; 4] = [0, 400, 800, 1200];

/// The style select's value for a piece.
pub fn transition_style(item: &VideoItem) -> &str {
    if item.dissolve { &item.effect } else { "dip" }
}

/// Pick a transition style; one with no length gets the middle one (0.8 s).
pub fn set_transition_style(item: &mut VideoItem, style: &str) {
    item.dissolve = style != "dip";
    if style != "dip" {
        item.effect = style.into();
    }
    if item.transition_ms == 0 {
        item.transition_ms = TRANSITIONS[2];
    }
}

/// The length segment a piece shows: its length when it is one on offer, else None.
pub fn transition_choice(ms: i64) -> i64 {
    if TRANSITIONS.contains(&ms) { ms } else { 0 }
}

/// A length segment's label: "None" or seconds as JS prints them ("0.4s").
pub fn transition_label(ms: i64) -> String {
    if ms == 0 { "None".into() } else { format!("{}s", ms as f64 / 1000.0) }
}

/// Whether the engine still has every blend the style select offers, and no more.
pub fn transition_styles_match_engine() -> bool {
    let offered: Vec<&str> = TRANSITION_STYLES[1..].iter().map(|(v, _)| *v).collect();
    offered.len() == TRANSITION_EFFECTS.len() && TRANSITION_EFFECTS.iter().all(|e| offered.contains(e))
}

// ---- colour sliders ----------------------------------------------------------------

/// One of ColorPanel's sliders: how a look maps onto it and back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSlider {
    Brightness,
    Contrast,
    Saturation,
    Temperature,
}

impl ColorSlider {
    pub const ALL: [ColorSlider; 4] =
        [ColorSlider::Brightness, ColorSlider::Contrast, ColorSlider::Saturation, ColorSlider::Temperature];

    pub fn label(self) -> &'static str {
        match self {
            ColorSlider::Brightness => "Brightness",
            ColorSlider::Contrast => "Contrast",
            ColorSlider::Saturation => "Saturation",
            ColorSlider::Temperature => "Temperature",
        }
    }

    /// (min, max, step) as the slider shows them.
    pub fn range(self) -> (f64, f64, f64) {
        match self {
            ColorSlider::Brightness => (-50.0, 50.0, 1.0),
            ColorSlider::Contrast => (50.0, 200.0, 1.0),
            ColorSlider::Saturation => (0.0, 300.0, 1.0),
            ColorSlider::Temperature => (2000.0, 12000.0, 100.0),
        }
    }

    pub fn value(self, c: &ColorAdjust) -> f64 {
        match self {
            ColorSlider::Brightness => js_round(c.brightness * 100.0),
            ColorSlider::Contrast => js_round(c.contrast * 100.0),
            ColorSlider::Saturation => js_round(c.saturation * 100.0),
            ColorSlider::Temperature => c.temperature,
        }
    }

    pub fn display(self, c: &ColorAdjust) -> String {
        match self {
            ColorSlider::Brightness => format!("{}", self.value(c)),
            ColorSlider::Contrast | ColorSlider::Saturation => format!("{}%", self.value(c)),
            ColorSlider::Temperature => temperature_label(c.temperature).into(),
        }
    }

    /// The colour with the slider at `v` (as its value), back to None when as shot.
    pub fn apply(self, color: Option<ColorAdjust>, v: f64) -> Option<ColorAdjust> {
        tune_color(color, |c| match self {
            ColorSlider::Brightness => c.brightness = v / 100.0,
            ColorSlider::Contrast => c.contrast = v / 100.0,
            ColorSlider::Saturation => c.saturation = v / 100.0,
            ColorSlider::Temperature => c.temperature = v,
        })
    }
}

/// Whether a look button shows pressed for a piece's colour.
pub fn look_pressed(color: Option<ColorAdjust>, look: ColorAdjust) -> bool {
    color.unwrap_or(AS_SHOT) == look
}

#[cfg(test)]
mod tests {
    use super::*;
    use clip_engine::render::project::CropKey;

    fn piece() -> VideoItem {
        serde_json::from_value(serde_json::json!({
            "id": "v0", "src_in_ms": 0, "src_out_ms": 3000, "framing": "auto", "volume": 1.0,
            "fade_in_ms": 2000, "fade_out_ms": 500, "speed": 1.0, "transition_ms": 0,
            "dissolve": false, "effect": "fade", "crop": null, "media": null,
            "keyframes": [], "color": null
        }))
        .unwrap()
    }

    #[test]
    fn speed_rescales_fades_and_keyframes() {
        let mut p = piece();
        p.keyframes = vec![CropKey { t_ms: 1000, x: 0.5, y: 0.5, zoom: 1.0 }, CropKey { t_ms: 2999, x: 0.2, y: 0.5, zoom: 2.0 }];
        let fast = with_speed(&p, 2.0);
        assert_eq!(fast.speed, 2.0);
        assert_eq!((fast.fade_in_ms, fast.fade_out_ms), (1500, 500));
        assert_eq!(fast.keyframes.iter().map(|k| k.t_ms).collect::<Vec<_>>(), [500, 1500]);
        // Back to 1x: the keyframes come back (rounding half up as JS does).
        let back = with_speed(&fast, 1.0);
        assert_eq!(back.keyframes.iter().map(|k| k.t_ms).collect::<Vec<_>>(), [1000, 3000]);
        assert_eq!(back.keyframes[1].zoom, 2.0);
        // Slower never lengthens the fades.
        let slow = with_speed(&p, 0.5);
        assert_eq!((slow.fade_in_ms, slow.fade_out_ms), (2000, 500));
    }

    #[test]
    fn looks_and_tuning() {
        assert_eq!(pick_look(LOOKS[0].1), None);
        assert_eq!(pick_look(LOOKS[4].1).unwrap().saturation, 0.0);
        let warm = tune_color(None, |c| c.temperature = 4800.0).unwrap();
        assert_eq!(warm, ColorAdjust { temperature: 4800.0, ..AS_SHOT });
        assert_eq!(tune_color(Some(warm), |c| c.temperature = 6500.0), None);
        assert_eq!(temperature_label(4800.0), "Warmer");
        assert_eq!(temperature_label(6500.0), "As shot");
        assert_eq!(temperature_label(9000.0), "Cooler");
    }

    #[test]
    fn slider_readings() {
        assert_eq!(fade_max(12_345), 5000);
        assert_eq!(fade_max(2_350), 2300);
        assert_eq!(fade_max(50), 0);
        assert_eq!(fade_label(0), "Off");
        assert_eq!(fade_label(1200), "1.2s");
        assert_eq!(volume_label(0.0), "Muted");
        assert_eq!(volume_label(1.25), "125%");
    }

    #[test]
    fn framing_and_backdrop() {
        let mut p = piece();
        assert_eq!(framing_choice("split3"), "split");
        set_framing(&mut p, "crop");
        assert_eq!((p.framing.as_str(), p.crop.unwrap().zoom), ("crop", 1.0));
        p.framing = "split4".into();
        set_framing(&mut p, "split");
        assert_eq!(p.framing, "split4");
        set_framing(&mut p, "fit");
        assert_eq!(p.framing, "fit");
        assert_eq!(backdrop_mode(None), "blur");
        assert_eq!(backdrop_mode(Some("#000000")), "black");
        assert_eq!(backdrop_mode(Some("#ff0000")), "colour");
        assert_eq!(set_backdrop_mode(Some("#000000"), "colour").as_deref(), Some(DEFAULT_BACKDROP));
        assert_eq!(set_backdrop_mode(Some("#ff0000"), "colour").as_deref(), Some("#ff0000"));
        assert_eq!(set_backdrop_mode(Some("#ff0000"), "blur"), None);
        assert_eq!(valid_backdrop("#12abEF").as_deref(), Some("#12abEF"));
        assert_eq!(valid_backdrop("red"), None);
    }

    #[test]
    fn speed_and_zoom_sliders() {
        assert_eq!(speed_to_slider(1.0), 20.0);
        assert_eq!(speed_to_slider(0.55), 11.0);
        assert_eq!(slider_to_speed(11.0), 0.55);
        assert_eq!(speed_label(1.0), "1×");
        assert_eq!(speed_label(slider_to_speed(30.0)), "1.5×");
        assert_eq!(speed_label(slider_to_speed(11.0)), "0.55×");
        assert_eq!(zoom_to_slider(1.25), 13.0);
        assert_eq!(zoom_label(1.25), "1.3×");
        assert_eq!(zoom_label(2.0), "2.0×");
        assert_eq!(fade_label(1250), "1.3s");
    }

    #[test]
    fn crop_keyframes_at_the_playhead() {
        let mut a = piece();
        a.framing = "crop".into();
        let mut b = a.clone();
        b.src_in_ms = 0;
        b.src_out_ms = 2000;
        let video = vec![a.clone(), b];
        assert_eq!(piece_local(&video, 1, 3500.0), 500.0);
        assert_eq!(piece_local(&video, 1, 100.0), 0.0);
        assert_eq!(piece_local(&video, 1, 9000.0), 2000.0);
        // No keyframes: the zoom sets the fixed crop.
        let zoomed = Crop { x: 0.5, y: 0.5, zoom: 2.0 };
        set_crop_here(&mut a, 500.0, zoomed);
        assert_eq!((a.crop, a.keyframes.len()), (Some(zoomed), 0));
        // Add a keyframe, then one more; the zoom then edits the one under the playhead.
        a.keyframes = toggle_crop_key(&a, 500.0);
        assert_eq!(key_here(&a, 520.0), Some(0));
        a.keyframes = toggle_crop_key(&a, 2000.0);
        assert_eq!(a.keyframes.iter().map(|k| k.t_ms).collect::<Vec<_>>(), [500, 2000]);
        set_crop_here(&mut a, 2010.0, Crop { zoom: 3.0, ..zoomed });
        assert_eq!(a.keyframes[1].zoom, 3.0);
        assert_eq!(crop_hint(a.keyframes.len()), "Zooms and pans through 2 keyframes. Move the playhead to one to change it.");
        // Remove the one under the playhead.
        a.keyframes = toggle_crop_key(&a, 1990.0);
        assert_eq!(a.keyframes.len(), 1);
        // Clear keeps the crop as shown.
        clear_crop_keys(&mut a, 0.0);
        assert_eq!((a.crop, a.keyframes.len()), (Some(zoomed), 0));
    }

    #[test]
    fn transitions() {
        assert!(transition_styles_match_engine());
        let mut p = piece();
        assert_eq!(transition_style(&p), "dip");
        set_transition_style(&mut p, "slideup");
        assert_eq!((p.dissolve, p.effect.as_str(), p.transition_ms), (true, "slideup", 800));
        p.transition_ms = 400;
        set_transition_style(&mut p, "dip");
        assert_eq!((p.dissolve, p.effect.as_str(), p.transition_ms), (false, "slideup", 400));
        assert_eq!(transition_style(&p), "dip");
        assert_eq!(transition_choice(400), 400);
        assert_eq!(transition_choice(650), 0);
        assert_eq!(TRANSITIONS.map(transition_label), ["None", "0.4s", "0.8s", "1.2s"]);
    }

    #[test]
    fn color_sliders() {
        let vivid = LOOKS[1].1;
        assert_eq!(ColorSlider::Brightness.value(&vivid), 2.0);
        assert_eq!(ColorSlider::Contrast.display(&vivid), "115%");
        assert_eq!(ColorSlider::Saturation.display(&vivid), "140%");
        assert_eq!(ColorSlider::Temperature.display(&LOOKS[2].1), "Warmer");
        assert_eq!(ColorSlider::Brightness.apply(None, -10.0), Some(ColorAdjust { brightness: -0.1, ..AS_SHOT }));
        assert_eq!(ColorSlider::Saturation.apply(Some(ColorAdjust { saturation: 1.5, ..AS_SHOT }), 100.0), None);
        assert!(look_pressed(None, AS_SHOT));
        assert!(look_pressed(Some(vivid), LOOKS[1].1));
        assert!(!look_pressed(Some(vivid), AS_SHOT));
    }
}
