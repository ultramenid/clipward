//! Media playback for the editor's preview on Windows and Linux (and on macOS with
//! `--features portable-video`): the bundled ffmpeg decodes, GPUI draws the frames
//! as images and rodio plays the sound. Same API as the AVFoundation player (video.rs).
//!
//! Each open or seek starts a session: one ffmpeg piping BGRA frames at a fixed
//! rate and size, one piping 48 kHz stereo samples (tempo-adjusted to the rate) into
//! a rodio player. The clock is the wall clock from the last seek, so sound can sit
//! up to one ffmpeg start-up (~0.1 s) behind when playback starts mid-seek.
//! ponytail: wall clock, not audio-driven; drive it from samples played if drift shows.
//! Must be used on the main thread.

use clipward_core::AppPaths;
use clipward_core::tools::{Tool, resolve_binary};
use gpui_kit::*;
use std::cell::RefCell;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

/// Frames per second decoded for the preview.
const FPS: f64 = 30.0;
/// Longest edge of a decoded frame: enough for the preview pane, cheap to pipe.
const MAX_EDGE: u32 = 960;
const SAMPLE_RATE: u32 = 48_000;
/// Decoded frames buffered ahead of the playhead.
const FRAME_QUEUE: usize = 6;
/// Audio chunks (100 ms each) queued in rodio ahead of the playhead.
const AUDIO_QUEUE: usize = 4;

static TOOLS: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();

/// Where ffmpeg and ffprobe live; call once at startup.
pub fn init(paths: &AppPaths) {
    let _ = TOOLS.set((resolve_binary(paths, Tool::Ffmpeg), resolve_binary(paths, Tool::Ffprobe)));
}

fn tools() -> (PathBuf, PathBuf) {
    TOOLS.get().cloned().unwrap_or_else(|| ("ffmpeg".into(), "ffprobe".into()))
}

