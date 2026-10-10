//! Editable clips: the project file the editor saves, and the one FFmpeg graph that
//! both the editor's preview and the export render, so what the preview shows is
//! what the export writes.
//!
//! A project is built from what the renderer actually did (framing plan, pacing cuts,
//! caption words), so an untouched project re-renders the clip as it was cut. The main
//! track is magnetic, like CapCut's: items play back to back in order, and each is a
//! read of the source video. Everything else (captions, title, banner, logos) is drawn
//! on the edited timeline by the same code that renders AI clips.
//!
//! Rendering a time range for the preview composes the whole timeline's captions and
//! overlays with the video's clock shifted to the range start, then scales at the very
//! end: a preview frame is the export's frame, only smaller.
use super::captions;
use super::clip_editor::{remap_segments, TimeMap};
use super::layout_renderer::{
    banner_y, build_layout_graph, is_backdrop_color, title_y, loudness_filter_default, measured_loudness_filter, GraphOptions, AUDIO_FORMAT, LOUDNESS_TARGET,
};
use super::process::{MEDIA_INPUT_OPTIONS, MEDIA_TIMEOUT};
use super::pyfmt::Rational;
use super::service::{
    compose_overlays_from, MAX_LOGO_SIZE, MIN_LOGO_SIZE, filter_transcript_for_clip, io_render_error, render_error, FfmpegJob, RenderRequest,
    RenderResult, Renderer,
};
use super::video_speed::{scaled_duration_ms, speed_audio_filter, speed_video_filter, validate_video_speed};
use crate::error::{EngineError, Result};
use crate::media::{probe_untrusted, run_bounded, MAX_TOOL_OUTPUT_BYTES};
use crate::layout::analysis::{track_faces, FrameInfo};
use crate::layout::types::{Box as LBox, ClipLayoutPlan, ShotLayout, SCREEN, STACK};
use crate::types::{LogoOverlay, TranscriptSegment};
use super::service::{sibling, Overlay};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

pub const PROJECT_VERSION: u32 = 1;
/// Transcript kept around the clip, so a trim handle dragged outward still has captions.
pub const TRANSCRIPT_MARGIN_MS: i64 = 60_000;
pub const FRAMING_AUTO: &str = "auto";
pub const FRAMING_FIT: &str = "fit";
/// A fixed crop of the source, chosen by the user (`VideoItem::crop`).
pub const FRAMING_CROP: &str = "crop";
/// The people in the piece stacked in 2, 3 or 4 full-width panels.
pub const FRAMING_SPLIT: &str = "split";
pub const FRAMING_SPLIT3: &str = "split3";
pub const FRAMING_SPLIT4: &str = "split4";

/// How many panels a split framing stacks; None for other framings.
pub fn split_panels(framing: &str) -> Option<usize> {
    match framing {
        FRAMING_SPLIT => Some(2),
        FRAMING_SPLIT3 => Some(3),
        FRAMING_SPLIT4 => Some(4),
        _ => None,
    }
}
/// Longest video the editor imports, and how long an imported image lasts.
pub const MAX_MEDIA_MS: i64 = 10 * 60 * 1000;
pub const IMAGE_MS: i64 = 60_000;
pub const MAX_MEDIA_FILES: usize = 50;
/// Picture-in-picture width range, as a fraction of the canvas width.
pub const PIP_WIDTH_RANGE: (f64, f64) = (0.1, 1.0);
/// Per-piece speed range (one atempo stage covers it).
pub const PIECE_SPEED_RANGE: (f64, f64) = (0.5, 4.0);
pub const MAX_TRANSITION_MS: i64 = 2_000;
pub const MAX_CROP_ZOOM: f64 = 5.0;
pub const MAX_ITEM_VOLUME: f64 = 2.0;
pub const MAX_TEXT_LAYERS: usize = 50;
pub const MAX_AUDIO_ITEMS: usize = 50;
/// Auto-duck: music drops under speech (the main track and the source's own sound).
const DUCK: &str = "sidechaincompress=threshold=0.03:ratio=6:attack=20:release=400";
/// A preview range with ducked sound renders this much earlier sound first, so the
/// ducking is already where the export has it when the range starts (over DUCK's release).
const DUCK_LEAD_MS: i64 = 600;
pub const MAX_TEXT_CHARS: usize = 500;
/// Text layer font size range, canvas px.
pub const TEXT_SIZE_RANGE: (i64, i64) = (16, 400);

/// The part of the source a "crop" piece shows: a canvas-shaped window centred on
/// (x, y) (fractions of the source), `zoom` times smaller than the largest that fits.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Crop {
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

fn one() -> f64 {
    1.0
}

/// A crop at a moment in a piece (edited ms from its start): with two or more, the crop
/// eases from one to the next (zoom and pan).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct CropKey {
    pub t_ms: i64,
    pub x: f64,
    pub y: f64,
    pub zoom: f64,
}

pub const MAX_KEYFRAMES: usize = 50;

/// Blends a piece can overlap the previous one with (FFmpeg xfade names).
pub const TRANSITION_EFFECTS: &[&str] =
    &["fade", "slideleft", "slideup", "wipeleft", "zoomin", "circleopen", "smoothleft", "coverleft", "fadewhite", "radial"];

fn fade_effect() -> String {
    "fade".into()
}

/// A piece's colour: the look the user picked or tuned.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ColorAdjust {
    /// Added to the picture's brightness, -0.5..0.5 of full scale.
    pub brightness: f64,
    /// Contrast around mid-grey, 0.5..2 (1 as shot).
    pub contrast: f64,
    /// 0 is black and white, 1 as shot, up to 3.
    pub saturation: f64,
    /// White balance in kelvin, 2000..12000 (6500 as shot): lower is warmer.
    pub temperature: f64,
}

impl ColorAdjust {
    fn valid(&self) -> bool {
        (-0.5..=0.5).contains(&self.brightness)
            && (0.5..=2.0).contains(&self.contrast)
            && (0.0..=3.0).contains(&self.saturation)
            && (2000.0..=12000.0).contains(&self.temperature)
    }

    /// FFmpeg filters for this look (none for the untouched picture).
    fn filters(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.brightness != 0.0 || self.contrast != 1.0 {
            out.push(format!("lutyuv=y='clip((val-128)*{:.4}+128+{:.4},0,255)'", self.contrast, self.brightness * 255.0));
        }
        if self.saturation != 1.0 {
            out.push(format!("hue=s={:.4}", self.saturation));
        }
        if self.temperature != 6500.0 {
            out.push(format!("colortemperature=temperature={:.0}", self.temperature));
        }
        out
    }
}

/// An FFmpeg expression of `u` (seconds) through `keys` (u, value): held before the first
/// and after the last, eased in and out (smoothstep) between neighbours.
fn eased_expr(keys: &[(f64, f64)], u: &str) -> String {
    let Some(&(_, last)) = keys.last() else { return "0".into() };
    let mut expr = format!("{last:.6}");
    for pair in keys.windows(2).rev() {
        let ((u0, v0), (u1, v1)) = (pair[0], pair[1]);
        let p = format!("clip(({u}-{u0:.6})/{:.6},0,1)", (u1 - u0).max(1e-6));
        expr = format!("if(lt({u},{u1:.6}),{v0:.6}+{:.6}*{p}*{p}*(3-2*{p}),{expr})", v1 - v0);
    }
    expr
}

/// One piece of the main track: a read of the source video.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VideoItem {
    pub id: String,
    /// Source time, ms.
    pub src_in_ms: i64,
    pub src_out_ms: i64,
    /// "auto": the clip's smart framing; "fit": the whole frame.
    pub framing: String,
    /// 1 is as recorded; 0 mutes.
    pub volume: f64,
    /// Sound fades at the piece's start and end, ms.
    #[serde(default)]
    pub fade_in_ms: i64,
    #[serde(default)]
    pub fade_out_ms: i64,
    /// Playback speed of this piece.
    #[serde(default = "one")]
    pub speed: f64,
    /// Transition before this piece. A dip to black: the previous piece fades out over the
    /// first half, this one fades in over the second (before the first piece, a fade from
    /// black). With `dissolve`, the two pieces overlap this long and blend.
    #[serde(default)]
    pub transition_ms: i64,
    /// Overlap the previous piece and blend into it with `effect` instead of dipping to
    /// black (not on the first piece).
    #[serde(default)]
    pub dissolve: bool,
    /// The blend, an FFmpeg xfade transition (TRANSITION_EFFECTS); "fade" is a dissolve.
    #[serde(default = "fade_effect")]
    pub effect: String,
    /// With framing "crop".
    #[serde(default)]
    pub crop: Option<Crop>,
    /// A video or image the user added (`Project::media` file name); None reads the source.
    #[serde(default)]
    pub media: Option<String>,
    /// With framing "crop": the crop over time, by edited time into the piece (sorted).
    /// Empty uses `crop` throughout.
    #[serde(default)]
    pub keyframes: Vec<CropKey>,
    /// Colour look; None as shot.
    #[serde(default)]
    pub color: Option<ColorAdjust>,
}

impl Default for VideoItem {
    fn default() -> Self {
        Self {
            id: String::new(),
            src_in_ms: 0,
            src_out_ms: 0,
            framing: FRAMING_AUTO.into(),
            volume: 1.0,
            fade_in_ms: 0,
            fade_out_ms: 0,
            speed: 1.0,
            transition_ms: 0,
            dissolve: false,
            effect: fade_effect(),
            crop: None,
            media: None,
            keyframes: Vec::new(),
            color: None,
        }
    }
}

impl VideoItem {
    /// Source length, ms.
    pub fn len_ms(&self) -> i64 {
        self.src_out_ms - self.src_in_ms
    }

    /// Length on the edited timeline (after this piece's speed), ms.
    pub fn out_ms(&self) -> i64 {
        (self.len_ms() as f64 / self.speed).round() as i64
    }

    /// Edited time `t` into this piece, as source time.
    fn src_at(&self, t: i64) -> i64 {
        (self.src_in_ms + (t as f64 * self.speed).round() as i64).min(self.src_out_ms)
    }

