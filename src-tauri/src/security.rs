//! Path and URL checks.
//!
//! The renderer can only name media files that are inside the library (the output
//! folder) or that the user picked with a native dialog in this session; every
//! other path is refused before it is read, served, opened or copied.
use crate::shared::TRUSTED_EXTERNAL_URLS;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "avi", "mov", "flv", "jpg", "jpeg", "png", "webp",
];

pub fn is_web_url(value: &str) -> bool {
    match url::Url::parse(value) {
        Ok(url) => {
            matches!(url.scheme(), "http" | "https")
                && url.username().is_empty()
                && url.password().is_none()
        }
        Err(_) => false,
    }
}

pub fn is_trusted_external_url(value: &str) -> bool {
    is_web_url(value) && TRUSTED_EXTERNAL_URLS.contains(&value)
}

/// Whether `path` resolves to `directory` or somewhere inside it (symlinks resolved).
pub fn is_within_directory(path: &Path, directory: &Path) -> bool {
    match (
        std::fs::canonicalize(directory),
        std::fs::canonicalize(path),
    ) {
        (Ok(dir), Ok(target)) => target.starts_with(&dir),
        _ => false,
    }
}

pub fn has_media_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| MEDIA_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

pub fn assert_absolute_path(value: &str) -> Result<PathBuf, String> {
    if value.trim().is_empty() || value.contains('\0') || !Path::new(value).is_absolute() {
        return Err("An absolute path is required".into());
    }
    Ok(PathBuf::from(value))
}

/// Media files the user picked with a native dialog this session, by identity, so a
/// later swap of the path for another file is not accepted.
#[derive(Default)]
pub struct MediaRegistry {
    selected: Mutex<HashMap<PathBuf, file_id::FileId>>,
}

impl MediaRegistry {
    pub fn authorize(&self, path: &Path) -> Result<PathBuf, String> {
        let canonical =
            std::fs::canonicalize(path).map_err(|_| "Choose a supported media file".to_string())?;
        let meta = std::fs::metadata(&canonical)
            .map_err(|_| "Choose a supported media file".to_string())?;
        if !has_media_extension(&canonical) || !meta.is_file() {
            return Err("Choose a supported media file".into());
        }
        let id = file_id::get_file_id(&canonical)
            .map_err(|_| "Choose a supported media file".to_string())?;
        if let Ok(mut selected) = self.selected.lock() {
            selected.insert(canonical.clone(), id);
        }
        Ok(canonical)
    }

    /// The canonical path of a media file inside the library or picked this session.
    pub fn assert_media_path(
        &self,
        path: &str,
        output_directory: &Path,
    ) -> Result<PathBuf, String> {
        let invalid = || "Invalid media path".to_string();
        if path.contains('\0')
            || !Path::new(path).is_absolute()
            || !has_media_extension(Path::new(path))
        {
            return Err(invalid());
        }
        let outside = || "Media file is outside the library".to_string();
        let canonical = std::fs::canonicalize(path).map_err(|_| outside())?;
        let meta = std::fs::metadata(&canonical).map_err(|_| outside())?;
        let is_selected = self
            .selected
            .lock()
            .ok()
            .and_then(|selected| selected.get(&canonical).copied())
            .is_some_and(|id| file_id::get_file_id(&canonical).is_ok_and(|current| current == id));
        if !has_media_extension(&canonical)
            || !meta.is_file()
            || (!is_selected && !is_within_directory(&canonical, output_directory))
        {
            return Err(outside());
        }
        Ok(canonical)
    }

    /// Open the authorized file once so a later path replacement cannot change the
    /// bytes read. Returns the handle, its size and canonical path.
    pub fn open_authorized(
        &self,
        path: &str,
        output_directory: &Path,
    ) -> Result<(File, u64, PathBuf), String> {
        let canonical = self.assert_media_path(path, output_directory)?;
        let file = crate::paths::open_no_follow(&canonical)
            .map_err(|_| "Media unavailable".to_string())?;
        let opened = file
            .metadata()
            .map_err(|_| "Media unavailable".to_string())?;
        let current = std::fs::canonicalize(&canonical)
            .map_err(|_| "Media changed while opening".to_string())?;
        let current_str = current
            .to_str()
            .ok_or_else(|| "Invalid media path".to_string())?;
        self.assert_media_path(current_str, output_directory)?;
        let same = match (
            file.try_clone().map(same_file::Handle::from_file),
            same_file::Handle::from_path(&current),
        ) {
            (Ok(Ok(a)), Ok(b)) => a == b,
            _ => false,
        };
        if !opened.is_file() || current != canonical || !same {
            return Err("Media changed while opening".into());
        }
        Ok((file, opened.len(), canonical))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_urls() {
        assert!(is_web_url("https://example.com/a"));
        assert!(!is_web_url("https://user:pw@example.com"));
        assert!(!is_web_url("file:///etc/passwd"));
        assert!(is_trusted_external_url(
            "https://github.com/clipward/clipward"
        ));
        assert!(!is_trusted_external_url(
            "https://github.com/clipward/clipward/evil"
        ));
    }

    #[test]
    fn media_paths_are_confined() {
        let root = std::env::temp_dir().join(format!("clipward-sec-{}", uuid::Uuid::new_v4()));
        let library = root.join("library");
        std::fs::create_dir_all(&library).unwrap();
        let inside = library.join("a.mp4");
        let outside = root.join("b.mp4");
        std::fs::write(&inside, b"x").unwrap();
        std::fs::write(&outside, b"y").unwrap();
        let registry = MediaRegistry::default();
        assert!(registry
            .assert_media_path(inside.to_str().unwrap(), &library)
            .is_ok());
        assert!(registry
            .assert_media_path(outside.to_str().unwrap(), &library)
            .is_err());
        registry.authorize(&outside).unwrap();
        assert!(registry
            .assert_media_path(outside.to_str().unwrap(), &library)
            .is_ok());
        assert!(registry
            .assert_media_path(library.join("a.txt").to_str().unwrap(), &library)
            .is_err());
        let (_, size, _) = registry
            .open_authorized(inside.to_str().unwrap(), &library)
            .unwrap();
        assert_eq!(size, 1);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
