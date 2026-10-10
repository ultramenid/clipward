//! Self-update from GitHub releases: find the latest published release, download
//! this platform's package, check it against the release's SHA256SUMS.txt, swap it
//! in, and relaunch.
//!
//! The packages aren't code-signed. A file the app downloads itself carries no
//! quarantine flag (macOS) or Mark of the Web (Windows), so the new version opens
//! without the Gatekeeper / SmartScreen warning the first install had.
//!
//! - macOS: the `.app` bundle is replaced whole (the DMG is mounted and copied).
//! - Windows / Linux: each file in the install folder is renamed to `*.old` and the
//!   new one moved in. Running files can be renamed, not overwritten; the `.old`
//!   leftovers are removed on the next launch (`cleanup`).
use crate::logger;
use crate::shared::REPO_URL;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const CHECKSUMS: &str = "SHA256SUMS.txt";
const OLD: &str = "old";

#[derive(Clone, Debug, PartialEq)]
pub struct Update {
    pub version: String,
    /// The release page, for the changelog.
    pub page_url: String,
    pub asset_name: String,
    pub asset_url: String,
    pub checksums_url: String,
    pub size: u64,
}

/// The end of this platform's package name (scripts/bundle*.sh).
fn asset_suffix() -> &'static str {
    let arm = cfg!(target_arch = "aarch64");
    if cfg!(target_os = "macos") {
        if arm { "_aarch64.dmg" } else { "_x64.dmg" }
    } else if cfg!(target_os = "windows") {
        if arm { "_windows_aarch64.zip" } else { "_windows_x64.zip" }
    } else if arm {
        "_linux_aarch64.tar.gz"
    } else {
        "_linux_x64.tar.gz"
    }
}

/// `x.y.z` as numbers; a leading `v` and anything after `-` or `+` are ignored.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let version = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(version)
}

pub fn is_newer(current: &str, latest: &str) -> bool {
    matches!((parse_version(current), parse_version(latest)), (Some(c), Some(l)) if l > c)
}

/// The update in a `releases/latest` response, if it is newer and has this
/// platform's package and the checksum list.
fn pick(release: &Value, current: &str) -> Option<Update> {
    let tag = release["tag_name"].as_str()?;
    if !is_newer(current, tag) || release["draft"].as_bool() == Some(true) {
        return None;
    }
    let assets = release["assets"].as_array()?;
    let url_of = |name: &str| {
        assets.iter().find(|a| a["name"].as_str() == Some(name)).and_then(|a| a["browser_download_url"].as_str())
    };
    let asset = assets
        .iter()
        .find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(asset_suffix())))?;
    let asset_name = asset["name"].as_str()?.to_string();
    Some(Update {
        version: tag.trim_start_matches('v').to_string(),
        page_url: release["html_url"].as_str().unwrap_or_default().to_string(),
        asset_url: url_of(&asset_name)?.to_string(),
        checksums_url: url_of(CHECKSUMS)?.to_string(),
        asset_name,
        size: asset["size"].as_u64().unwrap_or(0),
    })
}

fn client(timeout: Option<Duration>) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        // GitHub's API refuses requests without one.
        .user_agent(concat!("Clipward/", env!("CARGO_PKG_VERSION")));
    if let Some(timeout) = timeout {
        builder = builder.timeout(timeout);
    }
    builder.build().map_err(|_| "Could not check for updates".to_string())
}

/// The newer release for this platform, or None when this is the latest.
pub async fn check(current: &str) -> Result<Option<Update>, String> {
    let failed = || "Could not check for updates. Check your connection and try again.".to_string();
    // The repository must be public: GitHub answers 404 for private ones.
    let api = format!("https://api.github.com/repos/{}/releases/latest", REPO_URL.trim_start_matches("https://github.com/"));
    let response = client(Some(Duration::from_secs(20)))?
        .get(api)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|_| failed())?;
    if !response.status().is_success() {
        logger::warn("update.check_failed", json!({ "status": response.status().as_u16() }));
        return Err(failed());
    }
    let body = response.bytes().await.map_err(|_| failed())?;
    let release: Value = serde_json::from_slice(&body).map_err(|_| failed())?;
    Ok(pick(&release, current))
}

