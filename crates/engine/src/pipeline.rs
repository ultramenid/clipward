//! Orchestration: download -> transcribe -> plan -> layout -> render -> result.json.
//! TODO(integration): port ai_clipping_pipeline.py + bridge_runner.py once the modules land.
use crate::error::{EngineError, Result};
use crate::types::{ClipJobRequest, EngineConfig, ProgressFn};
use tokio_util::sync::CancellationToken;

/// Runs one job and returns the JobOutput JSON (src/shared/job-output.ts), also saved as
/// `result.json` in `cfg.output_dir`.
pub async fn run_job(
    _req: ClipJobRequest,
    _cfg: EngineConfig,
    _progress: ProgressFn,
    _cancel: CancellationToken,
) -> Result<serde_json::Value> {
    Err(EngineError::msg("pipeline not ported yet"))
}

/// Known failure classes -> (markers, message, hint). Port of bridge_runner.FAILURES; first match wins.
const FAILURES: &[(&[&str], &str, &str)] = &[
    (
        &["selected planner requires a video with speech"],
        "The selected AI provider cannot analyze a video without speech.",
        "CLI providers read text only. Use an API key provider in Settings for silent videos.",
    ),
    (
        &["cli is not logged in"],
        "The AI CLI is not logged in.",
        "Open a terminal, run the CLI once and sign in, then retry.",
    ),
    (
        &["cli was not found", "cli could not be started"],
        "The selected AI CLI could not be started.",
        "Open Settings → AI provider and pick a detected provider.",
    ),
    (
        &["cli timed out"],
        "The AI CLI took too long to plan clips.",
        "Retry, or trim the video to a shorter range.",
    ),
    (
        &["unsupported twitch source"],
        "Choose a public, completed Twitch VOD.",
        "Copy the video link from a saved Twitch video. Live channels, collections and Twitch clips are not supported.",
    ),
    (
        &["twitch vod is not completed"],
        "This Twitch video is still live or processing.",
        "Wait until the broadcast has ended and its saved video is ready, then retry.",
    ),
    (
        &["twitch vod duration is invalid or too long"],
        "This Twitch video has no usable duration or exceeds the six hour limit.",
        "Choose a completed video under six hours, or trim a downloaded file before adding it.",
    ),
    (
        &["twitch vod unavailable"],
        "The Twitch VOD could not be downloaded.",
        "Check that the saved video plays while signed out. Deleted, expired and subscriber-only videos are not supported. You can also use a local file.",
    ),
    (
        &["not enough disk space to save clips", "no space left on device", "disk quota exceeded"],
        "There is not enough free disk space to finish this video.",
        "Free space on your startup disk and the output drive, then retry. Source videos can use several GB while clipping.",
    ),
    (
        &["whisper model is not downloaded"],
        "The offline transcription model is not downloaded.",
        "Open Settings → Transcription and download a Whisper model, then retry.",
    ),
    (
        &["local transcription failed", "whisper.cpp is not installed"],
        "Offline transcription failed.",
        "Run Settings → System check. A smaller Whisper model uses less memory; try one if this persists.",
    ),
    (
        &["audio extraction failed", "audio duration could not be determined", "transcription audio preparation failed"],
        "Clipward could not prepare this video's audio for transcription.",
        "Run Settings → System check. If the tools are ready, report this run with its failure code.",
    ),
    (
        &["transcription audio chunk exceeded the size limit"],
        "The transcription audio exceeded Clipward's size limit.",
        "Set a shorter start and end time, or report this run so the chunk size can be adjusted.",
    ),
    (
        &["transcription failed"],
        "Audio transcription failed.",
        "Check that the video has a playable audio track, then retry.",
    ),
    (
        &["video render failed"],
        "Clip rendering failed.",
        "Run Settings → System check. If all tools are ready, report this run so the render can be diagnosed.",
    ),
    (
        &["http error 403", "sign in to confirm", "blocking this request"],
        "The video service refused the download.",
        "Update Clipward and retry. If it keeps happening, download the video yourself and clip it as a local file.",
    ),
    (
        &["video unavailable", "private video", "members-only", "has been removed", "not available in your country"],
        "This video is private, removed or unavailable in your region.",
        "Check the link opens in a signed-out browser window, or clip a local file instead.",
    ),
    (
        &["exceeds maximum allowed duration"],
        "This video is longer than Clipward can process.",
        "Choose a shorter source video, or trim a downloaded file before adding it.",
    ),
    (
        &["no clip-worthy moments"],
        "Clipward couldn't find any clips in this video.",
        "No clear spoken or visual moment met the selected clip length. If you set a start and end time, widen it or pick a shorter clip length.",
    ),
    (
        &["out of credits", "quota exceeded"],
        "Your AI provider account is out of credits.",
        "Add credits to the provider account or raise the key's limit.",
    ),
    (
        &["(401)", "status_code: 401", "unauthorized", "invalid api key", "invalid_api_key"],
        "An API key was rejected.",
        "Check the AI provider key in Settings.",
    ),
    (
        &["timed out", "connection", "name resolution", "network is unreachable"],
        "A network request failed.",
        "Check your internet connection and retry.",
    ),
    (
        &["video download failed"],
        "The video could not be downloaded.",
        "Check the link opens in a browser and your connection works, then retry. You can also clip a local file instead.",
    ),
];

