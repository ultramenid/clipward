//! Sentence/word boundary helpers used to snap planned clip edges. They live in
//! `transcribe` (as they lived in `transcription_service.py`); re-exported here.
pub use crate::transcribe::{
    find_sentence_end_boundary, find_sentence_start_boundary, last_sentence_end_between,
    next_start_at_or_after, SearchDirection as Direction, SENTENCE_END_TOLERANCE_MS,
};
use crate::types::TranscriptSegment;

/// Forward search with the default tolerance.
pub fn sentence_end_forward(
    segments: &[TranscriptSegment],
    timestamp_ms: i64,
    max_extension_ms: i64,
) -> i64 {
    find_sentence_end_boundary(
        segments,
        timestamp_ms,
        max_extension_ms,
        Direction::Forward,
        SENTENCE_END_TOLERANCE_MS,
    )
}
