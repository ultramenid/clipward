//! The Create wizard's draft and the run request it submits (port of the helpers in
//! src/renderer/components/JobForm.tsx, the draft in src/renderer/store/use-draft-store.ts
//! and the option lists in src/shared/job-contract.ts).
use crate::format::{js_round, js_trim, parse_timecode, source_label};
use crate::manual_clips::{
    ManualClipRow, ManualClipsCheck, check_manual_clips, format_clip_length, new_manual_row,
    to_manual_clips,
};
use crate::shared::MAX_PARALLEL_JOBS;
use crate::video_source::{normalize_video_source, twitch_source_error};
use clip_engine::LogoOverlay;
use serde_json::{Value, json};

pub use crate::shared::{
    MAX_BANNER_CHARS, MAX_LOGOS, MAX_MANUAL_CLIP_SECONDS, MAX_MANUAL_CLIPS, MAX_MANUAL_TITLE_CHARS,
    MAX_REQUIREMENTS_CHARS, MIN_MANUAL_CLIP_SECONDS,
};

/// Clip length presets: (id, label, range).
pub const DURATION_OPTIONS: &[(&str, &str, &str)] = &[
    ("xshort", "Extra short", "10–30s"),
    ("short", "Short", "30–60s"),
    ("medium", "Medium", "1–2m"),
    ("long", "Long", "2–5m"),
    ("xlong", "Extra long", "5–10m"),
    ("extended", "Extended", "10–15m"),
    ("feature", "Feature", "15–30m"),
];

/// The Auto clip length: sent when no preset is picked, so the AI picks each clip's
/// length within 10 s – 3 min ("auto" in DURATION_RANGES, planner/config.rs).
pub const AUTO_DURATION_ID: &str = "auto";
pub const AUTO_DURATION_RANGE: &str = "10s–3m";

pub const VIDEO_SPEED_OPTIONS: [f64; 6] = [1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

/// Logo corners: (value, label).
pub const LOGO_POSITION_OPTIONS: &[(&str, &str)] = &[
    ("top-left", "Top left"),
    ("top-right", "Top right"),
    ("bottom-left", "Bottom left"),
    ("bottom-right", "Bottom right"),
];
/// Logo widths, as a fraction of the frame width: (value, label).
pub const LOGO_SIZES: &[(f64, &str)] = &[(0.12, "Small"), (0.18, "Medium"), (0.25, "Large")];

/// Most clips an AI run can be asked for.
pub const MAX_CLIPS: u32 = 100;

/// The Create wizard's steps, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WizardStep {
    #[default]
    Video,
    Format,
    Clips,
    Captions,
    Branding,
    Review,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WizardStepInfo {
    pub id: WizardStep,
    pub label: &'static str,
    pub title: &'static str,
    pub description: &'static str,
}

const fn step(
    id: WizardStep,
    label: &'static str,
    title: &'static str,
    description: &'static str,
) -> WizardStepInfo {
    WizardStepInfo {
        id,
        label,
        title,
        description,
    }
}

pub const WIZARD_STEPS: [WizardStepInfo; 6] = [
    step(
        WizardStep::Video,
        "Video",
        "Choose a video",
        "Clip the whole video, or only part of it.",
    ),
    step(
        WizardStep::Format,
        "Format",
        "Format, framing and speed",
        "Choose the look and pace of every clip in this job.",
    ),
    step(
        WizardStep::Clips,
        "Clips",
        "Moments to clip",
        "Tell the AI what you want, or type the moments yourself.",
    ),
    step(
        WizardStep::Captions,
        "Captions",
        "Captions",
        "Word-by-word captions burned into each clip. Silent videos are clipped without them.",
    ),
    step(
        WizardStep::Branding,
        "Branding",
        "Logos and channel link",
        "Stamp your logos and channel link on every clip. Both are optional.",
    ),
    step(
        WizardStep::Review,
        "Review",
        "Review and generate",
        "Check the run, then generate. You can queue another video right after.",
    ),
];

/// Header text for the Clips step in manual mode.
pub const MANUAL_CLIPS_TITLE: &str = "Clips to cut";
pub const MANUAL_CLIPS_DESCRIPTION: &str = "Type the moments you want. Clipward transcribes, frames, captions and renders each one; no AI provider is used.";

/// ai: the planner picks the moments; manual: the user types them (no AI provider).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClipMode {
    #[default]
    Ai,
    Manual,
}

/// The run the wizard just queued, shown as a confirmation until the next video.
#[derive(Debug, Clone, PartialEq)]
pub struct StartedJob {
    pub job_id: String,
    pub source: String,
    /// True when every slot was busy and the job is waiting its turn.
    pub queued: bool,
}

/// Length of a local source, once its preview has loaded (validates manual clips).
#[derive(Debug, Clone, PartialEq)]
pub struct SourceDuration {
    pub source: String,
    pub seconds: f64,
}

