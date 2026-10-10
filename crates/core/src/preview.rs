//! The editor's engine side: exact preview chunks and export. Both render through
//! `Renderer::render_project`, the export at full size and chunks scaled down, with the
//! same whole-mix loudness, so a chunk shows and sounds like that part of the export.
use crate::editor::{run_dir, source_of};
use crate::file_manager::read_library_file;
use crate::paths::AppPaths;
use crate::security::MediaRegistry;
use crate::tools::{resolve_binary, Tool};
use clip_engine::render::project::{Project, ProjectRender};
use clip_engine::render::service::video_codec_args;
use clip_engine::render::{scaled_duration_ms, Renderer};
use clip_engine::EngineConfig;
use serde_json::Value;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Edited time per preview chunk, ms.
pub const CHUNK_MS: i64 = 2_000;
/// Longer side of a preview chunk, px.
const PREVIEW_LONG_SIDE: i64 = 960;
const MAX_RESULT_BYTES: u64 = 16 * 1024 * 1024;
/// Chunk files kept, newest first: the editor renders the whole edit after each change,
/// about 0.25 MB per second of clip, so older edits' chunks go.
const MAX_CHUNK_BYTES: u64 = 400 * 1024 * 1024;
/// VideoToolbox bitrate for preview chunks (see `chunk`).
const PREVIEW_BITRATE: &str = "2M";

/// A chunk for an edit the user has since changed; the editor ignores it.
pub const SUPERSEDED: &str = "Superseded by a newer edit";

pub struct Previews {
    /// Per run: the newest edit's generation and the token that cancels its renders.
    latest: Mutex<HashMap<String, (u64, CancellationToken)>>,
    /// Loudness filter per (source, audio-relevant edit), measured once.
    loudness: Mutex<HashMap<u64, Option<String>>>,
    /// One loudness measurement at a time: the chunks asked for together (opening the
    /// editor asks for all of them) wait for it and share the result.
    measuring: tokio::sync::Mutex<()>,
    /// At most two chunk renders at once; later requests wait their turn.
    slots: tokio::sync::Semaphore,
}

impl Previews {
    pub fn new() -> Self {
        Self {
            latest: Mutex::default(),
            loudness: Mutex::default(),
            measuring: tokio::sync::Mutex::new(()),
            slots: tokio::sync::Semaphore::new(2),
        }
    }
}

fn key(parts: impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    parts.hash(&mut hasher);
    hasher.finish()
}

/// Scratch folder for chunks, emptied when the app starts.
pub fn preview_dir(user_data: &Path) -> std::io::Result<PathBuf> {
    crate::paths::private_dir(&user_data.join("preview"))
}

pub fn clear(user_data: &Path) {
    let _ = std::fs::remove_dir_all(user_data.join("preview"));
}

fn renderer(paths: &AppPaths, run: &Path) -> Renderer {
    Renderer::new(&EngineConfig {
        ffmpeg: resolve_binary(paths, Tool::Ffmpeg),
        ffprobe: resolve_binary(paths, Tool::Ffprobe),
        ytdlp: None,
        assets_dir: paths.assets_dir(),
        whisper_model_path: PathBuf::new(),
        ai: Default::default(),
        custom_vocabulary: Vec::new(),
        work_dir: run.to_path_buf(),
        output_dir: run.to_path_buf(),
    })
}

/// Preview size: the canvas scaled so its longer side is 960 px, even.
pub fn preview_size(project: &Project) -> (i64, i64) {
    let scale = PREVIEW_LONG_SIDE as f64 / project.width.max(project.height) as f64;
    let even = |v: i64| ((v as f64 * scale) as i64 / 2 * 2).max(2);
    (even(project.width), even(project.height))
}

/// A checked edit and its source, from the library.
fn open(library: &Path, output_dir: &str, project: &Project) -> Result<(PathBuf, PathBuf), String> {
    let run = run_dir(output_dir, library)?;
    let (source, duration_ms, _) = source_of(&run, library)?;
    let source = source.ok_or("This clip's source video is no longer available.")?;
    project.validate(duration_ms).map_err(|e| e.to_string())?;
    Ok((run, source))
}

async fn loudness(
    previews: &Previews,
    r: &Renderer,
    project: &Project,
    source: &Path,
    run: &Path,
) -> Result<Option<String>, String> {
    let id = key((
        source,
        serde_json::to_string(&(&project.video, project.speed, &project.audio)).unwrap_or_default(),
    ));
    let cached = || {
        previews
            .loudness
            .lock()
            .ok()
            .and_then(|m| m.get(&id).cloned())
    };
    if let Some(found) = cached() {
        return Ok(found);
    }
    let _measuring = previews.measuring.lock().await;
    if let Some(found) = cached() {
        return Ok(found);
    }
    let measured = r
        .measure_project_loudness(project, source, run, &CancellationToken::new())
        .await
        .map_err(|e| e.to_string())?;
    if let Ok(mut cache) = previews.loudness.lock() {
        cache.insert(id, measured.clone());
    }
    Ok(measured)
}

