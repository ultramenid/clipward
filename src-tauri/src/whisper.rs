//! Offline Whisper model downloads (port of src/main/whisper-models.ts).
use crate::logger;
use crate::shared::{is_whisper_model_id, WHISPER_MODEL_IDS};
use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";
/// `total` is stored as bytes + 1 so 0 can mean "unknown".
struct Download {
    cancel: CancellationToken,
    received: AtomicU64,
    total_plus_one: AtomicU64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WhisperModelStatus {
    pub id: String,
    pub downloaded: bool,
    pub received_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
}

pub struct WhisperModels {
    dir: PathBuf,
    downloads: Mutex<HashMap<String, Arc<Download>>>,
}

impl WhisperModels {
    pub fn new(user_data: &Path) -> Self {
        Self {
            dir: user_data.join("whisper-models"),
            downloads: Mutex::new(HashMap::new()),
        }
    }

    fn models_dir(&self) -> Result<PathBuf, String> {
        crate::paths::private_dir(&self.dir)
            .map_err(|_| "Could not create the model folder".to_string())
    }

    fn model_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("ggml-{id}.bin"))
    }

    /// The downloaded file for `id`, or None when it has not been downloaded yet.
    pub fn downloaded(&self, id: &str) -> Option<PathBuf> {
        if !is_whisper_model_id(id) {
            return None;
        }
        let path = self.model_path(id);
        path.is_file().then_some(path)
    }

    fn active(&self, id: &str) -> Option<Arc<Download>> {
        self.downloads.lock().ok()?.get(id).cloned()
    }

    pub fn list(&self) -> Vec<WhisperModelStatus> {
        WHISPER_MODEL_IDS
            .iter()
            .map(|id| {
                let active = self.active(id);
                WhisperModelStatus {
                    id: (*id).into(),
                    downloaded: self.downloaded(id).is_some(),
                    received_bytes: active.as_ref().map(|a| a.received.load(Ordering::Relaxed)),
                    total_bytes: active
                        .as_ref()
                        .and_then(|a| a.total_plus_one.load(Ordering::Relaxed).checked_sub(1)),
                }
            })
            .collect()
    }

    /// Downloads a model to a `.part` file and renames it only when complete, so a
    /// crash or cancel never leaves a truncated model that looks usable.
    pub async fn download(
        &self,
        id: &str,
        on_progress: impl Fn(WhisperModelStatus),
    ) -> Result<Vec<WhisperModelStatus>, String> {
        if !is_whisper_model_id(id) {
            return Err("Unknown Whisper model".into());
        }
        let state = {
            let mut downloads = self
                .downloads
                .lock()
                .map_err(|_| "Downloads are unavailable".to_string())?;
            if downloads.contains_key(id) || self.downloaded(id).is_some() {
                return Ok(self.list());
            }
            let state = Arc::new(Download {
                cancel: CancellationToken::new(),
                received: AtomicU64::new(0),
                total_plus_one: AtomicU64::new(0),
            });
            downloads.insert(id.to_string(), state.clone());
            state
        };
        let target = self.model_path(id);
        let part = target.with_extension("bin.part");
        let result = self.fetch(id, &part, &target, &state, &on_progress).await;
        if let Some(map) = self.downloads.lock().ok().as_mut() {
            map.remove(id);
        }
        on_progress(WhisperModelStatus {
            id: id.into(),
            downloaded: self.downloaded(id).is_some(),
            received_bytes: None,
            total_bytes: None,
        });
        match result {
            Ok(()) => {
                logger::info("whisper.download.done", json!({ "id": id }));
                Ok(self.list())
            }
            Err(_) if state.cancel.is_cancelled() => {
                let _ = std::fs::remove_file(&part);
                Ok(self.list())
            }
            Err(_) => {
                let _ = std::fs::remove_file(&part);
                logger::warn("whisper.download.failed", json!({ "id": id }));
                Err(
                    "The Whisper model could not be downloaded. Check your connection and retry."
                        .into(),
                )
            }
        }
    }

    async fn fetch(
        &self,
        id: &str,
        part: &Path,
        target: &Path,
        state: &Download,
        on_progress: &impl Fn(WhisperModelStatus),
    ) -> Result<(), ()> {
        self.models_dir().map_err(|_| ())?;
        let _ = std::fs::remove_file(part);
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| ())?;
        let mut response = tokio::select! {
            _ = state.cancel.cancelled() => return Err(()),
            response = client.get(format!("{BASE_URL}/ggml-{id}.bin")).send() => response.map_err(|_| ())?,
        };
        if !response.status().is_success() {
            return Err(());
        }
        let total = response.content_length();
        if let Some(total) = total {
            state.total_plus_one.store(total + 1, Ordering::Relaxed);
        }
        let mut file = crate::paths::open_private_new(part).map_err(|_| ())?;
        let mut last_report = Instant::now();
        loop {
            let chunk = tokio::select! {
                _ = state.cancel.cancelled() => return Err(()),
                chunk = response.chunk() => chunk.map_err(|_| ())?,
            };
            let Some(chunk) = chunk else { break };
            file.write_all(&chunk).map_err(|_| ())?;
            let received = state
                .received
                .fetch_add(chunk.len() as u64, Ordering::Relaxed)
                + chunk.len() as u64;
            if last_report.elapsed() > Duration::from_millis(250) {
                last_report = Instant::now();
                on_progress(WhisperModelStatus {
                    id: id.into(),
                    downloaded: false,
                    received_bytes: Some(received),
                    total_bytes: total,
                });
            }
        }
        file.sync_all().map_err(|_| ())?;
        drop(file);
        if total.is_some_and(|total| std::fs::metadata(part).map(|m| m.len()).unwrap_or(0) != total)
        {
            return Err(());
        }
        std::fs::rename(part, target).map_err(|_| ())
    }

    pub fn cancel(&self, id: &str) {
        if let Some(active) = self.active(id) {
            active.cancel.cancel();
        }
    }

    pub fn delete(&self, id: &str) -> Result<Vec<WhisperModelStatus>, String> {
        if !is_whisper_model_id(id) {
            return Err("Unknown Whisper model".into());
        }
        match std::fs::remove_file(self.model_path(id)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("Could not delete the Whisper model".into()),
        }
        Ok(self.list())
    }
}
