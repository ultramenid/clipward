"""Offline tests for whisper.cpp transcription: word timing, grouping and failures."""

import asyncio
import sys
import types
from types import SimpleNamespace

import pytest

from clip_engine.error_policy import safe_failure_code, safe_processing_error
from clip_engine.services.transcription_service import (
    TranscriptionError,
    TranscriptionService,
    TranscriptWord,
    group_words,
    normalize_keyterms,
    whisper_model_label,
)


def service_with_model(tmp_path, segments, *, fail=False):
    """A TranscriptionService whose whisper.cpp returns `segments` (t0, t1 in 10 ms units, text)."""
    model = tmp_path / "ggml-small.bin"
    model.write_bytes(b"model")
    audio = tmp_path / "audio.wav"
    audio.write_bytes(b"audio")
    seen = {}

    class Model:
        def __init__(self, path, **params):
            seen["path"], seen["params"] = path, params

        def transcribe(self, media, new_segment_callback=None):
            if fail:
                raise RuntimeError("private decoder detail")
            out = [SimpleNamespace(t0=t0, t1=t1, text=text) for t0, t1, text in segments]
            for segment in out:
                new_segment_callback(segment)
            return out

    service = TranscriptionService.__new__(TranscriptionService)
    service.settings = SimpleNamespace(whisper_model_path=str(model))
    service.progress_callback = None
    service._audio_duration = lambda path: 10.0
    return service, str(audio), seen, {"pywhispercpp.model": types.SimpleNamespace(Model=Model)}


def test_whisper_words_land_on_the_source_timeline_and_group_into_sentences(tmp_path, monkeypatch):
    service, audio, seen, modules = service_with_model(tmp_path, [
        (0, 40, " So"), (40, 90, " anyway."), (90, 100, " [BLANK_AUDIO]"),
        (500, 540, " Next"), (540, 600, " (music)"), (600, 650, " part"),
    ])
    monkeypatch.setitem(sys.modules, "pywhispercpp.model", modules["pywhispercpp.model"])
    result = asyncio.run(service.transcribe_audio(audio, keyterms=["Acme"], timeline_offset_seconds=12.5))
    # 10 ms units, shifted by where the audio starts in the source.
    assert [(w.word, w.start_time_ms, w.end_time_ms) for s in result.segments for w in s.words] == [
        ("So", 12500, 12900), ("anyway.", 12900, 13400), ("Next", 17500, 17900), ("part", 18500, 19000),
    ]
    assert [s.text for s in result.segments] == ["So anyway.", "Next part"]
    assert result.model == "whisper.cpp small"
    assert result.api_costs.estimated_cost_usd == 0.0
    assert seen["params"]["token_timestamps"] is True and seen["params"]["max_len"] == 1
    assert seen["params"]["initial_prompt"] == "Vocabulary: Acme"


def test_missing_whisper_model_fails_with_its_own_code(tmp_path):
    service = TranscriptionService.__new__(TranscriptionService)
    service.settings = SimpleNamespace(whisper_model_path=str(tmp_path / "missing.bin"))
    audio = tmp_path / "audio.wav"
    audio.write_bytes(b"audio")
    with pytest.raises(TranscriptionError) as raised:
        asyncio.run(service.transcribe_audio(str(audio)))
    assert safe_failure_code(raised.value) == "transcription.whisper_model_missing"
    assert safe_processing_error(raised.value) == "Whisper model is not downloaded"


def test_decoder_failures_do_not_expose_details(tmp_path, monkeypatch):
    service, audio, _, modules = service_with_model(tmp_path, [], fail=True)
    monkeypatch.setitem(sys.modules, "pywhispercpp.model", modules["pywhispercpp.model"])
    with pytest.raises(TranscriptionError) as raised:
        asyncio.run(service.transcribe_audio(audio))
    assert safe_failure_code(raised.value) == "transcription.whisper_failed"
    assert "private decoder detail" not in str(raised.value)


def test_long_pauses_split_segments_even_mid_sentence():
    words = [TranscriptWord("wait", 0, 300), TranscriptWord("for", 2400, 2600), TranscriptWord("it.", 2600, 2900)]
    assert [s.text for s in group_words(words)] == ["wait", "for it."]
    assert group_words([]) == []


def test_model_label():
    assert whisper_model_label("/x/ggml-large-v3-turbo-q5_0.bin") == "whisper.cpp large-v3-turbo-q5_0"
    assert whisper_model_label("") == "whisper.cpp model"


def test_missing_word_timestamps_has_its_own_code():
    error = TranscriptionError("Transcription response did not include word timestamps", reason="missing_word_timestamps")
    assert safe_processing_error(error) == "Transcription response lacked word timestamps"
    assert safe_failure_code(error) == "transcription.missing_word_timestamps"


class TestNormalizeKeyterms:
    def test_enforces_provider_limits(self):
        terms = ["  Clipward  ", "clipward", "one two three four five six", "bad<chars>", "x" * 80, "", None]
        cleaned = normalize_keyterms(terms)
        assert cleaned[0] == "Clipward"
        assert "clipward" not in cleaned
        assert not any("six" in t or "<" in t for t in cleaned)
        assert all(len(t) < 50 for t in cleaned)
        assert len(cleaned) == 2

    def test_caps_count(self):
        assert len(normalize_keyterms([f"term{i}" for i in range(1500)])) == 200
