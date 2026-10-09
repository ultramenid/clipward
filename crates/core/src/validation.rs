//! Job option validation.
use crate::security::is_web_url;
use crate::shared::{
    DURATION_IDS, LOGO_EXTENSIONS, LOGO_POSITIONS, MAX_BANNER_CHARS, MAX_LOGOS, MAX_MANUAL_CLIPS,
    MAX_MANUAL_CLIP_SECONDS, MAX_MANUAL_TITLE_CHARS, MAX_REQUIREMENTS_CHARS,
    MIN_MANUAL_CLIP_SECONDS,
};
use clip_engine::render::service::{MAX_LOGO_SIZE, MIN_LOGO_SIZE};
use clip_engine::{ClipJobRequest, ManualClip};
use regex::Regex;
use serde_json::Value;
use std::path::Path;
use std::sync::LazyLock;

pub const TWITCH_VOD_HINT: &str = "Choose a public, completed Twitch VOD using its video link. Live channels, collections and Twitch clips are not supported.";
const TWITCH_HOSTS: &[&str] = &["twitch.tv", "www.twitch.tv", "m.twitch.tv", "go.twitch.tv"];

static TWITCH_VOD_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/videos/([0-9]+)/?$").unwrap());
static SLUG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^[a-z0-9_-]{1,64}$").unwrap());

pub fn twitch_vod_id(value: &str) -> Option<String> {
    let url = url::Url::parse(value.trim()).ok()?;
    let host = url.host_str()?;
    if !TWITCH_HOSTS.contains(&host)
        || !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return None;
    }
    TWITCH_VOD_PATH
        .captures(url.path())
        .map(|captures| captures[1].to_string())
}

pub fn twitch_source_error(value: &str) -> Option<&'static str> {
    let url = url::Url::parse(value.trim()).ok()?;
    let host = url.host_str()?.to_lowercase();
    let host = host.trim_end_matches('.');
    ((host == "twitch.tv" || host.ends_with(".twitch.tv")) && twitch_vod_id(value).is_none())
        .then_some(TWITCH_VOD_HINT)
}

pub fn normalize_video_source(value: &str) -> String {
    match twitch_vod_id(value) {
        Some(id) => format!("https://www.twitch.tv/videos/{id}"),
        None => value.trim().to_string(),
    }
}

fn is_video_speed(value: f64) -> bool {
    value.is_finite() && (1.0..=2.0).contains(&value)
}