/// The Create form, kept outside the view so the chosen video and options survive
/// navigating away (e.g. to Settings to add a key) and a failed run.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipDraft {
    pub source: String,
    /// The prompt the AI picks moments by. Kept for the next video: clippers usually cut several videos for one campaign.
    pub requirements: String,
    /// "9:16" | "16:9"
    pub aspect_ratio: String,
    /// 9:16 framing: smart per-shot layouts ("auto"), always full frame ("fill"), or letterbox ("fit").
    pub layout_style: String,
    /// Paid vision verification for ambiguous shots in Smart framing.
    pub layout_vision: bool,
    /// tight: cut dead air and filler words; natural: original timing.
    pub pacing: String,
    pub video_speed: f64,
    pub durations: Vec<String>,
    pub auto_clip_count: bool,
    pub max_clips: u32,
    pub include_captions: bool,
    pub caption_preset: String,
    pub trim_open: bool,
    pub trim_start: String,
    pub trim_end: String,
    pub clip_mode: ClipMode,
    /// Manual mode's clip list, as typed.
    pub manual_clips: Vec<ManualClipRow>,
    /// Channel link drawn at the bottom of every clip; empty for none. Kept for the next video.
    pub banner_url: String,
    /// Logos stamped on every clip. Kept for the next video, like the prompt.
    pub logos: Vec<LogoOverlay>,
    pub step: WizardStep,
    pub started: Option<StartedJob>,
    pub source_duration: Option<SourceDuration>,
}

impl Default for ClipDraft {
    fn default() -> Self {
        Self {
            source: String::new(),
            requirements: String::new(),
            aspect_ratio: "9:16".into(),
            layout_style: "auto".into(),
            layout_vision: true,
            pacing: "tight".into(),
            video_speed: 1.0,
            durations: vec!["short".into()],
            auto_clip_count: false,
            max_clips: 5,
            include_captions: true,
            caption_preset: "pop".into(),
            trim_open: false,
            trim_start: String::new(),
            trim_end: String::new(),
            clip_mode: ClipMode::Ai,
            manual_clips: vec![new_manual_row()],
            banner_url: String::new(),
            logos: Vec::new(),
            step: WizardStep::Video,
            started: None,
            source_duration: None,
        }
    }
}

impl ClipDraft {
    pub fn clear_source(&mut self) {
        self.source.clear();
        self.trim_start.clear();
        self.trim_end.clear();
    }

    /// Back to the first step for the next video, keeping every other choice.
    /// The typed moments belong to the last video; the mode stays.
    pub fn start_another(&mut self) {
        self.clear_source();
        self.trim_open = false;
        self.manual_clips = vec![new_manual_row()];
        self.step = WizardStep::Video;
        self.started = None;
    }

    /// Length of the chosen video when its preview reported one.
    pub fn known_duration(&self) -> Option<f64> {
        self.source_duration
            .as_ref()
            .filter(|duration| duration.source == self.source)
            .map(|duration| duration.seconds)
    }

    /// The prompt sent to the AI: it lives under "Let AI decide", so it only
    /// counts when the AI picks the moments and how many.
    pub fn prompt(&self) -> &str {
        if self.clip_mode == ClipMode::Ai && self.auto_clip_count {
            js_trim(&self.requirements)
        } else {
            ""
        }
    }
}

/// A parsed trim range: None for an open end, NaN for unreadable text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrimRange {
    pub start: Option<f64>,
    pub end: Option<f64>,
    pub error: Option<&'static str>,
}

pub fn parse_trim_range(enabled: bool, start_text: &str, end_text: &str) -> TrimRange {
    let start = if enabled {
        parse_timecode(start_text)
    } else {
        None
    };
    let end = if enabled {
        parse_timecode(end_text)
    } else {
        None
    };
    let error = if start.is_some_and(f64::is_nan) || end.is_some_and(f64::is_nan) {
        Some("Use seconds (90) or a timecode (1:30).")
    } else if end.is_some_and(|end| end <= start.unwrap_or(0.0)) {
        Some("End must be after the start.")
    } else {
        None
    };
    TrimRange { start, end, error }
}