/// A media tool with no console window (Windows) and piped stdout.
fn tool(program: &Path) -> Command {
    let mut command = Command::new(program);
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

thread_local! {
    /// The default output device, opened on first use and kept for the process; None
    /// when there is no sound device (playback goes on silently).
    static SPEAKERS: RefCell<Option<Option<rodio::MixerDeviceSink>>> = const { RefCell::new(None) };
}

fn audio_player() -> Option<Arc<rodio::Player>> {
    SPEAKERS.with(|cell| {
        let mut cell = cell.borrow_mut();
        let sink = cell.get_or_insert_with(|| {
            rodio::DeviceSinkBuilder::open_default_sink().ok().map(|mut sink| {
                sink.log_on_drop(false);
                sink
            })
        });
        sink.as_ref().map(|sink| Arc::new(rodio::Player::connect_new(sink.mixer())))
    })
}

#[derive(Clone, Debug, PartialEq)]
struct Info {
    duration_ms: Option<f64>,
    /// Decoded frame size (display orientation, scaled to MAX_EDGE, even), if video.
    size: Option<(u32, u32)>,
    audio: bool,
}

/// ffprobe's JSON (`-show_format -show_streams`) → what playback needs.
fn parse_info(json: &serde_json::Value) -> Info {
    let streams = json["streams"].as_array().cloned().unwrap_or_default();
    let video = streams.iter().find(|s| {
        s["codec_type"] == "video" && s["disposition"]["attached_pic"].as_i64() != Some(1)
    });
    let size = video.and_then(|s| {
        let (w, h) = (s["width"].as_u64()? as f64, s["height"].as_u64()? as f64);
        // Phone videos store a rotation; ffmpeg turns frames upright when decoding.
        let rotation = s["side_data_list"]
            .as_array()
            .and_then(|list| list.iter().find_map(|d| d["rotation"].as_f64()))
            .or_else(|| s["tags"]["rotate"].as_str().and_then(|r| r.parse().ok()))
            .unwrap_or(0.0);
        let (w, h) = if (rotation.abs() / 90.0).round() as i64 % 2 == 1 { (h, w) } else { (w, h) };
        let scale = (MAX_EDGE as f64 / w.max(h)).min(1.0);
        let even = |v: f64| ((v * scale / 2.0).round() as u32).max(1) * 2;
        (w > 0.0 && h > 0.0).then(|| (even(w), even(h)))
    });
    let duration_ms = json["format"]["duration"]
        .as_str()
        .and_then(|d| d.parse::<f64>().ok())
        .map(|s| s * 1000.0);
    Info { duration_ms, size, audio: streams.iter().any(|s| s["codec_type"] == "audio") }
}

fn probe(ffprobe: &Path, file: &Path) -> Option<Info> {
    let output = tool(ffprobe)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(file)
        .output()
        .ok()?;
    let json = serde_json::from_slice(&output.stdout).ok()?;
    output.status.success().then(|| parse_info(&json))
}

/// atempo takes 0.5–2 per stage; chain stages for rates outside that.
fn atempo(mut rate: f64) -> String {
    let mut stages = Vec::new();
    while rate > 2.0 {
        stages.push("atempo=2".to_string());
        rate /= 2.0;
    }
    while rate < 0.5 {
        stages.push("atempo=0.5".to_string());
        rate /= 0.5;
    }
    stages.push(format!("atempo={rate:.4}"));
    stages.join(",")
}

struct Frame {
    ms: f64,
    image: Arc<RenderImage>,
}

/// One decode from a position: killed (and its threads end) when dropped.
struct Session {
    children: Arc<Mutex<Vec<Child>>>,
    stop: Arc<AtomicBool>,
    frames: Option<Receiver<Frame>>,
    audio: Option<Arc<rodio::Player>>,
}

impl Session {
    fn start(file: &Path, info: &Info, video: bool, from_ms: f64, rate: f32) -> Self {
        let (ffmpeg, _) = tools();
        let stop = Arc::new(AtomicBool::new(false));
        let children = Arc::new(Mutex::new(Vec::new()));
        let seek = format!("{:.3}", from_ms.max(0.0) / 1000.0);
        let input = |command: &mut Command| {
            command.args(["-nostdin", "-v", "error", "-ss", &seek, "-i"]).arg(file);
        };

        let frames = video.then_some(info.size).flatten().and_then(|(w, h)| {
            let mut command = tool(&ffmpeg);
            input(&mut command);
            command.args(["-an", "-sn", "-vf", &format!("fps={FPS},scale={w}:{h}"), "-pix_fmt", "bgra", "-f", "rawvideo", "-"]);
            let mut child = command.spawn().ok()?;
            let stdout = child.stdout.take()?;
            children.lock().unwrap().push(child);
            let (tx, rx) = sync_channel(FRAME_QUEUE);
            std::thread::spawn(move || read_frames(stdout, w, h, from_ms, tx));
            Some(rx)
        });

        let audio = info.audio.then(audio_player).flatten().and_then(|player| {
            player.pause();
            let mut command = tool(&ffmpeg);
            input(&mut command);
            command.args(["-vn", "-sn", "-af", &atempo(rate as f64), "-ac", "2", "-ar", &SAMPLE_RATE.to_string(), "-f", "f32le", "-"]);
            let mut child = command.spawn().ok()?;
            let stdout = child.stdout.take()?;
            children.lock().unwrap().push(child);
            let (feed, stop) = (player.clone(), stop.clone());
            std::thread::spawn(move || read_audio(stdout, feed, stop));
            Some(player)
        });

        Self { children, stop, frames, audio }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(audio) = &self.audio {
            audio.stop();
        }
        for mut child in self.children.lock().unwrap().drain(..) {
            let _ = child.kill();
            // Reap off the main thread.
            std::thread::spawn(move || child.wait());
        }
    }
}

fn read_frames(mut stdout: impl Read, w: u32, h: u32, from_ms: f64, tx: SyncSender<Frame>) {
    let mut index = 0u64;
    loop {
        let mut bytes = vec![0u8; (w * h * 4) as usize];
        if stdout.read_exact(&mut bytes).is_err() {
            return;
        }
        let Some(buffer) = image::RgbaImage::from_raw(w, h, bytes) else { return };
        let image = Arc::new(RenderImage::new([image::Frame::new(buffer)]));
        let ms = from_ms + index as f64 * 1000.0 / FPS;
        index += 1;
        if tx.send(Frame { ms, image }).is_err() {
            return;
        }
    }
}

fn read_audio(mut stdout: impl Read, player: Arc<rodio::Player>, stop: Arc<AtomicBool>) {
    let channels = std::num::NonZero::new(2u16).unwrap();
    let rate = std::num::NonZero::new(SAMPLE_RATE).unwrap();
    let chunk = (SAMPLE_RATE / 10 * 2 * 4) as usize;
    loop {
        while player.len() > AUDIO_QUEUE && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let mut bytes = Vec::with_capacity(chunk);
        let read = (&mut stdout).take(chunk as u64).read_to_end(&mut bytes);
        let usable = bytes.len() / 8 * 8;
        if read.is_err() || usable == 0 {
            return;
        }
        let samples: Vec<f32> = bytes[..usable]
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        player.append(rodio::buffer::SamplesBuffer::new(channels, rate, samples));
    }
}

pub struct MediaPlayer {
    path: Option<PathBuf>,
    video: bool,
    /// The probe, filled in by a background thread: Some(None) when it failed.
    info: Arc<Mutex<Option<Option<Info>>>>,
    session: Option<Session>,
    /// The next decoded frame, held until its time comes.
    pending: Option<Frame>,
    image: Option<Arc<RenderImage>>,
    /// Images shown before, to free from the window's atlas.
    retired: Vec<Arc<RenderImage>>,
    /// Waiting for the first frame after a seek.
    seeking: bool,
    /// Position at `since`; the clock runs from there while playing.
    anchor_ms: f64,
    since: Instant,
    playing: bool,
    rate: f32,
    volume: f32,
    muted: bool,
    primed: bool,
}

impl MediaPlayer {
    pub fn new() -> Self {
        Self {
            path: None,
            video: false,
            info: Arc::new(Mutex::new(None)),
            session: None,
            pending: None,
            image: None,
            retired: Vec::new(),
            seeking: false,
            anchor_ms: 0.0,
            since: Instant::now(),
            playing: false,
            rate: 1.0,
            volume: 1.0,
            muted: false,
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
        self.video = video;
        self.session = None;
        self.pending = None;
        self.retired.extend(self.image.take());
        self.seeking = false;
        self.playing = false;
        self.primed = false;
        self.anchor_ms = 0.0;
        // A fresh slot, so a late probe of the previous file can't land here.
        self.info = Arc::new(Mutex::new(None));
        if let Some(path) = path {
            let (slot, path, (_, ffprobe)) = (self.info.clone(), path.to_path_buf(), tools());
            std::thread::spawn(move || {
                let info = probe(&ffprobe, &path);
                *slot.lock().unwrap() = Some(info);
            });
        }
    }

    fn info(&self) -> Option<Info> {
        self.info.lock().unwrap().clone().flatten()
    }

    /// The file is open and decodable.
    pub fn ready(&self) -> bool {
        self.path.is_some() && self.info().is_some()
    }

    /// The file could not be played (missing or damaged).
    pub fn failed(&self) -> bool {
        self.path.is_some() && matches!(*self.info.lock().unwrap(), Some(None))
    }

    pub fn duration_ms(&self) -> Option<f64> {
        self.info().and_then(|info| info.duration_ms)
    }

    pub fn current_ms(&self) -> f64 {
        let mut ms = self.anchor_ms;
        if self.playing && !self.seeking {
            ms += self.since.elapsed().as_secs_f64() * 1000.0 * self.rate as f64;
        }
        self.duration_ms().map_or(ms, |duration| ms.min(duration))
    }

    pub fn is_seeking(&self) -> bool {
        self.seeking
    }

    /// Played to (within a frame of) the end.
    pub fn ended(&self) -> bool {
        self.duration_ms()
            .is_some_and(|duration| self.current_ms() >= duration - 30.0)
    }

    /// Jump to `ms`. Seeks are always frame-exact (`exact` is for the AVFoundation player).
    pub fn seek(&mut self, ms: f64, _exact: bool) {
        let (Some(path), Some(info)) = (self.path.clone(), self.info()) else {
            return;
        };
        self.session = None;
        self.pending = None;
        let session = Session::start(&path, &info, self.video, ms, self.rate);
        self.seeking = session.frames.is_some();
        self.anchor_ms = ms;
        self.since = Instant::now();
        self.session = Some(session);
        self.apply_audio();
    }

    /// Keep this player on `ms` the way the React player followed the playhead: while
    /// playing it may drift `drift_ms` before seeking; paused, it lands on the frame.
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
        let at = self.current_ms();
        let rate_changed = rate != self.rate;
        self.playing = playing;
        self.rate = rate;
        self.anchor_ms = at;
        self.since = Instant::now();
        if self.session.is_none() || rate_changed {
            // The sound's tempo is fixed per session.
            self.seek(at, true);
        }
        self.apply_audio();
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// 0..1, as the AVFoundation player allows.
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        self.apply_audio();
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        self.apply_audio();
    }

    fn apply_audio(&self) {
        let Some(audio) = self.session.as_ref().and_then(|s| s.audio.as_ref()) else {
            return;
        };
        audio.set_volume(if self.muted { 0.0 } else { self.volume });
        if self.playing && !self.seeking && !self.ended() {
            audio.play();
        } else {
            audio.pause();
        }
    }

    /// Take the frame for the current time if a new one is ready, and free the
    /// frames shown before. True when the picture changed.
    pub fn poll_frame(&mut self, window: &mut Window) -> bool {
        for image in self.retired.drain(..) {
            let _ = window.drop_image(image);
        }
        let Some(frames) = self.session.as_ref().and_then(|s| s.frames.as_ref()) else {
            return false;
        };
        let mut next = None;
        loop {
            let frame = match self.pending.take() {
                Some(frame) => frame,
                None => match frames.try_recv() {
                    Ok(frame) => frame,
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                },
            };
            // The first frame after a seek shows at once and starts the clock.
            if self.seeking {
                self.seeking = false;
                self.since = Instant::now();
                next = Some(frame);
                self.apply_audio();
                break;
            }
            if frame.ms > self.current_ms() {
                self.pending = Some(frame);
                break;
            }
            next = Some(frame);
        }
        let Some(frame) = next else { return false };
        self.retired.extend(self.image.replace(frame.image));
        true
    }

    /// A decoded frame is there to show.
    pub fn has_frame(&self) -> bool {
        self.image.is_some()
    }

    /// The latest frame filling its box with `fit`.
    pub fn surface(&self, fit: ObjectFit) -> Option<Img> {
        self.image.clone().map(|image| img(image).object_fit(fit))
    }
}

#[cfg(test)]
mod tests {
    use super::{Info, atempo, parse_info};

    #[test]
    fn probe_info_turns_rotated_video_upright_and_scales_it() {
        let parse = |text: &str| parse_info(&serde_json::from_str(text).unwrap());
        let rotated = r#"{"format": {"duration": "12.5"}, "streams": [
            {"codec_type": "video", "width": 1920, "height": 1080, "side_data_list": [{"rotation": -90}]},
            {"codec_type": "audio"}]}"#;
        assert_eq!(parse(rotated), Info { duration_ms: Some(12_500.0), size: Some((540, 960)), audio: true });
        let cover_art = r#"{"format": {}, "streams": [{"codec_type": "audio"},
            {"codec_type": "video", "width": 500, "height": 500, "disposition": {"attached_pic": 1}}]}"#;
        assert_eq!(parse(cover_art), Info { duration_ms: None, size: None, audio: true });
    }

    #[test]
    fn atempo_chains_stages_outside_its_range() {
        assert_eq!(atempo(1.0), "atempo=1.0000");
        assert_eq!(atempo(4.0), "atempo=2,atempo=2.0000");
        assert_eq!(atempo(0.25), "atempo=0.5,atempo=0.5000");
    }
}
