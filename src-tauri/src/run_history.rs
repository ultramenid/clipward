//! Per-run status records kept in each run folder (port of src/main/run-history.ts).
use crate::paths::{iso_now, open_no_follow, write_atomic};
use crate::validation::twitch_vod_id;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

const RUN_FILE: &str = "run-history.json";
const MAX_RECORD_BYTES: u64 = 16 * 1024;
const FAILURE_STAGES: &[&str] = &[
    "setup",
    "download",
    "transcription",
    "planning",
    "rendering",
    "saving",
    "uploading",
];

pub static UUID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$").unwrap()
});
static FAILURE_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z]+(?:[._][a-z]+)*$").unwrap());

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub job_id: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub source_label: String,
    /// running | completed | failed | cancelled
    pub status: String,
    pub error_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_stage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
}

#[derive(Debug, Clone, Default)]
pub struct FailureDetails {
    pub failure_code: Option<String>,
    pub failure_stage: Option<String>,
    pub http_status: Option<u16>,
}

fn run_directory(base_dir: &Path, job_id: &str) -> Result<PathBuf, String> {
    if !UUID.is_match(job_id) {
        return Err("Invalid run identifier".into());
    }
    Ok(base_dir.join(job_id))
}

fn checked_directory(base_dir: &Path, job_id: &str) -> Result<PathBuf, String> {
    let dir = run_directory(base_dir, job_id)?;
    let meta = std::fs::symlink_metadata(&dir).map_err(|_| "Invalid run directory".to_string())?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err("Invalid run directory".into());
    }
    if !crate::security::is_within_directory(&dir, base_dir) {
        return Err("Run is outside the output folder".into());
    }
    Ok(dir)
}

/// A short, printable label for the run's source: never a full local path or URL.
pub fn source_label(source: &str) -> String {
    if let Some(id) = twitch_vod_id(source) {
        return format!("Twitch VOD · {id}").chars().take(160).collect();
    }
    let label = match url::Url::parse(source) {
        Ok(url) if matches!(url.scheme(), "http" | "https") => {
            let host = url.host_str().unwrap_or_default().to_lowercase();
            let video_id = if matches!(
                host.as_str(),
                "youtube.com" | "www.youtube.com" | "m.youtube.com"
            ) && url.path() == "/watch"
            {
                url.query_pairs()
                    .find(|(k, _)| k == "v")
                    .map(|(_, v)| v.into_owned())
            } else {
                None
            };
            match video_id {
                Some(id)
                    if id.len() == 11
                        && id
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') =>
                {
                    format!("YouTube · {id}")
                }
                _ => host,
            }
        }
        _ => Path::new(source)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    let clean: String = label
        .chars()
        .map(|c| {
            if (c as u32) < 32 || c as u32 == 127 {
                ' '
            } else {
                c
            }
        })
        .collect();
    let clean: String = clean.trim().chars().take(160).collect();
    if clean.is_empty() {
        "Video source".into()
    } else {
        clean
    }
}

fn valid_date(value: &str) -> bool {
    value.len() <= 40 && chrono::DateTime::parse_from_rfc3339(value).is_ok()
}

pub fn read_run_record(base_dir: &Path, job_id: &str) -> Option<RunRecord> {
    let dir = checked_directory(base_dir, job_id).ok()?;
    let file_path = dir.join(RUN_FILE);
    let mut file = open_no_follow(&file_path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_RECORD_BYTES {
        return None;
    }
    let entry = std::fs::symlink_metadata(&file_path).ok()?;
    let same = match (
        file.try_clone().map(same_file::Handle::from_file),
        same_file::Handle::from_path(&file_path),
    ) {
        (Ok(Ok(a)), Ok(b)) => a == b,
        _ => false,
    };
    if entry.file_type().is_symlink() || !same {
        return None;
    }
    let mut text = String::new();
    file.by_ref()
        .take(MAX_RECORD_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    let record: RunRecord = serde_json::from_str(&text).ok()?;
    let valid = record.job_id == job_id
        && valid_date(&record.started_at)
        && record.finished_at.as_deref().is_none_or(valid_date)
        && record.source_label.chars().count() <= 160
        && ["running", "completed", "failed", "cancelled"].contains(&record.status.as_str())
        && record
            .error_message
            .as_deref()
            .is_none_or(|m| m.chars().count() <= 300)
        && record
            .failure_code
            .as_deref()
            .is_none_or(|c| c.len() <= 64 && FAILURE_CODE.is_match(c))
        && record
            .failure_stage
            .as_deref()
            .is_none_or(|s| FAILURE_STAGES.contains(&s))
        && record.http_status.is_none_or(|s| (100..=599).contains(&s));
    valid.then_some(record)
}

fn write_run_record(base_dir: &Path, record: &RunRecord) -> Result<(), String> {
    let dir = checked_directory(base_dir, &record.job_id)?;
    let bytes = serde_json::to_vec(record).map_err(|_| "Could not write run record".to_string())?;
    write_atomic(&dir.join(RUN_FILE), &bytes).map_err(|_| "Could not write run record".to_string())
}

pub fn create_run_record(base_dir: &Path, job_id: &str, source: &str) -> Result<(), String> {
    let dir = run_directory(base_dir, job_id)?;
    std::fs::create_dir_all(&dir).map_err(|_| "Could not create run folder".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    write_run_record(
        base_dir,
        &RunRecord {
            job_id: job_id.into(),
            started_at: iso_now(),
            finished_at: None,
            source_label: source_label(source),
            status: "running".into(),
            error_message: None,
            failure_code: None,
            failure_stage: None,
            http_status: None,
        },
    )
}

/// Record how a running run ended. A record that is already final is left alone.
pub fn finish_run_record(
    base_dir: &Path,
    job_id: &str,
    status: &str,
    error_message: Option<&str>,
    details: FailureDetails,
) -> Result<(), String> {
    let Some(previous) = read_run_record(base_dir, job_id) else {
        return Ok(());
    };
    if previous.status != "running" {
        return Ok(());
    }
    write_run_record(
        base_dir,
        &RunRecord {
            status: status.into(),
            finished_at: Some(iso_now()),
            error_message: error_message.map(|m| m.chars().take(300).collect()),
            failure_code: details.failure_code,
            failure_stage: details.failure_stage,
            http_status: details.http_status,
            ..previous
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_sources_without_leaking_paths() {
        assert_eq!(
            source_label("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=1"),
            "YouTube · dQw4w9WgXcQ"
        );
        assert_eq!(
            source_label("https://example.com/private/token"),
            "example.com"
        );
        assert_eq!(source_label("/Users/me/Movies/talk.mp4"), "talk.mp4");
        assert_eq!(
            source_label("https://www.twitch.tv/videos/42"),
            "Twitch VOD · 42"
        );
    }

    #[test]
    fn round_trips_records() {
        let base = std::env::temp_dir().join(format!("clipward-runs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        create_run_record(&base, &id, "/x/y.mp4").unwrap();
        assert_eq!(read_run_record(&base, &id).unwrap().status, "running");
        finish_run_record(
            &base,
            &id,
            "failed",
            Some("boom"),
            FailureDetails::default(),
        )
        .unwrap();
        let record = read_run_record(&base, &id).unwrap();
        assert_eq!(record.status, "failed");
        assert_eq!(record.error_message.as_deref(), Some("boom"));
        finish_run_record(&base, &id, "completed", None, FailureDetails::default()).unwrap();
        assert_eq!(read_run_record(&base, &id).unwrap().status, "failed");
        assert!(create_run_record(&base, "../escape", "x").is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
