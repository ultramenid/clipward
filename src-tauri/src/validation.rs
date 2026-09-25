//! Job option validation (port of src/main/validation.ts and src/shared/video-source.ts).
use crate::security::is_web_url;
use crate::shared::{DURATION_IDS, MAX_REQUIREMENTS_CHARS};
use clip_engine::ClipJobRequest;
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
    if let Some(banner) = &request.banner_channel_url {
        if !is_web_url(banner) || banner.encode_utf16().count() > 8192 {
            return Err("Invalid banner URL".into());
        }
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
    fn normalizes_twitch_vods() {
        assert_eq!(
            normalize_video_source("https://m.twitch.tv/videos/123/"),
            "https://www.twitch.tv/videos/123"
        );
        assert_eq!(twitch_vod_id("https://twitch.tv:8080/videos/1"), None);
    }
}
