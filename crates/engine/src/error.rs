//! Engine errors carry a lowercase marker phrase (the same phrases the Python
//! engine raised, e.g. "whisper model is not downloaded", "video render failed").
//! `pipeline::describe_failure` maps those markers to the fixed UI message + hint,
//! so raw detail (paths, URLs, provider output) never reaches the UI.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Failed(String),
}

impl EngineError {
    pub fn msg(text: impl Into<String>) -> Self {
        Self::Failed(text.into())
    }
}

impl From<std::io::Error> for EngineError {
    fn from(e: std::io::Error) -> Self {
        Self::Failed(format!("io error: {e}"))
    }
}

pub type Result<T> = std::result::Result<T, EngineError>;
