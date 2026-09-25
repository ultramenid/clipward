//! Rendering: FFmpeg filter graphs for smart framing, pacing cuts, burned-in
//! captions, title/banner overlays, speed and longform sidecars.
//!
//! Ports of rendering_service.py (`service`), layout_renderer.py (`layout_renderer`),
//! caption_generator.py + config caption presets (`captions`), clip_editor.py
//! (`clip_editor`) and video_speed.py (`video_speed`). FFmpeg arguments and ASS
//! text match the Python engine byte for byte (see `pyfmt` for the number formatting).
//!
//! Entry points: [`Renderer::render_clip`] for one clip, [`render_clips`] for a job's
//! clips in parallel. Errors carry "video render failed" (or "not enough disk space
//! to save clips"); cancellation kills FFmpeg and returns `EngineError::Cancelled`.
pub mod captions;
pub mod clip_editor;
pub mod fonts;
pub mod layout_renderer;
pub mod process;
pub mod pyfmt;
pub mod service;
pub mod video_speed;

#[cfg(test)]
mod tests;

pub use captions::{caption_style_for, get_caption_preset, CaptionStyle, CAPTION_PRESETS, DEFAULT_CAPTION_PRESET};
pub use clip_editor::{TimeMap, NATURAL, TIGHT};
pub use process::{MediaOutput, MediaRunner, SystemRunner};
pub use service::{
    default_analyzer, filter_transcript_for_clip, get_landscape_dimensions, get_output_dimensions,
    max_concurrent_renders, render_clips, render_error, AnalyzeFn, RenderRequest, RenderResult, Renderer, DISK_FULL,
    RENDER_FAILED,
};
pub use video_speed::{scaled_duration_ms, validate_video_speed};