    /// Source time `src` as edited time from the piece's start.
    fn out_at(&self, src: i64) -> i64 {
        ((src - self.src_in_ms) as f64 / self.speed).round() as i64
    }
}

/// Where the user put an overlay; without one it sits where the renderer places it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct OverlayBox {
    /// Centre, as a fraction of the canvas.
    pub x: f64,
    pub y: f64,
    /// Size against the automatic one (title and banner; logos use their `size`).
    pub scale: f64,
    /// Shown over this edited time, ms.
    pub start_ms: i64,
    pub end_ms: i64,
}

impl OverlayBox {
    fn valid(&self) -> bool {
        (0.0..=1.0).contains(&self.x) && (0.0..=1.0).contains(&self.y) && (0.25..=4.0).contains(&self.scale) && self.start_ms >= 0 && self.end_ms > self.start_ms
    }
}

/// A logo on the clip: an image in the run folder (bare file name).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LogoLayer {
    pub path: String,
    /// Automatic corner: "top-left" | "top-right" | "bottom-left" | "bottom-right".
    pub position: String,
    /// Width as a fraction of the frame width.
    pub size: f64,
    #[serde(default)]
    pub placed: Option<OverlayBox>,
}

/// A sound on the audio track: music or an effect the user imported (an .m4a in the run
/// folder), or a stretch of the source's own sound detached from the video.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioItem {
    pub id: String,
    /// What the user calls it (the imported file's name); display only.
    #[serde(default)]
    pub name: String,
    /// File name in the run folder; None is the source video's sound.
    pub file: Option<String>,
    /// Time in the file, ms.
    pub src_in_ms: i64,
    pub src_out_ms: i64,
    /// Where it starts on the edited timeline, ms.
    pub start_ms: i64,
    pub volume: f64,
    pub fade_in_ms: i64,
    pub fade_out_ms: i64,
    /// Lower it while someone speaks.
    pub duck: bool,
    /// A voiceover: speech that ducked sounds drop under (never ducked itself).
    #[serde(default)]
    pub voice: bool,
    /// Playback speed (a sped-up piece's detached sound keeps its speed).
    #[serde(default = "one")]
    pub speed: f64,
}

impl AudioItem {
    /// Length in the file, ms.
    pub fn len_ms(&self) -> i64 {
        self.src_out_ms - self.src_in_ms
    }

    /// Length on the edited timeline, ms.
    pub fn out_ms(&self) -> i64 {
        (self.len_ms() as f64 / self.speed).round() as i64
    }
}

/// The ffmpeg `volume` filter for a sound read that starts `offset_ms` into a clip of
/// `len_ms`, with its fades. The read's clock must start at 0.
fn gain_filter(volume: f64, len_ms: i64, offset_ms: i64, fade_in_ms: i64, fade_out_ms: i64) -> String {
    let (o, len) = (offset_ms as f64 / 1000.0, len_ms as f64 / 1000.0);
    let mut expr = format!("{volume}");
    if fade_in_ms > 0 {
        expr += &format!("*min(1,(t+{o:.6})/{:.6})", fade_in_ms as f64 / 1000.0);
    }
    if fade_out_ms > 0 {
        expr += &format!("*min(1,max(0,({len:.6}-t-{o:.6})/{:.6}))", fade_out_ms as f64 / 1000.0);
    }
    format!("volume='{expr}':eval=frame")
}

/// Reads and graph for one render of a project (see `Project::pieces_graph`).
pub struct ProjectGraph {
    /// (start_ms, duration_ms) reads of the main track, inputs 0..
    pub reads: Vec<(i64, i64)>,
    /// Each read's file: None is the source, else an added video in the media folder.
    pub read_paths: Vec<Option<PathBuf>>,
    /// (file, start_ms, duration_ms) sound reads, inputs after `reads`.
    pub audio_reads: Vec<(PathBuf, i64, i64)>,
    pub graph: String,
    /// The graph outputs [aout].
    pub audio: bool,
}

impl ProjectGraph {
    /// Input arguments for every read, in input order.
    pub fn input_args(&self, source: &Path) -> Vec<String> {
        let mut args = Vec::new();
        for (path, start_ms, duration_ms) in self.inputs(source) {
            args.extend(["-ss".into(), format!("{:.6}", start_ms as f64 / 1000.0), "-t".into(), format!("{:.6}", duration_ms as f64 / 1000.0)]);
            args.extend(MEDIA_INPUT_OPTIONS.iter().map(|v| v.to_string()));
            args.extend(["-i".into(), path.to_string_lossy().into_owned()]);
        }
        args
    }

    /// Every read as (file, start_ms, duration_ms), in input order.
    pub fn inputs(&self, source: &Path) -> Vec<(PathBuf, i64, i64)> {
        let main = self.reads.iter().zip(&self.read_paths).map(|(&(s, d), p)| (p.clone().unwrap_or_else(|| source.to_path_buf()), s, d));
        main.chain(self.audio_reads.iter().cloned()).collect()
    }
}

/// A video or image the user added, converted into the run folder (`import_media`): an
/// .mp4 at the project's frame rate with a sound track (silent when it had none). An
/// image becomes IMAGE_MS of still video.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MediaFile {
    /// File name in the run folder.
    pub file: String,
    /// What the user calls it (the picked file's name); display only.
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub duration_ms: i64,
    pub image: bool,
}

/// A video or image over the main video (picture-in-picture), with its own sound.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PipItem {
    pub id: String,
    /// `Project::media` file name.
    pub media: String,
    /// Time in the media file, ms.
    pub src_in_ms: i64,
    pub src_out_ms: i64,
    /// Where it starts on the edited timeline, ms.
    pub start_ms: i64,
    /// Centre, as a fraction of the canvas.
    pub x: f64,
    pub y: f64,
    /// Width as a fraction of the canvas width.
    pub width: f64,
    pub volume: f64,
}

impl PipItem {
    pub fn len_ms(&self) -> i64 {
        self.src_out_ms - self.src_in_ms
    }
}

/// Text the user placed over the video, drawn in a caption preset's look.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TextLayer {
    pub id: String,
    pub text: String,
    /// Edited time (before speed), ms.
    pub start_ms: i64,
    pub end_ms: i64,
    /// Centre of the text, as a fraction of the canvas.
    pub x: f64,
    pub y: f64,
    /// Font size, canvas px.
    pub size: i64,
    /// Caption preset id.
    pub preset: String,
    /// Entrance: "none" | "fade" | "pop" | "slide" | "typewriter".
    #[serde(default = "no_animation")]
    pub animation_in: String,
    /// Exit: "none" | "fade" | "pop".
    #[serde(default = "no_animation")]
    pub animation_out: String,
}

fn no_animation() -> String {
    "none".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub version: u32,
    pub source_width: u32,
    pub source_height: u32,
    /// Output canvas.
    pub width: i64,
    pub height: i64,
    pub fps: String,
    /// "9:16" | "16:9"
    pub aspect_ratio: String,
    pub video: Vec<VideoItem>,
    /// Smart framing in source time. Source ranges no shot covers render whole-frame.
    pub shots: Vec<ShotLayout>,
    /// Faces per analyzed frame (source ms), which captions stay clear of.
    pub face_samples: Vec<(i64, Vec<LBox>)>,
    /// Caption words in source time; editing a word's text changes the caption.
    pub transcript: Vec<TranscriptSegment>,
    pub captions: bool,
    pub caption_preset: String,
    pub emphasis_words: Vec<String>,
    pub title: Option<String>,
    pub banner_platform: Option<String>,
    pub banner_channel_url: Option<String>,
    pub logos: Vec<LogoLayer>,
    pub speed: f64,
    #[serde(default)]
    pub texts: Vec<TextLayer>,
    #[serde(default)]
    pub audio: Vec<AudioItem>,
    #[serde(default)]
    pub title_box: Option<OverlayBox>,
    #[serde(default)]
    pub banner_box: Option<OverlayBox>,
    /// Videos and images the user added, for main-track pieces and picture-in-picture.
    #[serde(default)]
    pub media: Vec<MediaFile>,
    #[serde(default)]
    pub pip: Vec<PipItem>,
    /// Background of whole-frame (landscape) shots: None blurs the picture, "#rrggbb"
    /// is a solid colour.
    #[serde(default)]
    pub backdrop: Option<String>,
}

impl Project {
    /// The project for a rendered clip: its cuts become main-track items, its framing plan
    /// and transcript move to source time. None when the result carries no edit.
    pub fn from_render(
        request: &RenderRequest,
        result: &RenderResult,
        caption_preset: &str,
        full_transcript: &[TranscriptSegment],
    ) -> Option<Self> {
        let edit = result.edit.as_ref()?;
        let at = |t: i64| edit.window_start_ms + t;
        let video = edit
            .keeps
            .iter()
            .enumerate()
            .map(|(i, &(start, end))| VideoItem {
                id: format!("v{i}"),
                src_in_ms: at(start),
                src_out_ms: at(end),
                ..Default::default()
            })
            .collect();
        let shots = edit
            .plan
            .shots
            .iter()
            .map(|shot| ShotLayout { start_ms: at(shot.start_ms), end_ms: at(shot.end_ms), ..shot.clone() })
            .collect();
        let face_samples = edit.plan.face_samples.iter().map(|(t, faces)| (at(*t), faces.clone())).collect();
        Some(Self {
            version: PROJECT_VERSION,
            source_width: edit.plan.source_width,
            source_height: edit.plan.source_height,
            width: result.output_width,
            height: result.output_height,
            fps: result.fps.clone(),
            aspect_ratio: request.aspect_ratio.clone(),
            video,
            shots,
            face_samples,
            transcript: filter_transcript_for_clip(
                full_transcript,
                request.start_time_ms - TRANSCRIPT_MARGIN_MS,
                request.end_time_ms + TRANSCRIPT_MARGIN_MS,
            ),
            captions: request.include_captions,
            caption_preset: caption_preset.into(),
            emphasis_words: request.emphasis_words.clone(),
            title: request.title_text.clone(),
            banner_platform: request.banner_platform.clone(),
            banner_channel_url: request.banner_channel_url.clone(),
            // The run keeps copies named logo_N.<ext> (pipeline::save_edit_inputs).
            logos: request
                .logos
                .iter()
                .enumerate()
                .map(|(i, logo)| {
                    let ext = Path::new(&logo.path).extension().and_then(|e| e.to_str()).unwrap_or("png").to_ascii_lowercase();
                    LogoLayer { path: format!("logo_{i}.{ext}"), position: logo.position.clone(), size: logo.size, placed: None }
                })
                .collect(),
            speed: request.video_speed,
            texts: Vec::new(),
            audio: Vec::new(),
            title_box: None,
            banner_box: None,
            media: Vec::new(),
            pip: Vec::new(),
            backdrop: None,
        })
    }