const FALLBACK: (&str, &str) = (
    "The clipping pipeline failed.",
    "Check System check and retry. If it persists, report the steps that reproduce it.",
);

/// Fixed, safe UI text (message, hint) for an engine error. Port of bridge_runner.describe_failure:
/// raw error text can hold URLs, credentials and paths, so only these fixed strings reach the UI.
pub fn describe_failure(err: &EngineError) -> (String, String) {
    let (message, hint) = match err {
        EngineError::Cancelled => ("Pipeline cancelled by user", ""),
        EngineError::Failed(text) => describe_text(text),
    };
    (message.into(), hint.into())
}

fn describe_text(text: &str) -> (&'static str, &'static str) {
    let text = text.to_lowercase();
    FAILURES
        .iter()
        .find(|(markers, _, _)| markers.iter().any(|m| text.contains(m)))
        .map(|&(_, message, hint)| (message, hint))
        .unwrap_or(FALLBACK)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str) -> String {
        describe_failure(&EngineError::msg(text)).0
    }

    #[test]
    fn maps_known_failures_to_fixed_text() {
        assert_eq!(message("YouTube download failed. Last error: ERROR: unable to download video data: HTTP Error 403: Forbidden"), "The video service refused the download.");
        let (m, h) = describe_failure(&EngineError::msg("socks5h://user:secret-pass@10.0.0.1:1 /Users/someone/private.mp4"));
        assert_eq!(m, "The clipping pipeline failed.");
        assert!(!h.contains("secret-pass"));
        assert_eq!(message("No clip-worthy moments found (the video may have no speech)"), "Clipward couldn't find any clips in this video.");
        assert_eq!(message("Whisper model is not downloaded"), "The offline transcription model is not downloaded.");
        assert_eq!(message("Local transcription failed"), "Offline transcription failed.");
        assert_eq!(message("Claude Code CLI is not logged in (unauthorized)"), "The AI CLI is not logged in.");
        assert_eq!(message("Codex CLI timed out"), "The AI CLI took too long to plan clips.");
        assert_eq!(message("Video render failed"), "Clip rendering failed.");
        assert_eq!(message("Not enough disk space to save clips"), "There is not enough free disk space to finish this video.");
        assert_eq!(message("io error: No space left on device (os error 28)"), "There is not enough free disk space to finish this video.");
        assert_eq!(message("Video download failed"), "The video could not be downloaded.");
        assert!(describe_failure(&EngineError::msg("twitch vod unavailable")).1.contains("signed out"));
    }
}
