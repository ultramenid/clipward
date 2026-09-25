"""
Clip Editor - decides which parts of a clip's timeline to keep.

"Tight" pacing removes dead air and filler words ("um", "uh") the way a
short-form editor would, while protecting moments that only look like
silence: screen demos, laughter and applause.

All times here are milliseconds from the start of the render window. The
result is a list of keep intervals plus a TimeMap that converts window time
to output time, which captions, layout switches and overlays share so
everything stays in sync after the cuts.
"""

import re
from dataclasses import dataclass
from typing import Optional

from clip_engine.services.layout_analyzer import ClipLayoutPlan, LayoutType, ShotLayout
from clip_engine.services.transcription_service import TranscriptSegment, TranscriptWord


class Pacing:
    TIGHT = "tight"      # cut dead air and filler words
    NATURAL = "natural"  # keep the original timing

    ALL = (TIGHT, NATURAL)


# Only pure hesitation sounds: words like "like", "so" or "ah" often carry meaning.
FILLER_WORDS = frozenset({"um", "uh", "uhm", "umm", "uhh", "erm", "er", "hmm", "hm", "mm", "mhm"})

# Longest pause kept as-is, per layout. Silence on a talking head is dead air;
# on a screen recording it is usually something happening on screen.
MAX_PAUSE_MS = {
    LayoutType.TALKING_HEAD: 700,
    LayoutType.TWO_SHOT: 700,
    LayoutType.SCREEN_CAM: 2000,
    LayoutType.SCREEN: None,  # never cut
}
# How much of a removed pause survives (split across both sides of the cut),
# so speech keeps a natural breath instead of machine-gun jump cuts.
KEEP_PAUSE_MS = 260
# Silence kept before the first and after the last word.
LEAD_IN_MS = 120
TAIL_MS = 380
# Cuts shorter than this aren't worth a jump; pieces shorter than this look glitchy.
MIN_CUT_MS = 200
MIN_PIECE_MS = 400
# Cut points land on this grid so every video piece is a whole number of frames.
FRAME_MS = 1000 / 30
# Longform episodes breathe more: a 700 ms limit over 15 minutes is hundreds
# of jump cuts. Pauses up to this stay, and a cut one keeps more of its air.
LONGFORM_MIN_PAUSE_MS = 1300
LONGFORM_KEEP_PAUSE_MS = 450


def is_filler(word: str) -> bool:
    return re.sub(r"[^a-z]", "", word.lower()) in FILLER_WORDS


def _snap(t: float) -> int:
    return int(round(round(t / FRAME_MS) * FRAME_MS))


@dataclass
class WindowWord:
    start_ms: int
    end_ms: int
    text: str


def window_words(segments: list[TranscriptSegment], window_start_ms: int, window_ms: int) -> list[WindowWord]:
    """Words inside the window, in window time."""
    words: list[WindowWord] = []
    for seg in segments:
        for w in seg.words:
            s, e = w.start_time_ms - window_start_ms, w.end_time_ms - window_start_ms
            if e > 0 and s < window_ms:
                words.append(WindowWord(max(0, s), min(window_ms, e), w.word))
    words.sort(key=lambda w: w.start_ms)
    return words


def reaction_intervals(segments: list[TranscriptSegment], window_start_ms: int, window_ms: int) -> list[tuple[int, int]]:
    """Gaps right after sentences that drew a reaction (laughter, applause)."""
    protected = []
    ordered = sorted(segments, key=lambda s: s.start_time_ms)
    for seg, nxt in zip(ordered, ordered[1:] + [None]):
        if not getattr(seg, "audio_events", None):
            continue
        start = seg.end_time_ms - window_start_ms
        end = (nxt.start_time_ms if nxt else seg.end_time_ms + 3000) - window_start_ms
        if end > 0 and start < window_ms:
            protected.append((max(0, start), min(window_ms, end)))
    return protected


# Pause threshold when shot content is unknown (analysis failed or skipped):
# the screen+webcam limit, so a silent demo isn't jump-cut like dead air.
UNKNOWN_MAX_PAUSE_MS = MAX_PAUSE_MS[LayoutType.SCREEN_CAM]


def _max_pause_at(plan: Optional[ClipLayoutPlan], t_ms: int) -> Optional[int]:
    """Pause threshold for the shot playing at t_ms."""
    if plan is None:
        return UNKNOWN_MAX_PAUSE_MS
    shot: Optional[ShotLayout] = next((s for s in plan.shots if s.start_ms <= t_ms < s.end_ms), None)
    layout = (shot.detected_layout or shot.layout) if shot else LayoutType.TALKING_HEAD
    return MAX_PAUSE_MS.get(layout, MAX_PAUSE_MS[LayoutType.TALKING_HEAD])