    /// Edited length before the whole clip's speed, ms.
    pub fn duration_ms(&self) -> i64 {
        self.placements().last().map_or(0, |&(i, start)| start + self.video[i].out_ms())
    }

    /// Dip-to-black length before piece `i` (0 when it dissolves in instead).
    fn dip(&self, i: usize) -> i64 {
        self.video.get(i).map_or(0, |v| if i > 0 && v.dissolve { 0 } else { v.transition_ms })
    }

    /// How long piece `i` overlaps the one before it (its cross-dissolve).
    fn overlap(&self, i: usize) -> i64 {
        self.video.get(i).map_or(0, |v| if i > 0 && v.dissolve { v.transition_ms } else { 0 })
    }

    pub fn media_file(&self, name: &str) -> Option<&MediaFile> {
        self.media.iter().find(|m| m.file == name)
    }

    /// Pixel size of what a piece reads: its media file, or the source.
    fn read_size(&self, item: &VideoItem) -> (u32, u32) {
        item.media.as_deref().and_then(|m| self.media_file(m)).map_or((self.source_width, self.source_height), |m| (m.width, m.height))
    }

    /// Rejects what the renderer can't draw. The editor saves only projects that pass.
    pub fn validate(&self, source_duration_ms: i64) -> Result<()> {
        validate_video_speed(self.speed)?;
        if self.video.is_empty() {
            return Err(render_error("The edit contains no video"));
        }
        if self.width <= 0 || self.height <= 0 || self.width % 2 != 0 || self.height % 2 != 0 {
            return Err(render_error("Invalid output size"));
        }
        // Media files are bare names in the run folder, never paths.
        let bare = |f: &str| !f.is_empty() && !f.starts_with('.') && !f.contains(['/', '\\', ':']) && f.len() <= 128;
        let media_ok = self.media.len() <= MAX_MEDIA_FILES
            && self.media.iter().all(|m| bare(&m.file) && m.width > 0 && m.height > 0 && m.duration_ms > 0 && m.name.chars().count() <= 200);
        if !media_ok {
            return Err(render_error("An added video or image has invalid settings"));
        }
        if self.backdrop.as_deref().is_some_and(|c| !is_backdrop_color(c)) {
            return Err(render_error("The background colour is invalid"));
        }
        let length_of = |media: &Option<String>| match media {
            None => Some(source_duration_ms),
            Some(name) => self.media_file(name).map(|m| m.duration_ms),
        };
        for item in &self.video {
            let within = length_of(&item.media).is_some_and(|len| item.src_out_ms <= len);
            if item.src_in_ms < 0 || item.len_ms() <= 0 || !within {
                return Err(render_error("A clip piece is outside its video"));
            }
            let out = if (PIECE_SPEED_RANGE.0..=PIECE_SPEED_RANGE.1).contains(&item.speed) { item.out_ms() } else { -1 };
            let fades = (0..=out).contains(&item.fade_in_ms) && (0..=out).contains(&item.fade_out_ms);
            let transition = (0..=MAX_TRANSITION_MS.min(out)).contains(&item.transition_ms);
            let keys = item.keyframes.len() <= MAX_KEYFRAMES
                && item.keyframes.windows(2).all(|w| w[0].t_ms < w[1].t_ms)
                && item.keyframes.iter().all(|k| (0..=out.max(0)).contains(&k.t_ms) && (0.0..=1.0).contains(&k.x) && (0.0..=1.0).contains(&k.y) && (1.0..=MAX_CROP_ZOOM).contains(&k.zoom));
            let crop = keys && match (&item.crop, item.framing.as_str()) {
                (Some(c), FRAMING_CROP) => (0.0..=1.0).contains(&c.x) && (0.0..=1.0).contains(&c.y) && (1.0..=MAX_CROP_ZOOM).contains(&c.zoom),
                (None, FRAMING_CROP) => !item.keyframes.is_empty(),
                (None, FRAMING_AUTO | FRAMING_FIT) | (Some(_), FRAMING_AUTO | FRAMING_FIT) => true,
                // Split stacks the source's people; an added video has no face data.
                (_, f) if split_panels(f).is_some() => item.media.is_none(),
                _ => false,
            };
            let color = item.color.is_none_or(|c| c.valid()) && TRANSITION_EFFECTS.contains(&item.effect.as_str());
            if out <= 0 || !(0.0..=MAX_ITEM_VOLUME).contains(&item.volume) || !fades || !transition || !crop || !color {
                return Err(render_error("A clip piece has invalid settings"));
            }
        }
        // Dissolves into and out of a piece fit inside it, so at most two pieces overlap.
        for (i, item) in self.video.iter().enumerate() {
            if self.overlap(i) + self.overlap(i + 1) > item.out_ms() {
                return Err(render_error("A transition is longer than its piece"));
            }
        }
        if self.audio.len() > MAX_AUDIO_ITEMS {
            return Err(render_error("Too many sounds"));
        }
        for a in &self.audio {
            // Imported sounds are bare file names in the run folder, never paths.
            let named = a.file.as_deref().is_none_or(|f| {
                !f.is_empty() && !f.starts_with('.') && !f.contains(['/', '\\', ':']) && f.len() <= 128
            });
            let out = if (PIECE_SPEED_RANGE.0..=PIECE_SPEED_RANGE.1).contains(&a.speed) { a.out_ms() } else { -1 };
            let fades = out > 0 && (0..=out).contains(&a.fade_in_ms) && (0..=out).contains(&a.fade_out_ms);
            let in_source = (a.file.is_some() || a.src_out_ms <= source_duration_ms) && a.name.chars().count() <= 200;
            if !named || a.src_in_ms < 0 || a.len_ms() <= 0 || a.start_ms < 0 || !in_source || !fades || !(0.0..=MAX_ITEM_VOLUME).contains(&a.volume) {
                return Err(render_error("A sound has invalid settings"));
            }
        }
        if self.pip.len() > MAX_AUDIO_ITEMS {
            return Err(render_error("Too many picture-in-picture videos"));
        }
        for p in &self.pip {
            let within = self.media_file(&p.media).is_some_and(|m| p.src_out_ms <= m.duration_ms);
            let placed = (0.0..=1.0).contains(&p.x) && (0.0..=1.0).contains(&p.y) && (PIP_WIDTH_RANGE.0..=PIP_WIDTH_RANGE.1).contains(&p.width);
            if !within || p.src_in_ms < 0 || p.len_ms() <= 0 || p.start_ms < 0 || !placed || !(0.0..=MAX_ITEM_VOLUME).contains(&p.volume) {
                return Err(render_error("A picture-in-picture video has invalid settings"));
            }
        }
        // Logos, like sounds, are bare file names in the run folder.
        let logos = self.logos.len() <= 8
            && self.logos.iter().all(|l| bare(&l.path) && (MIN_LOGO_SIZE..=MAX_LOGO_SIZE).contains(&l.size) && l.placed.is_none_or(|b| b.valid()));
        if !logos || !self.title_box.is_none_or(|b| b.valid()) || !self.banner_box.is_none_or(|b| b.valid()) {
            return Err(render_error("An overlay has invalid settings"));
        }
        if self.texts.len() > MAX_TEXT_LAYERS {
            return Err(render_error("Too many text layers"));
        }
        for text in &self.texts {
            let placed = (0.0..=1.0).contains(&text.x) && (0.0..=1.0).contains(&text.y);
            let sized = (TEXT_SIZE_RANGE.0..=TEXT_SIZE_RANGE.1).contains(&text.size);
            let animated = ["none", "fade", "pop", "slide", "typewriter"].contains(&text.animation_in.as_str())
                && ["none", "fade", "pop"].contains(&text.animation_out.as_str());
            if text.start_ms < 0 || text.end_ms <= text.start_ms || text.text.chars().count() > MAX_TEXT_CHARS || !placed || !sized || !animated {
                return Err(render_error("A text layer has invalid settings"));
            }
        }
        Ok(())
    }

    /// The caption file's content with the text layers added (their own document when
    /// there are no captions). None when there is nothing to draw.
    pub fn with_text_layers(&self, captions: Option<String>) -> Option<String> {
        let mut events = Vec::new();
        for layer in &self.texts {
            let style = captions::caption_style_for(&layer.preset);
            let generator = captions::CaptionGenerator::new(&style, &[], None);
            let (x, y) = ((layer.x * self.width as f64).round() as i64, (layer.y * self.height as f64).round() as i64);
            events.extend(generator.text_events(&layer.text, (layer.start_ms, layer.end_ms), (x, y), layer.size, (&layer.animation_in, &layer.animation_out)));
        }
        if events.is_empty() {
            return captions;
        }
        let style = captions::caption_style_for(&self.caption_preset);
        let document = captions
            .unwrap_or_else(|| captions::CaptionGenerator::new(&style, &[], None).empty_document(self.width, self.height));
        Some(format!("{}\n{}", document.trim_end_matches('\n'), events.join("\n")))
    }

    /// The source window (w, h, x, y) px a crop shows: the canvas's shape, the largest
    /// that fits divided by `zoom`, centred on the crop point and kept inside the frame.
    pub fn crop_rect(&self, crop: Crop) -> (i64, i64, i64, i64) {
        self.crop_rect_in((self.source_width, self.source_height), crop)
    }