/// The hash listed for `name` in a `sha256sum` file.
fn listed_hash<'a>(sums: &'a str, name: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name).then_some(hash.trim())
    })
}

/// Downloads the package into `dir` and checks it against the release's checksum
/// list; returns its path. `on_progress(received, total)` runs about four times a second.
pub async fn download(update: &Update, dir: &Path, on_progress: impl Fn(u64, u64)) -> Result<PathBuf, String> {
    let failed = || "The update could not be downloaded. Check your connection and try again.".to_string();
    let _ = std::fs::remove_dir_all(dir);
    let dir = crate::paths::private_dir(dir).map_err(|_| failed())?;
    let client = client(None)?;
    let sums = client
        .get(&update.checksums_url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|_| failed())?
        .text()
        .await
        .map_err(|_| failed())?;
    let expected = listed_hash(&sums, &update.asset_name)
        .ok_or_else(|| "This release has no checksum for its package, so it wasn't installed.".to_string())?
        .to_ascii_lowercase();

    let mut response = client
        .get(&update.asset_url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|_| failed())?;
    let total = response.content_length().unwrap_or(update.size);
    let path = dir.join(&update.asset_name);
    let mut file = crate::paths::open_private_new(&path).map_err(|_| failed())?;
    let mut hasher = Sha256::new();
    let (mut received, mut last_report) = (0u64, Instant::now());
    while let Some(chunk) = response.chunk().await.map_err(|_| failed())? {
        file.write_all(&chunk).map_err(|_| failed())?;
        hasher.update(&chunk);
        received += chunk.len() as u64;
        if last_report.elapsed() > Duration::from_millis(250) {
            last_report = Instant::now();
            on_progress(received, total);
        }
    }
    file.sync_all().map_err(|_| failed())?;
    let actual: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if actual != expected {
        let _ = std::fs::remove_file(&path);
        logger::warn("update.checksum_mismatch", json!({ "asset": update.asset_name }));
        return Err("The downloaded update was damaged, so it wasn't installed. Try again.".into());
    }
    on_progress(total, total);
    Ok(path)
}

/// What gets replaced: the `.app` bundle on macOS, the install folder elsewhere.
/// None when this isn't an installed copy (a development build).
pub fn install_target() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let dir = exe.parent()?;
    if cfg!(target_os = "macos") {
        let app = dir.parent()?.parent()?;
        (dir.file_name()? == "MacOS" && app.extension()? == "app").then(|| app.to_path_buf())
    } else {
        dir.join("resources").join("engine-bin").is_dir().then(|| dir.to_path_buf())
    }
}

