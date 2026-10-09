//! Test helpers: a scripted MediaRunner and small builders mirroring the Python fixtures.
use crate::error::{EngineError, Result};
use crate::layout::types::{Box as LBox, ClipLayoutPlan, ShotLayout};
use crate::render::process::{LineSink, MediaOutput, MediaRunner};
use crate::render::service::Renderer;
use crate::types::{TranscriptSegment, TranscriptWord};
use futures::future::BoxFuture;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub type Responder = Box<dyn Fn(&Path, &[String]) -> Result<MediaOutput> + Send + Sync>;

/// Records every call and answers with `respond`; `lines` feeds run_lines.
pub struct FakeRunner {
    pub calls: Mutex<Vec<(PathBuf, Vec<String>)>>,
    pub respond: Responder,
    pub lines: Mutex<(Vec<Vec<u8>>, i32)>,
}

impl FakeRunner {
    pub fn new(respond: impl Fn(&Path, &[String]) -> Result<MediaOutput> + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self { calls: Mutex::new(Vec::new()), respond: Box::new(respond), lines: Mutex::new((Vec::new(), 0)) })
    }

    pub fn with_lines(self: &Arc<Self>, lines: &[&[u8]], code: i32) -> Arc<Self> {
        *self.lines.lock().unwrap() = (lines.iter().map(|l| l.to_vec()).collect(), code);
        self.clone()
    }

    pub fn calls(&self) -> Vec<(PathBuf, Vec<String>)> {
        self.calls.lock().unwrap().clone()
    }
}

impl MediaRunner for FakeRunner {
    fn run<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [String],
        _timeout: Duration,
        _max_output: usize,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<MediaOutput>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(EngineError::Cancelled);
            }
            self.calls.lock().unwrap().push((program.to_path_buf(), args.to_vec()));
            (self.respond)(program, args)
        })
    }

    fn run_lines<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [String],
        _timeout: Duration,
        max_line: usize,
        on_line: LineSink<'a>,
        _cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<MediaOutput>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push((program.to_path_buf(), args.to_vec()));
            let (lines, code) = self.lines.lock().unwrap().clone();
            for line in lines {
                for chunk in line.chunks(max_line.max(1)) {
                    on_line(chunk).map_err(EngineError::Failed)?;
                }
            }
            Ok(MediaOutput { code, ..Default::default() })
        })
    }
}

pub fn ok(stdout: &[u8]) -> Result<MediaOutput> {
    Ok(MediaOutput { code: 0, stdout: stdout.to_vec(), stderr: Vec::new() })
}

pub fn fail(stderr: &[u8]) -> Result<MediaOutput> {
    Ok(MediaOutput { code: 1, stdout: Vec::new(), stderr: stderr.to_vec() })
}

pub fn fonts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/fonts")
}

/// A renderer with the given runner, no analyzer and libx264 args.
pub fn renderer(runner: Arc<dyn MediaRunner>) -> Renderer {
    let fonts = fonts_dir();
    Renderer {
        ffmpeg: PathBuf::from("ffmpeg"),
        ffprobe: PathBuf::from("ffprobe"),
        font_path: Some(fonts.join("Montserrat-Black.ttf")),
        fonts_dir: Some(fonts),
        runner,
        analyzer: None,
        video_codec_override: None,
        videotoolbox: false,
        hw_encoder: tokio::sync::OnceCell::new_with(Some(None)),
        audio_padding_ms: 150,
        temp_dir: None,
    }
}

pub fn b(x: f64, y: f64, w: f64, h: f64) -> LBox {
    LBox { x, y, w, h }
}

pub fn shot(start_ms: i64, end_ms: i64, layout: &str) -> ShotLayout {
    ShotLayout { start_ms, end_ms, layout: layout.into(), source: "heuristic".into(), ..Default::default() }
}

pub fn plan(shots: Vec<ShotLayout>, w: u32, h: u32) -> ClipLayoutPlan {
    ClipLayoutPlan { shots, source_width: w, source_height: h, ..Default::default() }
}

pub fn word(text: &str, start: i64, end: i64) -> TranscriptWord {
    TranscriptWord { word: text.into(), start_time_ms: start, end_time_ms: end }
}

pub fn segment(start: i64, end: i64, text: &str, words: Vec<TranscriptWord>) -> TranscriptSegment {
    TranscriptSegment { start_time_ms: start, end_time_ms: end, text: text.into(), words, ..Default::default() }
}

pub fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

pub fn ffmpeg_tools() -> Option<(PathBuf, PathBuf)> {
    let ffmpeg = std::env::var_os("TEST_FFMPEG").map(PathBuf::from).or_else(|| which::which("ffmpeg").ok())?;
    let ffprobe = std::env::var_os("TEST_FFPROBE").map(PathBuf::from).or_else(|| which::which("ffprobe").ok())?;
    Some((ffmpeg, ffprobe))
}
