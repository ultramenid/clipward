//! What Settings shows about AI providers, Whisper models and source retention
//! (src/shared/ai-providers.ts, whisper-models.ts, brand.ts and the pure bits of
//! SettingsPage.tsx), for the GPUI shell.
use crate::shared::AI_CLIS;

pub const APP_NAME: &str = "Clipward";
pub const LICENSE_NAME: &str = "MIT";
pub const CLIPWARD_URL: &str = "https://www.clipward.ai";

/// The plan each CLI bills to (AI_CLIS `hint`).
pub fn cli_hint(id: &str) -> &'static str {
    match id {
        "claude" => "Claude Pro or Max plan",
        "codex" => "ChatGPT plan",
        "agy" | "gemini" => "Google account",
        "grok" => "xAI / SuperGrok",
        "cursor-agent" => "Cursor plan",
        "opencode" => "Your OpenCode providers",
        _ => "",
    }
}

/// A provider the user brings their own key for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiProvider {
    pub id: &'static str,
    pub label: &'static str,
    /// The secret's name in settings (`settings_replace_api_key`).
    pub key: &'static str,
    pub placeholder: &'static str,
    pub default_model: &'static str,
    /// Where to create a key.
    pub url: &'static str,
}

pub const API_PROVIDERS: [ApiProvider; 3] = [
    ApiProvider {
        id: "openrouter",
        label: "OpenRouter",
        key: "openrouterApiKey",
        placeholder: "sk-or-…",
        default_model: "anthropic/claude-opus-5.5",
        url: "https://openrouter.ai/keys",
    },
    ApiProvider {
        id: "anthropic",
        label: "Anthropic",
        key: "anthropicApiKey",
        placeholder: "sk-ant-…",
        default_model: "claude-opus-5",
        url: "https://console.anthropic.com/settings/keys",
    },
    ApiProvider {
        id: "openai",
        label: "OpenAI",
        key: "openaiApiKey",
        placeholder: "sk-…",
        default_model: "gpt-6-sol",
        url: "https://platform.openai.com/api-keys",
    },
];

pub fn api_provider(id: &str) -> Option<&'static ApiProvider> {
    API_PROVIDERS.iter().find(|provider| provider.id == id)
}

/// The user's own OpenAI-compatible server (LOCAL_PROVIDER).
pub const LOCAL_ID: &str = "local";
pub const LOCAL_LABEL: &str = "Local AI";
pub const LOCAL_KEY: &str = "localApiKey";
pub const LOCAL_DEFAULT_BASE_URL: &str = "http://localhost:11434/v1";
pub const LOCAL_EXAMPLE_MODEL: &str = "qwen2.5:14b";

/// "Claude Code CLI", "Local AI", "OpenRouter"; unknown ids as they are.
pub fn provider_label(provider: &str) -> String {
    let cli = provider.strip_prefix("cli:");
    if let Some((_, label, _)) = AI_CLIS.iter().find(|(id, _, _)| Some(*id) == cli) {
        return format!("{label} CLI");
    }
    if provider == LOCAL_ID {
        return LOCAL_LABEL.into();
    }
    api_provider(provider).map_or_else(|| provider.into(), |api| api.label.into())
}

/// "Low", "Extra high"…; empty (the provider's default) is "Default".
pub fn effort_label(level: &str) -> &'static str {
    match level {
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Extra high",
        "max" => "Max",
        _ => "Default",
    }
}

/// Why the Effort picker is off for a provider without effort control.
pub fn effort_note(provider: &str) -> &'static str {
    match provider {
        "cli:cursor-agent" => "Cursor sets this through its model list.",
        "cli:opencode" => "OpenCode sets this with a model variant (provider/model#variant).",
        "cli:agy" => "Pick a model to choose its effort.",
        _ => "This provider has no effort setting.",
    }
}

/// Antigravity puts the effort in the model id (`gemini-3.8-flash-high`): the base
/// id and its effort, or the id as it is when it has none.
pub fn agy_effort_split(id: &str) -> (&str, Option<&'static str>) {
    clip_engine::llm::EFFORT_LEVELS
        .iter()
        .find_map(|level| {
            let base = id.strip_suffix(level)?.strip_suffix('-')?;
            Some((base, Some(*level))).filter(|_| !base.is_empty())
        })
        .unwrap_or((id, None))
}

/// An Antigravity label without its effort: "Gemini 3.8 Flash (High)" -> "Gemini 3.8 Flash".
pub fn agy_base_label<'a>(label: &'a str, level: &str) -> &'a str {
    label
        .strip_suffix(&format!(" ({})", effort_label(level)))
        .unwrap_or(label)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WhisperModelInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub size_mb: u32,
    pub note: &'static str,
}

pub const WHISPER_MODELS: [WhisperModelInfo; 6] = [
    WhisperModelInfo { id: "base", label: "Base", size_mb: 148, note: "Fastest. Rough on names and noisy audio." },
    WhisperModelInfo { id: "small", label: "Small", size_mb: 488, note: "Fast. Good for clear speech." },
    WhisperModelInfo {
        id: "large-v3-turbo-q5_0",
        label: "Large v3 Turbo (compact)",
        size_mb: 574,
        note: "Recommended. Near-best accuracy, quick.",
    },
    WhisperModelInfo { id: "medium", label: "Medium", size_mb: 1533, note: "Slower. Good accuracy." },
    WhisperModelInfo { id: "large-v3-turbo", label: "Large v3 Turbo", size_mb: 1624, note: "Best accuracy for its speed." },
    WhisperModelInfo { id: "large-v3", label: "Large v3", size_mb: 3098, note: "Best overall accuracy. Slowest and largest." },
];

