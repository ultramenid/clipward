//! Library reads, thumbnails and clip export (port of src/main/file-manager.ts and
//! the clips:bulkExport handler).
use crate::job_output::parse_job_output;
use crate::paths::AppPaths;
use crate::paths::{iso_from, open_no_follow};
use crate::run_history::{read_run_record, UUID};
use crate::security::MediaRegistry;
use crate::tools::{command, resolve_binary, run_ok, Tool};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const MAX_JOB_OUTPUT_BYTES: u64 = 20 * 1024 * 1024;
/// The Electron engine wrote job_output.json; the Rust engine writes result.json.
pub const OUTPUT_FILES: [&str; 2] = ["job_output.json", "result.json"];
const MAX_THUMBNAIL_SECONDS: f64 = 6.0 * 60.0 * 60.0;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobHistoryEntry {
    pub job_id: String,
    pub date: String,
    pub video_title: String,
    pub clip_count: usize,
    /// completed | failed | cancelled | running | interrupted | incomplete
    pub status: String,
    pub output_dir: String,
    pub total_cost_usd: Option<f64>,
    pub finished_at: Option<String>,
    pub duration_ms: Option<i64>,
    pub error_message: Option<String>,
}

enum OutputRead {
    Missing,
    Invalid,
    Valid(Value, SystemTime),
}

/// Read a result file inside the library without following links out of it.
fn read_job_output_file(output_path: &Path, library: &Path) -> OutputRead {
    let entry = match std::fs::symlink_metadata(output_path) {
        Ok(entry) => entry,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return OutputRead::Missing,
        Err(_) => return OutputRead::Invalid,
    };
    if !entry.is_file() || entry.file_type().is_symlink() {
        return OutputRead::Invalid;
    }
    let Ok(mut file) = open_no_follow(output_path) else {
        return OutputRead::Invalid;
    };
    let Ok(meta) = file.metadata() else {
        return OutputRead::Invalid;
    };
    if !meta.is_file() || meta.len() > MAX_JOB_OUTPUT_BYTES {
        return OutputRead::Invalid;
    }
    // Check the name again after opening and compare it with the handle, so a
    // swapped link is not accepted (Windows has no O_NOFOLLOW).
    let same = match (
        file.try_clone().map(same_file::Handle::from_file),
        same_file::Handle::from_path(output_path),
    ) {
        (Ok(Ok(a)), Ok(b)) => a == b,
        _ => false,
    };
    if !same || !crate::security::is_within_directory(output_path, library) {
        return OutputRead::Invalid;
    }
    let mut text = String::new();
    if file
        .by_ref()
        .take(MAX_JOB_OUTPUT_BYTES)
        .read_to_string(&mut text)
        .is_err()
    {
        return OutputRead::Invalid;
    }
    match serde_json::from_str::<Value>(&text)
        .ok()
        .as_ref()
        .and_then(parse_job_output)
    {
        Some(output) => {
            OutputRead::Valid(output, meta.modified().unwrap_or(SystemTime::UNIX_EPOCH))
        }
        None => OutputRead::Invalid,
    }
}

fn read_job_output(run_dir: &Path, library: &Path) -> OutputRead {
    for name in OUTPUT_FILES {
        match read_job_output_file(&run_dir.join(name), library) {
            OutputRead::Missing => continue,
            other => return other,
        }
    }
    OutputRead::Missing
}

pub fn get_job_output(run_dir: &Path, library: &Path) -> Option<Value> {
    match read_job_output(run_dir, library) {
        OutputRead::Valid(output, _) => Some(output),
        _ => None,
    }
}

fn millis_between(start: &str, end: &str) -> Option<i64> {
    let start = chrono::DateTime::parse_from_rfc3339(start).ok()?;
    let end = chrono::DateTime::parse_from_rfc3339(end).ok()?;
    Some((end - start).num_milliseconds().max(0))
}