/// Validate a job request from the renderer and return it normalized.
pub fn validate_job_config(value: Value) -> Result<ClipJobRequest, String> {
    let object = value.as_object().ok_or("Invalid job options")?;
    let video_url = object
        .get("videoUrl")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if video_url.encode_utf16().count() > 8192
        || !(is_web_url(video_url) || Path::new(video_url).is_absolute())
    {
        return Err("Choose a video file or an HTTP(S) URL".into());
    }
    if let Some(error) = twitch_source_error(video_url) {
        return Err(error.into());
    }
    if !object.get("layoutVision").is_some_and(Value::is_boolean) {
        return Err("Invalid vision option".into());
    }
    if let Some(speed) = object.get("videoSpeed").filter(|speed| !speed.is_null()) {
        if !speed.as_f64().is_some_and(is_video_speed) {
            return Err("Video speed must be between 1× and 2×".into());
        }
    }
    if let Some(requirements) = object.get("requirements").filter(|r| !r.is_null()) {
        let ok = requirements.as_str().is_some_and(|text| {
            text.encode_utf16().count() <= MAX_REQUIREMENTS_CHARS && !text.contains('\0')
        });
        if !ok {
            return Err(format!(
                "Clip requirements must be at most {MAX_REQUIREMENTS_CHARS} characters"
            ));
        }
    }
    if let Some(max) = object.get("maxClips").filter(|max| !max.is_null()) {
        if !max.as_u64().is_some_and(|max| (1..=100).contains(&max)) {
            return Err("Clip count must be between 1 and 100".into());
        }
    }
    let mut request: ClipJobRequest =
        serde_json::from_value(value).map_err(|_| "Invalid job options".to_string())?;

    for (name, value, allowed) in [
        ("aspectRatio", &request.aspect_ratio, &["9:16", "16:9"][..]),
        (
            "layoutStyle",
            &request.layout_style,
            &["auto", "fill", "fit"][..],
        ),
        ("pacing", &request.pacing, &["tight", "natural"][..]),
    ] {
        if !allowed.contains(&value.as_str()) {
            return Err(format!("Invalid {name}"));
        }
    }
    if !SLUG.is_match(&request.caption_preset) {
        return Err("Invalid caption preset".into());
    }
    if let Some(ranges) = &request.duration_ranges {
        if ranges.len() > DURATION_IDS.len()
            || ranges.iter().any(|id| !DURATION_IDS.contains(&id.as_str()))
        {
            return Err("Invalid clip duration".into());
        }
    }
    for time in [request.start_time_seconds, request.end_time_seconds]
        .into_iter()
        .flatten()
    {
        if !time.is_finite() || time < 0.0 {
            return Err("Invalid trim time".into());
        }
    }
    if let Some(end) = request.end_time_seconds {
        if end <= request.start_time_seconds.unwrap_or(0.0) {
            return Err("Trim end must follow trim start".into());
        }
    }
    if let Some(platform) = &request.banner_platform {
        if !SLUG.is_match(platform) {
            return Err("Invalid banner platform".into());
        }
    }
    // The channel link is only drawn on the clip, never opened: plain text on one line.
    if let Some(banner) = request.banner_channel_url.take() {
        let banner = banner.trim();
        if banner.chars().count() > MAX_BANNER_CHARS || banner.chars().any(char::is_control) {
            return Err(format!(
                "The channel link must be at most {MAX_BANNER_CHARS} characters on one line"
            ));
        }
        request.banner_channel_url = (!banner.is_empty()).then(|| banner.to_string());
    }
    if request.logos.len() > MAX_LOGOS {
        return Err(format!("Add at most {MAX_LOGOS} logos"));
    }
    for (i, logo) in request.logos.iter().enumerate() {
        let n = i + 1;
        let path = Path::new(&logo.path);
        let image = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| LOGO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()));
        if !path.is_absolute() || logo.path.contains('\0') || !image {
            return Err(format!("Logo {n} must be a PNG, JPEG or WebP image"));
        }
        if !LOGO_POSITIONS.contains(&logo.position.as_str()) {
            return Err(format!("Logo {n} has an invalid position"));
        }
        if !(logo.size.is_finite() && (MIN_LOGO_SIZE..=MAX_LOGO_SIZE).contains(&logo.size)) {
            return Err(format!("Logo {n} has an invalid size"));
        }
    }
    if let Some(clips) = request.manual_clips.as_mut() {
        validate_manual_clips(clips)?;
        // Manual ranges are explicit and no AI provider runs: drop the trim and vision check.
        request.start_time_seconds = None;
        request.end_time_seconds = None;
        request.layout_vision = false;
    }
    request.video_url = normalize_video_source(&request.video_url);
    request.requirements = Some(
        request
            .requirements
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string(),
    );
    Ok(request)
}