/// The run request for the current draft, as `validation::validate_job_config` reads it.
pub fn build_job_request(draft: &ClipDraft, trim: &TrimRange) -> Value {
    // The banner and logos are the same in AI and manual mode.
    let link = js_trim(&draft.banner_url);
    let mut request = json!({
        "videoUrl": normalize_video_source(&draft.source),
        "aspectRatio": draft.aspect_ratio,
        "layoutStyle": draft.layout_style,
        "pacing": draft.pacing,
        "videoSpeed": draft.video_speed,
        "includeCaptions": draft.include_captions,
        "captionPreset": draft.caption_preset,
        "bannerPlatform": (!link.is_empty()).then_some("link"),
        "bannerChannelUrl": (!link.is_empty()).then_some(link),
        "logos": draft.logos,
    });
    let mode = if draft.clip_mode == ClipMode::Manual {
        // The typed ranges are the plan: no prompt, trim, clip count or AI vision.
        json!({
            "requirements": "",
            "maxClips": null,
            "autoClipCount": true,
            "durationRanges": null,
            "layoutVision": false,
            "startTimeSeconds": null,
            "endTimeSeconds": null,
            "manualClips": to_manual_clips(&draft.manual_clips),
        })
    } else {
        let durations = if draft.durations.is_empty() {
            vec![AUTO_DURATION_ID.to_string()]
        } else {
            draft.durations.clone()
        };
        json!({
            "requirements": draft.prompt(),
            "maxClips": (!draft.auto_clip_count).then_some(draft.max_clips),
            "autoClipCount": draft.auto_clip_count,
            "durationRanges": durations,
            "layoutVision": draft.aspect_ratio == "9:16" && draft.layout_style == "auto" && draft.layout_vision,
            "startTimeSeconds": trim.start,
            "endTimeSeconds": trim.end,
        })
    };
    let fields = request.as_object_mut().unwrap();
    fields.extend(mode.as_object().unwrap().clone());
    request
}

/// Framing styles for 9:16: (id, label, hint).
pub const LAYOUT_STYLES: &[(&str, &str, &str)] = &[
    ("auto", "Smart", "Auto frames each shot"),
    (
        "fill",
        "Full frame",
        "Fills for one person; groups and media whole",
    ),
    ("fit", "Classic", "Whole frame, blurred"),
];

/// Output formats: (id, label, hint).
pub const FORMATS: &[(&str, &str, &str)] = &[
    ("9:16", "Vertical", "Shorts, Reels, TikTok"),
    ("16:9", "Horizontal", "YouTube, X, LinkedIn"),
];

/// Everything the wizard derives from the draft: the effective step, parsed trim,
/// manual clip checks and what may move on or submit (JobForm's derived state).
#[derive(Debug, Clone, PartialEq)]
pub struct FormCheck {
    pub step: WizardStep,
    pub steps: Vec<WizardStepInfo>,
    pub index: usize,
    pub trim: TrimRange,
    pub manual_check: ManualClipsCheck,
    pub source_error: Option<&'static str>,
    pub has_source: bool,
    pub brief_valid: bool,
    pub branding_valid: bool,
    pub manual_valid: bool,
    /// The current step lets Next go on.
    pub step_valid: bool,
    pub can_submit: bool,
}

impl FormCheck {
    pub fn next(&self) -> Option<WizardStepInfo> {
        self.steps.get(self.index + 1).copied()
    }
    pub fn back(&self) -> Option<WizardStepInfo> {
        self.index
            .checked_sub(1)
            .and_then(|i| self.steps.get(i).copied())
    }
}

/// `blocked` is whether setup blocks a start; `submitting` while a start is in flight.
pub fn check_form(draft: &ClipDraft, blocked: bool, submitting: bool) -> FormCheck {
    let manual = draft.clip_mode == ClipMode::Manual;
    let steps = WIZARD_STEPS.to_vec();
    let step = draft.step;
    let index = steps.iter().position(|s| s.id == step).unwrap_or(0);
    // Manual clips carry their own ranges, so the source trim only applies to AI runs.
    let trim = parse_trim_range(
        draft.trim_open && !manual,
        &draft.trim_start,
        &draft.trim_end,
    );
    let manual_check = check_manual_clips(&draft.manual_clips, draft.known_duration());
    let manual_valid = !manual || manual_check.valid;
    let source_error = twitch_source_error(&draft.source);
    let has_source = !js_trim(&draft.source).is_empty() && source_error.is_none();
    let brief_valid = draft.prompt().encode_utf16().count() <= MAX_REQUIREMENTS_CHARS;
    let branding_valid = js_trim(&draft.banner_url).chars().count() <= MAX_BANNER_CHARS;
    let step_valid = match step {
        WizardStep::Video => has_source && trim.error.is_none(),
        WizardStep::Branding => branding_valid,
        WizardStep::Clips => brief_valid && manual_valid,
        _ => true,
    };
    let can_submit = has_source
        && brief_valid
        && branding_valid
        && manual_valid
        && !blocked
        && trim.error.is_none()
        && !submitting
        && draft.started.is_none();
    FormCheck {
        step,
        steps,
        index,
        trim,
        manual_check,
        source_error,
        has_source,
        brief_valid,
        branding_valid,
        manual_valid,
        step_valid,
        can_submit,
    }
}