fn run(command: &mut Command) -> Result<(), String> {
    let output = command.output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Swaps in the downloaded package; returns what to launch (the new `.app` or
/// executable). On failure the current version stays in place.
pub fn install(package: &Path) -> Result<PathBuf, String> {
    let target = install_target().ok_or("Updates install only into the downloaded app.")?;
    let parent = target.parent().ok_or("The app's folder can't be found.")?.to_path_buf();
    let result = if cfg!(target_os = "macos") {
        install_bundle(package, &target, &parent)
    } else {
        install_folder(package, &target, &parent)
    };
    let _ = std::fs::remove_file(package);
    match result {
        Ok(launch) => {
            logger::info("update.installed", json!({}));
            Ok(launch)
        }
        Err(error) => {
            logger::warn("update.install_failed", json!({ "error": error }));
            Err(format!(
                "The update couldn't be installed, so the current version stays. Download it from the release page instead. ({error})"
            ))
        }
    }
}

fn install_bundle(dmg: &Path, app: &Path, parent: &Path) -> Result<PathBuf, String> {
    let name = app.file_name().ok_or("bad app path")?.to_string_lossy().to_string();
    let mount = dmg.with_extension("mount");
    let _ = std::fs::create_dir_all(&mount);
    run(Command::new("hdiutil").arg("attach").args(["-nobrowse", "-readonly", "-noautoopen", "-mountpoint"]).arg(&mount).arg(dmg))?;
    // The DMG holds Clipward.app; the installed copy may have been renamed.
    let staged = parent.join(format!(".{name}.update"));
    let _ = std::fs::remove_dir_all(&staged);
    let copied = run(Command::new("ditto").arg(mount.join("Clipward.app")).arg(&staged));
    let _ = run(Command::new("hdiutil").arg("detach").arg(&mount).arg("-force"));
    let _ = std::fs::remove_dir(&mount);
    copied?;
    let old = parent.join(format!(".{name}.{OLD}"));
    let _ = std::fs::remove_dir_all(&old);
    std::fs::rename(app, &old).map_err(|e| format!("{e}"))?;
    if let Err(e) = std::fs::rename(&staged, app) {
        let _ = std::fs::rename(&old, app);
        let _ = std::fs::remove_dir_all(&staged);
        return Err(e.to_string());
    }
    // The running copy keeps working from its open files.
    let _ = std::fs::remove_dir_all(&old);
    Ok(app.to_path_buf())
}

fn install_folder(archive: &Path, dir: &Path, parent: &Path) -> Result<PathBuf, String> {
    let staging = parent.join(".clipward-update");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    // bsdtar (Windows 10+) reads zips; GNU tar detects gzip.
    let result = run(Command::new("tar").arg("-xf").arg(archive).arg("-C").arg(&staging))
        .and_then(|()| replace_tree(&staging.join("Clipward"), dir));
    let _ = std::fs::remove_dir_all(&staging);
    result?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(dir.join(exe.file_name().ok_or("bad exe path")?))
}

fn old_name(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{OLD}"));
    PathBuf::from(name)
}

fn files(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    Ok(out)
}

/// Moves every file under `new` to the same place under `dir`, keeping the file it
/// replaces as `*.old`; puts everything back if a move fails.
// ponytail: files the new version dropped stay behind; delete them here if that starts to matter.
fn replace_tree(new: &Path, dir: &Path) -> Result<(), String> {
    if !new.join("resources").is_dir() {
        return Err("the package has an unexpected layout".into());
    }
    let mut done: Vec<(PathBuf, bool)> = Vec::new();
    let mut step = |file: &Path| -> std::io::Result<()> {
        let dest = dir.join(file.strip_prefix(new).map_err(std::io::Error::other)?);
        std::fs::create_dir_all(dest.parent().unwrap_or(dir))?;
        let existed = dest.exists();
        if existed {
            let old = old_name(&dest);
            let _ = std::fs::remove_file(&old);
            std::fs::rename(&dest, &old)?;
        }
        done.push((dest.clone(), existed));
        std::fs::rename(file, &dest)
    };
    let result = files(new).and_then(|list| list.iter().try_for_each(|file| step(file)));
    if let Err(error) = result {
        for (dest, existed) in done.into_iter().rev() {
            let _ = std::fs::remove_file(&dest);
            if existed {
                let _ = std::fs::rename(old_name(&dest), &dest);
            }
        }
        return Err(error.to_string());
    }
    Ok(())
}

/// Removes what an update left behind: the old bundle or `*.old` files, and the
/// downloaded package. Run at launch.
pub fn cleanup(downloads: &Path) {
    let _ = std::fs::remove_dir_all(downloads);
    let Some(target) = install_target() else { return };
    if cfg!(target_os = "macos") {
        if let (Some(parent), Some(name)) = (target.parent(), target.file_name()) {
            let name = name.to_string_lossy();
            let _ = std::fs::remove_dir_all(parent.join(format!(".{name}.{OLD}")));
            let _ = std::fs::remove_dir_all(parent.join(format!(".{name}.update")));
        }
    } else if let Ok(list) = files(&target) {
        for file in list.iter().filter(|f| f.extension().is_some_and(|e| e == OLD)) {
            let _ = std::fs::remove_file(file);
        }
    }
}

/// Starts the new version once this one has quit.
pub fn relaunch(launch: &Path) -> std::io::Result<()> {
    if cfg!(target_os = "macos") {
        // `$0` keeps the path out of the shell text.
        Command::new("/bin/sh").args(["-c", "sleep 1; open \"$0\""]).arg(launch).spawn()?;
    } else {
        Command::new(launch).spawn()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("0.2.0", "v0.2.1"));
        assert!(is_newer("0.2.9", "0.10.0"));
        assert!(!is_newer("0.2.0", "v0.2.0"));
        assert!(!is_newer("0.3.0", "v0.2.9"));
        assert!(!is_newer("0.2.0", "nightly"));
        assert!(is_newer("0.2.0", "v0.3.0-beta.1"));
    }

    #[test]
    fn picks_this_platforms_package() {
        let name = format!("Clipward_0.3.0{}", asset_suffix());
        let release = json!({
            "tag_name": "v0.3.0",
            "html_url": "https://github.com/clipward/clipward/releases/tag/v0.3.0",
            "assets": [
                { "name": "Clipward_0.3.0_other.bin", "browser_download_url": "https://x/other", "size": 1 },
                { "name": name, "browser_download_url": "https://x/pkg", "size": 42 },
                { "name": "SHA256SUMS.txt", "browser_download_url": "https://x/sums", "size": 1 },
            ],
        });
        let update = pick(&release, "0.2.0").expect("an update");
        assert_eq!((update.version.as_str(), update.asset_url.as_str(), update.size), ("0.3.0", "https://x/pkg", 42));
        assert_eq!(update.checksums_url, "https://x/sums");
        assert_eq!(pick(&release, "0.3.0"), None);
        // No checksum list: not offered.
        let mut bare = release.clone();
        bare["assets"].as_array_mut().unwrap().pop();
        assert_eq!(pick(&bare, "0.2.0"), None);
    }

    #[test]
    fn reads_sha256sum_lines() {
        let sums = "abc123  Clipward_0.3.0_aarch64.dmg\ndef456 *Clipward_0.3.0_windows_x64.zip\n";
        assert_eq!(listed_hash(sums, "Clipward_0.3.0_aarch64.dmg"), Some("abc123"));
        assert_eq!(listed_hash(sums, "Clipward_0.3.0_windows_x64.zip"), Some("def456"));
        assert_eq!(listed_hash(sums, "missing"), None);
    }

    /// Mounts a real DMG and swaps the bundle, as an update does.
    #[cfg(target_os = "macos")]
    #[test]
    fn swaps_the_app_bundle_from_a_dmg() {
        let root = std::env::temp_dir().join(format!("clipward-dmg-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (src, installed) = (root.join("src"), root.join("Applications"));
        for (base, text) in [(&src, "new"), (&installed, "old")] {
            std::fs::create_dir_all(base.join("Clipward.app/Contents/MacOS")).unwrap();
            std::fs::write(base.join("Clipward.app/Contents/MacOS/clipward-app"), text).unwrap();
        }
        let dmg = root.join("Clipward_9.9.9_aarch64.dmg");
        run(Command::new("hdiutil").args(["create", "-quiet", "-volname", "Clipward", "-fs", "HFS+", "-format", "UDZO", "-srcfolder"]).arg(&src).arg(&dmg)).unwrap();
        let app = installed.join("Clipward.app");
        assert_eq!(install_bundle(&dmg, &app, &installed).unwrap(), app);
        assert_eq!(std::fs::read_to_string(app.join("Contents/MacOS/clipward-app")).unwrap(), "new");
        assert!(!installed.join(".Clipward.app.old").exists() && !installed.join(".Clipward.app.update").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn replaces_files_and_rolls_back() {
        let root = std::env::temp_dir().join(format!("clipward-update-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (new, dir) = (root.join("new"), root.join("dir"));
        for (base, text) in [(&new, "new"), (&dir, "old")] {
            std::fs::create_dir_all(base.join("resources")).unwrap();
            std::fs::write(base.join("clipward"), text).unwrap();
            std::fs::write(base.join("resources/a.txt"), text).unwrap();
        }
        std::fs::write(new.join("resources/added.txt"), "new").unwrap();
        replace_tree(&new, &dir).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("clipward")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(dir.join("resources/a.txt")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(dir.join("resources/added.txt")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(dir.join("clipward.old")).unwrap(), "old");
        // A package without resources/ is refused and nothing moves.
        std::fs::remove_dir_all(&new).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        assert!(replace_tree(&new, &dir).is_err());
        assert_eq!(std::fs::read_to_string(dir.join("clipward")).unwrap(), "new");
        let _ = std::fs::remove_dir_all(&root);
    }
}
