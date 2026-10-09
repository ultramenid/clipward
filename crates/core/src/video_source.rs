//! Video links: Twitch VODs and the source picker's link check (port of
//! src/shared/video-source.ts and SourcePicker's isValidSourceLink). The Twitch
//! helpers live with the job validator, which applies the same rules.
pub use crate::validation::{
    normalize_video_source, twitch_source_error, twitch_vod_id, TWITCH_VOD_HINT,
};

/// A full HTTP(S) link with a host, no username or password, and not a Twitch page
/// other than a VOD.
pub fn is_valid_source_link(value: &str) -> bool {
    url::Url::parse(crate::format::js_trim(value)).is_ok_and(|url| {
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some_and(|host| !host.is_empty())
            && url.username().is_empty()
            && url.password().is_none()
            && twitch_source_error(value).is_none()
    })
}

/// Why the source picker refuses a link, or None to use it.
pub fn source_link_error(value: &str) -> Option<&'static str> {
    if is_valid_source_link(value) {
        return None;
    }
    Some(
        twitch_source_error(value)
            .unwrap_or("Paste a valid HTTP(S) link without a username or password."),
    )
}

/// What the source picker shows a chosen source as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceKind {
    Twitch,
    /// A YouTube link, with its video id (for the thumbnail).
    YouTube(String),
    Link,
    Local,
}

impl SourceKind {
    pub fn of(source: &str) -> Self {
        if !crate::format::is_url(source) {
            return Self::Local;
        }
        if twitch_vod_id(source).is_some() {
            return Self::Twitch;
        }
        crate::format::youtube_id(source).map_or(Self::Link, Self::YouTube)
    }

    /// The badge text.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Twitch => "Twitch VOD",
            Self::YouTube(_) => "YouTube",
            Self::Link => "Link",
            Self::Local => "Local file",
        }
    }
}

/// The preview card's title: host and path for a link, the file name for a file.
pub fn display_source(source: &str) -> String {
    if !crate::format::is_url(source) {
        return crate::format::basename(source).to_string();
    }
    match url::Url::parse(source) {
        Ok(url) => {
            let path = if url.path() == "/" { "" } else { url.path() };
            format!("{}{path}", url.host_str().unwrap_or_default())
        }
        Err(_) => "Video link".into(),
    }
}

/// A file dropped on the picker is taken when the picker would offer it.
pub fn is_video_file(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            crate::shared::VIDEO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
        })
}

/// A local source's length in seconds (ffprobe), for the preview's duration chip
/// and the manual clip checks; None when it can't be read. The file must be one
/// the user picked this session or in the library.
pub async fn local_duration(core: &crate::Core, path: &str) -> Result<Option<f64>, String> {
    use crate::tools::{command, resolve_binary, run_ok, Tool};
    let library = core.output_directory()?;
    let video = core.media.assert_media_path(path, &library)?;
    let mut cmd = command(resolve_binary(&core.paths, Tool::Ffprobe));
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
    let Some(stdout) = run_ok(cmd, std::time::Duration::from_secs(10)).await else {
        return Ok(None);
    };
    Ok(String::from_utf8_lossy(&stdout)
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|d| d.is_finite() && *d > 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_picker_accepts_full_http_links_and_rejects_malformed_or_credentialed_links() {
        assert!(is_valid_source_link(
            " https://www.youtube.com/watch?v=abc "
        ));
        assert!(is_valid_source_link("http://example.com/video.mp4"));
        assert!(!is_valid_source_link("https://"));
        assert!(!is_valid_source_link("https://user:pass@example.com/video"));
        assert!(!is_valid_source_link("file:///tmp/video.mp4"));
        assert_eq!(
            source_link_error("file:///tmp/video.mp4"),
            Some("Paste a valid HTTP(S) link without a username or password.")
        );
        assert_eq!(source_link_error("http://example.com/video.mp4"), None);
    }

    #[test]
    fn twitch_vod_links_canonicalize_while_other_twitch_pages_are_rejected() {
        for host in ["twitch.tv", "www.twitch.tv", "m.twitch.tv", "go.twitch.tv"] {
            let source = format!("https://{host}/videos/12345/?t=1h&tracking=secret");
            assert!(is_valid_source_link(&source));
            assert_eq!(
                normalize_video_source(&source),
                "https://www.twitch.tv/videos/12345"
            );
            assert_eq!(twitch_vod_id(&source).as_deref(), Some("12345"));
        }
        for source in [
            "https://twitch.tv/channel",
            "https://clips.twitch.tv/Clip",
            "https://player.twitch.tv/?video=123",
            "https://twitch.tv/videos/nope",
            "https://twitch.tv:8443/videos/123",
        ] {
            assert!(!is_valid_source_link(source), "{source}");
        }
        assert_eq!(
            source_link_error("https://twitch.tv/channel"),
            Some(TWITCH_VOD_HINT)
        );
        assert_eq!(
            twitch_vod_id("https://twitch.tv.evil.test/videos/123"),
            None
        );
    }
    #[test]
    fn the_preview_names_and_badges_each_kind_of_source() {
        assert_eq!(
            SourceKind::of("https://youtu.be/abc"),
            SourceKind::YouTube("abc".into())
        );
        assert_eq!(SourceKind::of("https://twitch.tv/videos/1"), SourceKind::Twitch);
        assert_eq!(SourceKind::of("https://example.com/a.mp4"), SourceKind::Link);
        assert_eq!(SourceKind::of("/Users/me/a.mp4").label(), "Local file");
        assert_eq!(display_source("https://example.com/"), "example.com");
        assert_eq!(display_source("https://www.example.com/v/1?x=2"), "www.example.com/v/1");
        assert_eq!(display_source("/Users/me/talk.mov"), "talk.mov");
        assert!(is_video_file(std::path::Path::new("/a/B.MOV")));
        assert!(!is_video_file(std::path::Path::new("/a/b.png")));
    }
}
