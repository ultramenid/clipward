//! Planner settings from `config.py` and the desktop bridge's per-provider overrides.
use crate::llm;
use crate::types::AiSettings;
use std::time::Duration;

/// User-facing clip length presets: id -> (min_s, max_s, prompt description).
/// The first seven mirror DURATION_OPTIONS in src/shared/job-contract.ts.
pub const DURATION_RANGES: &[(&str, i64, i64, &str)] = &[
    ("xshort", 10, 30, "10-30 seconds (quick, punchy clips)"),
    ("short", 30, 60, "30-60 seconds (short clips)"),
    ("medium", 60, 120, "1-2 minutes (moderate length clips)"),
    ("long", 120, 300, "2-5 minutes (longer, in-depth clips)"),
    ("xlong", 300, 600, "5-10 minutes (extra long clips)"),
    ("extended", 600, 900, "10-15 minutes (extended clips)"),
    ("feature", 900, 1800, "15-30 minutes (feature-length edits)"),
    // Legacy keys (still accepted)
    (
        "landscape_short",
        60,
        120,
        "1-2 minutes (short landscape clips)",
    ),
    (
        "landscape_medium",
        120,
        300,
        "2-5 minutes (medium landscape clips)",
    ),
    (
        "landscape_long",
        300,
        600,
        "5-10 minutes (long landscape clips)",
    ),
    (
        "landscape_extended",
        600,
        1200,
        "10-20 minutes (extended landscape clips)",
    ),
];
pub const DEFAULT_CLIP_DURATION_SECONDS: (i64, i64) = (15, 90);

/// 16:9 clips at least this long are planned, paced and rendered as longform episodes.
pub const LONGFORM_MIN_SECONDS: i64 = 300;

/// Whether a job's clips are longform edits (16:9 and 5+ minutes each).
pub fn is_longform(aspect_ratio: &str, min_clip_seconds: i64) -> bool {
    aspect_ratio == "16:9" && min_clip_seconds >= LONGFORM_MIN_SECONDS
}