/// The footer's status line: why the run can't go yet, or the ⌘↵ hint.
pub fn status_line(check: &FormCheck, blocked_reason: Option<&str>, mod_key: &str) -> String {
    if let Some(reason) = blocked_reason {
        return reason.to_string();
    }
    if !check.brief_valid {
        return format!("Shorten the prompt to {MAX_REQUIREMENTS_CHARS} characters.");
    }
    if !check.branding_valid {
        return format!("Shorten the channel link to {MAX_BANNER_CHARS} characters.");
    }
    if check.step == WizardStep::Video && !check.has_source {
        return "Add a video to continue.".into();
    }
    if !check.manual_valid && check.step != WizardStep::Video {
        return if check.step == WizardStep::Clips {
            "Fill in or fix each clip’s times to continue.".into()
        } else {
            "Add valid clip times in the Clips step.".into()
        };
    }
    format!("{mod_key}↵ generates from any step")
}

/// "1.25×"
pub fn speed_label(speed: f64) -> String {
    format!("{speed}×")
}

/// Under each speed tile: "Normal" or how long a minute of source becomes.
pub fn speed_hint(speed: f64) -> String {
    if speed == 1.0 {
        "Normal".into()
    } else {
        format!("{}s per minute", js_round(60.0 / speed))
    }
}

/// The note under the clip lengths when the clips are sped up.
pub fn speed_length_note(speed: f64) -> Option<String> {
    (speed > 1.0).then(|| {
        format!(
            "Lengths refer to the original footage. At {speed}×, 60 seconds becomes about {} seconds before dead-air cuts.",
            js_round(60.0 / speed)
        )
    })
}

/// The clip count field as typed: digits only, clamped to 1..=MAX_CLIPS.
pub fn parse_max_clips(text: &str) -> u32 {
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    match digits.parse::<u64>() {
        Ok(n) => n.clamp(1, MAX_CLIPS as u64) as u32,
        Err(_) if digits.is_empty() => 1,
        // More digits than fit: the most there is.
        Err(_) => MAX_CLIPS,
    }
}

/// Add picked logos, skipping duplicates, up to MAX_LOGOS; each takes the first
/// free corner, starting top right, at the medium size.
pub fn add_logos(logos: &[LogoOverlay], picked: &[String]) -> Vec<LogoOverlay> {
    const CORNERS: [&str; 4] = ["top-right", "top-left", "bottom-right", "bottom-left"];
    let mut logos = logos.to_vec();
    for path in picked {
        if logos.len() >= MAX_LOGOS {
            break;
        }
        if logos.iter().any(|logo| &logo.path == path) {
            continue;
        }
        let position = CORNERS
            .into_iter()
            .find(|corner| !logos.iter().any(|logo| logo.position == *corner))
            .unwrap_or("top-right");
        logos.push(LogoOverlay {
            path: path.clone(),
            position: position.into(),
            size: LOGO_SIZES[1].0,
        });
    }
    logos
}

/// One line for Review: the channel link and logo count.
pub fn branding_summary(draft: &ClipDraft) -> String {
    let link = js_trim(&draft.banner_url);
    let count = draft.logos.len();
    let logos = match count {
        0 => String::new(),
        1 => "1 logo".into(),
        n => format!("{n} logos"),
    };
    let parts: Vec<&str> = [link, logos.as_str()]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        "None".into()
    } else {
        parts.join(" · ")
    }
}

/// m:ss for the trim range in Review.
fn format_seconds(total: f64) -> String {
    let m = (total / 60.0).floor();
    let s = (total % 60.0).floor();
    format!("{m}:{s:02}")
}

/// One Review row: the step its Edit link opens, a label and the value.
#[derive(Debug, Clone, PartialEq)]
pub struct ReviewRow {
    pub step: WizardStep,
    pub label: &'static str,
    pub value: String,
}

/// The settings that apply to Review's summary but live outside the draft.
pub struct ReviewContext<'a> {
    /// Display name of the chosen caption preset.
    pub caption_name: &'a str,
    /// "Claude Code CLI", "OpenRouter"… and the model, if one is set.
    pub provider_label: &'a str,
    pub model: &'a str,
}