    /// `crop_rect` for a read `size` px (the source or an added video).
    fn crop_rect_in(&self, size: (u32, u32), crop: Crop) -> (i64, i64, i64, i64) {
        let (sw, sh) = (size.0 as f64, size.1 as f64);
        let aspect = self.width as f64 / self.height as f64;
        let (fit_w, fit_h) = if sw / sh > aspect { (sh * aspect, sh) } else { (sw, sw / aspect) };
        let zoom = crop.zoom.clamp(1.0, MAX_CROP_ZOOM);
        let even = |v: f64| ((v / 2.0).floor() as i64 * 2).max(2);
        let (w, h) = (even(fit_w / zoom), even(fit_h / zoom));
        let x = (crop.x * sw - w as f64 / 2.0).round().clamp(0.0, sw - w as f64) as i64;
        let y = (crop.y * sh - h as f64 / 2.0).round().clamp(0.0, sh - h as f64) as i64;
        (w, h, x, y)
    }

    /// Framing for source [src_start, src_end), in time from src_start. Gaps and "fit"
    /// items get the whole-frame layout.
    fn local_plan(&self, item: &VideoItem, src_start: i64, src_end: i64) -> ClipLayoutPlan {
        let len = src_end - src_start;
        let whole = |start_ms: i64, end_ms: i64| ShotLayout {
            start_ms,
            end_ms,
            layout: SCREEN.into(),
            source: "style".into(),
            ..Default::default()
        };
        let mut shots = Vec::new();
        // Smart framing is the source's; an added video shows whole (or cropped).
        if item.framing == FRAMING_AUTO && item.media.is_none() {
            let mut cursor = 0;
            for shot in &self.shots {
                let (s, e) = (shot.start_ms.max(src_start) - src_start, shot.end_ms.min(src_end) - src_start);
                if e <= s {
                    continue;
                }
                if s > cursor {
                    shots.push(whole(cursor, s));
                }
                // Focus points are relative to the shot start; shifting them keeps the pan
                // exactly where it was when the shot starts before this piece does.
                let shift = shot.start_ms - src_start - s;
                let focus_path = shot.focus_path.iter().map(|&(t, x, y)| (t + shift, x, y)).collect();
                shots.push(ShotLayout { start_ms: s, end_ms: e, focus_path, ..shot.clone() });
                cursor = e;
            }
            if cursor < len {
                shots.push(whole(cursor, len));
            }
        } else if let Some(n) = split_panels(&item.framing).filter(|_| item.media.is_none()) {
            // Per camera shot (people move between them); fewer than two people found
            // there: nothing to split, show the whole frame.
            let mut cuts: Vec<i64> = self
                .shots
                .iter()
                .flat_map(|s| [s.start_ms, s.end_ms])
                .filter(|t| (src_start + 1..src_end).contains(t))
                .map(|t| t - src_start)
                .collect();
            cuts.extend([0, len]);
            cuts.sort_unstable();
            cuts.dedup();
            for pair in cuts.windows(2) {
                let people = self.people_in(src_start + pair[0], src_start + pair[1], n);
                shots.push(if people.len() >= 2 {
                    ShotLayout { start_ms: pair[0], end_ms: pair[1], layout: STACK.into(), source: "user".into(), people, ..Default::default() }
                } else {
                    whole(pair[0], pair[1])
                });
            }
        } else {
            shots.push(whole(0, len));
        }
        let (source_width, source_height) = self.read_size(item);
        ClipLayoutPlan { shots, source_width, source_height, vision_cost_usd: 0.0, face_samples: Vec::new() }
    }

    /// Up to `n` people on screen longest in source [src_start, src_end), left to
    /// right, from the analyzed faces. A face cut by the frame edge comes last; a
    /// track seen in under a third of the frames, or where someone already picked
    /// sits (a broken-up track), is nobody new.
    fn people_in(&self, src_start: i64, src_end: i64, n: usize) -> Vec<LBox> {
        let frames: Vec<FrameInfo> = self
            .face_samples
            .iter()
            .filter(|(t, _)| (src_start..src_end).contains(t))
            .map(|(t, faces)| FrameInfo { t_ms: *t, faces: faces.clone(), ..Default::default() })
            .collect();
        let cut = |m: &LBox| m.x < 0.01 || m.x + m.w > 0.99;
        let mut tracks: Vec<(bool, usize, LBox)> =
            track_faces(&frames).iter().map(|t| (cut(&t.median_box()), t.samples.len(), t.median_box())).collect();
        tracks.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        let mut people: Vec<LBox> = Vec::new();
        for (_, seen, face) in tracks {
            let apart = people.iter().all(|p| (p.cx() - face.cx()).abs() > p.w.max(face.w));
            if people.len() < n && seen * 3 >= frames.len() && apart {
                people.push(face);
            }
        }
        people.sort_by(|a, b| a.cx().total_cmp(&b.cx()));
        people
    }