pub fn get_job_history(
    base_dir: &Path,
    active: &HashSet<String>,
) -> Result<Vec<JobHistoryEntry>, String> {
    if !base_dir.exists() {
        return Ok(Vec::new());
    }
    let read_dir =
        std::fs::read_dir(base_dir).map_err(|_| "Could not read the clip library".to_string())?;
    let mut entries = Vec::new();
    for dir in read_dir.flatten() {
        if !dir.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Some(name) = dir.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let run_dir = base_dir.join(&name);
        let record = read_run_record(base_dir, &name);
        let duration_ms = record.as_ref().and_then(|r| {
            r.finished_at
                .as_deref()
                .and_then(|end| millis_between(&r.started_at, end))
        });
        let output_dir = run_dir.to_string_lossy().into_owned();
        match read_job_output(&run_dir, base_dir) {
            OutputRead::Valid(data, modified) => {
                let cost = data["metrics"]["api_costs"]["total_estimated_cost_usd"].as_f64();
                let modified = iso_from(modified);
                entries.push(JobHistoryEntry {
                    job_id: name.clone(),
                    date: record
                        .as_ref()
                        .map_or(modified.clone(), |r| r.started_at.clone()),
                    video_title: data["source_video_title"]
                        .as_str()
                        .unwrap_or("Untitled video")
                        .into(),
                    clip_count: data["clips"].as_array().map_or(0, Vec::len),
                    status: "completed".into(),
                    output_dir,
                    total_cost_usd: cost,
                    finished_at: Some(
                        record
                            .as_ref()
                            .and_then(|r| r.finished_at.clone())
                            .unwrap_or(modified),
                    ),
                    duration_ms: duration_ms.or_else(|| {
                        data["processing_time_seconds"]
                            .as_f64()
                            .map(|s| (s * 1000.0).round() as i64)
                    }),
                    error_message: None,
                });
            }
            OutputRead::Missing => {
                // Desktop jobs use UUIDs. Keep interrupted runs visible without
                // treating unrelated folders in the output directory as clip jobs.
                if !UUID.is_match(&name) {
                    continue;
                }
                let Ok(meta) = std::fs::symlink_metadata(&run_dir) else {
                    continue;
                };
                if !meta.is_dir() || meta.file_type().is_symlink() {
                    continue;
                }
                let status = match record.as_ref().map(|r| r.status.as_str()) {
                    Some("running") if active.contains(&name) => "running",
                    Some("running") => "interrupted",
                    Some(status @ ("failed" | "cancelled")) => status,
                    _ => "incomplete",
                };
                let mtime = meta.modified().map(iso_from).unwrap_or_default();
                entries.push(JobHistoryEntry {
                    job_id: name.clone(),
                    date: record.as_ref().map_or(mtime, |r| r.started_at.clone()),
                    video_title: record
                        .as_ref()
                        .map_or("Unfinished run".into(), |r| r.source_label.clone()),
                    clip_count: 0,
                    status: status.into(),
                    output_dir,
                    total_cost_usd: None,
                    finished_at: record.as_ref().and_then(|r| r.finished_at.clone()),
                    duration_ms,
                    error_message: record.as_ref().and_then(|r| r.error_message.clone()),
                });
            }
            OutputRead::Invalid => entries.push(JobHistoryEntry {
                job_id: name.clone(),
                date: record.as_ref().map_or_else(
                    || iso_from(SystemTime::UNIX_EPOCH),
                    |r| r.started_at.clone(),
                ),
                video_title: record
                    .as_ref()
                    .map_or("Unreadable run".into(), |r| r.source_label.clone()),
                clip_count: 0,
                status: "failed".into(),
                output_dir,
                total_cost_usd: None,
                finished_at: record.as_ref().and_then(|r| r.finished_at.clone()),
                duration_ms,
                error_message: Some(
                    record
                        .as_ref()
                        .and_then(|r| r.error_message.clone())
                        .unwrap_or_else(|| "The saved result could not be read.".into()),
                ),
            }),
        }
    }
    entries.sort_by(|a, b| b.date.cmp(&a.date));
    Ok(entries)
}

async fn video_duration_seconds(paths: &AppPaths, video: &Path) -> Option<f64> {
    let mut cmd = command(resolve_binary(paths, Tool::Ffprobe));
    cmd.args([
        "-v",
        "error",
        "-protocol_whitelist",
        "file,pipe,fd",
        "-format_whitelist",
        "mov,matroska,webm,avi,flv",
        "-show_entries",
        "format=duration",
        "-of",
        "csv=p=0",
    ])
    .arg(video);
    let stdout = run_ok(cmd, Duration::from_secs(10)).await?;
    String::from_utf8_lossy(&stdout)
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|d| d.is_finite())
}

/// A cached JPEG frame for a video (the middle of the clip unless `seek` is given),
/// generated with ffmpeg into the private thumbnails folder.
pub async fn generate_thumbnail(
    paths: &AppPaths,
    video_path: &Path,
    seek: Option<f64>,
) -> Option<PathBuf> {
    let ext = video_path.extension()?.to_str()?.to_ascii_lowercase();
    if !["mp4", "m4v", "mkv", "webm", "mov", "avi", "flv"].contains(&ext.as_str()) {
        return None;
    }
    if seek.is_some_and(|s| !(0.0..=MAX_THUMBNAIL_SECONDS).contains(&s)) {
        return None;
    }
    let source = std::fs::canonicalize(video_path).ok()?;
    let meta = std::fs::metadata(&source).ok()?;
    if !meta.is_file() {
        return None;
    }
    let dir = crate::paths::private_dir(&paths.user_data.join("thumbnails")).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis());
    let identity = format!(
        "{}:{:?}:{}:{}:{}",
        source.display(),
        file_id::get_file_id(&source).ok(),
        meta.len(),
        mtime,
        seek.map_or("middle".into(), |s| s.to_string())
    );
    let hash = Sha256::digest(identity.as_bytes());
    let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
    let thumb = dir.join(format!("{hex}.jpg"));
    if std::fs::symlink_metadata(&thumb).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink()) {
        return Some(thumb);
    }
    let temp = dir.join(format!("{}.jpg", uuid::Uuid::new_v4()));

    let seek_to = match seek {
        Some(seek) => seek,
        None => match video_duration_seconds(paths, &source).await {
            Some(duration) if duration > MAX_THUMBNAIL_SECONDS => return None,
            Some(duration) if duration > 0.5 => (duration * 0.5).min(duration - 0.1),
            _ => 0.0,
        },
    };

    let ffmpeg = resolve_binary(paths, Tool::Ffmpeg);
    let attempt = |seek_to: f64| {
        let mut cmd = command(&ffmpeg);
        cmd.arg("-n");
        if seek_to > 0.0 {
            cmd.args(["-ss", &format!("{seek_to:.2}")]);
        }
        cmd.args([
            "-protocol_whitelist",
            "file,pipe,fd",
            "-format_whitelist",
            "mov,matroska,webm,avi,flv",
            "-i",
        ])
        .arg(&source)
        .args(["-frames:v", "1", "-q:v", "2", "-vf", "scale=640:-2"])
        .arg(&temp);
        run_ok(cmd, Duration::from_secs(15))
    };
    let mut made = attempt(seek_to).await.is_some() && temp.exists();
    if !made {
        // Fallback: the first frame.
        let _ = std::fs::remove_file(&temp);
        made = attempt(0.0).await.is_some() && temp.exists();
    }
    let result = made && std::fs::rename(&temp, &thumb).is_ok();
    let _ = std::fs::remove_file(&temp);
    result.then_some(thumb)
}