def compute_keep_intervals(
    words: list[WindowWord],
    window_ms: int,
    plan: Optional[ClipLayoutPlan] = None,
    protected: Optional[list[tuple[int, int]]] = None,
    longform: bool = False,
) -> list[tuple[int, int]]:
    """Intervals of the window to keep under tight pacing.

    `longform` keeps pauses up to LONGFORM_MIN_PAUSE_MS (fillers are still cut).
    """
    protected = protected or []
    keep_pause_ms = LONGFORM_KEEP_PAUSE_MS if longform else KEEP_PAUSE_MS

    def pause_limit(t_ms: int) -> Optional[int]:
        threshold = _max_pause_at(plan, t_ms)
        if longform and threshold is not None:
            return max(threshold, LONGFORM_MIN_PAUSE_MS)
        return threshold

    spoken = [w for w in words if not is_filler(w.text)]
    if not spoken:
        return [(0, window_ms)]

    cuts: list[tuple[int, int]] = []

    def propose(gap_start: int, gap_end: int, keep_ms: int, threshold: Optional[int]) -> None:
        if threshold is None or gap_end - gap_start <= max(threshold, keep_ms):
            return
        cuts.append((gap_start + keep_ms // 2, gap_end - keep_ms // 2))

    # Lead-in and tail silence.
    first, last = spoken[0], spoken[-1]
    lead_threshold = pause_limit(first.start_ms)
    if lead_threshold is not None and first.start_ms > LEAD_IN_MS + MIN_CUT_MS:
        cuts.append((0, first.start_ms - LEAD_IN_MS))
    tail_threshold = pause_limit(last.end_ms)
    if tail_threshold is not None and window_ms - last.end_ms > TAIL_MS + MIN_CUT_MS:
        cuts.append((last.end_ms + TAIL_MS, window_ms))

    # Pauses between words. A gap that swallowed a filler word is always
    # compressed; plain pauses only past the layout's threshold.
    all_words = sorted(words, key=lambda w: w.start_ms)
    for prev, nxt in zip(spoken, spoken[1:]):
        fillers = [
            w for w in all_words
            if prev.end_ms <= w.start_ms and w.end_ms <= nxt.start_ms and is_filler(w.text)
        ]
        threshold = pause_limit(prev.end_ms)
        if fillers and threshold is not None:
            threshold = 0
        before = len(cuts)
        propose(prev.end_ms, nxt.start_ms, keep_pause_ms, threshold)
        if fillers and len(cuts) > before:
            # Frame rounding must remove each filler in full. Otherwise its
            # first/last syllable can remain audible after the jump cut.
            cut_start, cut_end = cuts[-1]
            cuts[-1] = (
                min(cut_start, fillers[0].start_ms - FRAME_MS),
                max(cut_end, fillers[-1].end_ms + FRAME_MS),
            )

    # Never cut into protected moments; drop cuts too short to matter.
    trimmed: list[tuple[int, int]] = []
    for start, end in sorted(cuts):
        for p_start, p_end in protected:
            if start < p_end and end > p_start:
                if p_start - start >= MIN_CUT_MS:
                    trimmed.append((start, p_start))
                start = max(start, p_end)
                if start >= end:
                    break
        if end - start >= MIN_CUT_MS:
            trimmed.append((start, end))

    # Snap to the frame grid, then invert cuts into keeps.
    keeps: list[tuple[int, int]] = []
    cursor = 0
    for start, end in sorted(trimmed):
        start = _snap(max(start, cursor))
        # A cut through the end of the window must stay flush with that end.
        # Snapping it backward creates a sub-frame tail that the sliver merge
        # then glues to the previous piece, undoing the whole tail cut.
        end = window_ms if end >= window_ms else _snap(end)
        if end - start < MIN_CUT_MS:
            continue
        if start > cursor:
            keeps.append((cursor, start))
        cursor = end
    if cursor < window_ms:
        keeps.append((cursor, window_ms))

    # Absorb slivers: a tiny piece is glued back to its neighbour.
    merged: list[tuple[int, int]] = []
    for piece in keeps:
        if merged and piece[1] - piece[0] < MIN_PIECE_MS:
            # Restore the gap before the sliver rather than show a flash.
            merged[-1] = (merged[-1][0], piece[1])
        else:
            merged.append(piece)
    if len(merged) > 1 and merged[0][1] - merged[0][0] < MIN_PIECE_MS:
        merged[:2] = [(merged[0][0], merged[1][1])]
    return merged or [(0, window_ms)]


def subtract_intervals(
    keeps: list[tuple[int, int]],
    cuts: list[tuple[int, int]],
) -> list[tuple[int, int]]:
    """`keeps` with every `cuts` interval removed; pieces under MIN_PIECE_MS drop.

    Used for planner-chosen skips (tangents cut out of a longform episode),
    which apply on top of pacing and at natural timing alike.
    """
    result = keeps
    for c_start, c_end in sorted(cuts):
        c_start, c_end = _snap(c_start), _snap(c_end)
        if c_end <= c_start:
            continue
        pieces = []
        for k_start, k_end in result:
            if c_end <= k_start or c_start >= k_end:
                pieces.append((k_start, k_end))
                continue
            if c_start - k_start >= MIN_PIECE_MS:
                pieces.append((k_start, c_start))
            if k_end - c_end >= MIN_PIECE_MS:
                pieces.append((c_end, k_end))
        result = pieces
    return result or keeps


class TimeMap:
    """Maps window time to output time for a list of keep intervals."""

    def __init__(self, keeps: list[tuple[int, int]], window_ms: Optional[int] = None):
        self.keeps = keeps
        # The final keep can end before the window does when only trailing
        # silence was cut. The keeps alone cannot reveal that tail cut.
        self.window_ms = window_ms if window_ms is not None else (keeps[-1][1] if keeps else 0)
        self._offsets = []
        total = 0
        for start, end in keeps:
            self._offsets.append(total)
            total += end - start
        self.output_ms = total

    @property
    def removed_ms(self) -> int:
        return self.window_ms - self.output_ms

    @property
    def has_cuts(self) -> bool:
        return self.removed_ms > 0

    @property
    def cut_count(self) -> int:
        if not self.has_cuts or not self.keeps:
            return 0
        return (
            len(self.keeps) - 1
            + int(self.keeps[0][0] > 0)
            + int(self.keeps[-1][1] < self.window_ms)
        )

    def to_output(self, t_ms: int) -> Optional[int]:
        """Output time of a window instant, or None if it was cut."""
        for (start, end), offset in zip(self.keeps, self._offsets):
            if start <= t_ms <= end:
                return offset + t_ms - start
        return None

    def to_output_clamped(self, t_ms: int) -> int:
        """Output time of a window instant; cut instants map to the next kept frame."""
        for (start, end), offset in zip(self.keeps, self._offsets):
            if t_ms < start:
                return offset
            if t_ms <= end:
                return offset + t_ms - start
        return self.output_ms

    def pieces_within(self, start_ms: int, end_ms: int) -> list[tuple[int, int]]:
        """Kept sub-intervals of [start_ms, end_ms)."""
        pieces = []
        for k_start, k_end in self.keeps:
            s, e = max(start_ms, k_start), min(end_ms, k_end)
            if e - s > 0:
                pieces.append((s, e))
        return pieces


def remap_plan(plan: ClipLayoutPlan, time_map: TimeMap) -> ClipLayoutPlan:
    """Copy of the plan with shot boundaries in output time (for overlays/captions)."""
    shots = []
    for shot in plan.shots:
        start, end = time_map.to_output_clamped(shot.start_ms), time_map.to_output_clamped(shot.end_ms)
        if end > start:
            copy = ShotLayout(**{**shot.__dict__})
            copy.start_ms, copy.end_ms = start, end
            shots.append(copy)
    if shots:
        shots[-1].end_ms = time_map.output_ms
    return ClipLayoutPlan(
        shots=shots or plan.shots[:1],
        source_width=plan.source_width,
        source_height=plan.source_height,
        vision_cost_usd=plan.vision_cost_usd,
    )


def remap_segments(
    segments: list[TranscriptSegment],
    window_start_ms: int,
    time_map: TimeMap,
) -> list[TranscriptSegment]:
    """Transcript with filler words dropped and word times moved to output time.

    Times stay absolute (window_start + output offset) so the caption
    generator's clip-relative math is unchanged.
    """
    remapped: list[TranscriptSegment] = []
    for seg in segments:
        if not seg.words:
            # Segment-level transcript (no word timings): keep it whole.
            start = time_map.to_output_clamped(seg.start_time_ms - window_start_ms)
            end = time_map.to_output_clamped(seg.end_time_ms - window_start_ms)
            text = seg.text
            if text and end > start:
                remapped.append(TranscriptSegment(
                    start_time_ms=window_start_ms + start, end_time_ms=window_start_ms + end,
                    text=text, speaker_label=seg.speaker_label,
                ))
            continue
        words = []
        for w in seg.words:
            # Only hide a filler when the corresponding audio was removed.
            # Protected reactions and short gaps can leave it audible.
            relative_start = w.start_time_ms - window_start_ms
            relative_end = w.end_time_ms - window_start_ms
            if not time_map.pieces_within(relative_start, relative_end):
                continue
            start = time_map.to_output(relative_start)
            end = time_map.to_output(relative_end)
            if start is None and end is None:
                continue
            start = start if start is not None else time_map.to_output_clamped(w.start_time_ms - window_start_ms)
            end = end if end is not None else start + 1
            words.append(TranscriptWord(
                word=w.word,
                start_time_ms=window_start_ms + start,
                end_time_ms=window_start_ms + max(end, start + 1),
            ))
        if not words:
            continue
        remapped.append(TranscriptSegment(
            start_time_ms=words[0].start_time_ms,
            end_time_ms=words[-1].end_time_ms,
            text=" ".join(w.word for w in words),
            speaker_label=seg.speaker_label,
            words=words,
            audio_events=list(getattr(seg, "audio_events", []) or []),
        ))
    return remapped
