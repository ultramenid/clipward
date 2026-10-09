//! Product constants the UI and the core share (ported from the old src/shared/*.ts).

/// src/shared/whisper-models.ts
pub const WHISPER_MODEL_IDS: &[&str] = &[
    "base",
    "small",
    "large-v3-turbo-q5_0",
    "medium",
    "large-v3-turbo",
    "large-v3",
];
pub const DEFAULT_WHISPER_MODEL: &str = "large-v3-turbo-q5_0";

pub fn is_whisper_model_id(value: &str) -> bool {
    WHISPER_MODEL_IDS.contains(&value)
}

/// src/shared/ai-providers.ts AI_CLIS: (id, label, binaries).
pub const AI_CLIS: &[(&str, &str, &[&str])] = &[
    ("claude", "Claude Code", &["claude"]),
    ("codex", "Codex", &["codex"]),
    ("agy", "Antigravity", &["agy"]),
    ("grok", "Grok", &["grok"]),
    ("cursor-agent", "Cursor", &["cursor-agent", "agent"]),
    ("opencode", "OpenCode", &["opencode"]),
    ("gemini", "Gemini CLI", &["gemini"]),
];

/// src/shared/job-contract.ts
pub const DURATION_IDS: &[&str] = &[
    "xshort", "short", "medium", "long", "xlong", "extended", "feature", "auto",
];
pub const MAX_REQUIREMENTS_CHARS: usize = 4000;
/// Logos stamped per job, and the longest channel link shown in the banner.
pub const MAX_LOGOS: usize = 4;
pub const MAX_BANNER_CHARS: usize = 100;
pub const LOGO_POSITIONS: &[&str] = &["top-left", "top-right", "bottom-left", "bottom-right"];
pub const LOGO_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];
/// Source videos the video picker offers.
pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "m4v", "mkv", "webm", "avi", "mov", "flv"];

/// src/shared/jobs.ts
pub const MAX_PARALLEL_JOBS: usize = 2;
pub const MAX_FINISHED_JOBS: usize = 50;
pub const ACTIVE_JOB_STATUSES: &[&str] = &[
    "queued",
    "pending",
    "downloading",
    "transcribing",
    "planning",
    "rendering",
    "uploading",
];

pub fn is_active_job_status(status: &str) -> bool {
    ACTIVE_JOB_STATUSES.contains(&status)
}

/// src/shared/brand.ts — the only external links the app opens.
pub const REPO_URL: &str = "https://github.com/clipward/clipward";
pub const ISSUES_URL: &str = "https://github.com/clipward/clipward/issues";
pub const TRUSTED_EXTERNAL_URLS: &[&str] = &[
    "https://www.clipward.ai",
    "https://www.clipward.ai/discord",
    ISSUES_URL,
    REPO_URL,
    "https://openrouter.ai/keys",
    "https://console.anthropic.com/settings/keys",
    "https://platform.openai.com/api-keys",
];

/// Manual clip mode limits, from the engine's `manual` module.
pub const MAX_MANUAL_CLIPS: usize = clip_engine::manual::MAX_CLIPS;
pub const MIN_MANUAL_CLIP_SECONDS: f64 = clip_engine::manual::MIN_CLIP_SECONDS;
pub const MAX_MANUAL_CLIP_SECONDS: f64 = clip_engine::manual::MAX_CLIP_SECONDS;
pub const MAX_MANUAL_TITLE_CHARS: usize = clip_engine::manual::MAX_TITLE_CHARS;

#[cfg(test)]
mod tests {
    #[test]
    fn duration_ids_match_the_planner() {
        let planner: Vec<&str> = clip_engine::planner::config::DURATION_RANGES.iter().map(|r| r.0).collect();
        // The planner also has landscape-only ranges after these.
        assert_eq!(super::DURATION_IDS, &planner[..super::DURATION_IDS.len()]);
    }
}