#[derive(Debug, Deserialize)]
pub struct ExportClip {
    pub path: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub success: bool,
    pub count: usize,
    pub failed_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dest_dir: Option<String>,
}

fn safe_export_name(name: &str) -> String {
    let printable: String = name.chars().filter(|c| (*c as u32) >= 32).collect();
    let mut out = String::new();
    let mut in_bad = false;
    for c in printable.chars() {
        if "<>:\"/\\|?*".contains(c) {
            if !in_bad {
                out.push('-');
            }
            in_bad = true;
        } else {
            out.push(c);
            in_bad = false;
        }
    }
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let clipped: String = collapsed.chars().take(120).collect();
    if clipped.is_empty() {
        "clip".into()
    } else {
        clipped
    }
}

/// Copy each authorized clip into `dest_dir` without overwriting anything there.
pub fn export_clips(
    media: &MediaRegistry,
    library: &Path,
    clips: &[ExportClip],
    dest_dir: &Path,
) -> ExportResult {
    let mut count = 0;
    for clip in clips {
        let copied = (|| -> Result<(), ()> {
            let (mut source, _, _) = media.open_authorized(&clip.path, library).map_err(|_| ())?;
            let ext = Path::new(&clip.path)
                .extension()
                .and_then(|e| e.to_str())
                .map_or(".mp4".to_string(), |e| format!(".{e}"));
            let name = safe_export_name(&clip.name);
            for suffix in 0..=10_000 {
                let file_name = if suffix == 0 {
                    format!("{name}{ext}")
                } else {
                    format!("{name} ({suffix}){ext}")
                };
                let dest = dest_dir.join(file_name);
                match crate::paths::open_private_new(&dest) {
                    Ok(mut target) => {
                        let copied =
                            std::io::copy(&mut source, &mut target).and_then(|_| target.sync_all());
                        if copied.is_err() {
                            drop(target);
                            let _ = std::fs::remove_file(&dest);
                            return Err(());
                        }
                        return Ok(());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(_) => return Err(()),
                }
            }
            Err(())
        })();
        if copied.is_ok() {
            count += 1;
        }
    }
    ExportResult {
        success: count > 0,
        count,
        failed_count: clips.len() - count,
        dest_dir: Some(dest_dir.to_string_lossy().into_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_names_are_safe() {
        assert_eq!(safe_export_name("a/b\\c:  d\u{7}"), "a-b-c- d");
        assert_eq!(safe_export_name("   "), "clip");
    }

    #[test]
    fn history_lists_runs() {
        let base = std::env::temp_dir().join(format!("clipward-hist-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();
        let running = uuid::Uuid::new_v4().to_string();
        crate::run_history::create_run_record(&base, &running, "/v.mp4").unwrap();
        let done = uuid::Uuid::new_v4().to_string();
        std::fs::create_dir_all(base.join(&done)).unwrap();
        std::fs::write(
            base.join(&done).join("result.json"),
            r#"{"source_video_title":"Talk","clips":[],"metrics":{"api_costs":{"total_estimated_cost_usd":0.5}}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(base.join("not-a-run")).unwrap();
        let history = get_job_history(&base, &HashSet::new()).unwrap();
        assert_eq!(history.len(), 2);
        let by_id = |id: &str| history.iter().find(|e| e.job_id == id).unwrap();
        assert_eq!(by_id(&running).status, "interrupted");
        assert_eq!(by_id(&done).status, "completed");
        assert_eq!(by_id(&done).total_cost_usd, Some(0.5));
        std::fs::remove_dir_all(&base).unwrap();
    }
}