/// The cancel token for `generation` of a run's edit (the editor numbers its edits,
/// newest highest). A newer generation cancels the older one's renders.
fn generation_token(
    previews: &Previews,
    output_dir: &str,
    generation: u64,
) -> Result<CancellationToken, String> {
    let mut latest = previews
        .latest
        .lock()
        .map_err(|_| "Preview unavailable".to_string())?;
    let entry = latest
        .entry(output_dir.to_string())
        .or_insert_with(|| (generation, CancellationToken::new()));
    if generation < entry.0 {
        return Err(SUPERSEDED.into());
    }
    if generation > entry.0 {
        entry.1.cancel();
        *entry = (generation, CancellationToken::new());
    }
    Ok(entry.1.clone())
}

/// The exact preview of chunk `index` (edited time index*CHUNK_MS..), rendered once per
/// edit and reused while the edit is unchanged. Returns the file for the media protocol.
/// Renders for an older `generation` of the edit are dropped (see `generation_token`).
pub async fn chunk(
    (previews, paths, media): (&Previews, &AppPaths, &MediaRegistry),
    library: &Path,
    output_dir: &str,
    project: &Project,
    index: u32,
    generation: u64,
) -> Result<PathBuf, String> {
    let cancel = generation_token(previews, output_dir, generation)?;
    let (run, source) = open(library, output_dir, project)?;
    let start = index as i64 * CHUNK_MS;
    let total = project.duration_ms();
    if start >= total {
        return Err("Past the end of the edit".into());
    }
    let mut r = renderer(paths, &run);
    if r.videotoolbox {
        // VideoToolbox encodes at a fixed bitrate sized for the export (8 Mbps at
        // 1080×1920); a preview has a quarter of those pixels, so a quarter looks the same.
        let (w, h) = preview_size(project);
        let mut args = video_codec_args(true, w, h, &project.fps);
        if let Some(i) = args.iter().position(|a| a == "-b:v") {
            args[i + 1] = PREVIEW_BITRATE.into();
        }
        r.video_codec_override = Some(args);
    }
    let loudness = loudness(previews, &r, project, &source, &run).await?;
    let dir = preview_dir(&paths.user_data)
        .map_err(|_| "Could not create the preview folder".to_string())?;
    let id = key((
        &source,
        serde_json::to_string(project).unwrap_or_default(),
        index,
        &loudness,
    ));
    let path = dir.join(format!("{id:016x}.mp4"));
    // Reused: now the newest, so pruning keeps it.
    if let Ok(file) = std::fs::File::options().append(true).open(&path) {
        let _ = file.set_modified(std::time::SystemTime::now());
    }
    if !path.is_file() {
        let _slot = previews
            .slots
            .acquire()
            .await
            .map_err(|_| "Preview unavailable".to_string())?;
        if cancel.is_cancelled() {
            return Err(SUPERSEDED.into());
        }
        if !path.is_file() {
            let temp = dir.join(format!("{id:016x}.{}.mp4", uuid::Uuid::new_v4()));
            let job = ProjectRender {
                project,
                source: &source,
                output_path: &temp,
                range: Some((start, (start + CHUNK_MS).min(total))),
                scale_to: Some(preview_size(project)),
                loudness_filter: loudness,
                media_dir: &run,
                progress: None,
            };
            let rendered = r.render_project(&job, &cancel).await;
            if cancel.is_cancelled() {
                let _ = std::fs::remove_file(&temp);
                return Err(SUPERSEDED.into());
            }
            if let Err(e) = rendered
                .map_err(|e| e.to_string())
                .and_then(|_| std::fs::rename(&temp, &path).map_err(|e| e.to_string()))
            {
                let _ = std::fs::remove_file(&temp);
                return Err(format!("Preview failed: {e}"));
            }
            prune(&dir, MAX_CHUNK_BYTES);
        }
    }
    media.authorize(&path)
}

