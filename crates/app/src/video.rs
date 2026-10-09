//! Native media playback for the editor's preview (the `<video>` and `<audio>`
//! elements of the React player), on AVFoundation: hardware decoding, audio in
//! sync, and frames drawn by GPUI's `surface` element without a copy.
//!
//! A `MediaPlayer` holds one file at a time. Video frames come out as NV12
//! full-range pixel buffers, the format GPUI's Metal renderer draws; `poll_frame`
//! picks up the frame for the current display time. Must be used on the main thread.

use core_video::pixel_buffer::CVPixelBuffer;
use gpui_kit::*;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, MainThreadMarker};
use objc2_av_foundation::{
    AVPlayer, AVPlayerActionAtItemEnd, AVPlayerItem, AVPlayerItemStatus, AVPlayerItemVideoOutput,
};
use objc2_core_media::{CMTime, CMTimeFlags};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// `kCVPixelFormatType_420YpCbCr8BiPlanarFullRange` ('420f'): what GPUI's surfaces draw.
const NV12_FULL_RANGE: u32 = u32::from_be_bytes(*b"420f");

fn cm_time(ms: f64) -> CMTime {
    CMTime {
        value: (ms.max(0.0) * 1000.0).round() as i64,
        timescale: 1_000_000,
        flags: CMTimeFlags::Valid,
        epoch: 0,
    }
}

fn cm_zero() -> CMTime {
    CMTime { value: 0, timescale: 1, flags: CMTimeFlags::Valid, epoch: 0 }
}

/// Milliseconds, or None for an invalid or indefinite time.
fn cm_ms(time: CMTime) -> Option<f64> {
    let valid = time.flags.contains(CMTimeFlags::Valid)
        && !time.flags.contains(CMTimeFlags::Indefinite)
        && time.timescale > 0;
    valid.then(|| time.value as f64 * 1000.0 / time.timescale as f64)
}

/// Seconds on the host clock, as `CACurrentMediaTime` returns them.
fn host_seconds() -> f64 {
    unsafe extern "C" {
        fn CACurrentMediaTime() -> f64;
    }
    unsafe { CACurrentMediaTime() }
}

pub struct MediaPlayer {
    player: Retained<AVPlayer>,
    item: Option<Retained<AVPlayerItem>>,
    output: Option<Retained<AVPlayerItemVideoOutput>>,
    path: Option<PathBuf>,
    frame: Option<CVPixelBuffer>,
    /// Seeks in flight; a new one waits until the last lands (as the React player did).
    seeking: Rc<Cell<u32>>,
    playing: bool,
    rate: f32,
    /// A first seek was made since the file opened: a paused player hands out no
    /// frame until something asks for one.
    primed: bool,
}

