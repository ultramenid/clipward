//! Source videos kept in run folders so clips can be re-edited, and their cleanup.
//!
//! The engine moves a downloaded source to `<run>/source.<ext>` and writes
//! `request.json` (see `clip_engine::pipeline::save_edit_inputs`). Only those
//! copies are ever deleted here: a user's own local file is referenced, never kept.
use crate::run_history::UUID;
use clip_engine::pipeline::{REQUEST_FILE, SOURCE_STEM};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SourceUsage {
    pub bytes: u64,
    pub runs: u32,
}

/// Kept source files in the library, with the run id they belong to.
fn kept_sources(library: &Path) -> Vec<(String, PathBuf)> {
    let Ok(runs) = std::fs::read_dir(library) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for run in runs.flatten() {
        let id = run.file_name().to_string_lossy().into_owned();
        if !UUID.is_match(&id) || !run.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(files) = std::fs::read_dir(run.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            // file_type() does not follow symlinks: a link is never counted or deleted.
            if file.file_type().is_ok_and(|t| t.is_file())
                && path.file_stem().is_some_and(|stem| stem == SOURCE_STEM)
            {
                found.push((id.clone(), path));
            }
        }
    }
    found
}

pub fn usage(library: &Path) -> SourceUsage {
    let mut usage = SourceUsage::default();
    for (_, path) in kept_sources(library) {
        usage.bytes += std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        usage.runs += 1;
    }
    usage
}

/// Applies the Settings retention limit to the current library (off when sources are kept forever).
pub fn apply_retention(state: &crate::Core) {
    let Ok(settings) = state.settings.load() else {
        return;
    };
    if let Some(days) = settings.source_retention_days {
        let max_age = Duration::from_secs(u64::from(days) * 86_400);
        sweep(
            Path::new(&settings.output_directory),
            max_age,
            &state.jobs.live_ids(),
        );
    }
}

/// Deletes kept sources of runs finished longer than `max_age` ago (zero: all of them),
/// skipping runs still in progress. Returns how many were deleted.
pub fn sweep(library: &Path, max_age: Duration, live: &HashSet<String>) -> u32 {
    let now = SystemTime::now();
    let mut deleted = 0;
    for (id, path) in kept_sources(library) {
        if live.contains(&id) {
            continue;
        }
        // request.json is written when the run finishes; a source file's own time can be
        // the video's upload date.
        let finished = path
            .parent()
            .and_then(|run| std::fs::metadata(run.join(REQUEST_FILE)).ok())
            .and_then(|m| m.modified().ok());
        let old = match finished {
            Some(at) => now.duration_since(at).unwrap_or_default() >= max_age,
            None => true,
        };
        if old && std::fs::remove_file(&path).is_ok() {
            deleted += 1;
        }
    }
    deleted
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: &str = "0b7c2a52-6a0e-4a57-9d33-2f3a1c0e7d11";
    const LIVE: &str = "1b7c2a52-6a0e-4a57-9d33-2f3a1c0e7d11";

    #[test]
    fn sweep_deletes_only_kept_sources_of_finished_runs() {
        let base = std::env::temp_dir().join(format!("clipward-sources-{}", uuid::Uuid::new_v4()));
        struct Cleanup<'a>(&'a Path);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(self.0);
            }
        }
        let _cleanup = Cleanup(&base);
        let dir = &base;
        for id in [RUN, LIVE] {
            let run = dir.as_path().join(id);
            std::fs::create_dir_all(&run).unwrap();
            std::fs::write(run.join("source.mp4"), b"12345").unwrap();
            std::fs::write(run.join(REQUEST_FILE), b"{}").unwrap();
            std::fs::write(run.join("clip_00.mp4"), b"clip").unwrap();
        }
        std::fs::create_dir_all(dir.as_path().join("not-a-run")).unwrap();
        std::fs::write(dir.as_path().join("not-a-run/source.mp4"), b"x").unwrap();

        assert_eq!(usage(dir.as_path()), SourceUsage { bytes: 10, runs: 2 });
        let live = HashSet::from([LIVE.to_string()]);
        // Just finished: a 30-day limit keeps it.
        assert_eq!(
            sweep(dir.as_path(), Duration::from_secs(30 * 86_400), &live),
            0
        );
        assert_eq!(sweep(dir.as_path(), Duration::ZERO, &live), 1);
        assert!(!dir.as_path().join(RUN).join("source.mp4").exists());
        assert!(dir.as_path().join(RUN).join("clip_00.mp4").exists());
        assert!(dir.as_path().join(LIVE).join("source.mp4").exists());
        assert!(dir.as_path().join("not-a-run/source.mp4").exists());
    }
}
