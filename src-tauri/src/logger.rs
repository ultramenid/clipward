//! Structured JSON-lines logger (port of src/main/logger.ts).
//!
//! Context values are sanitized before they are written: keys that tend to carry
//! paths, URLs, secrets or untrusted text are replaced with "[omitted]", as are
//! string values that look like a path, URL or credential.
use regex::Regex;
use serde_json::{Map, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock};

const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

static LOG_FILE: OnceLock<PathBuf> = OnceLock::new();
static WRITE_LOCK: Mutex<()> = Mutex::new(());

static SENSITIVE_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)path|url|secret|token|password|key|stack|message|error|hint|chunk|stdout|stderr|reason|resources|userData|logs|exec").unwrap()
});
static SENSITIVE_VALUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)https?://|[\\/]|[\r\n]|(?:token|secret|key)\s*[:=]").unwrap()
});

pub fn init(logs_dir: &Path) {
    let _ = std::fs::create_dir_all(logs_dir);
    let _ = LOG_FILE.set(logs_dir.join("clipward.log"));
}

pub fn log_file_path() -> PathBuf {
    LOG_FILE
        .get()
        .cloned()
        .unwrap_or_else(|| std::env::temp_dir().join("clipward.log"))
}

fn sanitize(context: Value) -> Map<String, Value> {
    let mut safe = Map::new();
    let Value::Object(entries) = context else {
        return safe;
    };
    for (key, value) in entries {
        let clean = if SENSITIVE_KEY.is_match(&key) {
            Value::from("[omitted]")
        } else {
            match value {
                Value::String(text) => {
                    if SENSITIVE_VALUE.is_match(&text) {
                        Value::from("[omitted]")
                    } else {
                        Value::from(text.chars().take(160).collect::<String>())
                    }
                }
                Value::Number(_) | Value::Bool(_) | Value::Null => value,
                _ => Value::from("[omitted]"),
            }
        };
        safe.insert(key, clean);
    }
    safe
}

fn write(level: &str, event: &str, context: Value) {
    let safe = sanitize(context);
    let mut entry = Map::new();
    entry.insert("ts".into(), Value::from(crate::paths::iso_now()));
    entry.insert("level".into(), Value::from(level));
    entry.insert("event".into(), Value::from(event));
    entry.extend(safe.clone());
    let line = format!("{}\n", Value::Object(entry));

    if cfg!(debug_assertions) {
        eprintln!("[{level}] {event} {}", Value::Object(safe));
    }

    let _guard = WRITE_LOCK.lock();
    let path = log_file_path();
    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() > MAX_LOG_BYTES {
            let _ = std::fs::rename(&path, path.with_extension("log.old"));
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    if let Ok(mut file) = options.open(&path) {
        let _ = file.write_all(line.as_bytes());
    }
}

pub fn info(event: &str, context: Value) {
    write("info", event, context)
}

pub fn warn(event: &str, context: Value) {
    write("warn", event, context)
}

pub fn error(event: &str, context: Value) {
    write("error", event, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn omits_sensitive_context() {
        let safe = sanitize(
            json!({ "jobId": "abc", "outputDir": "/x", "note": "see https://x", "n": 3, "o": {} }),
        );
        assert_eq!(safe["jobId"], "abc");
        assert_eq!(safe["outputDir"], "[omitted]");
        assert_eq!(safe["note"], "[omitted]");
        assert_eq!(safe["n"], 3);
        assert_eq!(safe["o"], "[omitted]");
    }
}
