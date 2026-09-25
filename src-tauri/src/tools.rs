//! ffmpeg / ffprobe / yt-dlp discovery (port of src/main/tools.ts) and the helpers
//! for running them.
use crate::paths::AppPaths;
use regex::Regex;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

/// Directories a GUI-launched app misses: macOS apps start with a minimal PATH that
/// excludes Homebrew and per-user installers.
pub fn search_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    let extra: Vec<PathBuf> = if cfg!(windows) {
        vec![
            std::env::var_os("APPDATA")
                .map(|dir| PathBuf::from(dir).join("npm"))
                .unwrap_or_default(),
            home.join(".local").join("bin"),
            home.join(".bun").join("bin"),
        ]
    } else {
        vec![
            home.join(".local/bin"),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
            home.join(".npm-global/bin"),
            home.join(".bun/bin"),
            home.join(".opencode/bin"),
            home.join(".cursor/bin"),
            home.join(".grok/bin"),
        ]
    };
    dirs.extend(extra);
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|dir| !dir.as_os_str().is_empty() && seen.insert(dir.clone()));
    dirs
}

/// The process PATH extended with `search_dirs`, set once at startup so the
/// in-process engine (and any CLI it launches) finds per-user installs and their
/// helpers (node, etc.), as the Electron app passed to its Python bridge.
pub fn augment_process_path(home: &Path) {
    if let Ok(joined) = std::env::join_paths(search_dirs(home)) {
        // SAFETY-equivalent: called at startup before the app spawns worker threads.
        std::env::set_var("PATH", joined);
    }
}

const EXTENSIONS: &[&str] = if cfg!(windows) {
    &[".exe", ".cmd"]
} else {
    &[""]
};

/// First regular file named one of `names` in the search directories.
pub fn find_binary(home: &Path, names: &[&str]) -> Option<PathBuf> {
    for dir in search_dirs(home) {
        for name in names {
            for ext in EXTENSIONS {
                let candidate = dir.join(format!("{name}{ext}"));
                if std::fs::metadata(&candidate).is_ok_and(|meta| meta.is_file()) {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Ffmpeg,
    Ffprobe,
    YtDlp,
}

impl Tool {
    fn name(self) -> &'static str {
        match self {
            Tool::Ffmpeg => "ffmpeg",
            Tool::Ffprobe => "ffprobe",
            Tool::YtDlp => "yt-dlp",
        }
    }
}

/// Resolve a media tool. Prefer staged tools in development and bundled tools in
/// packaged builds; a damaged installation must fail, never execute a same-named
/// program from PATH.
pub fn resolve_binary(paths: &AppPaths, tool: Tool) -> PathBuf {
    let name = tool.name();
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    if let (false, Tool::YtDlp, Some(root)) = (paths.packaged, tool, &paths.dev_root) {
        let venv = root
            .join("engine")
            .join(".venv")
            .join(if cfg!(windows) { "Scripts" } else { "bin" })
            .join(&exe);
        if venv.exists() {
            return venv;
        }
    }
    let bundled = paths.bin_dir().join(&exe);
    if paths.packaged || bundled.exists() {
        return bundled;
    }
    find_binary(&paths.home, &[name]).unwrap_or_else(|| PathBuf::from(name))
}

/// Whether the tool can actually be found (an absolute path that exists).
pub fn tool_available(path: &Path) -> bool {
    path.is_absolute() && path.is_file()
}

/// A command that never opens a console window on Windows and dies with its task.
pub fn command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    command
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    command
}

/// Run a command to completion within `timeout`; Some(stdout) on a zero exit.
pub async fn run_ok(mut command: tokio::process::Command, timeout: Duration) -> Option<Vec<u8>> {
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .ok()?
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// Whether `program flag` runs and exits successfully within five seconds.
pub async fn check_runs(program: &Path, flag: &str) -> bool {
    let mut cmd = command(program);
    cmd.arg(flag);
    run_ok(cmd, Duration::from_secs(5)).await.is_some()
}

static ASS_FILTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*[A-Z.]{2,}\s+ass\s").unwrap());

/// Captioned renders require FFmpeg's libass-backed `ass` filter.
pub async fn supports_caption_filter(paths: &AppPaths) -> bool {
    let mut cmd = command(resolve_binary(paths, Tool::Ffmpeg));
    cmd.args(["-hide_banner", "-filters"]);
    match run_ok(cmd, Duration::from_secs(10)).await {
        Some(stdout) => String::from_utf8_lossy(&stdout)
            .lines()
            .any(|line| ASS_FILTER.is_match(line)),
        None => false,
    }
}