impl MediaPlayer {
    /// A player; `video` adds a frame output (audio-only players skip decoding pictures).
    pub fn new() -> Self {
        let mtm = MainThreadMarker::new().expect("media players run on the main thread");
        let player = unsafe { AVPlayer::new(mtm) };
        unsafe {
            // Pause on the last frame instead of advancing to nothing.
            player.setActionAtItemEnd(AVPlayerActionAtItemEnd::Pause);
            // Local files: start at once instead of buffering for smooth network playback.
            player.setAutomaticallyWaitsToMinimizeStalling(false);
        }
        Self {
            player,
            item: None,
            output: None,
            path: None,
            frame: None,
            seeking: Rc::new(Cell::new(0)),
            playing: false,
            rate: 1.0,
            primed: false,
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Load `path` (no-op when it's already loaded); None unloads. `video` decodes
    /// pictures as well as sound.
    pub fn open(&mut self, path: Option<&Path>, video: bool) {
        if self.path.as_deref() == path {
            return;
        }
        self.path = path.map(Path::to_path_buf);
        self.frame = None;
        self.output = None;
        self.playing = false;
        self.primed = false;
        unsafe { self.player.pause() };
        let Some(path) = path else {
            unsafe { self.player.replaceCurrentItemWithPlayerItem(None) };
            self.item = None;
            return;
        };
        let mtm = MainThreadMarker::new().expect("media players run on the main thread");
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let item = unsafe { AVPlayerItem::playerItemWithURL(&url, mtm) };
        if video {
            let key = NSString::from_str("PixelFormatType");
            let format = NSNumber::new_u32(NV12_FULL_RANGE);
            let metal_key = NSString::from_str("MetalCompatibility");
            let metal = NSNumber::new_bool(true);
            let attributes: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_retained_objects(
                &[&*key, &*metal_key],
                &[Retained::into_super(Retained::into_super(format)).into(), Retained::into_super(Retained::into_super(metal)).into()],
            );
            let output = unsafe {
                AVPlayerItemVideoOutput::initWithPixelBufferAttributes(
                    AVPlayerItemVideoOutput::alloc(),
                    Some(&attributes),
                )
            };
            unsafe { item.addOutput(&output) };
            self.output = Some(output);
        }
        unsafe { self.player.replaceCurrentItemWithPlayerItem(Some(&item)) };
        self.item = Some(item);
    }

    /// The file is open and decodable.
    pub fn ready(&self) -> bool {
        self.item
            .as_ref()
            .is_some_and(|item| unsafe { item.status() } == AVPlayerItemStatus::ReadyToPlay)
    }

    /// The file could not be played (missing or damaged).
    pub fn failed(&self) -> bool {
        self.item
            .as_ref()
            .is_some_and(|item| unsafe { item.status() } == AVPlayerItemStatus::Failed)
    }

    pub fn duration_ms(&self) -> Option<f64> {
        self.item.as_ref().and_then(|item| cm_ms(unsafe { item.duration() }))
    }

    pub fn current_ms(&self) -> f64 {
        cm_ms(unsafe { self.player.currentTime() }).unwrap_or(0.0)
    }

    pub fn is_seeking(&self) -> bool {
        self.seeking.get() > 0
    }

    /// Played to (within a frame of) the end.
    pub fn ended(&self) -> bool {
        self.duration_ms()
            .is_some_and(|duration| self.current_ms() >= duration - 30.0)
    }

    /// Jump to `ms`; `exact` lands on that frame (slower), otherwise the nearest
    /// keyframe within a little either side.
    pub fn seek(&mut self, ms: f64, exact: bool) {
        if self.item.is_none() {
            return;
        }
        let tolerance = if exact { cm_zero() } else { cm_time(100.0) };
        let seeking = self.seeking.clone();
        seeking.set(seeking.get() + 1);
        let done = block2::RcBlock::new(move |_finished: objc2::runtime::Bool| {
            seeking.set(seeking.get().saturating_sub(1));
        });
        unsafe {
            self.player.seekToTime_toleranceBefore_toleranceAfter_completionHandler(
                cm_time(ms),
                tolerance,
                tolerance,
                &done,
            )
        };
    }

    /// Keep this player on `ms` the way the React player followed the playhead: while
    /// playing it may drift `tolerance_ms` before seeking; paused, it lands on the frame.
    pub fn follow(&mut self, ms: f64, playing: bool, rate: f32, drift_ms: f64) {
        if !self.ready() {
            return;
        }
        let tolerance = if playing { drift_ms } else { 10.0 };
        if !self.primed || (!self.is_seeking() && (self.current_ms() - ms).abs() > tolerance) {
            self.primed = true;
            self.seek(ms, !playing);
        }
        self.set_playing(playing, rate);
    }

    pub fn set_playing(&mut self, playing: bool, rate: f32) {
        if playing == self.playing && rate == self.rate {
            return;
        }
        self.playing = playing;
        self.rate = rate;
        unsafe {
            if playing {
                self.player.setRate(rate);
            } else {
                self.player.pause();
            }
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// 0..1 (AVPlayer can't amplify, as the `<audio>` element couldn't).
    pub fn set_volume(&self, volume: f32) {
        unsafe { self.player.setVolume(volume.clamp(0.0, 1.0)) };
    }

    pub fn set_muted(&self, muted: bool) {
        unsafe { self.player.setMuted(muted) };
    }

    /// Take the frame for the current display time if a new one is ready. True when
    /// the picture changed.
    pub fn poll_frame(&mut self, _window: &mut Window) -> bool {
        let Some(output) = self.output.as_ref() else {
            return false;
        };
        unsafe {
            let at = output.itemTimeForHostTime(host_seconds());
            if !output.hasNewPixelBufferForItemTime(at) {
                return false;
            }
            let Some(buffer) = output.copyPixelBufferForItemTime_itemTimeForDisplay(at, std::ptr::null_mut())
            else {
                return false;
            };
            // Hand our +1 reference to the CoreVideo wrapper GPUI's surface takes.
            let raw = Retained::into_raw(buffer);
            self.frame = Some(<CVPixelBuffer as core_foundation::base::TCFType>::wrap_under_create_rule(
                raw.cast(),
            ));
        }
        true
    }

    /// A decoded frame is there to show.
    pub fn has_frame(&self) -> bool {
        self.frame.is_some()
    }

    /// The latest frame as a GPUI surface filling its box with `fit`.
    pub fn surface(&self, fit: ObjectFit) -> Option<Surface> {
        self.frame.clone().map(|frame| surface(frame).object_fit(fit))
    }
}

impl Drop for MediaPlayer {
    fn drop(&mut self) {
        unsafe {
            self.player.pause();
            self.player.replaceCurrentItemWithPlayerItem(None);
        }
    }
}