    /// Each item's (index, output start ms) on the edited timeline. A piece that
    /// dissolves in starts that much before the previous one ends.
    fn placements(&self) -> Vec<(usize, i64)> {
        let mut at = 0;
        self.video
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let start = at - self.overlap(i);
                at = start + item.out_ms();
                (i, start)
            })
            .collect()
    }

    /// The whole timeline's framing and faces in output time (captions and overlays
    /// are positioned from it), and its caption words in output time.
    pub fn timeline(&self) -> (ClipLayoutPlan, Vec<TranscriptSegment>) {
        let mut plan = ClipLayoutPlan {
            shots: Vec::new(),
            source_width: self.source_width,
            source_height: self.source_height,
            vision_cost_usd: 0.0,
            face_samples: Vec::new(),
        };
        let mut words = Vec::new();
        for (i, out_start) in self.placements() {
            let item = &self.video[i];
            // Where a dissolve overlaps two pieces, framing and captions switch half way.
            let (lo, hi) = (out_start + self.overlap(i) / 2, out_start + item.out_ms() - self.overlap(i + 1) / 2);
            let clipped = lo > out_start || hi < out_start + item.out_ms();
            let local = self.local_plan(item, item.src_in_ms, item.src_out_ms);
            // Source time in this piece -> edited time (its speed applies).
            let out = |src: i64| out_start + item.out_at(src);
            let local_out = |t: i64| item.out_at(item.src_in_ms + t);
            plan.shots.extend(local.shots.into_iter().filter_map(|shot| {
                let (start_ms, end_ms) = ((out_start + local_out(shot.start_ms)).max(lo), (out_start + local_out(shot.end_ms)).min(hi));
                // The focus path stays relative to the unclipped start.
                let shift = start_ms - out_start - local_out(shot.start_ms);
                (end_ms > start_ms).then(|| ShotLayout {
                    start_ms,
                    end_ms,
                    focus_path: shot.focus_path.iter().map(|&(t, x, y)| (local_out(t) - shift, x, y)).collect(),
                    ..shot
                })
            }));
            if (item.framing == FRAMING_AUTO || split_panels(&item.framing).is_some()) && item.media.is_none() {
                plan.face_samples.extend(
                    self.face_samples
                        .iter()
                        .filter(|(t, _)| (item.src_in_ms..item.src_out_ms).contains(t))
                        .map(|(t, faces)| (out(*t), faces.clone()))
                        .filter(|(t, _)| (lo..hi).contains(t)),
                );
            }
            // Captions are the source's words; an added video has none.
            if item.media.is_some() {
                continue;
            }
            // Same word rules as a rendered clip: a word shows while its audio plays.
            let len = item.len_ms();
            let piece = TimeMap::new(vec![(0, len)], Some(len));
            words.extend(remap_segments(&self.transcript, item.src_in_ms, &piece).into_iter().filter_map(|mut seg| {
                seg.start_time_ms = out(seg.start_time_ms);
                seg.end_time_ms = out(seg.end_time_ms);
                for w in &mut seg.words {
                    w.start_time_ms = out(w.start_time_ms);
                    w.end_time_ms = out(w.end_time_ms);
                }
                if clipped {
                    // A word plays with the piece it starts in.
                    let had_words = !seg.words.is_empty();
                    seg.words.retain(|w| (lo..hi).contains(&w.start_time_ms));
                    if had_words && seg.words.is_empty() || !had_words && !(lo..hi).contains(&seg.start_time_ms) {
                        return None;
                    }
                    for w in &mut seg.words {
                        w.end_time_ms = w.end_time_ms.min(hi);
                    }
                    (seg.start_time_ms, seg.end_time_ms) = (seg.start_time_ms.max(lo), seg.end_time_ms.min(hi));
                    if had_words {
                        seg.text = seg.words.iter().map(|w| w.word.as_str()).collect::<Vec<_>>().join(" ");
                    }
                }
                Some(seg)
            }));
        }
        words.sort_by_key(|s| s.start_time_ms);
        (plan, words)
    }

    /// The filters that cut a keyframed crop out of a read starting `offset` edited
    /// seconds into the piece, as a canvas-sized picture: the read is scaled so the window
    /// at its zoom is canvas-sized, then the window is cut out at its place, per frame.
    fn moving_crop(&self, item: &VideoItem, offset: f64) -> String {
        let (sw, sh) = self.read_size(item);
        let (sw, sh) = (sw as f64, sh as f64);
        let (cw, ch) = (self.width as f64, self.height as f64);
        let aspect = cw / ch;
        // The largest canvas-shaped window (zoom 1), in source px.
        let fit_w = if sw / sh > aspect { sh * aspect } else { sw };
        // Edited seconds into the piece at this frame.
        let u = format!("({offset:.6}+t/{})", item.speed);
        let curve = |f: fn(&CropKey) -> f64| eased_expr(&item.keyframes.iter().map(|k| (k.t_ms as f64 / 1000.0, f(k))).collect::<Vec<_>>(), &u);
        let (zoom, cx, cy) = (curve(|k| k.zoom), curve(|k| k.x), curve(|k| k.y));
        // Scale: source px -> canvas px at this zoom.
        let scale = format!("({cw}*({zoom})/{fit_w:.6})");
        let (w, h) = (format!("({sw}*{scale})"), format!("({sh}*{scale})"));
        format!(
            "setpts=PTS-STARTPTS,scale=w='max({cw},2*trunc({w}/2))':h='max({ch},2*trunc({h}/2))':eval=frame:flags=bicubic,\
             crop=w={cw}:h={ch}:x='clip(({cx})*{w}-{cw}/2,0,{w}-{cw})':y='clip(({cy})*{h}-{ch}/2,0,{h}-{ch})':exact=1,setsar=1"
        )
    }

    /// One read of piece `i` (placed at `out_start`) over edited time [s, e): pushes the
    /// read and its filters, and returns its picture and (with source audio) sound labels.
    #[allow(clippy::too_many_arguments)]
    fn piece_read(
        &self,
        i: usize,
        out_start: i64,
        (s, e): (i64, i64),
        (source_audio, track_audio): (bool, bool),
        media_dir: &Path,
        reads: &mut Vec<(i64, i64)>,
        read_paths: &mut Vec<Option<PathBuf>>,
        parts: &mut Vec<String>,
    ) -> Result<Option<(String, Option<String>)>> {
        let item = &self.video[i];
        let out_len = item.out_ms();
        let (src_start, src_end) = (item.src_at(s - out_start), item.src_at(e - out_start));
        if src_end <= src_start {
            return Ok(None);
        }
        let k = reads.len();
        reads.push((src_start, src_end - src_start));
        read_paths.push(item.media.as_ref().map(|m| media_dir.join(m)));
        // Added videos always carry sound (import_media); the source may not.
        let own_audio = item.media.is_some() || source_audio;
        // A crop piece reads its window, then fills the canvas with it. Keyframes move the
        // window: the read is scaled so the window is canvas-sized, then cut out per frame.
        let animated = item.framing == FRAMING_CROP && item.keyframes.len() > 1;
        let still = item.keyframes.first().map(|k| Crop { x: k.x, y: k.y, zoom: k.zoom });
        let crop = (item.framing == FRAMING_CROP && !animated)
            .then(|| self.crop_rect_in(self.read_size(item), still.or(item.crop).unwrap_or(Crop { x: 0.5, y: 0.5, zoom: 1.0 })));
        let mut plan = self.local_plan(item, src_start, src_end);
        if let Some((w, h, _, _)) = crop {
            (plan.source_width, plan.source_height) = (w as u32, h as u32);
        }
        if animated {
            (plan.source_width, plan.source_height) = (self.width as u32, self.height as u32);
        }
        let graph = build_layout_graph(
            &plan,
            self.width,
            self.height,
            &GraphOptions {
                keeps: None,
                with_audio: own_audio,
                landscape: self.aspect_ratio == "16:9",
                fps: &self.fps,
                // Loudness is measured over the whole mix, not per piece.
                loudness_filter: Some("anull"),
                video_speed: 1.0,
                backdrop: self.backdrop.as_deref(),
            },
        )?;
        // build_layout_graph reads input 0 with fixed labels: give this read its own.
        let video_in = match crop {
            Some((w, h, x, y)) => {
                parts.push(format!("[{k}:v]crop={w}:{h}:{x}:{y}[p{k}_crop]"));
                format!("[p{k}_crop]")
            }
            None if animated => {
                parts.push(format!("[{k}:v]{}[p{k}_crop]", self.moving_crop(item, (s - out_start) as f64 / 1000.0)));
                format!("[p{k}_crop]")
            }
            None => format!("[{k}:v]"),
        };
        let graph = graph
            .replace('[', &format!("[p{k}_"))
            .replace(&format!("[p{k}_0:v]"), &video_in)
            .replace(&format!("[p{k}_0:a:0]"), &format!("[{k}:a:0]"));
        parts.push(graph);
        // This piece's speed, then its dip-to-black fades (in edited time: the chain's
        // clock is moved to where this read starts in the piece, so a range that starts
        // mid-fade gets the same frames as the export).
        let offset = (s - out_start) as f64 / 1000.0;
        let (fade_in, fade_out) = (self.dip(i) / 2, self.dip(i + 1) / 2);
        // Colour first, on the canvas-sized picture.
        let mut chain = item.color.map(|c| c.filters()).unwrap_or_default();
        if item.speed != 1.0 {
            chain.push(format!("settb=AVTB,setpts=(PTS-STARTPTS)/{},fps={}:start_time=0:round=near", item.speed, self.fps));
        }
        if fade_in > 0 || fade_out > 0 {
            chain.push(format!("setpts=PTS-STARTPTS+{offset:.6}/TB"));
            if fade_in > 0 {
                chain.push(format!("fade=t=in:st=0:d={:.6}", fade_in as f64 / 1000.0));
            }
            if fade_out > 0 {
                chain.push(format!("fade=t=out:st={:.6}:d={:.6}", (out_len - fade_out) as f64 / 1000.0, fade_out as f64 / 1000.0));
            }
            chain.push("setpts=PTS-STARTPTS".into());
        }
        let video = if chain.is_empty() {
            format!("[p{k}_base]")
        } else {
            parts.push(format!("[p{k}_base]{}[p{k}_v]", chain.join(",")));
            format!("[p{k}_v]")
        };
        let audio = track_audio.then(|| {
            if !own_audio {
                // Silence where a source without sound sits among added videos.
                parts.push(format!("anullsrc=r=48000:cl=stereo,atrim=end_sample={},{AUDIO_FORMAT}[pa{k}]", (e - s) * 48));
                return format!("[pa{k}]");
            }
            let tempo = if item.speed != 1.0 {
                let samples = (e - s) * 48;
                format!("atempo={},apad=whole_len={samples},atrim=end_sample={samples},", item.speed)
            } else {
                String::new()
            };
            let gain = gain_filter(item.volume, out_len, s - out_start, item.fade_in_ms, item.fade_out_ms);
            parts.push(format!("[p{k}_aout]{tempo}{gain}[pa{k}]"));
            format!("[pa{k}]")
        });
        Ok(Some((video, audio)))
    }

    /// Reads and the video/audio graph for output range [start, end): the graph ends in
    /// [base] (clock starting at `start`) and, when there is any sound, [aout]. The main
    /// track's sound (when the source has some) and detached source sound are the voice
    /// that ducked sounds drop under. Imported sounds are files in `media_dir`.
    ///
    /// The range is cut where pieces start and end; where a dissolve overlaps two pieces,
    /// both are read and blended, timed from the dissolve's start, so any range gets the
    /// export's frames.
    pub fn pieces_graph(&self, range: (i64, i64), source: &Path, source_audio: bool, media_dir: &Path, loudness: &str) -> Result<ProjectGraph> {
        let (r0, r1) = range;
        // A range starting under ducked music renders a little earlier sound, then drops it.
        let ducks = self.audio.iter().any(|a| a.duck && a.file.is_some() && !a.voice && a.start_ms < r1 && a.start_ms + a.out_ms() > r0);
        let lead = if ducks { DUCK_LEAD_MS.min(r0) } else { 0 };
        let from = r0 - lead;
        let placed = self.placements();
        let mut cuts = vec![from, r1];
        for &(i, start) in &placed {
            cuts.extend([start, start + self.video[i].out_ms()].into_iter().filter(|t| (from + 1..r1).contains(t)));
        }
        cuts.sort_unstable();
        cuts.dedup();
        let mut reads = Vec::new();
        let mut read_paths = Vec::new();
        let mut parts = Vec::new();
        let mut outputs = String::new();
        let mut n = 0;
        // The main track has sound when the source or any added video does.
        let track_audio = source_audio || self.video.iter().any(|v| v.media.is_some());
        for w in cuts.windows(2) {
            let (s, e) = (w[0], w[1]);
            let active: Vec<(usize, i64)> =
                placed.iter().copied().filter(|&(i, start)| start < e && start + self.video[i].out_ms() > s).collect();
            let mut read = |i: usize, start: i64, span: (i64, i64)| {
                self.piece_read(i, start, span, (source_audio, track_audio), media_dir, &mut reads, &mut read_paths, &mut parts)
            };
            match active.as_slice() {
                [] => continue,
                &[(i, start)] => {
                    let Some((v, a)) = read(i, start, (s, e))? else { continue };
                    outputs += &v;
                    outputs += a.as_deref().unwrap_or("");
                }
                &[(i, a_start), (j, b_start)] => {
                    // A blend: both pieces are read over the whole overlap, so the effect's
                    // progress is the export's however the range cuts it, then trimmed to it.
                    let (o0, o1) = (b_start, a_start + self.video[i].out_ms());
                    let (Some((va, aa)), Some((vb, ab))) = (read(i, a_start, (o0, o1))?, read(j, b_start, (o0, o1))?) else {
                        return Err(render_error("A transition is too short"));
                    };
                    let d = (o1 - o0) as f64 / 1000.0;
                    let fps = &self.fps;
                    let rate = Rational::parse(fps).map(|r| r.to_f64()).unwrap_or(30.0);
                    let frame = |t: i64| ((t - o0) as f64 * rate / 1000.0).round() as i64;
                    parts.push(format!("{va}fps={fps}[xa{n}]"));
                    parts.push(format!("{vb}fps={fps}[xb{n}]"));
                    parts.push(format!(
                        "[xa{n}][xb{n}]xfade=transition={}:duration={d:.6}:offset=0,trim=start_frame={}:end_frame={},setpts=PTS-STARTPTS[xv{n}]",
                        self.video[j].effect,
                        frame(s),
                        frame(e)
                    ));
                    outputs += &format!("[xv{n}]");
                    if let (Some(aa), Some(ab)) = (aa, ab) {
                        let (from, to) = ((s - o0) * 48, (e - o0) * 48);
                        parts.push(format!("{aa}volume='min(1,max(0,({d:.6}-t)/{d:.6}))':eval=frame[xo{n}]"));
                        parts.push(format!("{ab}volume='min(1,t/{d:.6})':eval=frame[xi{n}]"));
                        parts.push(format!(
                            "[xo{n}][xi{n}]amix=inputs=2:duration=longest:dropout_transition=0:normalize=0,atrim=start_sample={from}:end_sample={to},asetpts=PTS-STARTPTS[xa{n}]"
                        ));
                        outputs += &format!("[xa{n}]");
                    }
                }
                _ => return Err(render_error("Transitions overlap")),
            }
            n += 1;
        }
        if n == 0 {
            return Err(render_error("The edit contains no video in this range"));
        }
        let offset = r0 as f64 / 1000.0;
        let a = if track_audio { 1 } else { 0 };
        let audio_out = if track_audio { "[acat]" } else { "" };
        parts.push(format!("{outputs}concat=n={n}:v=1:a={a}[vcat]{audio_out}"));
        // Captions and overlays are timed on the whole timeline: start this range's clock at its place in it.
        let trim = if lead > 0 {
            let fps = Rational::parse(&self.fps).map(|r| r.to_f64()).unwrap_or(30.0);
            format!("trim=start_frame={},setpts=PTS-STARTPTS,", (lead as f64 * fps / 1000.0).round() as i64)
        } else {
            String::new()
        };
        // Picture-in-picture videos go over the main track (under captions and overlays).
        let pips: Vec<(usize, &PipItem, i64, i64)> = self
            .pip
            .iter()
            .enumerate()
            .map(|(j, p)| (j, p, p.start_ms.max(from), (p.start_ms + p.len_ms()).min(r1)))
            .filter(|&(_, _, s, e)| e > s)
            .collect();
        let main = if pips.is_empty() { "[base]" } else { "[main0]" };
        parts.push(format!("[vcat]{trim}setpts=PTS+{offset:.6}/TB{main}"));
        let mut audio_reads = Vec::new();
        let mut plain = Vec::new();
        for (m, &(j, p, s, e)) in pips.iter().enumerate() {
            let input = reads.len() + audio_reads.len();
            audio_reads.push((media_dir.join(&p.media), p.src_in_ms + s - p.start_ms, e - s));
            let (w, h) = (self.width as f64, self.height as f64);
            let width = ((p.width * w / 2.0).round() as i64 * 2).max(2);
            // Timed on the timeline's clock, like [main0].
            parts.push(format!("[{input}:v]scale={width}:-2,setsar=1,setpts=PTS-STARTPTS+{:.6}/TB[pv{j}]", s as f64 / 1000.0));
            let next = if m + 1 == pips.len() { "[base]".to_string() } else { format!("[main{}]", m + 1) };
            parts.push(format!("[main{m}][pv{j}]overlay=x={}-w/2:y={}-h/2:eof_action=pass{next}", (p.x * w).round(), (p.y * h).round()));
            let gain = gain_filter(p.volume, p.len_ms(), s - p.start_ms, 0, 0);
            parts.push(format!("[{input}:a:0]{AUDIO_FORMAT},asetpts=PTS-STARTPTS,{gain},adelay={}:all=1[pp{j}]", s - from));
            plain.push(format!("[pp{j}]"));
        }

        // Sounds on the audio track that play in this range, each placed with adelay.
        let mut voice: Vec<String> = if track_audio { vec!["[acat]".into()] } else { Vec::new() };
        let mut ducked = Vec::new();
        for (j, item) in self.audio.iter().enumerate() {
            let (s, e) = (item.start_ms.max(from), (item.start_ms + item.out_ms()).min(r1));
            if e <= s || (item.file.is_none() && !source_audio) {
                continue;
            }
            let path = match &item.file {
                Some(name) => media_dir.join(name),
                None => source.to_path_buf(),
            };
            let input = reads.len() + audio_reads.len();
            // Timeline time into the sound, as time in its file.
            let at = |t: i64| (t as f64 * item.speed).round() as i64;
            let (in_file, len_file) = (at(s - item.start_ms), at(e - item.start_ms));
            audio_reads.push((path, item.src_in_ms + in_file, (len_file - in_file).max(1)));
            let tempo = if item.speed != 1.0 { format!("atempo={},", item.speed) } else { String::new() };
            let gain = gain_filter(item.volume, item.out_ms(), s - item.start_ms, item.fade_in_ms, item.fade_out_ms);
            let label = format!("[au{j}]");
            parts.push(format!("[{input}:a:0]{AUDIO_FORMAT},asetpts=PTS-STARTPTS,{tempo}{gain},adelay={}:all=1{label}", s - from));
            if item.file.is_none() || item.voice {
                voice.push(label);
            } else if item.duck {
                ducked.push(label);
            } else {
                plain.push(label);
            }
        }
        let mix = |labels: &[String], out: &str| match labels {
            [one] => format!("{one}anull{out}"),
            _ => format!("{}amix=inputs={}:duration=longest:dropout_transition=0:normalize=0{out}", labels.concat(), labels.len()),
        };
        let mut finals = Vec::new();
        if !ducked.is_empty() && !voice.is_empty() {
            parts.push(mix(&voice, "[voice]"));
            parts.push(mix(&ducked, "[music]"));
            parts.push("[voice]asplit=2[voice_out][voice_key]".into());
            parts.push(format!("[music][voice_key]{DUCK}[ducked]"));
            finals.extend(["[voice_out]".to_string(), "[ducked]".to_string()]);
        } else {
            finals.extend(voice);
            finals.extend(ducked);
        }
        finals.extend(plain);
        let audio = !finals.is_empty();
        if audio {
            // Every range's sound lasts exactly as long as its picture (after the lead is cut).
            let samples = (r1 - from) * 48;
            let cut = if lead > 0 { format!("atrim=start_sample={}:end_sample={samples},asetpts=PTS-STARTPTS", lead * 48) } else { format!("atrim=end_sample={samples}") };
            parts.push(mix(&finals, "[amixed]"));
            parts.push(format!(
                "[amixed]{AUDIO_FORMAT},apad=whole_len={samples},{cut},{loudness},{}asettb=1/48000,asetpts=N[aout]",
                speed_audio_filter(self.speed, r1 - r0)?
            ));
        }
        Ok(ProjectGraph { reads, read_paths, audio_reads, graph: parts.join(";"), audio })
    }

    /// The RenderRequest the caption and overlay code reads (output time, window at 0).
    /// `range` only names the caption and image files, so concurrent range renders don't share them.
    /// Overlays the user placed are left out: `Renderer::placed_overlays` draws those.
    fn overlay_request(&self, words: Vec<TranscriptSegment>, output_path: &Path, range: (i64, i64), media_dir: &Path) -> RenderRequest {
        RenderRequest {
            output_path: output_path.to_path_buf(),
            start_time_ms: range.0,
            end_time_ms: range.1,
            source_width: self.source_width as i64,
            source_height: self.source_height as i64,
            transcript_segments: words,
            include_captions: self.captions,
            caption_style: Some(captions::caption_style_for(&self.caption_preset)),
            title_text: self.title.clone().filter(|_| self.title_box.is_none()),
            emphasis_words: self.emphasis_words.clone(),
            banner_platform: self.banner_platform.clone().filter(|_| self.banner_box.is_none()),
            banner_channel_url: self.banner_channel_url.clone(),
            logos: self
                .logos
                .iter()
                .filter(|l| l.placed.is_none())
                .map(|l| LogoOverlay { path: media_dir.join(&l.path).to_string_lossy().into_owned(), position: l.position.clone(), size: l.size })
                .collect(),
            aspect_ratio: self.aspect_ratio.clone(),
            video_speed: self.speed,
            ..Default::default()
        }
    }
}