pub fn whisper_label(id: &str) -> &str {
    WHISPER_MODELS
        .iter()
        .find(|model| model.id == id)
        .map_or(id, |model| model.label)
}

/// "574 MB", "1.5 GB" (WhisperModels.tsx formatSize).
pub fn format_size_mb(mb: f64) -> String {
    if mb >= 1000.0 {
        format!("{:.1} GB", mb / 1000.0)
    } else {
        format!("{} MB", crate::format::js_round(mb))
    }
}

/// Non-blank lines of the custom vocabulary.
pub fn vocabulary_terms(text: &str) -> usize {
    text.split('\n').filter(|line| !line.trim().is_empty()).count()
}

/// Settings → Output → Keep source videos: (value, label); `forever` is None.
pub const RETENTION_OPTIONS: [(&str, &str); 5] = [
    ("forever", "Until the run is deleted"),
    ("90", "90 days"),
    ("30", "30 days"),
    ("7", "7 days"),
    ("0", "Don't keep (clips can't be edited)"),
];

/// The option showing `days`; a value not offered shows as 30 days.
pub fn retention_value(days: Option<u32>) -> &'static str {
    let Some(days) = days else { return "forever" };
    let text = days.to_string();
    RETENTION_OPTIONS
        .iter()
        .find(|(value, _)| *value == text)
        .map_or("30", |(value, _)| value)
}

pub fn retention_days(value: &str) -> Option<u32> {
    value.parse().ok()
}

/// The section the settings nav highlights: the last whose top (relative to the
/// scroll area's top) has reached a line `offset` below the top. Sections near the
/// end can't scroll up to that line, so over the last stretch the line slides down
/// to the bottom edge; every section takes its turn and the last is selected at the
/// bottom (SettingsPage.tsx useActiveSection).
pub fn active_section(tops: &[f32], scroll_top: f32, max_scroll: f32, client_height: f32, offset: f32) -> usize {
    let tail = max_scroll.min(client_height - offset);
    let progress = if tail > 0.0 {
        ((scroll_top - (max_scroll - tail)).max(0.0) / tail).min(1.0)
    } else {
        0.0
    };
    let line = offset + progress * (client_height - offset);
    tops.iter().rposition(|&top| top <= line).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::WHISPER_MODEL_IDS;

    #[test]
    fn tables_match_the_shared_ids() {
        let ids: Vec<&str> = WHISPER_MODELS.iter().map(|m| m.id).collect();
        assert_eq!(ids, WHISPER_MODEL_IDS);
        assert!(AI_CLIS.iter().all(|(id, _, _)| !cli_hint(id).is_empty()));
    }

    #[test]
    fn labels() {
        assert_eq!(provider_label("cli:claude"), "Claude Code CLI");
        assert_eq!(provider_label("local"), "Local AI");
        assert_eq!(provider_label("openai"), "OpenAI");
        assert_eq!(provider_label("cli:nope"), "cli:nope");
        assert_eq!(whisper_label("large-v3"), "Large v3");
    }

    #[test]
    fn agy_model_ids_carry_their_effort() {
        assert_eq!(agy_effort_split("gemini-3.8-flash-high"), ("gemini-3.8-flash", Some("high")));
        assert_eq!(agy_effort_split("claude-opus-5-5-xhigh"), ("claude-opus-5-5", Some("xhigh")));
        assert_eq!(agy_effort_split("gpt-oss-120b-medium"), ("gpt-oss-120b", Some("medium")));
        assert_eq!(agy_effort_split("gemini-3.1-pro"), ("gemini-3.1-pro", None));
        assert_eq!(agy_effort_split("high"), ("high", None));
        assert_eq!(agy_effort_split("-low"), ("-low", None));
        assert_eq!(agy_base_label("Gemini 3.8 Flash (High)", "high"), "Gemini 3.8 Flash");
        assert_eq!(agy_base_label("Odd label", "low"), "Odd label");
        assert_eq!(effort_label(""), "Default");
        assert_eq!(effort_label("xhigh"), "Extra high");
    }

    #[test]
    fn sizes_terms_and_retention() {
        assert_eq!(format_size_mb(574.0), "574 MB");
        assert_eq!(format_size_mb(1533.0), "1.5 GB");
        assert_eq!(format_size_mb(12.5), "13 MB");
        assert_eq!(vocabulary_terms("Acme\n  \nMrBeast\n"), 2);
        assert_eq!(retention_value(None), "forever");
        assert_eq!(retention_value(Some(7)), "7");
        assert_eq!(retention_value(Some(45)), "30");
        assert_eq!(retention_days("forever"), None);
        assert_eq!(retention_days("0"), Some(0));
    }

    #[test]
    fn scroll_spy() {
        let tops = [0.0, 300.0, 700.0, 1100.0];
        // At the top: the first section.
        assert_eq!(active_section(&tops, 0.0, 1000.0, 600.0, 48.0), 0);
        // A section counts once its top passes 48px below the top.
        let scrolled = [-260.0, 40.0, 440.0, 840.0];
        assert_eq!(active_section(&scrolled, 260.0, 1000.0, 600.0, 48.0), 1);
        // At the very bottom the line is the bottom edge: the last section.
        let bottom = [-1000.0, -700.0, -300.0, 100.0];
        assert_eq!(active_section(&bottom, 1000.0, 1000.0, 600.0, 48.0), 3);
        // Nothing to scroll: the plain offset line.
        assert_eq!(active_section(&tops, 0.0, 0.0, 600.0, 48.0), 0);
    }
}
