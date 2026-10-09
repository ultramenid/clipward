//! Manual clip mode's list of typed moments: parsing, inline checks and the request's
//! clips (port of src/renderer/lib/manual-clips.ts).
use crate::format::{js_round, js_trim, parse_timecode, to_fixed};
use crate::shared::{
    MAX_MANUAL_CLIPS, MAX_MANUAL_CLIP_SECONDS, MAX_MANUAL_TITLE_CHARS, MIN_MANUAL_CLIP_SECONDS,
};
use clip_engine::ManualClip;
use std::sync::atomic::{AtomicU64, Ordering};

/// One editable row of the manual clip list, as typed.
#[derive(Debug, Clone, PartialEq)]
pub struct ManualClipRow {
    pub id: String,
    pub start: String,
    pub end: String,
    pub title: String,
}

/// Which field a row's error points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowField {
    Start,
    End,
    Title,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManualRowCheck {
    /// None when empty, NaN when unreadable (as `parse_clip_time`).
    pub start: Option<f64>,
    pub end: Option<f64>,
    /// Seconds between start and end once both parse.
    pub length: Option<f64>,
    /// First problem with the row, shown under it.
    pub error: Option<String>,
    /// Which field the error points at.
    pub field: Option<RowField>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ManualClipsCheck {
    pub rows: Vec<ManualRowCheck>,
    /// Problem with the list as a whole (none, or too many clips).
    pub list_error: Option<String>,
    pub valid: bool,
}

static NEXT_ROW_ID: AtomicU64 = AtomicU64::new(0);

/// An empty row with a fresh id; set the fields with struct update syntax.
pub fn new_manual_row() -> ManualClipRow {
    let n = NEXT_ROW_ID.fetch_add(1, Ordering::Relaxed) + 1;
    let millis = chrono::Utc::now().timestamp_millis().max(0) as u64;
    ManualClipRow {
        id: format!("clip-{}-{n}", base36(millis)),
        start: String::new(),
        end: String::new(),
        title: String::new(),
    }
}

fn base36(mut value: u64) -> String {
    let mut digits = Vec::new();
    loop {
        digits.push(char::from_digit((value % 36) as u32, 36).unwrap());
        value /= 36;
        if value == 0 {
            break;
        }
    }
    digits.iter().rev().collect()
}

/// "90", "1:30", "1:02:03" or "75.5" in seconds; None when empty, NaN when unreadable.
pub fn parse_clip_time(text: &str) -> Option<f64> {
    parse_timecode(text)
}

/// Seconds as m:ss or h:mm:ss, keeping a tenth of a second when there is one.
pub fn format_clip_time(seconds: f64) -> String {
    let tenths = js_round(seconds * 10.0) as i64;
    let whole = tenths.div_euclid(10);
    let h = whole.div_euclid(3600);
    let m = (whole % 3600).div_euclid(60);
    let s = whole % 60;
    let fraction = if tenths % 10 != 0 {
        format!(".{}", tenths % 10)
    } else {
        String::new()
    };
    if h > 0 {
        format!("{h}:{m:02}:{s:02}{fraction}")
    } else {
        format!("{m}:{s:02}{fraction}")
    }
}

/// A clip's length for the Duration column: "45s", "1m 30s", "15m".
pub fn format_clip_length(seconds: f64) -> String {
    let rounded = js_round(seconds * 10.0) / 10.0;
    if rounded < 60.0 {
        return if rounded.fract() == 0.0 {
            format!("{}s", rounded + 0.0)
        } else {
            format!("{}s", to_fixed(rounded, 1))
        };
    }
    let whole = js_round(rounded) as i64;
    let (m, s) = (whole.div_euclid(60), whole % 60);
    if s != 0 {
        format!("{m}m {s}s")
    } else {
        format!("{m}m")
    }
}

const TIME_HINT: &str = "Use seconds (90), m:ss (1:30) or h:mm:ss.";

/// Checks one row against the limits the app shell enforces (end after start,
/// 3 s to 15 min, a short one-line title) and, when the video length is known,
/// that the clip fits inside the video.
pub fn check_manual_row(row: &ManualClipRow, video_seconds: Option<f64>) -> ManualRowCheck {
    let start = parse_clip_time(&row.start);
    let end = parse_clip_time(&row.end);
    let length = match (start, end) {
        (Some(start), Some(end)) if !start.is_nan() && !end.is_nan() => Some(end - start),
        _ => None,
    };
    let fail = |error: String, field: RowField| ManualRowCheck {
        start,
        end,
        length,
        error: Some(error),
        field: Some(field),
    };
    let Some(start_s) = start else {
        return fail("Enter a start time.".into(), RowField::Start);
    };
    if start_s.is_nan() {
        return fail(TIME_HINT.into(), RowField::Start);
    }
    let Some(end_s) = end else {
        return fail("Enter an end time.".into(), RowField::End);
    };
    if end_s.is_nan() {
        return fail(TIME_HINT.into(), RowField::End);
    }
    if end_s <= start_s {
        return fail("End must be after the start.".into(), RowField::End);
    }
    if let Some(video) = video_seconds.filter(|v| v.is_finite() && *v > 0.0) {
        if start_s >= video {
            return fail(
                format!("Starts after the video ends ({}).", format_clip_time(video)),
                RowField::Start,
            );
        }
        if end_s > video + 0.05 {
            return fail(
                format!("Ends after the video ends ({}).", format_clip_time(video)),
                RowField::End,
            );
        }
    }
    if length.is_some_and(|l| l < MIN_MANUAL_CLIP_SECONDS) {
        return fail(
            format!("Clips must be at least {MIN_MANUAL_CLIP_SECONDS} seconds."),
            RowField::End,
        );
    }
    if length.is_some_and(|l| l > MAX_MANUAL_CLIP_SECONDS) {
        return fail("Clips can be at most 15 minutes.".into(), RowField::End);
    }
    // `.length` counts UTF-16 units, as the renderer did.
    if js_trim(&row.title).encode_utf16().count() > MAX_MANUAL_TITLE_CHARS {
        return fail(
            format!("Titles can be at most {MAX_MANUAL_TITLE_CHARS} characters."),
            RowField::Title,
        );
    }
    if row.title.chars().any(char::is_control) {
        return fail("Titles must be a single line.".into(), RowField::Title);
    }
    ManualRowCheck {
        start,
        end,
        length,
        error: None,
        field: None,
    }
}

pub fn check_manual_clips(rows: &[ManualClipRow], video_seconds: Option<f64>) -> ManualClipsCheck {
    let checks: Vec<ManualRowCheck> = rows
        .iter()
        .map(|row| check_manual_row(row, video_seconds))
        .collect();
    let list_error = if rows.is_empty() {
        Some("Add at least one clip.".to_string())
    } else if rows.len() > MAX_MANUAL_CLIPS {
        Some(format!("Add at most {MAX_MANUAL_CLIPS} clips."))
    } else {
        None
    };
    let valid = list_error.is_none() && checks.iter().all(|check| check.error.is_none());
    ManualClipsCheck {
        rows: checks,
        list_error,
        valid,
    }
}

/// The request's manualClips for valid rows (titles trimmed, empty titles dropped).
pub fn to_manual_clips(rows: &[ManualClipRow]) -> Vec<ManualClip> {
    rows.iter()
        .map(|row| {
            let title = js_trim(&row.title);
            ManualClip {
                start_seconds: parse_clip_time(&row.start).unwrap_or(0.0),
                end_seconds: parse_clip_time(&row.end).unwrap_or(0.0),
                title: (!title.is_empty()).then(|| title.to_string()),
            }
        })
        .collect()
}

/// Add a blank row (or a copy of `copy`) after `after`, or at the end; None when the
/// list is full. Returns the new row's id.
pub fn insert_row(
    rows: &mut Vec<ManualClipRow>,
    after: Option<usize>,
    copy: Option<&ManualClipRow>,
) -> Option<String> {
    if rows.len() >= MAX_MANUAL_CLIPS {
        return None;
    }
    let row = match copy {
        Some(copy) => ManualClipRow {
            start: copy.start.clone(),
            end: copy.end.clone(),
            title: copy.title.clone(),
            ..new_manual_row()
        },
        None => new_manual_row(),
    };
    let id = row.id.clone();
    let at = after.map_or(rows.len(), |i| (i + 1).min(rows.len()));
    rows.insert(at, row);
    Some(id)
}

/// Remove a row; the list never empties (removing the only clip leaves a blank
/// one). Returns the id of the row to focus next.
pub fn remove_row(rows: &mut Vec<ManualClipRow>, index: usize) -> String {
    if index < rows.len() {
        rows.remove(index);
    }
    if rows.is_empty() {
        rows.push(new_manual_row());
    }
    rows[index.min(rows.len() - 1)].id.clone()
}

/// "2 of 3 ready · 1m 15s total", and the limit once the list is full.
pub fn list_summary(rows: usize, check: &ManualClipsCheck) -> String {
    let valid = check.rows.iter().filter(|row| row.error.is_none()).count();
    let total: f64 = check
        .rows
        .iter()
        .filter(|row| row.error.is_none())
        .filter_map(|row| row.length)
        .sum();
    let mut text = format!("{valid} of {rows} ready");
    if total > 0.0 {
        text.push_str(&format!(" · {} total", format_clip_length(total)));
    }
    if rows >= MAX_MANUAL_CLIPS {
        text.push_str(&format!(" · {MAX_MANUAL_CLIPS} is the most per job"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn row(start: &str, end: &str, title: &str) -> ManualClipRow {
        ManualClipRow {
            id: "r".into(),
            start: start.into(),
            end: end.into(),
            title: title.into(),
        }
    }

    #[test]
    fn manual_clip_times_accept_seconds_m_ss_and_h_mm_ss() {
        assert_eq!(parse_clip_time("90"), Some(90.0));
        assert_eq!(parse_clip_time("75.5"), Some(75.5));
        assert_eq!(parse_clip_time("1:30"), Some(90.0));
        assert_eq!(parse_clip_time(" 1:02:03 "), Some(3723.0));
        assert_eq!(parse_clip_time(""), None);
        for bad in ["1:75", "abc", "1:2:3:4", "-5"] {
            assert!(parse_clip_time(bad).unwrap().is_nan(), "{bad}");
        }
        assert_eq!(format_clip_time(90.0), "1:30");
        assert_eq!(format_clip_time(3723.4), "1:02:03.4");
        assert_eq!(format_clip_length(45.0), "45s");
        assert_eq!(format_clip_length(3.5), "3.5s");
        assert_eq!(format_clip_length(90.0), "1m 30s");
        assert_eq!(format_clip_length(900.0), "15m");
        // Math.round halves go up: 59.95 s rounds to a whole minute.
        assert_eq!(format_clip_length(59.95), "1m");
        assert_eq!(format_clip_time(0.05), "0:00.1");
    }

    #[test]
    fn manual_clips_are_checked_inline_against_the_shell_limits_and_the_known_video_length() {
        assert_eq!(
            check_manual_row(&row("0:10", "0:40", ""), None),
            ManualRowCheck {
                start: Some(10.0),
                end: Some(40.0),
                length: Some(30.0),
                error: None,
                field: None
            }
        );
        let error = |r: ManualClipRow, video: Option<f64>| check_manual_row(&r, video).error;
        assert_eq!(
            error(row("", "10", ""), None).as_deref(),
            Some("Enter a start time.")
        );
        assert_eq!(
            error(row("5", "", ""), None).as_deref(),
            Some("Enter an end time.")
        );
        assert!(error(row("1:99", "2:00", ""), None)
            .unwrap()
            .contains("m:ss"));
        assert_eq!(
            error(row("40", "30", ""), None).as_deref(),
            Some("End must be after the start.")
        );
        assert_eq!(
            error(row("10", "12", ""), None).as_deref(),
            Some("Clips must be at least 3 seconds.")
        );
        assert_eq!(error(row("10", "13", ""), None), None);
        assert_eq!(error(row("0", "15:00", ""), None), None);
        assert_eq!(
            error(row("0", "15:01", ""), None).as_deref(),
            Some("Clips can be at most 15 minutes.")
        );
        assert_eq!(
            check_manual_row(&row("0", "10", &"x".repeat(121)), None).field,
            Some(RowField::Title)
        );
        assert_eq!(
            error(row("0", "10", "two\nlines"), None).as_deref(),
            Some("Titles must be a single line.")
        );
        // A known video length catches clips past the end; unknown lengths wait for the engine.
        assert_eq!(
            error(row("1:50", "2:10", ""), Some(120.0)).as_deref(),
            Some("Ends after the video ends (2:00).")
        );
        assert_eq!(
            error(row("2:00", "2:10", ""), Some(120.0)).as_deref(),
            Some("Starts after the video ends (2:00).")
        );
        assert_eq!(error(row("1:50", "2:10", ""), None), None);
        // Overlaps are allowed; the list needs one to fifty clips.
        assert!(check_manual_clips(&[row("0", "30", ""), row("10", "40", "")], Some(60.0)).valid);
        assert_eq!(
            check_manual_clips(&[], None).list_error.as_deref(),
            Some("Add at least one clip.")
        );
        let many: Vec<ManualClipRow> = (0..51)
            .map(|_| ManualClipRow {
                start: "0".into(),
                end: "10".into(),
                ..new_manual_row()
            })
            .collect();
        assert_eq!(
            check_manual_clips(&many, None).list_error.as_deref(),
            Some("Add at most 50 clips.")
        );
        assert!(check_manual_clips(&many[..50], None).valid);
        assert_eq!(many.iter().map(|r| &r.id).collect::<HashSet<_>>().len(), 51);
    }

    #[test]
    fn titles_count_utf16_units_after_trimming() {
        // 60 emoji are 120 UTF-16 units: the limit. One more is over.
        assert_eq!(error_of(&format!("  {}  ", "😀".repeat(60))), None);
        assert!(error_of(&"😀".repeat(61)).is_some());
        assert!(error_of("tab\there").is_some());
    }

    fn error_of(title: &str) -> Option<String> {
        check_manual_row(&row("0", "10", title), None).error
    }

    #[test]
    fn typed_rows_become_request_clips() {
        let clips = to_manual_clips(&[row("1:00", "1:30", "  Hook "), row("5", "20", "  ")]);
        assert_eq!(
            clips,
            vec![
                ManualClip {
                    start_seconds: 60.0,
                    end_seconds: 90.0,
                    title: Some("Hook".into())
                },
                ManualClip {
                    start_seconds: 5.0,
                    end_seconds: 20.0,
                    title: None
                },
            ]
        );
        assert!(new_manual_row().id.starts_with("clip-"));
    }
    #[test]
    fn rows_insert_after_copy_and_never_empty() {
        let mut rows = vec![row("1", "10", "a")];
        let copy = rows[0].clone();
        let id = insert_row(&mut rows, Some(0), Some(&copy)).unwrap();
        assert_eq!(rows[1].id, id);
        assert_eq!((rows[1].start.as_str(), rows[1].title.as_str()), ("1", "a"));
        insert_row(&mut rows, None, None).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].start, "");
        let focus = remove_row(&mut rows, 2);
        assert_eq!(focus, rows[1].id);
        remove_row(&mut rows, 0);
        let last = remove_row(&mut rows, 0);
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].id.clone(), rows[0].start.as_str()), (last, ""));
        let mut full: Vec<ManualClipRow> = (0..MAX_MANUAL_CLIPS).map(|_| new_manual_row()).collect();
        assert_eq!(insert_row(&mut full, None, None), None);
        let check = check_manual_clips(&full, None);
        assert_eq!(list_summary(full.len(), &check), "0 of 50 ready · 50 is the most per job");
        let rows = vec![row("0", "30", ""), row("1:00", "1:45", ""), row("", "", "")];
        let check = check_manual_clips(&rows, None);
        assert_eq!(list_summary(rows.len(), &check), "2 of 3 ready · 1m 15s total");
    }
}
