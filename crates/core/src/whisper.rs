//! Offline Whisper model downloads.
use crate::logger;
use crate::shared::{is_whisper_model_id, WHISPER_MODEL_IDS};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main";
const CATALOG_URL: &str = "https://huggingface.co/api/models/ggerganov/whisper.cpp/tree/main";
const CATALOG_FILE: &str = "catalog.json";

/// A model file in the Hugging Face repo, from the last catalog refresh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteModel {
    pub id: String,
    pub size: u64,
}
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
    /// The file's size in the repo; None until a catalog refresh has seen it.
    pub size_bytes: Option<u64>,
    /// Downloaded, but the repo now has a different file.
    pub update_available: bool,
}

pub struct WhisperModels {
    dir: PathBuf,
    downloads: Mutex<HashMap<String, Arc<Download>>>,
    /// The repo's models, cached on disk so the list survives going offline.
    remote: Mutex<Vec<RemoteModel>>,
}

impl WhisperModels {
    pub fn new(user_data: &Path) -> Self {
        let dir = user_data.join("whisper-models");
        let remote = std::fs::read(dir.join(CATALOG_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            dir,
            downloads: Mutex::new(HashMap::new()),
            remote: Mutex::new(remote),
        }
    }

    /// Re-reads the model list from the Hugging Face repo and caches it.
    pub async fn refresh(&self) -> Result<Vec<WhisperModelStatus>, String> {
        let failed = || "Could not check for new Whisper models. Check your connection and retry.".to_string();
        #[derive(Deserialize)]
        struct Entry {
            path: String,
            #[serde(default)]
            size: u64,
        }
        let entries: Vec<Entry> = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(60))
            .build()
            .map_err(|_| failed())?
            .get(CATALOG_URL)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|_| failed())?
            .json()
            .await
            .map_err(|_| failed())?;
        let models = parse_catalog(entries.into_iter().map(|e| (e.path, e.size)));
        if models.is_empty() {
            return Err(failed());
        }
        if let Ok(dir) = self.models_dir() {
            let _ = crate::paths::write_atomic(
                &dir.join(CATALOG_FILE),
                &serde_json::to_vec(&models).unwrap_or_default(),
            );
        }
        if let Ok(mut remote) = self.remote.lock() {
            *remote = models;
        }
        Ok(self.list())
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

    /// The curated models, then the rest of the repo's.
    pub fn list(&self) -> Vec<WhisperModelStatus> {
        let remote = self.remote.lock().map(|r| r.clone()).unwrap_or_default();
        let extra = remote
            .iter()
            .map(|m| m.id.as_str())
            .filter(|id| !WHISPER_MODEL_IDS.contains(id));
        WHISPER_MODEL_IDS
            .iter()
            .copied()
            .chain(extra)
            .map(|id| self.status(id, &remote))
            .collect()
    }

    fn status(&self, id: &str, remote: &[RemoteModel]) -> WhisperModelStatus {
        let active = self.active(id);
        let local = self.downloaded(id);
        let size_bytes = remote.iter().find(|m| m.id == id).map(|m| m.size);
        // ponytail: size comparison only; a same-size re-upload goes unnoticed.
        // Compare the LFS sha256 (in the tree API) if that ever matters.
        let update_available = match (&local, size_bytes) {
            (Some(path), Some(size)) => std::fs::metadata(path).is_ok_and(|m| m.len() != size),
            _ => false,
        };
        WhisperModelStatus {
            id: id.into(),
            downloaded: local.is_some(),
            received_bytes: active.as_ref().map(|a| a.received.load(Ordering::Relaxed)),
            total_bytes: active
                .as_ref()
                .and_then(|a| a.total_plus_one.load(Ordering::Relaxed).checked_sub(1)),
            size_bytes,
            update_available,
        }
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
            let current = self.downloaded(id).is_some()
                && !self.list().iter().any(|m| m.id == id && m.update_available);
            if downloads.contains_key(id) || current {
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
        let remote = self.remote.lock().map(|r| r.clone()).unwrap_or_default();
        on_progress(self.status(id, &remote));
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
                    size_bytes: total,
                    update_available: false,
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

/// `ggml-{id}.bin` files from a repo listing of (path, size). Skips the Core ML
/// encoders and anything whose id is not a safe file name.
fn parse_catalog(entries: impl IntoIterator<Item = (String, u64)>) -> Vec<RemoteModel> {
    entries
        .into_iter()
        .filter_map(|(path, size)| {
            let id = path.strip_prefix("ggml-")?.strip_suffix(".bin")?;
            (is_whisper_model_id(id) && !id.contains("encoder") && size > 0)
                .then(|| RemoteModel { id: id.into(), size })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_keeps_only_model_files() {
        let models = parse_catalog([
            ("ggml-large-v3-turbo.bin".to_string(), 1_624_555_275),
            ("ggml-base.en-q5_1.bin".into(), 59_721_011),
            ("ggml-base-encoder.mlmodelc.zip".into(), 1),
            ("README.md".into(), 10),
            ("ggml-../evil.bin".into(), 1),
            ("ggml-empty.bin".into(), 0),
        ]);
        let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["large-v3-turbo", "base.en-q5_1"]);
    }

    #[test]
    fn list_adds_repo_models_and_flags_changed_files() {
        let dir = std::env::temp_dir().join(format!("clipward-whisper-{}", uuid::Uuid::new_v4()));
        let models = WhisperModels::new(&dir);
        std::fs::create_dir_all(&models.dir).unwrap();
        std::fs::write(models.model_path("base"), b"old").unwrap();
        *models.remote.lock().unwrap() = vec![
            RemoteModel { id: "base".into(), size: 5 },
            RemoteModel { id: "large-v4".into(), size: 9 },
        ];
        let list = models.list();
        assert_eq!(list.len(), WHISPER_MODEL_IDS.len() + 1);
        assert!(list.iter().any(|m| m.id == "base" && m.update_available));
        assert!(list.iter().any(|m| m.id == "large-v4" && !m.downloaded && m.size_bytes == Some(9)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
