//! Whether a clip job can start: a usable AI provider, a downloaded Whisper model
//! and the required tools (use-settings-store.ts `isProviderReady`,
//! `useSetupState`, `setupBlockedReason`; glass.ts `glassActive`).
use crate::ai_providers::DetectedCli;
use crate::settings::PublicSettings;
use crate::whisper::WhisperModelStatus;
use crate::ToolStatus;

/// The user's own OpenAI-compatible server (shared/ai-providers.ts LOCAL_PROVIDER).
pub const LOCAL_PROVIDER: &str = "local";

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct SetupState {
    pub ready: bool,
    /// The selected AI provider can run: its CLI is installed, its key is saved, or
    /// (Local AI) a server URL and model are set.
    pub provider_ready: bool,
    /// The selected Whisper model is on disk.
    pub whisper_ready: bool,
    /// None until the system check has run.
    pub tools_ok: Option<bool>,
    /// Everything but the provider is ready: enough for manual clip mode.
    pub manual_ready: bool,
}

/// Whether the selected provider is usable, from settings alone (no network).
pub fn provider_ready(settings: &PublicSettings, clis: Option<&[DetectedCli]>) -> bool {
    if let Some(id) = settings.ai_provider.strip_prefix("cli:") {
        return clis
            .and_then(|clis| clis.iter().find(|cli| cli.id == id))
            .is_some_and(|cli| cli.path.is_some());
    }
    // The server itself is only checked by Test connection or the job.
    if settings.ai_provider == LOCAL_PROVIDER {
        return !settings.local_base_url.trim().is_empty() && !settings.local_model.trim().is_empty();
    }
    match settings.ai_provider.as_str() {
        "openrouter" => settings.openrouter_configured,
        "anthropic" => settings.anthropic_configured,
        "openai" => settings.openai_configured,
        _ => false,
    }
}

/// The setup state from settings, detected CLIs, Whisper models and the system check.
pub fn setup_state(
    settings: &PublicSettings,
    clis: Option<&[DetectedCli]>,
    whisper_models: &[WhisperModelStatus],
    tool_status: Option<&ToolStatus>,
    tool_error: bool,
    checking_tools: bool,
) -> SetupState {
    let provider_ready = provider_ready(settings, clis);
    let whisper_ready = whisper_models
        .iter()
        .any(|model| model.id == settings.whisper_model && model.downloaded);
    let tools_ok = if tool_error {
        Some(false)
    } else {
        tool_status.map(|t| t.smart_framing && t.ffmpeg && t.ffprobe && t.ytdlp && t.engine)
    };
    let manual_ready = whisper_ready && tools_ok == Some(true) && !checking_tools;
    SetupState {
        ready: provider_ready && manual_ready,
        provider_ready,
        whisper_ready,
        tools_ok,
        manual_ready,
    }
}

/// Why a job in this clip mode can't start yet, or None when it can. Manual clip
/// mode needs no AI provider; the Whisper model is still required (captions and
/// tight pacing need word timings).
pub fn setup_blocked_reason(setup: SetupState, manual: bool) -> Option<&'static str> {
    if !manual && !setup.provider_ready {
        return Some("Choose an AI provider above to start.");
    }
    if !setup.whisper_ready {
        return Some("Download the Whisper model above to start.");
    }
    if setup.tools_ok == Some(false) {
        return Some("Fix the missing tools in Settings to start.");
    }
    let ready = if manual { setup.manual_ready } else { setup.ready };
    (!ready).then_some("Checking required tools…")
}