/// Where and how big to render a project.
pub struct ProjectRender<'a> {
    pub project: &'a Project,
    pub source: &'a Path,
    pub output_path: &'a Path,
    /// Output range on the edited timeline (before speed), ms. None: all of it.
    pub range: Option<(i64, i64)>,
    /// Final size; None renders at the canvas size.
    pub scale_to: Option<(i64, i64)>,
    /// Linear loudness filter measured over the whole mix; None uses single-pass loudnorm.
    pub loudness_filter: Option<String>,
    /// Folder holding the project's imported sounds (the run folder).
    pub media_dir: &'a Path,
    /// Where FFmpeg writes its progress (see `FfmpegJob::progress_path`).
    pub progress: Option<&'a Path>,
}

/// Automatic sizes the placed title and banner scale from (as `Renderer::overlays`).
fn overlay_base(project: &Project) -> (f64, i64) {
    if project.aspect_ratio == "16:9" {
        let scale = project.height as f64 / 1080.0;
        (scale, (28.0 * scale).round() as i64)
    } else {
        (1.0, 34)
    }
}

/// The title or banner image as the editor draws it, and where the renderer puts it.
#[derive(Debug, Serialize)]
pub struct OverlayPreview {
    /// "title" | "banner"
    pub key: String,
    pub path: PathBuf,
    /// Canvas px at scale 1.
    pub width: i64,
    pub height: i64,
    /// Automatic centre at the asked time, as fractions of the canvas.
    pub auto_x: f64,
    pub auto_y: f64,
}

impl Renderer {
    /// The title and banner images (written into `dir`) with their automatic centres at
    /// edited time `t_ms`, for the editor to draw and drag.
    pub fn overlay_previews(&self, project: &Project, dir: &Path, t_ms: i64) -> Result<Vec<OverlayPreview>> {
        let (w, h) = (project.width, project.height);
        let landscape = project.aspect_ratio == "16:9";
        let (plan, _) = project.timeline();
        let shot = plan.shots.iter().find(|s| t_ms < s.end_ms).or(plan.shots.last()).cloned().unwrap_or_default();
        let (src_w, src_h) = (project.source_width as i64, project.source_height as i64);
        let request = RenderRequest {
            output_path: dir.join("overlay.mp4"),
            title_text: project.title.clone(),
            banner_platform: project.banner_platform.clone(),
            banner_channel_url: project.banner_channel_url.clone(),
            ..Default::default()
        };
        let (scale, banner_size) = overlay_base(project);
        let frac = |x: f64, y: f64| (x / w as f64, y / h as f64);
        let mut out = Vec::new();
        if let Some((path, iw, ih)) = self.title_overlay_image(&request, w, scale)? {
            let top = if landscape { (40.0 * scale).round() as i64 } else { title_y(&shot, src_w, src_h, w, h, ih) };
            let (auto_x, auto_y) = frac(w as f64 / 2.0, (top + ih / 2) as f64);
            out.push(OverlayPreview { key: "title".into(), path, width: iw, height: ih, auto_x, auto_y });
        }
        if let Some((path, iw, ih)) = self.banner_overlay_image(&request, banner_size)? {
            let (x, y) = if landscape {
                ((w - iw - (20.0 * scale).round() as i64 + iw / 2) as f64, (h - ih - (16.0 * scale).round() as i64 + ih / 2) as f64)
            } else {
                (w as f64 / 2.0, (banner_y(&shot, src_w, src_h, w, h) + ih / 2) as f64)
            };
            let (auto_x, auto_y) = frac(x, y);
            out.push(OverlayPreview { key: "banner".into(), path, width: iw, height: ih, auto_x, auto_y });
        }
        Ok(out)
    }

