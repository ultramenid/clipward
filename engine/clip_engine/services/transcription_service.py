"""
Transcription Service - offline word-timed transcription with whisper.cpp.
"""

import asyncio
import math
import logging
import os
import subprocess
from dataclasses import dataclass, field
from typing import Callable, Optional

from clip_engine.config import get_settings
from clip_engine.services.media_process import MEDIA_INPUT_OPTIONS, run_media

logger = logging.getLogger(__name__)


@dataclass
class TranscriptWord:
    """Word-level timing for precise caption display."""
    
    word: str
    start_time_ms: int
    end_time_ms: int


@dataclass
class TranscriptSegment:
    """A segment of transcribed audio with timing."""
    
    start_time_ms: int
    end_time_ms: int
    text: str
    speaker_label: Optional[str] = None
    words: list[TranscriptWord] = field(default_factory=list)
    # Non-speech audio events inside this segment, e.g. "(laughter)". Fed to
    # the clip planner as engagement signals; never rendered as captions.
    audio_events: list[str] = field(default_factory=list)


@dataclass
class TranscriptionApiCosts:
    """Cost tracking for transcription API calls."""

    provider: str = "local"
    model: str = "whisper.cpp"
    audio_duration_seconds: float = 0.0
    estimated_cost_usd: float = 0.0
    attempts: int = 0
    cost_incomplete: bool = False


# Context kept around a requested time range so sentence boundaries at its edges still resolve.
TRANSCRIPTION_RANGE_PAD_SECONDS = 5.0
WAV_INPUT_OPTIONS = ["-protocol_whitelist", "file,pipe,fd", "-format_whitelist", "wav"]


@dataclass
class TranscriptionResult:
    """Result of transcription operation."""
    
    segments: list[TranscriptSegment]
    full_text: str
    language: Optional[str] = None
    duration_seconds: Optional[float] = None
    provider: str = "local"
    model: str = "whisper.cpp"
    api_costs: Optional[TranscriptionApiCosts] = None


# Sentence-ending punctuation marks
SENTENCE_END_PUNCTUATION = {'.', '!', '?', '。', '！', '？'}

# The planner sees times rounded to 0.1 s, so a clip end it copies from a
# transcript line can land up to 50 ms after the real sentence end. Treat a
# sentence end this close to the requested time as the intended one.
SENTENCE_END_TOLERANCE_MS = 300

# Conservative application limits for vocabulary hints.
KEYTERM_MAX_CHARS = 49
KEYTERM_MAX_WORDS = 5
KEYTERM_MAX_COUNT = 200
KEYTERM_FORBIDDEN_CHARS = set("<>{}[]\\")


def normalize_keyterms(keyterms: Optional[list[str]]) -> list[str]:
    """Clean vocabulary hints before passing them to the provider.

    Trims, drops empties and terms containing forbidden characters or more
    than five words, truncates to under 50 chars, and dedupes
    case-insensitively.
    """
    normalized: list[str] = []
    seen: set[str] = set()
    for term in keyterms or []:
        if not isinstance(term, str):
            continue
        trimmed = " ".join(term.split())[:KEYTERM_MAX_CHARS].strip()
        if not trimmed or any(c in KEYTERM_FORBIDDEN_CHARS for c in trimmed):
            continue
        if len(trimmed.split()) > KEYTERM_MAX_WORDS:
            continue
        key = trimmed.lower()
        if key in seen:
            continue
        seen.add(key)
        normalized.append(trimmed)
        if len(normalized) >= KEYTERM_MAX_COUNT:
            break
    return normalized


def _format_speaker_label(speaker_id: object) -> Optional[str]:
    """MAI speaker indices are zero-based, including a valid speaker 0."""
    if type(speaker_id) is int and speaker_id >= 0:
        return f"S{speaker_id + 1}"
    if isinstance(speaker_id, str) and speaker_id.isdigit():
        return f"S{int(speaker_id) + 1}"
    return None