/// Manual clip mode: 1-50 clips, each 3 s to 15 min with finite, non-negative times and an
/// optional short title. Overlaps are allowed. Titles are trimmed; empty ones become None.
fn validate_manual_clips(clips: &mut [ManualClip]) -> Result<(), String> {
    if clips.is_empty() {
        return Err("Add at least one clip".into());
    }
    if clips.len() > MAX_MANUAL_CLIPS {
        return Err(format!("Add at most {MAX_MANUAL_CLIPS} clips"));
    }
    for (i, clip) in clips.iter_mut().enumerate() {
        let n = i + 1;
        let (start, end) = (clip.start_seconds, clip.end_seconds);
        if !start.is_finite() || !end.is_finite() || start < 0.0 {
            return Err(format!("Clip {n} has an invalid time"));
        }
        if end <= start {
            return Err(format!("Clip {n} must end after it starts"));
        }
        let length = end - start;
        if !(MIN_MANUAL_CLIP_SECONDS..=MAX_MANUAL_CLIP_SECONDS).contains(&length) {
            return Err(format!(
                "Clip {n} must be between 3 seconds and 15 minutes long"
            ));
        }
        if let Some(title) = clip.title.take() {
            let title = title.trim();
            if title.chars().count() > MAX_MANUAL_TITLE_CHARS || title.chars().any(char::is_control)
            {
                return Err(format!(
                    "Clip {n} title must be at most {MAX_MANUAL_TITLE_CHARS} characters on one line"
                ));
            }
            clip.title = (!title.is_empty()).then(|| title.to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base() -> Value {
        json!({
            "videoUrl": "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "requirements": "  be fun  ",
            "maxClips": 3,
            "autoClipCount": false,
            "durationRanges": ["short"],
            "aspectRatio": "9:16",
            "layoutStyle": "auto",
            "layoutVision": true,
            "pacing": "tight",
            "includeCaptions": true,
            "captionPreset": "bold",
            "startTimeSeconds": null,
            "endTimeSeconds": null,
            "bannerPlatform": null,
            "bannerChannelUrl": null
        })
    }

    #[test]
    fn accepts_and_normalizes() {
        let request = validate_job_config(base()).unwrap();
        assert_eq!(request.requirements.as_deref(), Some("be fun"));
        assert_eq!(request.video_speed, 1.0);
    }

    #[test]
    fn rejects_bad_options() {
        let mut bad = base();
        bad["aspectRatio"] = json!("4:3");
        assert!(validate_job_config(bad).is_err());
        let mut bad = base();
        bad["maxClips"] = json!(0);
        assert!(validate_job_config(bad).is_err());
        let mut bad = base();
        bad["videoUrl"] = json!("relative/file.mp4");
        assert!(validate_job_config(bad).is_err());
        let mut bad = base();
        bad["videoUrl"] = json!("https://www.twitch.tv/somechannel");
        assert_eq!(validate_job_config(bad).unwrap_err(), TWITCH_VOD_HINT);
        let mut bad = base();
        bad["endTimeSeconds"] = json!(0);
        assert!(validate_job_config(bad).is_err());
        let mut bad = base();
        bad.as_object_mut().unwrap().remove("layoutVision");
        assert!(validate_job_config(bad).is_err());
    }

    #[test]
    fn validates_branding() {
        let mut ok = base();
        ok["bannerPlatform"] = json!("link");
        ok["bannerChannelUrl"] = json!("  youtube.com/@me  ");
        ok["logos"] = json!([{ "path": "/tmp/logo.png", "position": "top-right", "size": 0.18 }]);
        let request = validate_job_config(ok).unwrap();
        assert_eq!(
            request.banner_channel_url.as_deref(),
            Some("youtube.com/@me")
        );
        assert_eq!(request.logos.len(), 1);
        assert!(validate_job_config(base()).unwrap().logos.is_empty());

        for (field, value) in [
            ("bannerChannelUrl", json!("a\nb")),
            ("bannerChannelUrl", json!("x".repeat(MAX_BANNER_CHARS + 1))),
            (
                "logos",
                json!([{ "path": "logo.png", "position": "top-left", "size": 0.2 }]),
            ),
            (
                "logos",
                json!([{ "path": "/tmp/logo.gif", "position": "top-left", "size": 0.2 }]),
            ),
            (
                "logos",
                json!([{ "path": "/tmp/logo.png", "position": "middle", "size": 0.2 }]),
            ),
            (
                "logos",
                json!([{ "path": "/tmp/logo.png", "position": "top-left", "size": 0.9 }]),
            ),
            (
                "logos",
                json!(vec![
                    json!({ "path": "/tmp/l.png", "position": "top-left", "size": 0.2 });
                    MAX_LOGOS + 1
                ]),
            ),
        ] {
            let mut bad = base();
            bad[field] = value;
            assert!(validate_job_config(bad).is_err(), "{field}");
        }
    }

    #[test]
    fn older_payloads_have_no_manual_clips() {
        assert_eq!(validate_job_config(base()).unwrap().manual_clips, None);
        let mut null = base();
        null["manualClips"] = Value::Null;
        assert_eq!(validate_job_config(null).unwrap().manual_clips, None);
    }

    #[test]
    fn accepts_and_normalizes_manual_clips() {
        let mut manual = base();
        manual["startTimeSeconds"] = json!(10);
        manual["endTimeSeconds"] = json!(20);
        manual["manualClips"] = json!([
            { "startSeconds": 0, "endSeconds": 3, "title": "  Hook  " },
            { "startSeconds": 1.5, "endSeconds": 901.5, "title": "   " },
            { "startSeconds": 2, "endSeconds": 30 }
        ]);
        let request = validate_job_config(manual).unwrap();
        let clips = request.manual_clips.unwrap();
        assert_eq!(clips.len(), 3);
        assert_eq!(clips[0].title.as_deref(), Some("Hook"));
        assert_eq!(clips[1].title, None);
        assert_eq!(clips[2].title, None);
        // Explicit ranges replace the trim, and no AI vision runs.
        assert_eq!(
            (request.start_time_seconds, request.end_time_seconds),
            (None, None)
        );
        assert!(!request.layout_vision);
    }

    #[test]
    fn rejects_bad_manual_clips() {
        let with = |clips: Value| {
            let mut manual = base();
            manual["manualClips"] = clips;
            validate_job_config(manual)
        };
        let clip = |start: f64, end: f64| json!({ "startSeconds": start, "endSeconds": end });
        assert_eq!(with(json!([])).unwrap_err(), "Add at least one clip");
        assert_eq!(
            with(json!([clip(10.0, 10.0)])).unwrap_err(),
            "Clip 1 must end after it starts"
        );
        assert_eq!(
            with(json!([clip(0.0, 5.0), clip(10.0, 5.0)])).unwrap_err(),
            "Clip 2 must end after it starts"
        );
        assert!(with(json!([clip(0.0, 2.9)]))
            .unwrap_err()
            .contains("between 3 seconds and 15 minutes"));
        assert!(with(json!([clip(0.0, 900.5)]))
            .unwrap_err()
            .contains("between 3 seconds and 15 minutes"));
        assert_eq!(
            with(json!([clip(-1.0, 5.0)])).unwrap_err(),
            "Clip 1 has an invalid time"
        );
        assert!(with(json!([{ "startSeconds": "0", "endSeconds": 5 }])).is_err());
        assert!(with(json!((0..51)
            .map(|i| clip(i as f64, i as f64 + 5.0))
            .collect::<Vec<_>>()))
        .unwrap_err()
        .contains("at most 50"));
        assert!(with(json!((0..50)
            .map(|i| clip(i as f64, i as f64 + 5.0))
            .collect::<Vec<_>>()))
        .is_ok());
        let titled = |title: &str| json!([{ "startSeconds": 0, "endSeconds": 5, "title": title }]);
        assert!(with(titled(&"a".repeat(121)))
            .unwrap_err()
            .contains("at most 120"));
        assert!(with(titled(&"é".repeat(120))).is_ok());
        assert!(with(titled("line\nbreak")).is_err());
        assert!(with(titled("nul\u{0}")).is_err());
    }

    #[test]
    fn normalizes_twitch_vods() {
        assert_eq!(
            normalize_video_source("https://m.twitch.tv/videos/123/"),
            "https://www.twitch.tv/videos/123"
        );
        assert_eq!(twitch_vod_id("https://twitch.tv:8080/videos/1"), None);
    }
}
