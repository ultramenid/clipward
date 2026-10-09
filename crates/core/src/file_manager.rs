//! Library reads, thumbnails and clip export.
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

#[derive(Debug, Clone, Serialize)]
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

/// Read a file inside the library without following links out of it.
/// `Err(true)`: it doesn't exist; `Err(false)`: unreadable, too large or outside the library.
pub fn read_library_file(
    path: &Path,
    library: &Path,
    max_bytes: u64,
) -> Result<(String, SystemTime), bool> {
    let entry = match std::fs::symlink_metadata(path) {
        Ok(entry) => entry,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(true),
        Err(_) => return Err(false),
    };
    if !entry.is_file() || entry.file_type().is_symlink() {
        return Err(false);
    }
    let mut file = open_no_follow(path).map_err(|_| false)?;
    let meta = file.metadata().map_err(|_| false)?;
    if !meta.is_file() || meta.len() > max_bytes {
        return Err(false);
    }
    // Check the name again after opening and compare it with the handle, so a
    // swapped link is not accepted (Windows has no O_NOFOLLOW).
    let same = match (
        file.try_clone().map(same_file::Handle::from_file),
        same_file::Handle::from_path(path),
    ) {
        (Ok(Ok(a)), Ok(b)) => a == b,
        _ => false,
    };
    if !same || !crate::security::is_within_directory(path, library) {
        return Err(false);
    }
    let mut text = String::new();
    file.by_ref()
        .take(max_bytes)
        .read_to_string(&mut text)
        .map_err(|_| false)?;
    Ok((text, meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)))
}

/// Read a result file inside the library without following links out of it.
fn read_job_output_file(output_path: &Path, library: &Path) -> OutputRead {
    let (text, modified) = match read_library_file(output_path, library, MAX_JOB_OUTPUT_BYTES) {
        Ok(read) => read,
        Err(true) => return OutputRead::Missing,
        Err(false) => return OutputRead::Invalid,
    };
    match serde_json::from_str::<Value>(&text)
        .ok()
        .as_ref()
        .and_then(parse_job_output)
    {
        Some(output) => OutputRead::Valid(output, modified),
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

/// Flags each clip that the editor can open, and those with edits not yet exported.
/// Clips from runs before the editor have no edit file (or kept brief/source).
pub fn mark_editable(run_dir: &Path, output: &mut Value) {
    let has_request = run_dir.join(clip_engine::pipeline::REQUEST_FILE).is_file();
    for clip in output["clips"].as_array_mut().into_iter().flatten() {
        let index = clip["clip_index"].as_u64().unwrap_or(u64::MAX);
        let editable = has_request
            && run_dir
                .join(format!("clip_{index:02}.project.json"))
                .is_file();
        clip["editable"] = Value::Bool(editable);
        let unexported =
            editable && crate::editor::unexported_file(run_dir, index as u32).is_file();
        clip["unexported"] = Value::Bool(unexported);
    }
}

pub fn get_job_output(run_dir: &Path, library: &Path) -> Option<Value> {
    match read_job_output(run_dir, library) {
        OutputRead::Valid(mut output, modified) => {
            mark_editable(run_dir, &mut output);
            // The Rust engine didn't write created_at at first; date it like the library card.
            if !output["created_at"].is_string() {
                let record = run_dir
                    .file_name()
                    .and_then(|n| read_run_record(library, &n.to_string_lossy()));
                output["created_at"] =
                    Value::from(record.map_or_else(|| iso_from(modified), |r| r.started_at));
            }
            Some(output)
        }
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

        // Only clips with an edit file (and the run's kept request) can open in the editor.
        let base = base.canonicalize().unwrap();
        let run = base.join(&done);
        std::fs::write(run.join("result.json"), r#"{"clips":[
            {"clip_index":0,"s3_url":"clip_00.mp4","duration_ms":1000,"start_time_ms":0,"end_time_ms":1000,"virality_score":0},
            {"clip_index":1,"s3_url":"clip_01.mp4","duration_ms":1000,"start_time_ms":0,"end_time_ms":1000,"virality_score":0}]}"#).unwrap();
        let editable = |output: Value| {
            output["clips"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["editable"] == true)
                .collect::<Vec<_>>()
        };
        std::fs::write(run.join("clip_01.project.json"), "{}").unwrap();
        assert_eq!(
            editable(get_job_output(&run, &base).unwrap()),
            [false, false]
        );
        std::fs::write(run.join(clip_engine::pipeline::REQUEST_FILE), "{}").unwrap();
        assert_eq!(
            editable(get_job_output(&run, &base).unwrap()),
            [false, true]
        );
        // Runs without created_at get the run's start (or the result's modified time).
        assert!(get_job_output(&run, &base).unwrap()["created_at"]
            .as_str()
            .unwrap()
            .starts_with("20"));
        crate::run_history::create_run_record(&base, &done, "/v.mp4").unwrap();
        let started = read_run_record(&base, &done).unwrap().started_at;
        assert_eq!(
            get_job_output(&run, &base).unwrap()["created_at"],
            started.as_str()
        );
        std::fs::remove_dir_all(&base).unwrap();
    }
}
