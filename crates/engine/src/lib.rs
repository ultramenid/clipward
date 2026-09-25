//! Clipward clipping engine: download -> transcribe -> plan -> layout -> render.
pub mod error;
pub mod media;
pub mod types;

pub mod download;
pub mod net;
pub mod transcribe;

pub mod llm;
pub mod planner;

pub mod layout;
pub mod render;

pub mod pipeline;

pub use error::{EngineError, Result};
pub use types::*;