def find_sentence_end_boundary(
    segments: list[TranscriptSegment],
    timestamp_ms: int,
    max_extension_ms: int = 5000,
    search_direction: str = "forward",
    tolerance_ms: int = SENTENCE_END_TOLERANCE_MS,
) -> int:
    """
    Find the nearest sentence boundary after a given timestamp.
    
    Uses word-level timing and punctuation detection to find natural
    sentence endings, preventing clips from cutting off mid-sentence.
    
    Args:
        segments: List of TranscriptSegment with word-level timing
        timestamp_ms: The timestamp to search from (in milliseconds)
        max_extension_ms: Maximum milliseconds to extend beyond timestamp (default 5000ms = 5s)
        search_direction: "forward" to find end after timestamp, "backward" to find before
        tolerance_ms: Forward search first takes the nearest sentence end within
            this many ms either side of the timestamp (absorbs display rounding)
        
    Returns:
        Adjusted timestamp in milliseconds at sentence boundary,
        or original timestamp if no boundary found within max_extension
    """
    if not segments:
        return timestamp_ms
    
    # Collect all words with their timing from segments that might contain our timestamp
    candidate_words: list[tuple[str, int]] = []  # (word, end_time_ms)
    
    for segment in segments:
        # Only consider segments that could contain words near our timestamp
        segment_start = segment.start_time_ms
        segment_end = segment.end_time_ms
        
        if search_direction == "forward":
            # For forward search, look at segments from timestamp onwards
            if segment_end < timestamp_ms - tolerance_ms:
                continue
            if segment_start > timestamp_ms + max_extension_ms:
                break
        else:
            # For backward search, look at segments before timestamp
            if segment_start > timestamp_ms:
                continue
            if segment_end < timestamp_ms - max_extension_ms:
                continue
        
        # If segment has word-level timing, use it
        if segment.words:
            for word in segment.words:
                candidate_words.append((word.word, word.end_time_ms))
        else:
            # Fall back to segment-level: treat segment text as ending at segment end
            # Check if segment text ends with sentence punctuation
            text = segment.text.strip()
            if text:
                candidate_words.append((text, segment_end))
    
    if not candidate_words:
        return timestamp_ms
    
    if search_direction == "forward":
        # A sentence end right at the requested time is the intended one, even
        # if rounding put it slightly before; don't extend into the next one.
        near = [
            (abs(end_time - timestamp_ms), end_time)
            for word, end_time in candidate_words
            if abs(end_time - timestamp_ms) <= tolerance_ms and _ends_sentence(word)
        ]
        if near:
            return min(near)[1]

        # Find first sentence boundary after timestamp within max_extension
        for word, end_time in candidate_words:
            if end_time < timestamp_ms:
                continue
            if end_time > timestamp_ms + max_extension_ms:
                # Exceeded max extension, return original
                logger.debug(
                    f"Sentence boundary: no boundary found within {max_extension_ms}ms of {timestamp_ms}ms, "
                    f"using original timestamp"
                )
                return timestamp_ms
            
            # Check if word ends with sentence punctuation
            word_stripped = word.rstrip()
            if word_stripped and word_stripped[-1] in SENTENCE_END_PUNCTUATION:
                logger.debug(
                    f"Sentence boundary: found end at {end_time}ms, "
                    f"extended from {timestamp_ms}ms by {end_time - timestamp_ms}ms"
                )
                return end_time
    else:
        # For backward search, find last sentence boundary before timestamp
        last_boundary = None
        for word, end_time in candidate_words:
            if end_time > timestamp_ms:
                continue
            if end_time < timestamp_ms - max_extension_ms:
                continue
            
            word_stripped = word.rstrip()
            if word_stripped and word_stripped[-1] in SENTENCE_END_PUNCTUATION:
                last_boundary = end_time
        
        if last_boundary is not None:
            logger.debug(
                f"Sentence boundary (backward): found end at {last_boundary}ms, "
                f"adjusted from {timestamp_ms}ms"
            )
            return last_boundary
    
    # No sentence boundary found, return original
    return timestamp_ms


