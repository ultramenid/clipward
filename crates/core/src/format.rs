//! Text for numbers, times, dates and sources as the UI shows them (port of the pure
//! helpers in src/renderer/lib/utils.ts, the Timeline ruler, the editor clock and the
//! run stats' cost line). JavaScript's rounding and number text are reproduced where
//! they change what the user reads.
use crate::job_view::ApiCosts;
use crate::validation::twitch_vod_id;
use chrono::{DateTime, Datelike, Local, TimeZone};
use regex::Regex;
use std::fmt::Display;
use std::sync::LazyLock;

static PLAIN_SECONDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]+(\.[0-9]+)?$").unwrap());
static TIMECODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]+(:[0-9]{1,2}){1,2}(\.[0-9]+)?$").unwrap());
static YOUTUBE_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/(shorts|live|embed)/([^/?#]+)").unwrap());

/// `Math.round`: halves go up (-2.5 is -2), unlike `f64::round`.
pub fn js_round(value: f64) -> f64 {
    let floor = value.floor();
    if value - floor >= 0.5 {
        floor + 1.0
    } else {
        floor
    }
}

/// `Number.prototype.toFixed`: rounds the exact value half away from zero, where
/// Rust's `{:.n}` rounds an exact tie to even (1.25 is "1.3" here, "1.2" there).
pub fn to_fixed(value: f64, digits: usize) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    // ponytail: JS switches to exponent notation from 1e21; no caller gets near it.
    if value.is_infinite() || value.abs() >= 1e21 {
        return value.to_string();
    }
    // 30 more digits than kept: far finer than any gap between a double and a tie.
    let exact = format!("{:.*}", digits + 30, value.abs());
    let cut = exact.len() - 30;
    let mut units: u128 = exact[..cut].replace('.', "").parse().unwrap_or(0);
    if exact.as_bytes()[cut] >= b'5' {
        units += 1;
    }
    let text = format!("{units:0>width$}", width = digits + 1);
    let (whole, fraction) = text.split_at(text.len() - digits);
    let sign = if value < 0.0 { "-" } else { "" };
    if digits == 0 {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{fraction}")
    }
}

/// `String.prototype.trim`: Unicode white space and the BOM, but not U+0085.
pub fn js_trim(text: &str) -> &str {
    text.trim_matches(|c: char| c == '\u{feff}' || (c.is_whitespace() && c != '\u{85}'))
}

pub fn format_duration(ms: f64) -> String {
    let seconds = (ms / 1000.0).floor() as i64;
    let minutes = seconds.div_euclid(60);
    let remaining_seconds = seconds % 60;
    if minutes == 0 {
        format!("{remaining_seconds}s")
    } else {
        format!("{minutes}m {remaining_seconds}s")
    }
}

/// Timecode for a position in a video: 0:07, 12:34, 1:02:03.
pub fn format_timecode(ms: f64) -> String {
    let total = ((ms / 1000.0).floor() as i64).max(0);
    let (h, m, s) = (total / 3600, total % 3600 / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Parse "90", "1:30" or "1:02:03" into seconds. Empty input is None (no bound);
/// anything unparseable is NaN so callers can flag it.
pub fn parse_timecode(input: &str) -> Option<f64> {
    let value = js_trim(input);
    if value.is_empty() {
        return None;
    }
    let finite_or_nan = |seconds: f64| {
        if seconds.is_finite() {
            seconds
        } else {
            f64::NAN
        }
    };
    if PLAIN_SECONDS.is_match(value) {
        return Some(finite_or_nan(value.parse().unwrap_or(f64::NAN)));
    }
    if !TIMECODE.is_match(value) {
        return Some(f64::NAN);
    }
    let parts: Vec<f64> = value
        .split(':')
        .map(|part| part.parse().unwrap_or(f64::NAN))
        .collect();
    if parts[1..].iter().any(|part| *part >= 60.0) {
        return Some(f64::NAN);
    }
    Some(finite_or_nan(
        parts.iter().fold(0.0, |acc, part| acc * 60.0 + part),
    ))
}

pub fn format_bytes(bytes: u64) -> String {
    let size = bytes as f64;
    if size < 1024.0 {
        format!("{bytes} B")
    } else if size < 1024.0 * 1024.0 {
        format!("{} KB", to_fixed(size / 1024.0, 1))
    } else if size < 1024f64.powi(3) {
        format!("{} MB", to_fixed(size / (1024.0 * 1024.0), 1))
    } else {
        format!("{} GB", to_fixed(size / 1024f64.powi(3), 1))
    }
}

pub fn format_usd(amount: f64) -> String {
    if amount == 0.0 {
        "$0.00".into()
    } else if amount < 0.01 {
        format!("${}", to_fixed(amount, 4))
    } else {
        format!("${}", to_fixed(amount, 2))
    }
}

/// `new Date(iso)` for the RFC 3339 timestamps the app writes, in local time.
// ponytail: JS Date also reads date-only and other loose forms; the app never stores those.
fn parse_date(iso: &str) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(iso)
        .ok()
        .map(|date| date.with_timezone(&Local))
}

/// "Sep 12, 2025, 03:42 PM" (en-US, month short, 2-digit hour and minute).
pub fn format_date(iso: &str) -> String {
    parse_date(iso).map_or_else(|| "Invalid Date".into(), |date| date_label(&date))
}

fn date_label<Tz: TimeZone>(date: &DateTime<Tz>) -> String
where
    Tz::Offset: Display,
{
    date.format("%b %-d, %Y, %I:%M %p").to_string()
}

/// "Today, 3:42 PM", "Yesterday, 9:10 AM", "Sep 12", "Sep 12, 2025".
pub fn format_relative_date(iso: &str) -> String {
    parse_date(iso).map_or_else(
        || "Invalid Date".into(),
        |date| relative_label(&date, &Local::now()),
    )
}

fn relative_label<Tz: TimeZone>(date: &DateTime<Tz>, now: &DateTime<Tz>) -> String
where
    Tz::Offset: Display,
{
    let time = date.format("%-I:%M %p");
    match (now.date_naive() - date.date_naive()).num_days() {
        0 => format!("Today, {time}"),
        1 => format!("Yesterday, {time}"),
        _ if date.year() != now.year() => date.format("%b %-d, %Y").to_string(),
        _ => date.format("%b %-d").to_string(),
    }
}

pub fn is_url(value: &str) -> bool {
    let value = js_trim(value);
    let starts = |prefix: &str| {
        value
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    };
    starts("http://") || starts("https://")
}

pub fn basename(path: &str) -> &str {
    path.split(['/', '\\'])
        .rfind(|part| !part.is_empty())
        .unwrap_or(path)
}

/// Video id for youtube.com/watch, youtu.be and /shorts links.
pub fn youtube_id(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    let host = url.host_str().unwrap_or_default();
    let host = host
        .strip_prefix("www.")
        .or_else(|| host.strip_prefix("m."))
        .unwrap_or(host);
    if host == "youtu.be" {
        let id = url.path().get(1..).unwrap_or_default();
        return (!id.is_empty()).then(|| id.to_string());
    }
    if host == "youtube.com" || host == "music.youtube.com" {
        let v = url
            .query_pairs()
            .find(|(key, _)| key == "v")
            .map(|(_, v)| v.into_owned());
        if let Some(v) = v.filter(|v| !v.is_empty()) {
            return Some(v);
        }
        return YOUTUBE_PATH
            .captures(url.path())
            .map(|captures| captures[2].to_string());
    }
    None
}

/// Short human label for a clip source: file name, or host + path for links.
pub fn source_label(source: &str) -> String {
    if let Some(id) = twitch_vod_id(source) {
        return format!("Twitch VOD · {id}");
    }
    if !is_url(source) {
        return basename(source).to_string();
    }
    match url::Url::parse(source) {
        Ok(url) => {
            let host = url.host_str().unwrap_or_default();
            let path = if url.path() == "/" { "" } else { url.path() };
            let search = url
                .query()
                .filter(|query| !query.is_empty())
                .map(|query| format!("?{query}"))
                .unwrap_or_default();
            format!(
                "{}{path}{search}",
                host.strip_prefix("www.").unwrap_or(host)
            )
        }
        Err(_) => source.to_string(),
    }
}

/// Seconds between timeline ruler labels at a zoom (pixels per second): at least 60 px apart.
pub fn tick_step(px_per_second: f64) -> f64 {
    [0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0]
        .into_iter()
        .find(|step| step * px_per_second >= 60.0)
        .unwrap_or(600.0)
}

/// A timeline ruler label: "0:05", "1:30", "0:02.5".
pub fn format_ruler(seconds: f64) -> String {
    let m = (seconds / 60.0).floor() as i64;
    let s = seconds % 60.0;
    let pad = if s < 10.0 { "0" } else { "" };
    let s = if s.fract() == 0.0 {
        (s + 0.0).to_string()
    } else {
        to_fixed(s, 1)
    };
    format!("{m}:{pad}{s}")
}

/// The editor's playhead clock: "0:07.3", "12:04.0".
pub fn format_clock(ms: f64) -> String {
    let s = ms.max(0.0) / 1000.0;
    let m = (s / 60.0).floor() as i64;
    format!("{m}:{:0>4}", to_fixed(s % 60.0, 1))
}

/// "Transcribe $0.02 · plan $0.11", from the parts the run recorded.
pub fn cost_parts(costs: &ApiCosts, manual: bool) -> Option<String> {
    if costs.cost_incomplete == Some(true) {
        return Some("Partial: a model didn’t report its price".into());
    }
    if costs.total_estimated_cost_usd == 0.0 {
        return Some(
            if manual {
                "Offline transcription · No AI used"
            } else {
                "Offline transcription · CLI plan"
            }
            .into(),
        );
    }
    let part = |label: &str, section: &Option<crate::job_view::CostSection>| {
        section.as_ref().map(|section| {
            format!(
                "{label} {}",
                format_usd(section.estimated_cost_usd.unwrap_or_default())
            )
        })
    };
    let parts: Vec<String> = [
        part("Transcribe", &costs.transcription),
        part("plan", &costs.planning),
        part("framing", &costs.layout_vision),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job_view::CostSection;
    use chrono::FixedOffset;

    #[test]
    fn js_number_rounding() {
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(0.499_999_999_999_999_94), 0.0);
        assert_eq!(to_fixed(1.25, 1), "1.3");
        assert_eq!(to_fixed(0.125, 2), "0.13");
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(1.005, 2), "1.00");
        assert_eq!(to_fixed(-0.001, 2), "-0.00");
        assert_eq!(to_fixed(-1.25, 1), "-1.3");
        assert_eq!(to_fixed(0.0, 4), "0.0000");
        assert_eq!(to_fixed(123.456, 1), "123.5");
        assert_eq!(js_trim("\u{feff} a \u{a0}\n"), "a");
        assert_eq!(js_trim("\u{85}a"), "\u{85}a");
    }

    #[test]
    fn timecode_parser_rejects_invalid_minute_second_fields_and_infinity() {
        assert_eq!(parse_timecode("1:02:03.5"), Some(3723.5));
        assert_eq!(parse_timecode("90"), Some(90.0));
        assert_eq!(parse_timecode(""), None);
        assert!(parse_timecode("1:99").unwrap().is_nan());
        assert!(parse_timecode("1:60:00").unwrap().is_nan());
        assert!(parse_timecode(&"9".repeat(400)).unwrap().is_nan());
        // JS \d is ASCII only.
        assert!(parse_timecode("٣").unwrap().is_nan());
        assert_eq!(parse_timecode(" 007 "), Some(7.0));
    }

    #[test]
    fn durations_and_timecodes() {
        assert_eq!(format_duration(0.0), "0s");
        assert_eq!(format_duration(59_999.0), "59s");
        assert_eq!(format_duration(61_500.0), "1m 1s");
        assert_eq!(format_duration(3_600_000.0), "60m 0s");
        assert_eq!(format_timecode(7_000.0), "0:07");
        assert_eq!(format_timecode(754_000.0), "12:34");
        assert_eq!(format_timecode(3_723_900.0), "1:02:03");
        assert_eq!(format_timecode(-5_000.0), "0:00");
    }

    #[test]
    fn bytes_and_money() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1280), "1.3 KB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024 / 2), "1.5 GB");
        assert_eq!(format_usd(0.0), "$0.00");
        assert_eq!(format_usd(0.005), "$0.0050");
        assert_eq!(format_usd(0.125), "$0.13");
        assert_eq!(format_usd(1.5), "$1.50");
        assert_eq!(format_usd(-1.0), "$-1.0000");
    }

    #[test]
    fn dates_read_like_en_us() {
        let zone = FixedOffset::east_opt(7 * 3600).unwrap();
        let at = |iso: &str| {
            DateTime::parse_from_rfc3339(iso)
                .unwrap()
                .with_timezone(&zone)
        };
        assert_eq!(
            date_label(&at("2025-09-12T08:42:00Z")),
            "Sep 12, 2025, 03:42 PM"
        );
        assert_eq!(
            date_label(&at("2025-09-11T17:05:00Z")),
            "Sep 12, 2025, 12:05 AM"
        );
        let now = at("2026-03-02T03:00:00Z");
        assert_eq!(
            relative_label(&at("2026-03-02T01:10:00Z"), &now),
            "Today, 8:10 AM"
        );
        assert_eq!(
            relative_label(&at("2026-03-01T14:05:00Z"), &now),
            "Yesterday, 9:05 PM"
        );
        assert_eq!(relative_label(&at("2026-02-12T00:00:00Z"), &now), "Feb 12");
        assert_eq!(
            relative_label(&at("2025-09-12T00:00:00Z"), &now),
            "Sep 12, 2025"
        );
        assert_eq!(format_date("not a date"), "Invalid Date");
        assert_eq!(format_relative_date(""), "Invalid Date");
    }

    #[test]
    fn links_and_sources() {
        assert!(is_url("  HTTPS://example.com"));
        assert!(!is_url("ftp://example.com"));
        assert_eq!(basename("/Users/me/video.mp4"), "video.mp4");
        assert_eq!(basename("C:\\clips\\a.mov\\"), "a.mov");
        assert_eq!(basename("//"), "//");
        assert_eq!(
            youtube_id("https://www.youtube.com/watch?v=abc&t=1").as_deref(),
            Some("abc")
        );
        assert_eq!(youtube_id("https://youtu.be/xyz").as_deref(), Some("xyz"));
        assert_eq!(youtube_id("https://youtu.be/"), None);
        assert_eq!(
            youtube_id("https://m.youtube.com/shorts/s1?feature=x").as_deref(),
            Some("s1")
        );
        assert_eq!(
            youtube_id("https://music.youtube.com/watch?v=&v=b").as_deref(),
            None
        );
        assert_eq!(youtube_id("https://vimeo.com/1"), None);
        assert_eq!(youtube_id("not a url"), None);
        assert_eq!(
            source_label("https://www.twitch.tv/videos/12345"),
            "Twitch VOD · 12345"
        );
        assert_eq!(source_label("/Users/me/talk.mp4"), "talk.mp4");
        assert_eq!(
            source_label("https://www.youtube.com/watch?v=abc"),
            "youtube.com/watch?v=abc"
        );
        assert_eq!(source_label("https://example.com/"), "example.com");
        assert_eq!(source_label("https://example.com/?"), "example.com");
        assert_eq!(source_label("https://"), "https://");
    }

    #[test]
    fn ruler_and_clock() {
        assert_eq!(tick_step(200.0), 0.5);
        assert_eq!(tick_step(60.0), 1.0);
        assert_eq!(tick_step(0.15), 600.0);
        assert_eq!(format_ruler(0.0), "0:00");
        assert_eq!(format_ruler(2.5), "0:02.5");
        assert_eq!(format_ruler(90.0), "1:30");
        assert_eq!(format_clock(7_300.0), "0:07.3");
        assert_eq!(format_clock(-1.0), "0:00.0");
        assert_eq!(format_clock(724_000.0), "12:04.0");
        // toFixed can round up to 60.0, as in the renderer.
        assert_eq!(format_clock(59_960.0), "0:60.0");
    }

    #[test]
    fn cost_line_names_the_recorded_parts() {
        let section = |cost: f64| {
            Some(CostSection {
                estimated_cost_usd: Some(cost),
                ..CostSection::default()
            })
        };
        let costs = ApiCosts {
            total_estimated_cost_usd: 0.13,
            transcription: section(0.02),
            planning: section(0.11),
            ..ApiCosts::default()
        };
        assert_eq!(
            cost_parts(&costs, false).as_deref(),
            Some("Transcribe $0.02 · plan $0.11")
        );
        let vision = ApiCosts {
            total_estimated_cost_usd: 0.004,
            layout_vision: section(0.004),
            ..ApiCosts::default()
        };
        assert_eq!(
            cost_parts(&vision, false).as_deref(),
            Some("framing $0.0040")
        );
        let free = ApiCosts::default();
        assert_eq!(
            cost_parts(&free, true).as_deref(),
            Some("Offline transcription · No AI used")
        );
        assert_eq!(
            cost_parts(&free, false).as_deref(),
            Some("Offline transcription · CLI plan")
        );
        let partial = ApiCosts {
            cost_incomplete: Some(true),
            ..costs.clone()
        };
        assert_eq!(
            cost_parts(&partial, false).as_deref(),
            Some("Partial: a model didn’t report its price")
        );
        let unnamed = ApiCosts {
            total_estimated_cost_usd: 0.5,
            ..ApiCosts::default()
        };
        assert_eq!(cost_parts(&unnamed, false), None);
    }
}