pub fn review_rows(
    draft: &ClipDraft,
    check: &FormCheck,
    context: &ReviewContext,
) -> Vec<ReviewRow> {
    let manual = draft.clip_mode == ClipMode::Manual;
    let row = |step, label, value: String| ReviewRow { step, label, value };
    let lengths = if draft.durations.is_empty() {
        format!("Auto ({AUTO_DURATION_RANGE})")
    } else {
        DURATION_OPTIONS
            .iter()
            .filter(|(id, _, _)| draft.durations.iter().any(|d| d == id))
            .map(|(_, _, range)| *range)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let framing = if draft.aspect_ratio == "9:16" {
        let label = LAYOUT_STYLES
            .iter()
            .find(|(id, _, _)| *id == draft.layout_style)
            .map_or("Smart", |(_, label, _)| *label);
        let vision = draft.layout_style == "auto" && draft.layout_vision && !manual;
        format!(
            "{label} framing{}",
            if vision { " · AI vision" } else { "" }
        )
    } else {
        "Whole frame".into()
    };
    let trim = &check.trim;
    let trim_label = if !manual && draft.trim_open && (trim.start.is_some() || trim.end.is_some()) {
        format!(
            " · {} to {}",
            trim.start.map_or("start".into(), format_seconds),
            trim.end.map_or("end".into(), format_seconds)
        )
    } else {
        String::new()
    };
    let format_label = FORMATS
        .iter()
        .find(|(id, _, _)| *id == draft.aspect_ratio)
        .map_or(draft.aspect_ratio.as_str(), |(_, label, _)| *label);
    let format = format!("{format_label} {} · {framing}", draft.aspect_ratio);
    let pacing = if draft.pacing == "tight" {
        "Cut dead air"
    } else {
        "Keep pauses"
    };
    let speed = format!(
        "{}{}",
        speed_label(draft.video_speed),
        if draft.video_speed == 1.0 {
            " · Normal"
        } else {
            " · All exported clips"
        }
    );
    let captions = if draft.include_captions {
        context.caption_name.to_string()
    } else {
        "Off".into()
    };
    let source = source_label(&draft.source);
    if manual {
        let total: f64 = check
            .manual_check
            .rows
            .iter()
            .map(|r| {
                if r.error.is_some() {
                    0.0
                } else {
                    r.length.unwrap_or(0.0)
                }
            })
            .sum();
        let count = draft.manual_clips.len();
        return vec![
            row(WizardStep::Video, "Video", source),
            row(WizardStep::Format, "Format", format),
            row(WizardStep::Format, "Pacing", pacing.into()),
            row(WizardStep::Format, "Speed", speed),
            row(
                WizardStep::Clips,
                "Moments",
                "You choose · No AI provider · Whisper offline".into(),
            ),
            row(
                WizardStep::Clips,
                "Clips",
                format!(
                    "{count} clip{}{}",
                    if count == 1 { "" } else { "s" },
                    if total > 0.0 {
                        format!(" · {} of source", format_clip_length(total))
                    } else {
                        String::new()
                    }
                ),
            ),
            row(WizardStep::Captions, "Captions", captions),
            row(WizardStep::Branding, "Branding", branding_summary(draft)),
        ];
    }
    let brief = draft.prompt();
    let brief = if brief.is_empty() {
        "None · most viral moments".into()
    } else {
        brief.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    let model = if context.model.is_empty() {
        String::new()
    } else {
        format!(" · {}", context.model)
    };
    let sped_source = draft.video_speed > 1.0 && !draft.durations.is_empty();
    let count = if draft.auto_clip_count {
        "AI decides how many".to_string()
    } else {
        format!("Up to {}", draft.max_clips)
    };
    vec![
        row(WizardStep::Video, "Video", format!("{source}{trim_label}")),
        row(WizardStep::Format, "Format", format),
        row(WizardStep::Format, "Pacing", pacing.into()),
        row(WizardStep::Format, "Speed", speed),
        row(WizardStep::Clips, "Prompt", brief),
        row(
            WizardStep::Clips,
            "AI",
            format!("{}{model} · Whisper offline", context.provider_label),
        ),
        row(
            WizardStep::Clips,
            "Clips",
            format!(
                "{lengths}{} · {count}",
                if sped_source {
                    " of source footage"
                } else {
                    ""
                }
            ),
        ),
        row(WizardStep::Captions, "Captions", captions),
        row(WizardStep::Branding, "Branding", branding_summary(draft)),
    ]
}

/// Review's note on where and when the run happens. `running` counts jobs past
/// the queue; `active` counts queued and running ones.
pub fn queue_note(running: usize, active: usize, manual: bool, provider: &str) -> String {
    if running >= MAX_PARALLEL_JOBS {
        format!("{running} jobs are running. This one waits in the queue and starts automatically.")
    } else if active > 0 {
        format!(
            "Runs alongside {active} other job{}. Up to {MAX_PARALLEL_JOBS} run at once.",
            if active == 1 { "" } else { "s" }
        )
    } else if manual {
        "Runs on this computer. No AI provider is used; transcription is offline.".into()
    } else if provider.starts_with("cli:") {
        "Runs on this computer. Clip planning uses your CLI plan; transcription is offline.".into()
    } else {
        "Runs on this computer. Clip planning bills your API key; transcription is offline.".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validation::validate_job_config;

    const NO_TRIM: TrimRange = TrimRange {
        start: None,
        end: None,
        error: None,
    };

    fn trim(start: f64, end: f64) -> TrimRange {
        TrimRange {
            start: Some(start),
            end: Some(end),
            error: None,
        }
    }

    fn row(start: &str, end: &str, title: &str) -> ManualClipRow {
        ManualClipRow {
            start: start.into(),
            end: end.into(),
            title: title.into(),
            ..new_manual_row()
        }
    }

    /// An absolute-path prefix on this OS.
    const ROOT: &str = if cfg!(windows) { "C:" } else { "" };

    fn logos() -> Vec<LogoOverlay> {
        vec![
            LogoOverlay {
                path: format!("{ROOT}/Users/me/logo.png"),
                position: "top-right".into(),
                size: 0.18,
            },
            LogoOverlay {
                path: format!("{ROOT}/Users/me/sponsor.webp"),
                position: "bottom-left".into(),
                size: 0.12,
            },
        ]
    }

    #[test]
    fn trim_validation_matches_main_process_bounds_including_an_end_at_zero() {
        assert_eq!(
            parse_trim_range(true, "", "0").error,
            Some("End must be after the start.")
        );
        assert_eq!(
            parse_trim_range(true, "1:30", "90").error,
            Some("End must be after the start.")
        );
        assert_eq!(
            parse_trim_range(true, "", "1:30"),
            TrimRange {
                start: None,
                end: Some(90.0),
                error: None
            }
        );
        assert_eq!(parse_trim_range(false, "oops", "0"), NO_TRIM);
        assert_eq!(
            parse_trim_range(true, "oops", "").error,
            Some("Use seconds (90) or a timecode (1:30).")
        );
    }

    #[test]
    fn the_wizard_opens_on_the_video_step_with_the_steps_listed_in_order() {
        let draft = ClipDraft::default();
        assert_eq!(draft.step, WizardStep::Video);
        let labels: Vec<&str> = WIZARD_STEPS.iter().map(|s| s.label).collect();
        assert_eq!(
            labels,
            ["Video", "Format", "Clips", "Captions", "Branding", "Review"]
        );
        assert_eq!(WIZARD_STEPS[0].title, "Choose a video");
    }

    #[test]
    fn the_prompt_travels_with_the_job_trimmed_and_stays_for_the_next_video() {
        let mut draft = ClipDraft {
            source: "https://example.com/video".into(),
            requirements: "  Mention Acme.\nNo swearing.\n".into(),
            auto_clip_count: true,
            ..ClipDraft::default()
        };
        let request = build_job_request(&draft, &NO_TRIM);
        assert_eq!(request["requirements"], "Mention Acme.\nNo swearing.");
        assert!(request.get("clippingMode").is_none());
        // A fixed clip count hides the prompt, so none is sent.
        draft.auto_clip_count = false;
        assert_eq!(build_job_request(&draft, &NO_TRIM)["requirements"], "");
        draft.auto_clip_count = true;
        draft.start_another();
        assert_eq!(draft.source, "");
        assert_eq!(draft.requirements, "  Mention Acme.\nNo swearing.\n");
    }

    #[test]
    fn the_channel_link_and_logos_travel_with_ai_and_manual_jobs_and_stay_for_the_next_video() {
        let plain = build_job_request(&ClipDraft::default(), &NO_TRIM);
        assert_eq!(
            (
                &plain["bannerPlatform"],
                &plain["bannerChannelUrl"],
                &plain["logos"]
            ),
            (&Value::Null, &Value::Null, &json!([]))
        );
        let mut draft = ClipDraft {
            source: "https://example.com/video".into(),
            banner_url: "  youtube.com/@me ".into(),
            logos: logos(),
            ..ClipDraft::default()
        };
        let ai = build_job_request(&draft, &NO_TRIM);
        assert_eq!(ai["bannerPlatform"], "link");
        assert_eq!(ai["bannerChannelUrl"], "youtube.com/@me");
        assert_eq!(ai["logos"], json!(logos()));
        draft.clip_mode = ClipMode::Manual;
        draft.manual_clips = vec![row("5", "20", "")];
        let manual = build_job_request(&draft, &NO_TRIM);
        assert_eq!(manual["bannerChannelUrl"], "youtube.com/@me");
        assert_eq!(manual["logos"], json!(logos()));
        draft.start_another();
        assert_eq!(draft.banner_url, "  youtube.com/@me ");
        assert_eq!(draft.logos.len(), 2);
    }

    #[test]
    fn speed_survives_navigation_and_another_job_and_appears_in_the_submitted_request() {
        let mut draft = ClipDraft::default();
        assert_eq!(draft.video_speed, 1.0);
        draft.source = "https://example.com/video".into();
        draft.video_speed = 1.5;
        draft.step = WizardStep::Review;
        assert_eq!(
            build_job_request(&draft, &trim(10.0, 70.0))["videoSpeed"],
            1.5
        );
        draft.start_another();
        assert_eq!(draft.video_speed, 1.5);
        assert_eq!(draft.step, WizardStep::Video);
    }

    #[test]
    fn manual_mode_sends_the_typed_clips_and_drops_the_prompt_trim_counts_and_ai_vision() {
        let mut draft = ClipDraft {
            source: "https://example.com/video".into(),
            requirements: "Mention Acme".into(),
            trim_open: true,
            trim_start: "10".into(),
            trim_end: "20".into(),
            auto_clip_count: false,
            max_clips: 3,
            layout_vision: true,
            clip_mode: ClipMode::Manual,
            manual_clips: vec![row("1:00", "1:30", "  Hook "), row("5", "20", "")],
            ..ClipDraft::default()
        };
        let request = build_job_request(&draft, &trim(10.0, 20.0));
        assert_eq!(
            request["manualClips"],
            json!([
                { "startSeconds": 60.0, "endSeconds": 90.0, "title": "Hook" },
                { "startSeconds": 5.0, "endSeconds": 20.0, "title": null }
            ])
        );
        assert_eq!(request["requirements"], "");
        assert_eq!(request["startTimeSeconds"], Value::Null);
        assert_eq!(request["endTimeSeconds"], Value::Null);
        assert_eq!(request["maxClips"], Value::Null);
        assert_eq!(request["layoutVision"], false);
        // AI mode keeps the trim and sends no manual clips.
        draft.clip_mode = ClipMode::Ai;
        let ai = build_job_request(&draft, &trim(10.0, 20.0));
        assert!(ai.get("manualClips").is_none());
        assert_eq!(ai["startTimeSeconds"], 10.0);
        assert_eq!(ai["maxClips"], 3);
        // The mode survives the next video; the typed moments belong to the last one.
        draft.clip_mode = ClipMode::Manual;
        draft.start_another();
        assert_eq!(draft.clip_mode, ClipMode::Manual);
        assert_eq!(draft.manual_clips.len(), 1);
        assert_eq!(draft.manual_clips[0].start, "");
    }

    #[test]
    fn ai_requests_pick_auto_length_and_vision_only_for_smart_vertical() {
        let mut draft = ClipDraft {
            source: "https://example.com/video".into(),
            durations: Vec::new(),
            ..ClipDraft::default()
        };
        let request = build_job_request(&draft, &NO_TRIM);
        assert_eq!(request["durationRanges"], json!(["auto"]));
        assert_eq!(request["layoutVision"], true);
        draft.aspect_ratio = "16:9".into();
        assert_eq!(build_job_request(&draft, &NO_TRIM)["layoutVision"], false);
    }

    #[test]
    fn known_duration_follows_the_chosen_source() {
        let mut draft = ClipDraft {
            source: "/a.mp4".into(),
            source_duration: Some(SourceDuration {
                source: "/a.mp4".into(),
                seconds: 90.0,
            }),
            ..ClipDraft::default()
        };
        assert_eq!(draft.known_duration(), Some(90.0));
        draft.source = "/b.mp4".into();
        assert_eq!(draft.known_duration(), None);
    }

    #[test]
    fn the_shell_validator_accepts_built_requests_in_both_modes() {
        let mut draft = ClipDraft {
            source: " https://m.twitch.tv/videos/123/?t=1 ".into(),
            requirements: "  Be fun ".into(),
            banner_url: " youtube.com/@me ".into(),
            logos: logos(),
            auto_clip_count: false,
            max_clips: 7,
            ..ClipDraft::default()
        };
        let ai = validate_job_config(build_job_request(&draft, &trim(10.0, 70.0))).unwrap();
        assert_eq!(ai.video_url, "https://www.twitch.tv/videos/123");
        assert_eq!(ai.requirements.as_deref(), Some(""));
        assert_eq!(ai.max_clips, Some(7));
        assert_eq!(ai.duration_ranges, Some(vec!["short".to_string()]));
        assert_eq!(
            (ai.start_time_seconds, ai.end_time_seconds),
            (Some(10.0), Some(70.0))
        );
        assert!(ai.layout_vision);
        assert_eq!(ai.banner_channel_url.as_deref(), Some("youtube.com/@me"));
        assert_eq!(ai.logos, logos());
        assert_eq!(ai.manual_clips, None);

        draft.clip_mode = ClipMode::Manual;
        draft.manual_clips = vec![row("1:00", "1:30", "  Hook "), row("5", "20", "")];
        let manual = validate_job_config(build_job_request(&draft, &NO_TRIM)).unwrap();
        let clips = manual.manual_clips.unwrap();
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].title.as_deref(), Some("Hook"));
        assert!(!manual.layout_vision);
        assert_eq!(manual.max_clips, None);
        assert_eq!(manual.requirements.as_deref(), Some(""));
    }
    #[test]
    fn the_form_check_gates_next_and_generate_like_the_wizard() {
        let mut draft = ClipDraft::default();
        let check = check_form(&draft, false, false);
        assert!(!check.step_valid && !check.can_submit);
        assert_eq!(status_line(&check, None, "⌘"), "Add a video to continue.");
        assert_eq!(status_line(&check, Some("Blocked."), "⌘"), "Blocked.");
        draft.source = "https://example.com/v".into();
        let check = check_form(&draft, false, false);
        assert!(check.step_valid && check.can_submit);
        assert_eq!(status_line(&check, None, "⌘"), "⌘↵ generates from any step");
        assert_eq!(check.next().map(|s| s.label), Some("Format"));
        assert!(check.back().is_none());
        assert!(!check_form(&draft, true, false).can_submit);
        assert!(!check_form(&draft, false, true).can_submit);
        // Manual mode: an empty row blocks.
        draft.step = WizardStep::Format;
        draft.clip_mode = ClipMode::Manual;
        let check = check_form(&draft, false, false);
        assert_eq!(check.back().map(|s| s.id), Some(WizardStep::Video));
        assert!(!check.can_submit);
        assert_eq!(
            status_line(&check, None, "⌘"),
            "Add valid clip times in the Clips step."
        );
        draft.clip_mode = ClipMode::Ai;
        draft.step = WizardStep::Clips;
        draft.auto_clip_count = true;
        draft.requirements = "x".repeat(MAX_REQUIREMENTS_CHARS + 1);
        let check = check_form(&draft, false, false);
        assert!(!check.step_valid && !check.can_submit);
        assert_eq!(
            status_line(&check, None, "⌘"),
            "Shorten the prompt to 4000 characters."
        );
        draft.requirements.clear();
        draft.started = Some(StartedJob {
            job_id: "j".into(),
            source: "s".into(),
            queued: false,
        });
        assert!(!check_form(&draft, false, false).can_submit);
    }

    #[test]
    fn speed_labels_clip_counts_and_logo_corners() {
        assert_eq!(
            (speed_label(1.0), speed_label(1.25)),
            ("1×".to_string(), "1.25×".to_string())
        );
        assert_eq!(speed_hint(1.0), "Normal");
        assert_eq!(speed_hint(1.1), "55s per minute");
        assert_eq!(speed_hint(2.0), "30s per minute");
        assert_eq!(speed_length_note(1.0), None);
        assert_eq!(
            (
                parse_max_clips(""),
                parse_max_clips("a7b"),
                parse_max_clips("0")
            ),
            (1, 7, 1)
        );
        assert_eq!(
            (
                parse_max_clips("250"),
                parse_max_clips("99999999999999999999999")
            ),
            (100, 100)
        );
        let picked: Vec<String> = ["/a.png", "/b.png", "/a.png", "/c.png", "/d.png", "/e.png"]
            .iter()
            .map(|p| p.to_string())
            .collect();
        let logos = add_logos(&[], &picked);
        let corners: Vec<&str> = logos.iter().map(|l| l.position.as_str()).collect();
        assert_eq!(
            corners,
            ["top-right", "top-left", "bottom-right", "bottom-left"]
        );
        assert_eq!(logos[0].size, 0.18);
        let mut kept = logos[1..2].to_vec();
        kept = add_logos(&kept, &["/z.png".to_string()]);
        assert_eq!(kept[1].position, "top-right");
    }

    #[test]
    fn review_summarizes_ai_and_manual_runs() {
        let mut draft = ClipDraft {
            source: "https://www.youtube.com/watch?v=abc".into(),
            requirements: "  Mention\n  Acme ".into(),
            trim_open: true,
            trim_start: "1:05".into(),
            video_speed: 1.5,
            banner_url: " yt.com/@me ".into(),
            logos: logos(),
            ..ClipDraft::default()
        };
        let context = ReviewContext {
            caption_name: "Pop",
            provider_label: "Claude Code CLI",
            model: "",
        };
        let rows = review_rows(&draft, &check_form(&draft, false, false), &context);
        let values: Vec<&str> = rows.iter().map(|r| r.value.as_str()).collect();
        assert_eq!(
            values,
            [
                "youtube.com/watch?v=abc · 1:05 to end",
                "Vertical 9:16 · Smart framing · AI vision",
                "Cut dead air",
                "1.5× · All exported clips",
                "None · most viral moments",
                "Claude Code CLI · Whisper offline",
                "30–60s of source footage · Up to 5",
                "Pop",
                "yt.com/@me · 2 logos",
            ]
        );
        draft.clip_mode = ClipMode::Manual;
        draft.include_captions = false;
        draft.manual_clips = vec![row("0", "30", ""), row("1:00", "1:45", "")];
        let rows = review_rows(&draft, &check_form(&draft, false, false), &context);
        assert_eq!(rows[0].value, "youtube.com/watch?v=abc");
        assert_eq!(rows[1].value, "Vertical 9:16 · Smart framing");
        assert_eq!(rows[5].value, "2 clips · 1m 15s of source");
        assert_eq!(rows[6].value, "Off");
        assert_eq!(
            queue_note(2, 2, false, "openrouter"),
            "2 jobs are running. This one waits in the queue and starts automatically."
        );
        assert_eq!(
            queue_note(1, 1, false, "openrouter"),
            "Runs alongside 1 other job. Up to 2 run at once."
        );
        assert!(queue_note(0, 0, false, "cli:claude").contains("CLI plan"));
    }
}