def _ends_sentence(word: str) -> bool:
    stripped = word.rstrip()
    return bool(stripped) and stripped[-1] in SENTENCE_END_PUNCTUATION


def last_sentence_end_between(
    segments: list[TranscriptSegment],
    earliest_ms: int,
    latest_ms: int,
) -> Optional[int]:
    """End time of the last sentence finishing inside [earliest_ms, latest_ms].

    Used to shorten an over-long clip to its last complete sentence instead
    of cutting it mid-sentence. Returns None when no sentence ends there.
    """
    last = None
    for segment in segments:
        if segment.end_time_ms < earliest_ms:
            continue
        if segment.start_time_ms > latest_ms:
            break
        timed = [(w.word, w.end_time_ms) for w in segment.words] or [(segment.text, segment.end_time_ms)]
        for word, end_time in timed:
            if earliest_ms <= end_time <= latest_ms and _ends_sentence(word):
                last = end_time
    return last


def next_start_at_or_after(
    segments: list[TranscriptSegment],
    earliest_ms: int,
    sentence_within_ms: int,
) -> Optional[int]:
    """Where a clip that can't start before earliest_ms should start instead.

    The first sentence starting within sentence_within_ms after earliest_ms,
    otherwise the first word starting at or after it (so the clip at least
    doesn't open mid-word). None when nothing starts after earliest_ms.
    """
    first_word = None
    for segment in segments:
        if segment.end_time_ms < earliest_ms:
            continue
        if segment.start_time_ms >= earliest_ms:
            if segment.start_time_ms <= earliest_ms + sentence_within_ms or first_word is None:
                return segment.start_time_ms
            return first_word
        # This sentence straddles earliest_ms: remember its first word after it.
        if first_word is None:
            first_word = next((w.start_time_ms for w in segment.words if w.start_time_ms >= earliest_ms), None)
    return first_word


def find_sentence_start_boundary(
    segments: list[TranscriptSegment],
    timestamp_ms: int,
    max_adjustment_ms: int = 3000,
) -> int:
    """
    Find the nearest sentence/word start boundary before a given timestamp.
    
    This prevents clips from starting mid-word by snapping the start time
    to the beginning of the word or sentence that contains the timestamp.
    
    Args:
        segments: List of TranscriptSegment with word-level timing
        timestamp_ms: The timestamp to search from (in milliseconds)
        max_adjustment_ms: Maximum milliseconds to adjust backward (default 3000ms = 3s)
        
    Returns:
        Adjusted timestamp in milliseconds at word/sentence start,
        or original timestamp if no suitable boundary found
    """
    if not segments:
        return timestamp_ms
    
    # Collect all words with their timing from relevant segments
    all_words: list[tuple[str, int, int]] = []  # (word, start_time_ms, end_time_ms)
    
    for segment in segments:
        segment_start = segment.start_time_ms
        segment_end = segment.end_time_ms
        
        # Only consider segments near our timestamp
        if segment_end < timestamp_ms - max_adjustment_ms:
            continue
        if segment_start > timestamp_ms + 1000:  # Small buffer to catch current word
            break
        
        if segment.words:
            for word in segment.words:
                all_words.append((word.word, word.start_time_ms, word.end_time_ms))
        else:
            # Fall back to segment-level
            all_words.append((segment.text, segment_start, segment_end))
    
    if not all_words:
        return timestamp_ms
    
    # Find the word that contains or is closest before the timestamp
    best_start = None
    best_is_sentence_start = False
    
    for i, (word, start_time, end_time) in enumerate(all_words):
        # Check if this word contains our timestamp (we're cutting mid-word)
        if start_time <= timestamp_ms <= end_time:
            # We're in the middle of this word - snap to its start
            logger.debug(
                f"Start boundary: timestamp {timestamp_ms}ms is mid-word '{word}', "
                f"snapping to word start at {start_time}ms"
            )
            return start_time
        
        # Check if this is just before our timestamp
        if start_time < timestamp_ms and end_time <= timestamp_ms:
            # Check if previous word ended a sentence (so this is a sentence start)
            if i > 0:
                prev_word = all_words[i - 1][0].rstrip()
                if prev_word and prev_word[-1] in SENTENCE_END_PUNCTUATION:
                    # This word starts a new sentence
                    if timestamp_ms - start_time <= max_adjustment_ms:
                        best_start = start_time
                        best_is_sentence_start = True
            
            # If not a sentence start but within range, consider it
            if not best_is_sentence_start and timestamp_ms - start_time <= max_adjustment_ms:
                best_start = start_time
    
    # Also check if timestamp falls between words
    for i, (word, start_time, end_time) in enumerate(all_words):
        if start_time > timestamp_ms:
            # This word starts after our timestamp - we should start at this word
            if start_time - timestamp_ms <= 500:  # Within 500ms after
                logger.debug(
                    f"Start boundary: timestamp {timestamp_ms}ms is between words, "
                    f"snapping to next word start at {start_time}ms"
                )
                return start_time
            break
    
    if best_start is not None:
        if best_is_sentence_start:
            logger.debug(
                f"Start boundary: found sentence start at {best_start}ms, "
                f"adjusted from {timestamp_ms}ms (-{timestamp_ms - best_start}ms)"
            )
        else:
            logger.debug(
                f"Start boundary: snapped to word start at {best_start}ms, "
                f"adjusted from {timestamp_ms}ms (-{timestamp_ms - best_start}ms)"
            )
        return best_start
    
    return timestamp_ms


