# Clipward patch to tract-core 0.23.8

This directory is the crates.io `tract-core` 0.23.8 source, used through
`[patch.crates-io]` in the workspace `Cargo.toml`, with one change in
`src/ops/cnn/conv/depth_wise.rs`. Each changed spot is marked `CLIPWARD PATCH`.
Licensing is unchanged (MIT or Apache-2.0, see `LICENSE*`). The crates.io
registry metadata files (`.cargo_vcs_info.json`, `Cargo.lock`,
`Cargo.toml.orig`) were dropped; everything else is as published.

## What

Upstream sends depthwise convolution zones with 1 to 4 kernel taps to an
unrolled loop (and, for f32 on aarch64, to tract-linalg's NEON kernel). Zones
with more taps fall back to `inner_loop_generic`, a scalar per-output-point
loop. A 3x3 kernel with padding has 9-tap centre zones and 6-tap edge zones,
so every 3x3 depthwise conv ran through the scalar fallback.

The patch:

1. Routes 6- and 9-tap zones to the existing `process_zone_n` (two new match
   arms in `process_zone_*`).
2. In `process_zone_n`, for more than 4 taps on rows that are contiguous in
   both input and output, computes blocks of 32, then 8, then 1 outputs with
   a plain Rust helper (`clipward_row_block`) that LLVM vectorises (NEON on
   aarch64, SSE on x86_64). Wide blocks keep several independent add chains
   in flight. Other strides use the code that was already there.

## Why it is bit-identical

Each output is still `bias + k[0]*x[0] + k[1]*x[1] + ...`, summed in the same
tap order as `inner_loop_generic`, with a separate multiply and add. The
vector lanes are different output points, never partial sums of one point,
and Rust does not contract `a + b * c` into a fused multiply-add (the
aarch64 build has `fmul`/`fadd`, no `fmla`, in these functions). The
tract-linalg NEON kernel, which does fuse, is not used for these zones. The
existing fallbacks they can still reach on other strides sum in the same
order too (one multiplies `x * k` instead of `k * x`, which gives the same
bits).

Checked on Clipward's YuNet face detector (face_detection_yunet_2023mar.onnx):
all 12 raw output tensors compared with `f32::to_bits` against unpatched
0.23.8 on 320 real frames at 640x360 (padded 640x384) and 40 frames at
640x1138 (portrait, which takes the non-contiguous path): identical. The
layout_eval plans (`crates/engine/examples/layout_eval.json`) are identical.

## Measured gain

YuNet at 640x384, one thread, Apple silicon, 2026-10-10 (busy machine,
interleaved runs, medians): CPU time per frame 77.4 ms before, 34.3 ms after
(2.26x); wall time per frame 683 ms before, 268 ms after. Before the patch,
59% of YuNet time was in `inner_loop_generic`. The 9-tap path is about a
quarter of what is left; maxpool and matmul are most of the rest.

## Dropping it

When a tract release vectorises depthwise zones with more than 4 taps
(see `process_zone` in `src/ops/cnn/conv/depth_wise.rs`), delete
`vendor/tract-core` and the `[patch.crates-io]` entry, bump `tract-onnx` in
`crates/engine/Cargo.toml`, and re-run the same bit-for-bit comparison. If
the new release fuses multiply-add there, outputs will differ slightly from
today's; that changes framing, so re-check with the layout_eval example.