/// The whole edit as one file: its chunks (rendered if missing) joined without
/// re-encoding. Playing it never hops between chunk files, which stutters.
pub async fn joined(
    (previews, paths, media): (&Previews, &AppPaths, &MediaRegistry),
    library: &Path,
    output_dir: &str,
    project: &Project,
    generation: u64,
) -> Result<PathBuf, String> {
    let count = (project.duration_ms() + CHUNK_MS - 1) / CHUNK_MS;
    let mut parts = Vec::new();
    for index in 0..count {
        parts.push(
            chunk(
                (previews, paths, media),
                library,
                output_dir,
                project,
                index as u32,
                generation,
            )
            .await?,
        );
    }
    let dir = preview_dir(&paths.user_data)
        .map_err(|_| "Could not create the preview folder".to_string())?;
    // The chunks' names already identify the edit.
    let path = dir.join(format!("{:016x}.mp4", key(&parts)));
    if let Ok(file) = std::fs::File::options().append(true).open(&path) {
        let _ = file.set_modified(std::time::SystemTime::now());
    } else {
        let temp = dir.join(format!(
            "{}.{}.mp4",
            path.file_stem().unwrap_or_default().to_string_lossy(),
            uuid::Uuid::new_v4()
        ));
        let cancel = generation_token(previews, output_dir, generation)?;
        let made = clip_engine::render::project::join_chunks(
            &resolve_binary(paths, Tool::Ffmpeg),
            &parts,
            &temp,
            &cancel,
        )
        .await
        .map_err(|e| e.to_string())
        .and_then(|_| std::fs::rename(&temp, &path).map_err(|e| e.to_string()));
        if let Err(e) = made {
            let _ = std::fs::remove_file(&temp);
            return Err(if cancel.is_cancelled() {
                SUPERSEDED.into()
            } else {
                format!("Preview failed: {e}")
            });
        }
    }
    media.authorize(&path)
}