class TranscriptionService:
    """Offline whisper.cpp speech recognition with word timing for captions."""

    def __init__(self):
        self.settings = get_settings()
        self.progress_callback: Optional[Callable[[str], None]] = None

    def _progress(self, message: str) -> None:
        callback = getattr(self, "progress_callback", None)
        if callback:
            try:
                callback(message)
            except Exception:
                logger.warning("Could not report transcription progress")

    async def transcribe(
        self,
        video_path: str,
        work_dir: str,
        language: Optional[str] = None,
        translate_to_english: bool = False,
        keyterms: Optional[list[str]] = None,
        start_seconds: Optional[float] = None,
        end_seconds: Optional[float] = None,
    ) -> TranscriptionResult:
        """
        Transcribe a video file by extracting audio first.

        Args:
            video_path: Path to video file
            work_dir: Working directory for temporary files
            language: Optional language code (auto-detected if not specified)
            translate_to_english: Whether to translate to English
            keyterms: Optional vocabulary biasing list (e.g., brand names,
                product names, jargon) forwarded as MAI phrase hints.
            start_seconds, end_seconds: Optional source window. Only that part
                of the audio (plus a little context) is extracted and sent to
                the provider; timestamps still refer to the full source.

        Returns:
            TranscriptionResult with segments and word-level timing
        """
        if not os.path.isfile(video_path):
            raise TranscriptionError("Video file not found", reason="source_missing")

        window_start = 0.0
        if start_seconds is not None and start_seconds > 0:
            window_start = max(0.0, start_seconds - TRANSCRIPTION_RANGE_PAD_SECONDS)
        window_end = None if end_seconds is None else max(window_start, end_seconds + TRANSCRIPTION_RANGE_PAD_SECONDS)

        # Extract audio from video
        audio_path = os.path.join(work_dir, "audio_extracted.wav")
        await self._extract_audio_from_video(video_path, audio_path, window_start, window_end)

        try:
            return await self.transcribe_audio(
                audio_path=audio_path,
                language=language,
                translate_to_english=translate_to_english,
                keyterms=keyterms,
                timeline_offset_seconds=window_start,
            )
        finally:
            # Cleanup extracted audio
            if os.path.exists(audio_path):
                try:
                    os.remove(audio_path)
                except Exception:
                    pass

    async def _extract_audio_from_video(
        self, video_path: str, audio_path: str, start_seconds: float = 0.0, end_seconds: Optional[float] = None,
    ) -> None:
        """Extract 16 kHz PCM WAV, which MAI Transcribe 2 accepts through OpenRouter.

        OpenRouter's MAI provider rejects the AAC/M4A produced here with HTTP 400.
        Audio stays at its original speed so timestamps map directly to video.
        A window trims the source before decoding; the output then starts at
        `start_seconds` of the source and the caller shifts timestamps back.
        """
        logger.info(f"Extracting audio from video: {video_path}")

        cmd = [
            "ffmpeg", "-nostdin", "-nostats", "-v", "error",
            "-y",
            "-protocol_whitelist", "file,pipe,fd", "-format_whitelist", "mov,matroska,webm,avi,flv,mpegts",
            *(["-ss", f"{start_seconds:.3f}"] if start_seconds > 0 else []),
            *(["-t", f"{end_seconds - start_seconds:.3f}"] if end_seconds is not None else []),
            "-i", video_path,
            "-map", "0:a:0",  # Match the track used by the render graph.
            "-vn",  # No video
            # Materialize silence at delayed starts/packet gaps so word times
            # remain on the video's clock in the PCM sent to the API.
            "-af", "aresample=16000:async=1:first_pts=0:min_hard_comp=0.001",
            "-acodec", "pcm_s16le",
            "-ar", "16000",  # 16kHz sample rate
            "-ac", "1",  # Mono
            audio_path,
        ]

        # Use run_in_executor for Windows compatibility
        loop = asyncio.get_event_loop()
        result = await loop.run_in_executor(
            None,
            lambda: run_media(cmd)
        )

        if result.returncode != 0:
            # An audio-less video is a valid input for visual-only planning.
            # Confirm that case with ffprobe; other FFmpeg errors must fail.
            try:
                streams = run_media(
                    ["ffprobe", "-v", "error", "-select_streams", "a",
                     "-show_entries", "stream=index", "-of", "csv=p=0", *MEDIA_INPUT_OPTIONS, video_path],
                    timeout=10,
                )
                if streams.returncode == 0 and not streams.stdout.strip():
                    raise NoAudioTrackError("This video has no audio track")
            except (OSError, subprocess.SubprocessError):
                pass
            error_msg = result.stderr.decode() if result.stderr else "Unknown error"
            raise TranscriptionError(f"Failed to extract audio from video: {error_msg}", reason="audio_extraction_failed")

        if not os.path.exists(audio_path):
            raise TranscriptionError("Audio extraction produced no output file", reason="audio_extraction_empty")

        logger.info(f"Audio extracted to: {audio_path}")

    async def transcribe_audio(
        self,
        audio_path: str,
        language: Optional[str] = None,
        translate_to_english: bool = False,
        keyterms: Optional[list[str]] = None,
        timeline_offset_seconds: float = 0.0,
    ) -> TranscriptionResult:
        """Transcribe offline with whisper.cpp, keeping word times on the source timeline.

        `timeline_offset_seconds` is where the audio file starts within the
        source video, so timestamps come back on the video's clock.
        """
        if not os.path.isfile(audio_path):
            raise TranscriptionError("Audio file not found", reason="audio_missing")
        model_path = self.settings.whisper_model_path
        if not model_path or not os.path.isfile(model_path):
            raise TranscriptionError("Whisper model is not downloaded", reason="whisper_model_missing")
        if translate_to_english:
            raise TranscriptionError("Audio translation is not supported", reason="translation_unsupported")
        duration = await asyncio.to_thread(self._audio_duration, audio_path)
        self._progress("Transcribing audio on this computer...")
        words = await asyncio.to_thread(self._whisper_words, model_path, audio_path, language, keyterms, duration)
        shift_ms = round(timeline_offset_seconds * 1000)
        segments = group_words([TranscriptWord(w.word, w.start_time_ms + shift_ms, w.end_time_ms + shift_ms) for w in words])
        model = whisper_model_label(model_path)
        return TranscriptionResult(
            segments=segments, full_text=" ".join(segment.text for segment in segments),
            language=language if language and language != "auto" else None,
            duration_seconds=duration, provider="local", model=model,
            api_costs=TranscriptionApiCosts(provider="local", model=model, audio_duration_seconds=duration, attempts=1),
        )

    def _whisper_words(
        self, model_path: str, audio_path: str, language: Optional[str],
        keyterms: Optional[list[str]], duration: float,
    ) -> list[TranscriptWord]:
        """Run whisper.cpp with one word per segment (max_len=1 + split_on_word)."""
        try:
            from pywhispercpp.model import Model
        except ImportError:
            raise TranscriptionError("whisper.cpp is not installed", reason="whisper_unavailable") from None
        phrases = normalize_keyterms(keyterms)
        reported = [0]

        def on_segment(segment) -> None:
            percent = min(99, int(segment.t1 / 100 / max(duration, 1) * 100))
            if percent >= reported[0] + 5:
                reported[0] = percent
                self._progress(f"Transcribing audio on this computer... {percent}%")

        try:
            model = Model(
                model_path,
                redirect_whispercpp_logs_to=None,
                n_threads=max(1, min(8, (os.cpu_count() or 4) - 1)),
                print_progress=False, print_realtime=False,
                token_timestamps=True, max_len=1, split_on_word=True,
                language=language if language and language != "auto" else "auto",
                **({"initial_prompt": "Vocabulary: " + ", ".join(phrases)} if phrases else {}),
            )
            raw = model.transcribe(audio_path, new_segment_callback=on_segment)
        except Exception as error:
            logger.error("whisper.cpp transcription failed (%s)", type(error).__name__)
            raise TranscriptionError("Local transcription failed", reason="whisper_failed") from None
        words = []
        for segment in raw:
            text = segment.text.strip()
            # Non-speech markers such as [BLANK_AUDIO] or (music) are not words.
            if not text or (text[0] in "[(" and text[-1] in "])"):
                continue
            # whisper.cpp times are in 10 ms units.
            words.append(TranscriptWord(text, segment.t0 * 10, max(segment.t0, segment.t1) * 10))
        return words

    @staticmethod
    def _audio_duration(path: str) -> float:
        try:
            result = run_media(
                ["ffprobe", "-v", "error", *WAV_INPUT_OPTIONS,
                 "-show_entries", "format=duration", "-of", "default=nw=1:nk=1", path],
                timeout=10, check=True,
            )
            duration = float(result.stdout)
            if not math.isfinite(duration) or not 0 < duration <= 6 * 3600 + 1:
                raise ValueError()
            return duration
        except (OSError, ValueError, subprocess.SubprocessError):
            raise TranscriptionError("Could not determine audio duration", reason="audio_duration_unknown") from None