    /// The title, banner and logos the user placed: centred where they put them, at
    /// their size, shown over their time.
    fn placed_overlays(&self, project: &Project, request: &RenderRequest, media_dir: &Path) -> Result<Vec<Overlay>> {
        let (w, h) = (project.width as f64, project.height as f64);
        let place = |b: &OverlayBox, path: PathBuf, filter: Option<String>| Overlay {
            path,
            x: format!("{}-w/2", (b.x * w).round()),
            y: format!("{}-h/2", (b.y * h).round()),
            filter,
            enable: Some(format!("between(t,{:.3},{:.3})", b.start_ms as f64 / 1000.0, b.end_ms as f64 / 1000.0)),
        };
        let (scale, banner_size) = overlay_base(project);
        let mut out = Vec::new();
        if let Some(b) = &project.title_box {
            let with_title = RenderRequest { title_text: project.title.clone(), ..request.clone() };
            if let Some((path, _, _)) = self.title_overlay_image(&with_title, project.width, scale * b.scale)? {
                out.push(place(b, path, None));
            }
        }
        if let Some(b) = &project.banner_box {
            let with_banner = RenderRequest { banner_platform: project.banner_platform.clone(), ..request.clone() };
            if let Some((path, _, _)) = self.banner_overlay_image(&with_banner, (banner_size as f64 * b.scale).round() as i64)? {
                out.push(place(b, path, None));
            }
        }
        for (i, logo) in project.logos.iter().enumerate() {
            let Some(b) = &logo.placed else { continue };
            let ext = Path::new(&logo.path).extension().and_then(|e| e.to_str()).unwrap_or("png").to_ascii_lowercase();
            // Copied like automatic logos: overlay inputs are deleted after the render.
            let path = sibling(&request.output_path, &format!("logo-placed-{}-{}-{i}.{ext}", request.start_time_ms, request.end_time_ms));
            if let Err(e) = std::fs::copy(media_dir.join(&logo.path), &path) {
                tracing::warn!("Logo {} unavailable; rendering without it: {e}", i + 1);
                continue;
            }
            let width = ((w * logo.size.clamp(MIN_LOGO_SIZE, MAX_LOGO_SIZE)) / 2.0).round() as i64 * 2;
            out.push(place(b, path, Some(format!("scale={width}:-1:flags=lanczos,format=rgba"))));
        }
        Ok(out)
    }

    /// Measures the whole edited mix's loudness and returns the linear filter that brings
    /// it to the platform target. Preview ranges and the export apply the same filter, so
    /// they sound the same (single-pass loudnorm would level each range differently).
    /// None when the edit has no sound or the measurement failed.
    pub async fn measure_project_loudness(&self, project: &Project, source: &Path, media_dir: &Path, cancel: &CancellationToken) -> Result<Option<String>> {
        let source_audio = self.has_audio(source, cancel).await?;
        // The picture is decoded anyway; a tiny canvas keeps the measuring pass cheap.
        let small = Project { width: 64, height: if project.height > project.width { 114 } else { 36 }, ..project.clone() };
        let measure = format!("loudnorm={LOUDNESS_TARGET}:print_format=json");
        let g = small.pieces_graph((0, small.duration_ms()), source, source_audio, media_dir, &measure)?;
        if !g.audio {
            return Ok(None);
        }
        let mut args: Vec<String> = ["-nostdin", "-nostats", "-hide_banner"].map(String::from).to_vec();
        args.extend(g.input_args(source));
        args.extend(["-filter_complex".into(), format!("{};[base]nullsink", g.graph), "-map".into(), "[aout]".into()]);
        args.extend(["-f", "null", "-"].map(String::from));
        let out = match self.runner.run(&self.ffmpeg, &args, MEDIA_TIMEOUT, MAX_TOOL_OUTPUT_BYTES, cancel).await {
            Err(EngineError::Cancelled) => return Err(EngineError::Cancelled),
            Err(e) => {
                tracing::warn!("Edit loudness measurement failed; using single-pass normalization: {e}");
                return Ok(None);
            }
            Ok(out) => out,
        };
        let text = String::from_utf8_lossy(&out.stderr);
        let measured = regex::Regex::new(r#"\{[^{}]*"input_i"[^{}]*\}"#)
            .ok()
            .and_then(|re| re.find(&text))
            .and_then(|m| serde_json::from_str::<serde_json::Value>(m.as_str()).ok());
        Ok(measured.as_ref().filter(|_| out.success()).and_then(measured_loudness_filter))
    }

    /// Renders a project (or a range of it) through the same graph for preview and export.
    pub async fn render_project(&self, job: &ProjectRender<'_>, cancel: &CancellationToken) -> Result<()> {
        let project = job.project;
        let total = project.duration_ms();
        let (r0, r1) = job.range.unwrap_or((0, total));
        let (r0, r1) = (r0.clamp(0, total), r1.clamp(0, total));
        if r1 <= r0 {
            return Err(render_error("Nothing to render in this range"));
        }
        if let Some(parent) = job.output_path.parent() {
            std::fs::create_dir_all(parent).map_err(io_render_error)?;
        }
        let source_audio = self.has_audio(job.source, cancel).await?;
        let loudness = job.loudness_filter.clone().unwrap_or_else(loudness_filter_default);
        let pieces = project.pieces_graph((r0, r1), job.source, source_audio, job.media_dir, &loudness)?;
        let inputs = pieces.inputs(job.source);
        let (audio, mut graph) = (pieces.audio, pieces.graph);

        // Captions and overlays for the whole timeline, exactly as the export draws them.
        let (plan, words) = project.timeline();
        let landscape = project.aspect_ratio == "16:9";
        let request = project.overlay_request(words, job.output_path, (r0, r1), job.media_dir);
        let identity = TimeMap::new(vec![(0, total)], Some(total));
        let caption_path =
            self.generate_captions(&request, project.width, project.height, 0, &identity, &plan, landscape, Some(&plan))?;
        // Text layers go in the same ASS file, drawn by the same libass pass.
        let text_path = job.output_path.with_extension(format!("texts-{r0}-{r1}.ass"));
        let caption_text = caption_path.as_deref().map(std::fs::read_to_string).transpose().map_err(io_render_error)?;
        let caption_path = match project.with_text_layers(caption_text.clone()) {
            Some(document) if Some(&document) != caption_text.as_ref() => {
                std::fs::write(&text_path, document).map_err(io_render_error)?;
                if let Some(old) = &caption_path {
                    let _ = std::fs::remove_file(old);
                }
                Some(text_path)
            }
            _ => caption_path,
        };
        graph += &format!(";[base]{}[captioned]", self.caption_filter(caption_path.as_deref()));
        let mut overlays = self.overlays(&request, &plan, project.width, project.height, landscape)?;
        overlays.extend(self.placed_overlays(project, &request, job.media_dir)?);
        let mut video_filter = speed_video_filter(project.speed, &project.fps)?;
        video_filter += ",setpts=PTS-STARTPTS";
        if let Some((w, h)) = job.scale_to {
            video_filter += &format!(",scale={w}:{h}:flags=bicubic,setsar=1");
        }
        let (filter_complex, extra_inputs) = compose_overlays_from(&graph, &overlays, &video_filter, inputs.len());

        let (first, more) = inputs.split_first().ok_or_else(|| render_error("The edit contains no video"))?;
        let output_size = job.scale_to.unwrap_or((project.width, project.height));
        let result = self
            .run_ffmpeg_complex(
                &FfmpegJob {
                    input_path: &first.0,
                    more_sources: &[],
                    audio_inputs: more,
                    output_path: job.output_path,
                    start_time_ms: first.1,
                    duration_ms: first.2,
                    filter_complex: &filter_complex,
                    audio_label: audio.then_some("[aout]"),
                    extra_inputs: &extra_inputs,
                    fps: &project.fps,
                    output_size,
                    output_duration_ms: Some(scaled_duration_ms(r1 - r0, project.speed)?),
                    progress_path: job.progress,
                },
                cancel,
            )
            .await;
        for path in extra_inputs.iter().chain(caption_path.iter()) {
            let _ = std::fs::remove_file(path);
        }
        result
    }
}

/// Files the editor adds as video: images become still video.
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];
pub const VIDEO_IMPORT_EXTENSIONS: &[&str] = &["mp4", "mov", "m4v", "mkv", "webm", "avi"];
/// Longest edge of an added video, px.
const MAX_MEDIA_EDGE: i64 = 1920;