/// Deletes the least recently used chunk files past `max_bytes` in total.
fn prune(dir: &Path, max_bytes: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut chunks: Vec<_> = entries
        .flatten()
        // Finished chunks only (`{id}.mp4`), not one still being written (`{id}.{uuid}.mp4`).
        .filter(|e| {
            e.path().extension().is_some_and(|x| x == "mp4")
                && e.path()
                    .file_stem()
                    .is_some_and(|s| !s.to_string_lossy().contains('.'))
        })
        .filter_map(|e| {
            e.metadata()
                .ok()
                .filter(|m| m.is_file())
                .map(|m| (m.modified().ok(), m.len(), e.path()))
        })
        .collect();
    chunks.sort_by_key(|c| std::cmp::Reverse(c.0));
    let mut kept = 0;
    for (_, len, path) in chunks {
        kept += len;
        if kept > max_bytes {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The last `out_time_us` in an FFmpeg `-progress` file, ms.
fn progress_ms(text: &str) -> Option<i64> {
    text.lines()
        .rev()
        .find_map(|l| l.strip_prefix("out_time_us="))
        .and_then(|v| v.trim().parse::<i64>().ok())
        .map(|us| us / 1000)
}

/// Saves the edit, renders it over the clip (the old clip stays until the new one is
/// complete) and updates the clip's length in result.json. `on_progress` gets 0..1.
pub async fn export(
    (previews, paths): (&Previews, &AppPaths),
    library: &Path,
    output_dir: &str,
    clip_index: u32,
    project: &Project,
    on_progress: impl Fn(f64) + Send + 'static,
) -> Result<(), String> {
    crate::editor::save(output_dir, clip_index, project, library)?;
    let (run, source) = open(library, output_dir, project)?;
    let clip = run.join(format!("clip_{clip_index:02}.mp4"));
    let temp = run.join(format!(".clip_{clip_index:02}.exporting.mp4"));
    let r = renderer(paths, &run);
    let loudness = loudness(previews, &r, project, &source, &run).await?;
    let progress = run.join(format!(".clip_{clip_index:02}.progress"));
    let job = ProjectRender {
        project,
        source: &source,
        output_path: &temp,
        range: None,
        scale_to: None,
        loudness_filter: loudness,
        media_dir: &run,
        progress: Some(&progress),
    };
    // Report how far FFmpeg has got until the render ends.
    let length_ms = scaled_duration_ms(project.duration_ms(), project.speed)
        .map_err(|e| e.to_string())?
        .max(1);
    let done = CancellationToken::new();
    let watcher = {
        let (progress, done) = (progress.clone(), done.clone());
        tokio::spawn(async move {
            loop {
                // One more read once the render ends: FFmpeg's last write, which a poll can miss.
                let finished = done.is_cancelled();
                if let Some(ms) = std::fs::read_to_string(&progress)
                    .ok()
                    .as_deref()
                    .and_then(progress_ms)
                {
                    on_progress((ms as f64 / length_ms as f64).clamp(0.0, 1.0));
                }
                if finished {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        })
    };
    let rendered = r
        .render_project(&job, &CancellationToken::new())
        .await
        .map_err(|e| e.to_string());
    done.cancel();
    let _ = watcher.await;
    let _ = std::fs::remove_file(&progress);
    if let Err(e) = rendered.and_then(|_| std::fs::rename(&temp, &clip).map_err(|e| e.to_string()))
    {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("Export failed: {e}"));
    }
    let duration_ms =
        scaled_duration_ms(project.duration_ms(), project.speed).map_err(|e| e.to_string())?;
    update_result_duration(&run, library, clip_index, duration_ms)?;
    crate::editor::mark_exported(output_dir, clip_index, project, library)
}

/// The title and banner images for the editor to draw, with their automatic centres at
/// edited time `t_ms`. Written under a folder named for their content, so an unchanged
/// title keeps its file (and the editor its cached image).
pub fn overlay_previews(
    (paths, media): (&AppPaths, &MediaRegistry),
    library: &Path,
    output_dir: &str,
    project: &Project,
    t_ms: i64,
) -> Result<Vec<clip_engine::render::project::OverlayPreview>, String> {
    let run = run_dir(output_dir, library)?;
    let id = key((
        &project.title,
        &project.banner_platform,
        &project.banner_channel_url,
        project.width,
        project.height,
    ));
    let dir = preview_dir(&paths.user_data)
        .map_err(|_| "Could not create the preview folder".to_string())?
        .join(format!("overlays-{id:016x}"));
    std::fs::create_dir_all(&dir).map_err(|_| "Could not create the preview folder".to_string())?;
    let mut previews = renderer(paths, &run)
        .overlay_previews(project, &dir, t_ms)
        .map_err(|e| e.to_string())?;
    for p in &mut previews {
        p.path = media.authorize(&p.path)?;
    }
    Ok(previews)
}

/// Copies a logo the user picked into the run folder; returns its file name there.
pub fn import_logo(library: &Path, output_dir: &str, file: &Path) -> Result<String, String> {
    let run = run_dir(output_dir, library)?;
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !crate::shared::LOGO_EXTENSIONS.contains(&ext.as_str()) {
        return Err("Choose a PNG, JPEG or WebP image".into());
    }
    let name = format!(
        "logo_{}.{ext}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    std::fs::copy(file, run.join(&name)).map_err(|_| "Could not copy that image".to_string())?;
    Ok(name)
}

/// Imports a voiceover the editor recorded (`bytes`, a WebM or MP4 from MediaRecorder).
pub async fn import_recording(
    paths: &AppPaths,
    library: &Path,
    output_dir: &str,
    bytes: &[u8],
    ext: &str,
) -> Result<(String, i64), String> {
    let run = run_dir(output_dir, library)?;
    if !["webm", "mp4", "m4a", "ogg"].contains(&ext) {
        return Err("Unsupported recording".into());
    }
    let temp = run.join(format!(
        ".recording-{}.{ext}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::write(&temp, bytes).map_err(|_| "Could not save the recording".to_string())?;
    let imported = import_audio(paths, library, output_dir, &temp).await;
    let _ = std::fs::remove_file(&temp);
    imported
}

/// Imports a sound the user picked into the run folder (converted to .m4a) and returns
/// (file name, length ms).
pub async fn import_audio(
    paths: &AppPaths,
    library: &Path,
    output_dir: &str,
    file: &Path,
) -> Result<(String, i64), String> {
    let run = run_dir(output_dir, library)?;
    let name = format!(
        "audio_{}.m4a",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    let length = clip_engine::render::project::import_audio(
        &resolve_binary(paths, Tool::Ffmpeg),
        &resolve_binary(paths, Tool::Ffprobe),
        file,
        &run.join(&name),
        &CancellationToken::new(),
    )
    .await
    .map_err(|e| e.to_string())?;
    Ok((name, length))
}

/// Imports a video or image the user picked into the run folder as an .mp4 at the
/// project's frame rate (Renderer::import_media).
pub async fn import_media(
    paths: &AppPaths,
    library: &Path,
    output_dir: &str,
    file: &Path,
    fps: &str,
) -> Result<clip_engine::render::project::MediaFile, String> {
    let run = run_dir(output_dir, library)?;
    // The frame rate reaches FFmpeg's arguments: only "30" or "30000/1001" shapes.
    let valid = |p: &str| !p.is_empty() && p.len() <= 6 && p.bytes().all(|b| b.is_ascii_digit());
    let mut parts = fps.split('/');
    if !parts.next().is_some_and(valid)
        || !parts.next().is_none_or(valid)
        || parts.next().is_some()
        || fps.starts_with('0')
    {
        return Err("Invalid frame rate".into());
    }
    let file_name = format!(
        "media_{}.mp4",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    );
    let (width, height, duration_ms, image) = renderer(paths, &run)
        .import_media(file, &run.join(&file_name), fps, &CancellationToken::new())
        .await
        .map_err(|e| e.to_string())?;
    let name = file
        .file_stem()
        .map(|n| n.to_string_lossy().chars().take(200).collect())
        .unwrap_or_default();
    Ok(clip_engine::render::project::MediaFile {
        file: file_name,
        name,
        width,
        height,
        duration_ms,
        image,
    })
}

/// A sound's waveform (the source's when `file` is None), kept next to it as `<name>.peaks`.
pub async fn waveform(
    paths: &AppPaths,
    library: &Path,
    output_dir: &str,
    file: Option<&str>,
) -> Result<Vec<u8>, String> {
    let run = run_dir(output_dir, library)?;
    let sound = match file {
        Some(name) => crate::editor::run_file(&run, name)?,
        None => source_of(&run, library)?
            .0
            .ok_or("This clip's source video is no longer available.")?,
    };
    let cache = run.join(format!(
        "{}.peaks",
        file.unwrap_or(clip_engine::pipeline::SOURCE_STEM)
    ));
    if let Ok(peaks) = std::fs::read(&cache) {
        return Ok(peaks);
    }
    let peaks = clip_engine::render::project::waveform(
        &resolve_binary(paths, Tool::Ffmpeg),
        &sound,
        &CancellationToken::new(),
    )
    .await
    .map_err(|e| e.to_string())?;
    let _ = crate::paths::write_atomic(&cache, &peaks);
    Ok(peaks)
}

/// One `filmstrip` block of a video (the source's when `file` is None), kept next to it
/// as `<name>.strip<height>-<block>.jpg` (a new frame height makes new images). Returns the image's path.
pub async fn filmstrip(
    paths: &AppPaths,
    library: &Path,
    output_dir: &str,
    file: Option<&str>,
    block: u32,
) -> Result<PathBuf, String> {
    let run = run_dir(output_dir, library)?;
    let video = match file {
        Some(name) => crate::editor::run_file(&run, name)?,
        None => source_of(&run, library)?
            .0
            .ok_or("This clip's source video is no longer available.")?,
    };
    let height = clip_engine::render::project::FILMSTRIP_HEIGHT;
    let cache = run.join(format!(
        "{}.strip{height}-{block}.jpg",
        file.unwrap_or(clip_engine::pipeline::SOURCE_STEM)
    ));
    if !cache.is_file() {
        let temp = run.join(format!(".strip-{}.jpg", uuid::Uuid::new_v4()));
        let made = clip_engine::render::project::filmstrip(
            &resolve_binary(paths, Tool::Ffmpeg),
            &video,
            block,
            &temp,
            &CancellationToken::new(),
        )
        .await
        .map_err(|e| e.to_string())
        .and_then(|_| std::fs::rename(&temp, &cache).map_err(|e| e.to_string()));
        if let Err(e) = made {
            let _ = std::fs::remove_file(&temp);
            return Err(e);
        }
    }
    Ok(cache)
}

fn update_result_duration(
    run: &Path,
    library: &Path,
    clip_index: u32,
    duration_ms: i64,
) -> Result<(), String> {
    let path = run.join(clip_engine::pipeline::RESULT_FILE);
    let (text, _) = read_library_file(&path, library, MAX_RESULT_BYTES)
        .map_err(|_| "Could not update the library entry".to_string())?;
    let mut result: Value = serde_json::from_str(&text)
        .map_err(|_| "Could not update the library entry".to_string())?;
    let name = format!("clip_{clip_index:02}.mp4");
    if let Some(clip) = result["clips"].as_array_mut().and_then(|clips| {
        clips
            .iter_mut()
            .find(|c| c["s3_url"].as_str().is_some_and(|u| u.ends_with(&name)))
    }) {
        clip["duration_ms"] = duration_ms.into();
    }
    let bytes = serde_json::to_vec_pretty(&result)
        .map_err(|_| "Could not update the library entry".to_string())?;
    crate::paths::write_atomic(&path, &bytes)
        .map_err(|_| "Could not update the library entry".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_keeps_the_newest_chunks_and_ones_being_written() {
        let dir = std::env::temp_dir().join(format!("clipward-prune-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = |secs| std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        for (name, secs) in [
            ("old.mp4", 1),
            ("mid.mp4", 2),
            ("new.mp4", 3),
            ("new.tmp-uuid.mp4", 0),
        ] {
            std::fs::write(dir.join(name), [0u8; 10]).unwrap();
            std::fs::File::options()
                .append(true)
                .open(dir.join(name))
                .unwrap()
                .set_modified(t(secs))
                .unwrap();
        }
        prune(&dir, 20);
        let mut left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["mid.mp4", "new.mp4", "new.tmp-uuid.mp4"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A real run (from the engine's pipeline test flow) as a library entry. None when
    /// the fixture or the bundled ffmpeg isn't there.
    fn library_with_run() -> Option<(PathBuf, PathBuf, AppPaths)> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)?
            .to_path_buf();
        let fixture = std::env::var_os("CLIPWARD_EDIT_FIXTURE").map(PathBuf::from)?;
        if !root.join("engine-bin/ffmpeg").is_file()
            || !fixture.join("clip_00.project.json").is_file()
        {
            return None;
        }
        let base = std::env::temp_dir().join(format!("clipward-preview-{}", uuid::Uuid::new_v4()));
        let run = base
            .join("lib")
            .join("0b7c2a52-6a0e-4a57-9d33-2f3a1c0e7d11");
        std::fs::create_dir_all(&run).ok()?;
        for entry in std::fs::read_dir(&fixture).ok()?.flatten() {
            std::fs::copy(entry.path(), run.join(entry.file_name())).ok()?;
        }
        let paths = AppPaths {
            user_data: base.join("data"),
            logs: base.join("logs"),
            home: base.clone(),
            legacy_user_data: None,
            resources: None,
            dev_root: Some(root),
            packaged: false,
        };
        let lib = base.join("lib").canonicalize().ok()?;
        Some((
            lib.clone(),
            lib.join("0b7c2a52-6a0e-4a57-9d33-2f3a1c0e7d11"),
            paths,
        ))
    }

    #[test]
    fn filmstrip_images_a_block_once() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf();
        let ffmpeg = root.join("engine-bin/ffmpeg");
        if !ffmpeg.is_file() {
            eprintln!("skipped: engine-bin/ffmpeg missing");
            return;
        }
        let base = std::env::temp_dir().join(format!("clipward-strip-{}", uuid::Uuid::new_v4()));
        let run = base.join("lib").join("run");
        std::fs::create_dir_all(&run).unwrap();
        // 65 s of 16:9: block 1 is a 5 s tail, padded.
        let made = std::process::Command::new(&ffmpeg)
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=320x180:rate=5:duration=65",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(run.join("added.mp4"))
            .status()
            .unwrap();
        assert!(made.success());
        let paths = AppPaths {
            user_data: base.join("data"),
            logs: base.join("logs"),
            home: base.clone(),
            legacy_user_data: None,
            resources: None,
            dev_root: Some(root),
            packaged: false,
        };
        let lib = base.join("lib").canonicalize().unwrap();
        let dir = lib.join("run").to_string_lossy().into_owned();
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let strip = filmstrip(&paths, &lib, &dir, Some("added.mp4"), 1)
                .await
                .unwrap();
            assert_eq!(strip.file_name().unwrap(), "added.mp4.strip180-1.jpg");
            let bytes = std::fs::read(&strip).unwrap();
            // JPEG SOF0: 60 frames of 320x180 side by side.
            let sof = bytes.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
            let (h, w) = (
                u16::from_be_bytes([bytes[sof + 5], bytes[sof + 6]]),
                u16::from_be_bytes([bytes[sof + 7], bytes[sof + 8]]),
            );
            assert_eq!((w, h), (60 * 320, 180));
            let made = std::fs::metadata(&strip).unwrap().modified().unwrap();
            assert_eq!(
                filmstrip(&paths, &lib, &dir, Some("added.mp4"), 1)
                    .await
                    .unwrap(),
                strip
            );
            assert_eq!(
                std::fs::metadata(&strip).unwrap().modified().unwrap(),
                made,
                "cached, not redone"
            );
            assert!(filmstrip(&paths, &lib, &dir, Some("../added.mp4"), 0)
                .await
                .is_err());
        });
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Set CLIPWARD_EDIT_FIXTURE to a finished run folder (with source and project) to run.
    #[test]
    fn chunks_and_export_render_a_real_run() {
        let Some((lib, run, paths)) = library_with_run() else {
            eprintln!("skipped: set CLIPWARD_EDIT_FIXTURE to a finished run folder");
            return;
        };
        let (previews, media) = (Previews::new(), MediaRegistry::default());
        let dir = run.to_string_lossy().into_owned();
        // Opened as the editor opens it (older edit files get their logos moved to the run's copies).
        let mut project = crate::editor::load(&dir, 0, &lib).unwrap().project;
        assert!(
            project.logos.iter().all(|l| !l.path.contains('/')),
            "{:?}",
            project.logos
        );
        // Keep the first 3 s and play it second, after 2 s from later on.
        let first = project.video[0].clone();
        project.video = vec![
            clip_engine::render::project::VideoItem {
                id: "b".into(),
                src_in_ms: first.src_in_ms + 6_000,
                src_out_ms: first.src_in_ms + 8_000,
                ..first.clone()
            },
            clip_engine::render::project::VideoItem {
                src_out_ms: first.src_in_ms + 3_000,
                ..first
            },
        ];
        project.texts = vec![clip_engine::render::project::TextLayer {
            id: "t0".into(),
            text: "Zoo day!\nPart 2".into(),
            start_ms: 500,
            end_ms: 4_500,
            x: 0.5,
            y: 0.3,
            size: 96,
            preset: "spotlight".into(),
            animation_in: "typewriter".into(),
            animation_out: "fade".into(),
        }];
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            // Music: a tone imported from a WAV, ducked under the speech.
            let wav = lib.parent().unwrap().join("tone.wav");
            let made = std::process::Command::new(resolve_binary(&paths, Tool::Ffmpeg))
                .args([
                    "-v",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=600:duration=4",
                ])
                .arg(&wav)
                .status()
                .unwrap();
            assert!(made.success());
            let (file, length) = import_audio(&paths, &lib, &dir, &wav).await.unwrap();
            assert!((length - 4_000).abs() < 100, "{length}");
            assert!(
                waveform(&paths, &lib, &dir, Some(&file))
                    .await
                    .unwrap()
                    .len()
                    >= 78
            );
            assert!(waveform(&paths, &lib, &dir, None).await.unwrap().len() > 100);
            assert!(
                run.join(format!("{file}.peaks")).is_file() && run.join("source.peaks").is_file()
            );
            assert!(waveform(&paths, &lib, &dir, Some("../result.json"))
                .await
                .is_err());
            // A voiceover as WebKit's MediaRecorder saves it (MP4/AAC), passed as bytes.
            let clip = lib.parent().unwrap().join("vo.mp4");
            std::process::Command::new(resolve_binary(&paths, Tool::Ffmpeg))
                .args([
                    "-v",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=300:duration=2",
                    "-c:a",
                    "aac",
                ])
                .arg(&clip)
                .status()
                .unwrap();
            let (vo, vo_ms) =
                import_recording(&paths, &lib, &dir, &std::fs::read(&clip).unwrap(), "mp4")
                    .await
                    .unwrap();
            assert!(
                run.join(&vo).is_file() && (vo_ms - 2_000).abs() < 100,
                "{vo} {vo_ms}"
            );
            assert!(import_recording(&paths, &lib, &dir, b"x", "exe")
                .await
                .is_err());
            project.audio = vec![clip_engine::render::project::AudioItem {
                id: "a0".into(),
                name: "tone".into(),
                file: Some(file),
                src_in_ms: 0,
                src_out_ms: length,
                start_ms: 500,
                volume: 0.8,
                fade_in_ms: 300,
                fade_out_ms: 800,
                duck: true,
                voice: false,
                speed: 1.0,
            }];
            // Phase 5: a fast piece, a dip, a crop, a placed title and an added logo.
            use clip_engine::render::project::{Crop, LogoLayer, OverlayBox};
            project.video[0].speed = 1.5;
            project.video[1].transition_ms = 400;
            project.video[1].framing = "crop".into();
            project.video[1].crop = Some(Crop {
                x: 0.4,
                y: 0.5,
                zoom: 1.3,
            });
            project.title = Some("Zoo trip".into());
            project.title_box = Some(OverlayBox {
                x: 0.5,
                y: 0.2,
                scale: 1.2,
                start_ms: 0,
                end_ms: 2_000,
            });
            let png = lib.parent().unwrap().join("mark.png");
            std::process::Command::new(resolve_binary(&paths, Tool::Ffmpeg))
                .args([
                    "-v",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "color=blue:size=64x32",
                    "-frames:v",
                    "1",
                ])
                .arg(&png)
                .status()
                .unwrap();
            let logo = import_logo(&lib, &dir, &png).unwrap();
            assert!(run.join(&logo).is_file());
            project.logos.push(LogoLayer {
                path: logo,
                position: "top-right".into(),
                size: 0.15,
                placed: Some(OverlayBox {
                    x: 0.8,
                    y: 0.8,
                    scale: 1.0,
                    start_ms: 500,
                    end_ms: 3_000,
                }),
            });
            // An added video, shown over the clip (picture-in-picture).
            let broll = lib.parent().unwrap().join("broll.mov");
            std::process::Command::new(resolve_binary(&paths, Tool::Ffmpeg))
                .args([
                    "-v",
                    "error",
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc=size=320x180:rate=25:duration=1",
                ])
                .arg(&broll)
                .status()
                .unwrap();
            let added = import_media(&paths, &lib, &dir, &broll, &project.fps)
                .await
                .unwrap();
            assert!(
                run.join(&added.file).is_file() && (added.duration_ms - 1_000).abs() < 100,
                "{added:?}"
            );
            assert!(import_media(&paths, &lib, &dir, &broll, "30 -i x")
                .await
                .is_err());
            project.pip.push(clip_engine::render::project::PipItem {
                id: "p0".into(),
                media: added.file.clone(),
                src_in_ms: 0,
                src_out_ms: added.duration_ms,
                start_ms: 2_000,
                x: 0.5,
                y: 0.5,
                width: 0.4,
                volume: 1.0,
            });
            project.media.push(added);
            let drawn = overlay_previews((&paths, &media), &lib, &dir, &project, 0).unwrap();
            assert!(
                drawn.iter().any(|p| p.key == "title" && p.path.is_file()),
                "{drawn:?}"
            );
            let chunk = chunk((&previews, &paths, &media), &lib, &dir, &project, 1, 1)
                .await
                .unwrap();
            assert!(chunk.is_file());
            // Unchanged edit: the cached file comes back without rendering again (its
            // modified time is refreshed for pruning, so compare when it was created).
            let created = std::fs::metadata(&chunk).unwrap().created().unwrap();
            let again = chunk_again(&previews, &paths, &media, &lib, &dir, &project, 1).await;
            assert_eq!(again, chunk);
            assert_eq!(
                std::fs::metadata(&again).unwrap().created().unwrap(),
                created
            );
            // The whole edit as one file, as long as the edit.
            let whole = joined((&previews, &paths, &media), &lib, &dir, &project, 1)
                .await
                .unwrap();
            let probed = std::process::Command::new(resolve_binary(&paths, Tool::Ffprobe))
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "format=duration",
                    "-of",
                    "csv=p=0",
                ])
                .arg(&whole)
                .output()
                .unwrap();
            let seconds: f64 = String::from_utf8_lossy(&probed.stdout)
                .trim()
                .parse()
                .unwrap();
            let expected = project.duration_ms() as f64 / 1000.0 / project.speed;
            assert!(
                (seconds - expected).abs() < 0.1,
                "{seconds} s joined, {expected} s edit"
            );
            assert_eq!(
                joined((&previews, &paths, &media), &lib, &dir, &project, 1)
                    .await
                    .unwrap(),
                whole
            );
            // Previews are a quarter of the export's pixels, at a quarter of its bitrate.
            let bytes = std::fs::metadata(&chunk).unwrap().len();
            assert!(bytes < 1_000_000, "{bytes} bytes for 2 s");
            assert!(chunk_again(
                &previews,
                &paths,
                &media,
                &lib,
                &dir,
                &Project {
                    video: project.video[..1].to_vec(),
                    ..project.clone()
                },
                0
            )
            .await
            .is_file());

            let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
            let log = seen.clone();
            export((&previews, &paths), &lib, &dir, 0, &project, move |p| {
                log.lock().unwrap().push(p)
            })
            .await
            .unwrap();
            let seen = seen.lock().unwrap();
            assert!(
                !seen.is_empty()
                    && seen.windows(2).all(|w| w[0] <= w[1])
                    && *seen.last().unwrap() > 0.5,
                "{seen:?}"
            );
        });
        let result: Value =
            serde_json::from_slice(&std::fs::read(run.join("result.json")).unwrap()).unwrap();
        // 2 s at 1.5x plays for 1.333 s, then 3 s.
        assert_eq!(result["clips"][0]["duration_ms"], 4_333);
        let saved: Project =
            serde_json::from_slice(&std::fs::read(run.join("clip_00.project.json")).unwrap())
                .unwrap();
        assert_eq!(saved.video.len(), 2);
        assert!(!run.join(".clip_00.exporting.mp4").exists());
        assert_eq!(saved.texts.len(), 1);
        assert_eq!(saved.audio.len(), 1);
        // To look at the result: CLIPWARD_EDIT_KEEP=<path.mp4>.
        if let Ok(keep) = std::env::var("CLIPWARD_EDIT_KEEP") {
            std::fs::copy(run.join("clip_00.mp4"), keep).unwrap();
        }
        let _ = std::fs::remove_dir_all(lib.parent().unwrap());
    }

    async fn chunk_again(
        previews: &Previews,
        paths: &AppPaths,
        media: &MediaRegistry,
        lib: &Path,
        dir: &str,
        project: &Project,
        index: u32,
    ) -> PathBuf {
        chunk((previews, paths, media), lib, dir, project, index, 1)
            .await
            .unwrap()
    }

    #[test]
    fn progress_reads_the_last_out_time() {
        assert_eq!(
            progress_ms(
                "out_time_us=1000000\nprogress=continue\nout_time_us=2500000\nprogress=end\n"
            ),
            Some(2500)
        );
        assert_eq!(progress_ms("progress=continue\n"), None);
    }

    #[test]
    fn a_newer_edit_cancels_older_previews() {
        let previews = Previews::new();
        let first = generation_token(&previews, "/run", 5).unwrap();
        assert!(
            !generation_token(&previews, "/run", 5)
                .unwrap()
                .is_cancelled(),
            "same edit, same renders"
        );
        let newer = generation_token(&previews, "/run", 7).unwrap();
        assert!(first.is_cancelled() && !newer.is_cancelled());
        assert_eq!(
            generation_token(&previews, "/run", 6).unwrap_err(),
            SUPERSEDED
        );
        assert!(
            !generation_token(&previews, "/other-run", 1)
                .unwrap()
                .is_cancelled(),
            "runs are separate"
        );
    }

    #[test]
    fn preview_size_fits_960_and_stays_even() {
        let mut p: Project = serde_json::from_value(serde_json::json!({
            "version": 1, "source_width": 1, "source_height": 1, "width": 1080, "height": 1920, "fps": "30",
            "aspect_ratio": "9:16", "video": [], "shots": [], "face_samples": [], "transcript": [], "captions": false,
            "caption_preset": "pop", "emphasis_words": [], "title": null, "banner_platform": null,
            "banner_channel_url": null, "logos": [], "speed": 1.0
        }))
        .unwrap();
        assert_eq!(preview_size(&p), (540, 960));
        (p.width, p.height) = (2560, 1440);
        assert_eq!(preview_size(&p), (960, 540));
        (p.width, p.height) = (1918, 1082);
        assert_eq!(preview_size(&p), (960, 540));
    }
}