def whisper_model_label(model_path: str) -> str:
    """"ggml-large-v3-turbo.bin" -> "whisper.cpp large-v3-turbo"."""
    name = os.path.basename(model_path or "")
    return "whisper.cpp " + (name.removeprefix("ggml-").removesuffix(".bin") or "model")


def group_words(words: list[TranscriptWord]) -> list[TranscriptSegment]:
    """Group timed words into sentence segments; a pause over 2 s also ends one."""
    segments: list[TranscriptSegment] = []
    current: list[TranscriptWord] = []

    def flush() -> None:
        if current:
            segments.append(TranscriptSegment(current[0].start_time_ms, current[-1].end_time_ms,
                                              " ".join(w.word for w in current), None, list(current)))
            current.clear()

    for word in words:
        if current and word.start_time_ms - current[-1].end_time_ms > 2000:
            flush()
        current.append(word)
        if word.word[-1] in SENTENCE_END_PUNCTUATION:
            flush()
    flush()
    return segments


class TranscriptionError(Exception):
    """Exception raised when transcription fails."""

    def __init__(self, message: str, *, reason: str = "unknown"):
        self.reason = reason
        super().__init__(message)


class NoAudioTrackError(TranscriptionError):
    """FFprobe confirmed that the source has no audio stream."""