/// Glass is drawn only where supported, switched on, and transparency isn't reduced.
pub fn glass_active(supported: bool, enabled: bool, reduced_transparency: bool) -> bool {
    supported && enabled && !reduced_transparency
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::appearance::Appearance;

    fn settings(provider: &str) -> PublicSettings {
        PublicSettings {
            output_directory: "/clips".into(),
            custom_vocabulary: String::new(),
            ai_provider: provider.into(),
            ai_model: String::new(),
            whisper_model: "base".into(),
            openrouter_configured: false,
            anthropic_configured: true,
            openai_configured: false,
            local_configured: false,
            local_base_url: "http://localhost:11434/v1".into(),
            local_model: String::new(),
            local_images: false,
            window_glass: false,
            window_glass_supported: false,
            appearance: Appearance::default(),
            window_blur_supported: false,
            source_retention_days: Some(30),
        }
    }

    fn cli(id: &str, path: Option<&str>) -> DetectedCli {
        DetectedCli { id: id.into(), label: id.into(), path: path.map(Into::into) }
    }

    #[test]
    fn setup_asks_for_an_ai_provider_until_one_is_ready() {
        let clis = [cli("claude", Some("/usr/local/bin/claude")), cli("codex", None)];
        // A logged-in CLI needs no API key; one that isn't installed doesn't count.
        assert!(provider_ready(&settings("cli:claude"), Some(&clis)));
        assert!(!provider_ready(&settings("cli:codex"), Some(&clis)));
        assert!(!provider_ready(&settings("cli:claude"), None));
        // An API provider is ready only with its own key.
        assert!(provider_ready(&settings("anthropic"), Some(&clis)));
        assert!(!provider_ready(&settings("openrouter"), Some(&clis)));
        assert!(!provider_ready(&settings("unknown"), Some(&clis)));
        // Local AI needs a server URL and a model, but no key.
        let local = |url: &str, model: &str| PublicSettings {
            local_base_url: url.into(),
            local_model: model.into(),
            ..settings(LOCAL_PROVIDER)
        };
        assert!(provider_ready(&local("http://localhost:11434/v1", "qwen2.5:14b"), None));
        assert!(!provider_ready(&local("http://localhost:11434/v1", " "), None));
        assert!(!provider_ready(&local("", "qwen2.5:14b"), None));
    }

    #[test]
    fn manual_mode_starts_without_an_ai_provider_but_still_needs_whisper_and_the_tools() {
        let no_provider = SetupState {
            ready: false,
            provider_ready: false,
            whisper_ready: true,
            tools_ok: Some(true),
            manual_ready: true,
        };
        assert_eq!(setup_blocked_reason(no_provider, false), Some("Choose an AI provider above to start."));
        assert_eq!(setup_blocked_reason(no_provider, true), None);
        let no_whisper = SetupState { whisper_ready: false, manual_ready: false, ..no_provider };
        assert_eq!(setup_blocked_reason(no_whisper, true), Some("Download the Whisper model above to start."));
        let tools_missing = SetupState { tools_ok: Some(false), manual_ready: false, ..no_provider };
        assert_eq!(setup_blocked_reason(tools_missing, true), Some("Fix the missing tools in Settings to start."));
        let checking = SetupState { tools_ok: None, manual_ready: false, ..no_provider };
        assert_eq!(setup_blocked_reason(checking, true), Some("Checking required tools…"));
        let ready = SetupState { ready: true, provider_ready: true, ..no_provider };
        assert_eq!(setup_blocked_reason(ready, false), None);
    }

    #[test]
    fn setup_state_needs_the_selected_whisper_model_and_every_required_tool() {
        let s = settings("anthropic");
        let models = [WhisperModelStatus { id: "base".into(), downloaded: true, received_bytes: None, total_bytes: None }];
        let tools = ToolStatus {
            smart_framing: true,
            ffmpeg: true,
            ffprobe: true,
            ytdlp: true,
            engine: true,
            ..Default::default()
        };
        let state = setup_state(&s, None, &models, Some(&tools), false, false);
        assert!(state.ready && state.manual_ready && state.whisper_ready);
        // Captions support is optional; a failed check is not.
        assert!(!setup_state(&s, None, &models, Some(&tools), true, false).ready);
        assert_eq!(setup_state(&s, None, &models, None, false, false).tools_ok, None);
        // Still checking: not ready yet.
        assert!(!setup_state(&s, None, &models, Some(&tools), false, true).manual_ready);
        let other_model = PublicSettings { whisper_model: "large".into(), ..s };
        assert!(!setup_state(&other_model, None, &models, Some(&tools), false, false).whisper_ready);
    }

    #[test]
    fn glass_is_drawn_only_where_supported_switched_on_and_transparency_not_reduced() {
        assert!(glass_active(true, true, false));
        assert!(!glass_active(false, true, false));
        assert!(!glass_active(true, false, false));
        assert!(!glass_active(true, true, true));
    }
}
