//! Where Clipward keeps its files. Mirrors Electron's app.getPath('userData' | 'logs' |
//! 'home') and process.resourcesPath, plus the development-only isolation used by
//! end-to-end tests (CLIPWARD_USER_DATA_DIR).
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppPaths {
    /// Private app data: settings.json, whisper-models/, thumbnails/, work/.
    pub user_data: PathBuf,
    pub logs: PathBuf,
    pub home: PathBuf,
    /// Electron's userData folder, read once to migrate settings.
    pub legacy_user_data: Option<PathBuf>,
    /// Bundled resources (engine-bin/, assets/) in packaged builds.
    pub resources: Option<PathBuf>,
    /// Repository root in development builds.
    pub dev_root: Option<PathBuf>,
    pub packaged: bool,
}

/// The bundle identifier: app data and logs live in folders named after it.
pub const APP_ID: &str = "com.clipward.desktop";

impl AppPaths {
    /// The same folders Tauri's path resolver gives for `APP_ID`, so every shell shares
    /// one library, settings file and model folder. `resources` is the bundle's resource
    /// folder in packaged builds.
    pub fn resolve(packaged: bool, resources: Option<PathBuf>) -> Self {
        let home = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
        let mut user_data = dirs::data_dir()
            .map(|dir| dir.join(APP_ID))
            .unwrap_or_else(|| home.join(".clipward"));
        let mut logs = if cfg!(target_os = "macos") {
            home.join("Library").join("Logs").join(APP_ID)
        } else {
            dirs::data_local_dir()
                .map(|dir| dir.join(APP_ID).join("logs"))
                .unwrap_or_else(|| user_data.join("logs"))
        };
        let mut legacy_user_data = dirs::config_dir().map(|dir| dir.join("Clipward"));
        let mut home = home;

        // Development-only: isolated settings for end-to-end tests, so a test run
        // never touches the developer's real app data.
        if !packaged {
            if let Some(isolated) = std::env::var_os("CLIPWARD_USER_DATA_DIR").map(PathBuf::from) {
                if isolated.is_absolute() {
                    home = isolated.join("isolated-home");
                    let _ = std::fs::create_dir_all(&home);
                    logs = isolated.join("logs");
                    user_data = isolated;
                    legacy_user_data = None;
                }
            }
        }

        let resources = if packaged { resources } else { None };
        // crates/core → the repository root.
        let dev_root = (!packaged)
            .then(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .ancestors()
                    .nth(2)
                    .map(Path::to_path_buf)
            })
            .flatten();

        Self {
            user_data,
            logs,
            home,
            legacy_user_data,
            resources,
            dev_root,
            packaged,
        }
    }

    /// Directory holding the engine's fonts/ and models/.
    pub fn assets_dir(&self) -> PathBuf {
        match (&self.resources, &self.dev_root) {
            (Some(resources), _) => resources.join("assets"),
            (None, Some(root)) => root.join("crates").join("engine").join("assets"),
            (None, None) => PathBuf::from("assets"),
        }
    }

    /// Bundled ffmpeg / ffprobe / yt-dlp.
    pub fn bin_dir(&self) -> PathBuf {
        match (&self.resources, &self.dev_root) {
            (Some(resources), _) => resources.join("engine-bin"),
            (None, Some(root)) => root.join("engine-bin"),
            (None, None) => PathBuf::from("engine-bin"),
        }
    }
}

/// Create a private (0700) directory and refuse a symlink in its place.
pub fn private_dir(dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(std::io::Error::other("not a private directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir.to_path_buf())
}

/// Write a file atomically: a private temp file next to `path`, then rename over it.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent directory"))?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let temp = dir.join(format!("{name}.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = open_private_new(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

/// Create a new file (fails when it exists) readable only by this user.
pub fn open_private_new(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Open a file for reading without following a final symlink (where the OS allows it).
pub fn open_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options.open(path)
}

/// ISO-8601 UTC timestamp with milliseconds, like JavaScript's Date#toISOString.
pub fn iso_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn iso_from(time: std::time::SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
