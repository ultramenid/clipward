//! The `local-file:` protocol that serves library media to <video> and <img>
//! (port of the protocol.handle('local-file') handler in src/main/index.ts).
//!
//! The renderer builds URLs with `convertFileSrc(path, 'local-file')`. Only files the
//! security checks authorize are served, byte ranges are honored, and an open-ended
//! request is answered in bounded chunks so a long video is never read into memory.
use crate::AppState;
use std::io::{Read, Seek, SeekFrom};
use tauri::http::{header, Method, Request, Response, StatusCode};
use tauri::{AppHandle, Manager, Runtime};

/// Largest body sent for one request; media elements ask for the rest by range.
const MAX_CHUNK: u64 = 4 * 1024 * 1024;
/// Files up to this size are sent whole when no range is requested (thumbnails).
const MAX_WHOLE_FILE: u64 = 16 * 1024 * 1024;

fn mime_type(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mp4" | "m4v") => "video/mp4",
        Some("mkv") => "video/x-matroska",
        Some("webm") => "video/webm",
        Some("avi") => "video/x-msvideo",
        Some("mov") => "video/quicktime",
        Some("flv") => "video/x-flv",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
}

fn empty(status: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .body(Vec::new())
        .unwrap_or_default()
}

fn unsatisfiable(size: u64) -> Response<Vec<u8>> {
    Response::builder()
        .status(StatusCode::RANGE_NOT_SATISFIABLE)
        .header(header::CONTENT_RANGE, format!("bytes */{size}"))
        .body(Vec::new())
        .unwrap_or_default()
}

/// Parse a single `bytes=start-end` range. None when malformed.
fn parse_range(value: &str, size: u64) -> Option<(u64, Option<u64>)> {
    let spec = value.strip_prefix("bytes=")?;
    let (start, end) = spec.split_once('-')?;
    let all_digits = |s: &str| s.chars().all(|c| c.is_ascii_digit());
    if !all_digits(start) || !all_digits(end) || (start.is_empty() && end.is_empty()) {
        return None;
    }
    if start.is_empty() {
        let suffix: u64 = end.parse().ok()?;
        return Some((size.saturating_sub(suffix), Some(size.saturating_sub(1))));
    }
    Some((
        start.parse().ok()?,
        if end.is_empty() {
            None
        } else {
            Some(end.parse().ok()?)
        },
    ))
}

pub fn handle<R: Runtime>(app: &AppHandle<R>, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return empty(StatusCode::METHOD_NOT_ALLOWED);
    }
    let forbidden = || {
        Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(b"Media unavailable".to_vec())
            .unwrap_or_default()
    };
    let encoded = request.uri().path().trim_start_matches('/');
    let Ok(path) = percent_encoding::percent_decode_str(encoded).decode_utf8() else {
        return forbidden();
    };
    let state = app.state::<AppState>();
    let Ok(settings) = state.settings.load() else {
        return forbidden();
    };
    let Ok((mut file, size, canonical)) = state
        .media
        .open_authorized(&path, std::path::Path::new(&settings.output_directory))
    else {
        return forbidden();
    };

    let range = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok());
    let (start, end, status) = match range {
        Some(range) => {
            let Some((start, end)) = parse_range(range, size) else {
                return unsatisfiable(size);
            };
            if start >= size || end.is_some_and(|end| end < start) {
                return unsatisfiable(size);
            }
            let end = end
                .unwrap_or(u64::MAX)
                .min(size - 1)
                .min(start + MAX_CHUNK - 1);
            (start, end, StatusCode::PARTIAL_CONTENT)
        }
        None if size <= MAX_WHOLE_FILE => (0, size.saturating_sub(1), StatusCode::OK),
        // Too large to send whole: answer with the first chunk; players fetch the rest.
        None => (0, MAX_CHUNK - 1, StatusCode::PARTIAL_CONTENT),
    };
    let length = if size == 0 { 0 } else { end - start + 1 };

    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime_type(&canonical))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .header(header::CONTENT_LENGTH, length.to_string());
    if status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
    }
    if request.method() == Method::HEAD || length == 0 {
        return builder.body(Vec::new()).unwrap_or_default();
    }
    let mut body = Vec::with_capacity(length as usize);
    let read = file
        .seek(SeekFrom::Start(start))
        .and_then(|_| file.by_ref().take(length).read_to_end(&mut body));
    match read {
        Ok(_) => builder.body(body).unwrap_or_default(),
        Err(_) => forbidden(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ranges() {
        assert_eq!(parse_range("bytes=0-", 100), Some((0, None)));
        assert_eq!(parse_range("bytes=10-19", 100), Some((10, Some(19))));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, Some(99))));
        assert_eq!(parse_range("bytes=-", 100), None);
        assert_eq!(parse_range("bytes=a-1", 100), None);
        assert_eq!(parse_range("items=0-1", 100), None);
    }
}
