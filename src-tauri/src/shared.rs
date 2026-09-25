//! Constants the renderer shares with the app shell (mirrors src/shared/*.ts).

/// src/shared/whisper-models.ts
pub const WHISPER_MODEL_IDS: &[&str] = &[
    "base",
    "small",
    "large-v3-turbo-q5_0",
    "medium",
    "large-v3-turbo",
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
    "xshort", "short", "medium", "long", "xlong", "extended", "feature",
];
pub const MAX_REQUIREMENTS_CHARS: usize = 4000;

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