impl Renderer {
    /// Converts a video or image the user picked into `dest`, an .mp4 the project reads
    /// like the source: its frame rate, at most MAX_MEDIA_EDGE px, upright, with a sound
    /// track (silent when it had none). An image becomes IMAGE_MS of still video.
    /// Returns (width, height, duration ms, image).
    pub async fn import_media(&self, file: &Path, dest: &Path, fps: &str, cancel: &CancellationToken) -> Result<(u32, u32, i64, bool)> {
        let ext = file.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
        let image = IMAGE_EXTENSIONS.contains(&ext.as_str());
        if !image && !VIDEO_IMPORT_EXTENSIONS.contains(&ext.as_str()) {
            return Err(render_error("Choose a video (MP4, MOV, MKV, WebM) or an image (PNG, JPEG, WebP)"));
        }
        let s = |v: &str| v.to_string();
        let mut args: Vec<String> = ["-nostdin", "-nostats", "-y", "-protocol_whitelist", "file,pipe,fd"].map(String::from).to_vec();
        // Seconds of silence for an input without sound: as long as its picture, since
        // -shortest alone lets endless silence run seconds past it when encoding lags.
        let silence = if image {
            // One picture, repeated at the project's frame rate.
            args.extend(["-format_whitelist", "image2", "-f", "image2", "-pattern_type", "none", "-loop", "1", "-framerate", fps].map(String::from));
            args.extend([s("-t"), format!("{}", IMAGE_MS / 1000)]);
            Some(IMAGE_MS as f64 / 1000.0)
        } else {
            let probed = probe_untrusted(&self.ffprobe, file, cancel).await.map_err(|_| render_error("Clipward can't read that video"))?;
            args.extend([s("-format_whitelist"), s("mov,matroska,webm,avi"), s("-t"), format!("{}", MAX_MEDIA_MS / 1000)]);
            let has_audio = probed["streams"].as_array().is_some_and(|all| all.iter().any(|st| st["codec_type"] == "audio"));
            let seconds = probed["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok()).filter(|d| *d > 0.0);
            (!has_audio).then(|| seconds.unwrap_or(f64::MAX).min(MAX_MEDIA_MS as f64 / 1000.0))
        };
        args.extend([s("-i"), file.to_string_lossy().into_owned()]);
        if let Some(seconds) = silence {
            args.extend([s("-f"), s("lavfi"), s("-i"), format!("anullsrc=r=48000:cl=stereo:d={seconds}")]);
        }
        let edge = MAX_MEDIA_EDGE;
        let scale = format!("scale='if(gte(iw,ih),trunc(min({edge},iw)/2)*2,-2)':'if(gte(iw,ih),-2,trunc(min({edge},ih)/2)*2)'");
        args.extend([s("-map"), s("0:v:0"), s("-map"), s(if silence.is_some() { "1:a:0" } else { "0:a:0" })]);
        args.extend([s("-vf"), format!("{scale},fps={fps},format=yuv420p,setsar=1")]);
        args.extend(self.video_codec_args(1080, 1920, fps));
        args.extend(["-c:a", "aac", "-b:a", "192k", "-ar", "48000", "-ac", "2", "-shortest", "-movflags", "+faststart", "-f", "mp4"].map(String::from));
        args.push(dest.to_string_lossy().into_owned());
        let out = self.runner.run(&self.ffmpeg, &args, MEDIA_TIMEOUT, MAX_TOOL_OUTPUT_BYTES, cancel).await?;
        let fail = |message: &str| {
            let _ = std::fs::remove_file(dest);
            render_error(message)
        };
        if !out.success() {
            return Err(fail(if image { "Clipward can't read that image" } else { "Clipward can't read that video" }));
        }
        let probed = probe_untrusted(&self.ffprobe, dest, cancel).await.map_err(|_| fail("Clipward can't read that video"))?;
        let video = probed["streams"].as_array().and_then(|all| all.iter().find(|st| st["codec_type"] == "video").cloned()).unwrap_or_default();
        let (w, h) = (video["width"].as_u64().unwrap_or(0) as u32, video["height"].as_u64().unwrap_or(0) as u32);
        let seconds = probed["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok()).unwrap_or(0.0);
        if w == 0 || h == 0 || seconds <= 0.0 {
            return Err(fail("Clipward can't read that video"));
        }
        Ok((w, h, (seconds * 1000.0).round() as i64, image))
    }
}

/// `clip_NN.project.json` next to `clip_NN.mp4`.
pub fn project_path(clip_path: &Path) -> PathBuf {
    clip_path.with_extension("project.json")
}

/// Sound files the editor imports, and how long an import may take.
pub const AUDIO_IMPORT_EXTENSIONS: &[&str] = &["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "mp4", "mov"];
const AUDIO_IMPORT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
/// Longest sound the editor imports.
pub const MAX_IMPORT_MS: i64 = 60 * 60 * 1000;

/// Converts a user's sound file into `dest` (an .m4a the renderer reads like any other
/// input) and returns its length. Only the conversion reads the user's format.
pub async fn import_audio(ffmpeg: &Path, ffprobe: &Path, file: &Path, dest: &Path, cancel: &CancellationToken) -> Result<i64> {
    let mut args: Vec<String> = ["-nostdin", "-nostats", "-y", "-protocol_whitelist", "file,pipe,fd", "-format_whitelist"].map(String::from).to_vec();
    args.push("mov,matroska,webm,mp3,aac,wav,flac,ogg".into());
    args.extend(["-t".into(), format!("{}", MAX_IMPORT_MS / 1000), "-i".into(), file.to_string_lossy().into_owned()]);
    args.extend(["-vn", "-sn", "-dn", "-map", "0:a:0", "-ac", "2", "-ar", "48000", "-c:a", "aac", "-b:a", "192k", "-f", "mp4"].map(String::from));
    args.push(dest.to_string_lossy().into_owned());
    let out = run_bounded(ffmpeg, &args, AUDIO_IMPORT_TIMEOUT, MAX_TOOL_OUTPUT_BYTES, cancel).await?;
    if !out.status.success() {
        let _ = std::fs::remove_file(dest);
        return Err(render_error("That file has no sound Clipward can read"));
    }
    let probed = probe_untrusted(ffprobe, dest, cancel).await?;
    let seconds = probed["format"]["duration"].as_str().and_then(|d| d.parse::<f64>().ok()).unwrap_or(0.0);
    if seconds <= 0.0 {
        let _ = std::fs::remove_file(dest);
        return Err(render_error("That file has no sound Clipward can read"));
    }
    Ok((seconds * 1000.0).round() as i64)
}

/// Seconds of video per `filmstrip` image, one frame per second.
pub const FILMSTRIP_BLOCK_S: u32 = 60;
/// Height of a filmstrip frame, px: sharp on the tallest timeline track (240 px, half of it
/// frames, half waveform), while a block's image stays well inside JPEG's size limit.
pub const FILMSTRIP_HEIGHT: u32 = 180;

/// Frames of seconds `block * FILMSTRIP_BLOCK_S ..` of a video, one per second, side by
/// side in one JPEG at `out` (a short last block is padded black).
pub async fn filmstrip(ffmpeg: &Path, file: &Path, block: u32, out: &Path, cancel: &CancellationToken) -> Result<()> {
    let mut args: Vec<String> = ["-nostdin", "-nostats", "-v", "error", "-y", "-ss"].map(String::from).to_vec();
    args.push(format!("{}", block * FILMSTRIP_BLOCK_S));
    args.extend(MEDIA_INPUT_OPTIONS.iter().map(|v| v.to_string()));
    args.extend(["-t".into(), FILMSTRIP_BLOCK_S.to_string(), "-i".into(), file.to_string_lossy().into_owned()]);
    args.extend(["-an", "-sn", "-vf"].map(String::from));
    args.push(format!("fps=1,scale=-2:{FILMSTRIP_HEIGHT},tile={FILMSTRIP_BLOCK_S}x1"));
    args.extend(["-frames:v", "1", "-q:v", "6"].map(String::from));
    args.push(out.to_string_lossy().into_owned());
    let done = run_bounded(ffmpeg, &args, MEDIA_TIMEOUT, MAX_TOOL_OUTPUT_BYTES, cancel).await?;
    if !done.status.success() || !out.is_file() {
        return Err(render_error("Could not read frames from this video"));
    }
    Ok(())
}

/// Joins preview chunks (one encoder setup, each starting on a keyframe) into one
/// file without re-encoding, so a player never hops between files.
pub async fn join_chunks(ffmpeg: &Path, parts: &[PathBuf], out: &Path, cancel: &CancellationToken) -> Result<()> {
    let list = out.with_extension("txt");
    let lines: String = parts.iter().map(|p| format!("file '{}'\n", p.to_string_lossy().replace('\'', "'\\''"))).collect();
    std::fs::write(&list, lines).map_err(|e| render_error(&format!("Could not join the preview: {e}")))?;
    let mut args: Vec<String> = ["-nostdin", "-nostats", "-v", "error", "-y", "-f", "concat", "-safe", "0", "-i"].map(String::from).to_vec();
    args.push(list.to_string_lossy().into_owned());
    args.extend(["-c", "copy", "-movflags", "+faststart", "-f", "mp4"].map(String::from));
    args.push(out.to_string_lossy().into_owned());
    let done = run_bounded(ffmpeg, &args, MEDIA_TIMEOUT, MAX_TOOL_OUTPUT_BYTES, cancel).await;
    let _ = std::fs::remove_file(&list);
    if !done?.status.success() || !out.is_file() {
        return Err(render_error("Could not join the preview"));
    }
    Ok(())
}

/// Peaks per second in `waveform`.
pub const WAVEFORM_RATE: usize = 20;

/// A file's sound as loudness peaks, WAVEFORM_RATE per second, 0..=255 each.
pub async fn waveform(ffmpeg: &Path, file: &Path, cancel: &CancellationToken) -> Result<Vec<u8>> {
    const RATE: usize = 1000;
    let mut args: Vec<String> = ["-nostdin", "-nostats", "-v", "error"].map(String::from).to_vec();
    args.extend(MEDIA_INPUT_OPTIONS.iter().map(|v| v.to_string()));
    args.extend(["-t".into(), format!("{}", MAX_IMPORT_MS / 1000), "-i".into(), file.to_string_lossy().into_owned()]);
    // Rectified before downsampling, so the loudness envelope survives the low-pass.
    args.extend(["-vn", "-map", "0:a:0", "-af", "aeval='abs(val(ch))':c=same", "-ac", "1", "-ar", "1000", "-f", "s16le", "-"].map(String::from));
    // An hour at 1000 samples/s of 16-bit mono.
    let max = (MAX_IMPORT_MS as usize) * 2 + 1024;
    let out = run_bounded(ffmpeg, &args, AUDIO_IMPORT_TIMEOUT, max, cancel).await?;
    if !out.status.success() {
        return Err(render_error("Could not read this sound"));
    }
    let samples: Vec<i32> = out.stdout.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as i32).collect();
    Ok(samples
        .chunks(RATE / WAVEFORM_RATE)
        .map(|c| {
            let peak = c.iter().map(|v| v.unsigned_abs()).max().unwrap_or(0) as f64 / 32768.0;
            // Square root: quiet speech still shows.
            (peak.sqrt() * 255.0).round().min(255.0) as u8
        })
        .collect())
}