/// (min_s, max_s) that the prompt, parser and end-snap cap all agree on.
///
/// Selected presets win and span their union (short + long -> 30-300 s).
/// Otherwise explicit bounds apply, then the 15-90 s default. A minimum on its
/// own gets a max of at least twice itself.
pub fn resolve_clip_duration_bounds(
    duration_ranges: Option<&[String]>,
    min_seconds: Option<i64>,
    max_seconds: Option<i64>,
) -> (i64, i64) {
    let known: Vec<(i64, i64)> = duration_ranges
        .unwrap_or(&[])
        .iter()
        .filter_map(|r| {
            DURATION_RANGES
                .iter()
                .find(|(id, ..)| id == r)
                .map(|(_, lo, hi, _)| (*lo, *hi))
        })
        .collect();
    if !known.is_empty() {
        return (
            known.iter().map(|k| k.0).min().unwrap(),
            known.iter().map(|k| k.1).max().unwrap(),
        );
    }
    let low = min_seconds.unwrap_or(DEFAULT_CLIP_DURATION_SECONDS.0);
    let high = max_seconds.unwrap_or_else(|| DEFAULT_CLIP_DURATION_SECONDS.1.max(2 * low));
    (low, low.max(high))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClippingMode {
    Quality,
    Economy,
    /// Any model: provider reasoning defaults, no fallbacks, catalog prices.
    Advanced,
}

// Clip count scaling (hardcoded in config.py).
pub const MAX_SUGGESTED_CLIPS: i64 = 50;
pub const CLIP_SCALING_ENABLED: bool = true;
pub const MIN_CLIPS: i64 = 3;
pub const CLIP_COUNT_TAU_MINUTES: f64 = 45.0;
pub const TRANSCRIPT_DENSITY_TARGET_WPM: f64 = 160.0;
pub const TRANSCRIPT_DENSITY_MIN_FACTOR: f64 = 0.8;
pub const TRANSCRIPT_DENSITY_MAX_FACTOR: f64 = 1.25;
pub const MAX_CLIPS_ABSOLUTE: i64 = 100;

/// Planner settings (the relevant part of config.Settings).
#[derive(Debug, Clone)]
pub struct PlannerSettings {
    /// "openrouter" | "anthropic" | "openai" | "cli:<id>" (recorded in costs).
    pub ai_provider: String,
    /// OpenRouter slug; direct providers use `ai.model` or their default.
    pub planner_model: String,
    /// Comma-separated OpenRouter failover chain.
    pub planner_fallback_models: String,
    /// none | minimal | low | medium | high | xhigh
    pub planner_reasoning_effort: String,
    /// Includes reasoning tokens; 100 clips of JSON is ~15k on its own.
    pub planner_max_output_tokens: u32,
    pub clipping_mode: ClippingMode,
    pub planner_supports_images: bool,
    pub planner_input_price: Option<f64>,
    pub planner_output_price: Option<f64>,
    /// Campaign brief from the Create form: rules every clip must satisfy.
    pub clip_requirements: String,
    pub sentence_snapping_enabled: bool,
    pub sentence_extension_max_seconds: f64,
    pub start_boundary_max_adjustment_seconds: f64,
    /// Provider base URL override (tests).
    pub base_url: Option<String>,
    /// First retry delay; doubles per attempt.
    pub retry_base_delay: Duration,
}

impl Default for PlannerSettings {
    fn default() -> Self {
        Self {
            ai_provider: "openrouter".into(),
            planner_model: llm::DEFAULT_OPENROUTER_MODEL.into(),
            planner_fallback_models: "google/gemini-3.8-flash,openai/gpt-6-sol".into(),
            planner_reasoning_effort: "medium".into(),
            planner_max_output_tokens: 32000,
            clipping_mode: ClippingMode::Quality,
            planner_supports_images: true,
            planner_input_price: None,
            planner_output_price: None,
            clip_requirements: String::new(),
            sentence_snapping_enabled: true,
            sentence_extension_max_seconds: 5.0,
            start_boundary_max_adjustment_seconds: 3.0,
            base_url: None,
            retry_base_delay: Duration::from_secs(1),
        }
    }
}

impl PlannerSettings {
    /// Settings for one desktop job, as bridge_runner.run() configured them.
    pub fn for_job(ai: &AiSettings, requirements: Option<&str>) -> Self {
        let mut settings = Self {
            ai_provider: ai.provider.clone(),
            ..Self::default()
        };
        settings.clip_requirements = requirements.unwrap_or("").to_string();
        if ai.provider.starts_with("cli:") {
            // Coding CLIs take text only: no frame images for planning.
            settings.planner_supports_images = false;
        } else if ai.provider != "openrouter" {
            // OpenRouter model slugs mean nothing to a direct API.
            settings.planner_fallback_models = String::new();
        }
        if ai.provider == "openrouter" && !ai.model.is_empty() {
            // The model picked in Settings (the Python engine recorded it as the
            // planner model but still sent PLANNER_MODEL).
            settings.planner_model = ai.model.clone();
        }
        settings
    }

    /// Fallback planner models, excluding blanks and the primary.
    pub fn planner_fallback_models(&self) -> Vec<String> {
        if self.clipping_mode == ClippingMode::Advanced {
            return vec![];
        }
        self.planner_fallback_models
            .split(',')
            .map(str::trim)
            .filter(|m| !m.is_empty() && *m != self.planner_model)
            .map(str::to_string)
            .collect()
    }
}

/// Python's round() (half to even) to an integer.
fn round_half_even(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// Clip count scaled by duration (saturating curve) and transcript density.
pub fn calculate_optimal_clip_count(
    video_duration_seconds: f64,
    user_max_clips: Option<i64>,
    auto_clip_count: bool,
    words_per_minute: Option<f64>,
) -> i64 {
    let effective_max = user_max_clips.unwrap_or(MAX_CLIPS_ABSOLUTE);
    if !auto_clip_count || !CLIP_SCALING_ENABLED {
        return effective_max.min(MAX_CLIPS_ABSOLUTE);
    }
    let minutes = (video_duration_seconds / 60.0).max(0.0);
    let max_auto = MAX_SUGGESTED_CLIPS.min(MAX_CLIPS_ABSOLUTE);
    let base = if CLIP_COUNT_TAU_MINUTES <= 0.0 {
        MIN_CLIPS as f64
    } else {
        let growth = 1.0 - (-minutes / CLIP_COUNT_TAU_MINUTES).exp();
        MIN_CLIPS as f64 + (max_auto - MIN_CLIPS) as f64 * growth
    };
    let base_clips = MIN_CLIPS.max(round_half_even(base));
    let mut density = 1.0;
    if let Some(wpm) = words_per_minute.filter(|w| *w > 0.0) {
        let raw = wpm / TRANSCRIPT_DENSITY_TARGET_WPM.max(1.0);
        density = TRANSCRIPT_DENSITY_MIN_FACTOR.max(raw.min(TRANSCRIPT_DENSITY_MAX_FACTOR));
    }
    let adjusted = round_half_even(base_clips as f64 * density).min(max_auto);
    let final_clips = adjusted.min(effective_max).min(MAX_CLIPS_ABSOLUTE);
    tracing::info!(
        "Clip count scaling: {minutes:.1} min video -> base {base_clips} clips, density_factor: {density:.2}, effective max: {effective_max}, final: {}",
        final_clips.max(MIN_CLIPS)
    );
    final_clips.max(MIN_CLIPS)
}
