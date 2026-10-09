//! The clip editor's files: `clip_NN.project.json` (the edit) and the run's
//! `request.json` (the kept source video and the campaign brief), both written by
//! the engine when the run finished (see `clip_engine::pipeline`).
use crate::file_manager::read_library_file;
use crate::security::{assert_absolute_path, is_within_directory};
use clip_engine::pipeline::REQUEST_FILE;
use clip_engine::render::project::Project;
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

const MAX_PROJECT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_REQUEST_BYTES: u64 = 1024 * 1024;
const MAX_CLIP_INDEX: u32 = 999;

/// What the editor opens.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorProject {
    pub project: Project,
    /// The source is still on disk (it may have been cleaned up or moved).
    pub source_available: bool,
    /// The source video, for the editor's instant (approximate) preview while dragging.
    pub source_path: Option<PathBuf>,
    pub source_duration_ms: i64,
    /// The campaign brief, shown next to the timeline.
    pub requirements: Option<String>,
    /// Saved edits the library's clip file doesn't have yet (see `unexported_file`).
    pub unexported: bool,
}

/// The run folder, checked to be a real folder inside the library.
pub fn run_dir(output_dir: &str, library: &Path) -> Result<PathBuf, String> {
    let dir = assert_absolute_path(output_dir)?;
    let meta = std::fs::symlink_metadata(&dir)
        .map_err(|_| "This run is no longer available".to_string())?;
    if !meta.is_dir() || meta.file_type().is_symlink() || !is_within_directory(&dir, library) {
        return Err("This run is outside the library".into());
    }
    Ok(dir)
}

/// A file the editor keeps in the run folder (an imported sound), by bare name.
pub fn run_file(run: &Path, name: &str) -> Result<PathBuf, String> {
    let named = Path::new(name);
    if name.starts_with('.') || named.file_name() != Some(named.as_os_str()) {
        return Err("Invalid file".into());
    }
    let path = run.join(named);
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => Ok(path),
        _ => Err("This sound is no longer in the run folder".into()),
    }
}

/// Present while a clip has saved edits its exported file lacks: saving an edit that
/// differs from the last export writes it, exporting removes it.
pub fn unexported_file(run: &Path, clip_index: u32) -> PathBuf {
    run.join(format!("clip_{clip_index:02}.unexported"))
}

/// The fingerprint of the edit last exported, beside the clip.
fn exported_file(run: &Path, clip_index: u32) -> PathBuf {
    run.join(format!("clip_{clip_index:02}.exported"))
}

fn fingerprint(project: &Project) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(project)
        .unwrap_or_default()
        .hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// The clip file now has this edit.
pub fn mark_exported(
    output_dir: &str,
    clip_index: u32,
    project: &Project,
    library: &Path,
) -> Result<(), String> {
    let run = run_dir(output_dir, library)?;
    crate::paths::write_atomic(
        &exported_file(&run, clip_index),
        fingerprint(project).as_bytes(),
    )
    .map_err(|e| e.to_string())?;
    match std::fs::remove_file(unexported_file(&run, clip_index)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
        _ => Ok(()),
    }
}

pub fn project_file(run: &Path, clip_index: u32) -> Result<PathBuf, String> {
    if clip_index > MAX_CLIP_INDEX {
        return Err("Invalid clip".into());
    }
    Ok(run.join(format!("clip_{clip_index:02}.project.json")))
}

/// The kept source (a file name in the run folder, or the user's own file) and its length.
pub fn source_of(
    run: &Path,
    library: &Path,
) -> Result<(Option<PathBuf>, i64, Option<String>), String> {
    let (text, _) = read_library_file(&run.join(REQUEST_FILE), library, MAX_REQUEST_BYTES)
        .map_err(|_| "This clip was made before clips could be edited.".to_string())?;
    let saved: Value = serde_json::from_str(&text)
        .map_err(|_| "This run's edit settings are damaged.".to_string())?;
    let duration_ms = (saved["source"]["metadata"]["duration_seconds"]
        .as_f64()
        .unwrap_or(0.0)
        * 1000.0)
        .round() as i64;
    let requirements = saved["request"]["requirements"]
        .as_str()
        .map(str::to_string)
        .filter(|r| !r.trim().is_empty());
    let path = saved["source"]["path"].as_str().and_then(|name| {
        let named = Path::new(name);
        if saved["source"]["local"].as_bool() == Some(true) {
            named.is_absolute().then(|| named.to_path_buf())
        } else {
            // A kept download is a bare file name inside this run folder.
            (named.file_name() == Some(named.as_os_str())).then(|| run.join(named))
        }
    });
    Ok((path.filter(|p| p.is_file()), duration_ms, requirements))
}

pub fn load(output_dir: &str, clip_index: u32, library: &Path) -> Result<EditorProject, String> {
    let run = run_dir(output_dir, library)?;
    let (text, _) = read_library_file(&project_file(&run, clip_index)?, library, MAX_PROJECT_BYTES)
        .map_err(|missing| {
            if missing {
                "This clip was made before clips could be edited.".to_string()
            } else {
                "This clip's edit file can't be read.".to_string()
            }
        })?;
    let mut project: Project =
        serde_json::from_str(&text).map_err(|_| "This clip's edit file is damaged.".to_string())?;
    // Edits saved before logos were stored by name point at the user's original file:
    // use the run's copy (logo_N.<ext>, written with request.json) instead.
    for (i, logo) in project.logos.iter_mut().enumerate() {
        let named = Path::new(&logo.path);
        if named.file_name() != Some(named.as_os_str()) {
            let ext = named
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("png")
                .to_ascii_lowercase();
            logo.path = format!("logo_{i}.{ext}");
        }
    }
    let (source, source_duration_ms, requirements) = source_of(&run, library)?;
    let unexported = unexported_file(&run, clip_index).is_file();
    Ok(EditorProject {
        project,
        source_available: source.is_some(),
        source_path: source,
        source_duration_ms,
        requirements,
        unexported,
    })
}

/// Save an edit; returns whether the library's clip file still lacks it.
pub fn save(
    output_dir: &str,
    clip_index: u32,
    project: &Project,
    library: &Path,
) -> Result<bool, String> {
    let run = run_dir(output_dir, library)?;
    let path = project_file(&run, clip_index)?;
    if !path.is_file() {
        return Err("This clip can't be edited.".into());
    }
    let (_, source_duration_ms, _) = source_of(&run, library)?;
    project
        .validate(source_duration_ms)
        .map_err(|e| e.to_string())?;
    let bytes =
        serde_json::to_vec_pretty(project).map_err(|_| "Could not save the edit".to_string())?;
    crate::paths::write_atomic(&path, &bytes).map_err(|_| "Could not save the edit".to_string())?;
    // An autosave landing after an export, with the same edit, keeps the clip "exported".
    let exported = std::fs::read_to_string(exported_file(&run, clip_index)).ok();
    let unexported = exported.as_deref() != Some(fingerprint(project).as_str());
    let marker = unexported_file(&run, clip_index);
    if unexported {
        crate::paths::write_atomic(&marker, b"")
            .map_err(|_| "Could not save the edit".to_string())?;
    } else {
        let _ = std::fs::remove_file(&marker);
    }
    Ok(unexported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clip_engine::render::project::VideoItem;

    const RUN: &str = "0b7c2a52-6a0e-4a57-9d33-2f3a1c0e7d11";

    fn library() -> PathBuf {
        let base = std::env::temp_dir().join(format!("clipward-editor-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(base.join(RUN)).unwrap();
        base.canonicalize().unwrap()
    }

    fn project() -> Project {
        serde_json::from_value(serde_json::json!({
            "version": 1, "source_width": 320, "source_height": 240, "width": 1080, "height": 1920,
            "fps": "30", "aspect_ratio": "9:16",
            "video": [{ "id": "v0", "src_in_ms": 1000, "src_out_ms": 4000, "framing": "auto", "volume": 1.0 }],
            "shots": [], "face_samples": [], "transcript": [], "captions": true, "caption_preset": "pop",
            "emphasis_words": [], "title": null, "banner_platform": null, "banner_channel_url": null,
            "logos": [], "speed": 1.0
        }))
        .unwrap()
    }

    #[test]
    fn loads_and_saves_a_clip_edit_with_its_source_and_brief() {
        let lib = library();
        let run = lib.join(RUN);
        std::fs::write(run.join("source.mp4"), b"video").unwrap();
        std::fs::write(
            run.join(REQUEST_FILE),
            serde_json::to_vec(&serde_json::json!({
                "request": { "requirements": "Say the brand name" },
                "source": { "path": "source.mp4", "local": false, "metadata": { "duration_seconds": 10.0 } }
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            run.join("clip_00.project.json"),
            serde_json::to_vec(&project()).unwrap(),
        )
        .unwrap();
        let dir = run.to_string_lossy().into_owned();

        let opened = load(&dir, 0, &lib).unwrap();
        assert!(opened.source_available);
        assert_eq!(opened.source_duration_ms, 10_000);
        assert_eq!(opened.requirements.as_deref(), Some("Say the brand name"));

        assert!(!opened.unexported, "a clip nobody edited is as exported");

        let mut edited = opened.project;
        edited.video.push(VideoItem {
            id: "v1".into(),
            src_in_ms: 6000,
            src_out_ms: 7000,
            framing: "fit".into(),
            volume: 0.5,
            ..Default::default()
        });
        assert!(
            save(&dir, 0, &edited, &lib).unwrap(),
            "an edit the clip file lacks"
        );
        assert_eq!(load(&dir, 0, &lib).unwrap().project.video.len(), 2);
        assert!(load(&dir, 0, &lib).unwrap().unexported);

        // Exported; an autosave of that same edit landing afterwards keeps it exported.
        mark_exported(&dir, 0, &edited, &lib).unwrap();
        assert!(!unexported_file(&run, 0).exists());
        assert!(!save(&dir, 0, &edited, &lib).unwrap());
        assert!(!load(&dir, 0, &lib).unwrap().unexported);
        // A new change is unexported again.
        edited.video[1].volume = 1.0;
        assert!(save(&dir, 0, &edited, &lib).unwrap());
        edited.video[1].volume = 0.5;

        // Past the end of the source: rejected, and the saved edit is untouched.
        edited.video[1].src_out_ms = 12_000;
        assert!(save(&dir, 0, &edited, &lib).is_err());
        assert_eq!(
            load(&dir, 0, &lib).unwrap().project.video[1].src_out_ms,
            7000
        );

        // Clips without an edit file (older runs), and folders outside the library.
        assert!(load(&dir, 1, &lib)
            .unwrap_err()
            .contains("before clips could be edited"));
        assert!(load(&std::env::temp_dir().to_string_lossy(), 0, &lib).is_err());
        let _ = std::fs::remove_dir_all(&lib);
    }

    #[test]
    fn a_kept_source_must_be_a_file_in_the_run_folder() {
        let lib = library();
        let run = lib.join(RUN);
        std::fs::write(
            run.join(REQUEST_FILE),
            br#"{"source": {"path": "../elsewhere.mp4", "local": false, "metadata": {"duration_seconds": 1}}}"#,
        )
        .unwrap();
        std::fs::write(lib.join("elsewhere.mp4"), b"x").unwrap();
        assert_eq!(source_of(&run, &lib).unwrap().0, None);
        let _ = std::fs::remove_dir_all(&lib);
    }
}
