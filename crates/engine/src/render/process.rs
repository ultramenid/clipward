//! How the renderer runs FFmpeg/FFprobe. Real runs go through `media::run_bounded`
//! (minimal environment, wall-clock limit, capped output, cancellation - each kills
//! the child); tests substitute a fake [`MediaRunner`] to check the exact arguments.
use crate::error::{EngineError, Result};
use crate::media::{self, MAX_TOOL_OUTPUT_BYTES};
use futures::future::BoxFuture;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio_util::sync::CancellationToken;

pub use crate::media::{MEDIA_INPUT_OPTIONS, PROBE_TIMEOUT};

/// Long renders: two hours, as media_process.MEDIA_TIMEOUT_SECONDS.
pub const MEDIA_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
pub const TIME_LIMIT_ERROR: &str = "media processing exceeded its time limit";
pub const SIZE_LIMIT_ERROR: &str = "media output exceeded the size limit";

/// Captured result of a finished tool.
#[derive(Debug, Clone, Default)]
pub struct MediaOutput {
    /// Exit code; -1 when killed by a signal.
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl MediaOutput {
    pub fn success(&self) -> bool {
        self.code == 0
    }
}

pub type LineSink<'a> = &'a mut (dyn FnMut(&[u8]) -> std::result::Result<(), String> + Send);

pub trait MediaRunner: Send + Sync {
    /// Run to completion, capturing at most `max_output` bytes of stdout and of stderr.
    fn run<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [String],
        timeout: Duration,
        max_output: usize,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<MediaOutput>>;

    /// Stream stdout lines (each at most `max_line` bytes, like `readline(max_line)`)
    /// into `on_line` with bounded memory. An `on_line` error kills the tool.
    fn run_lines<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [String],
        timeout: Duration,
        max_line: usize,
        on_line: LineSink<'a>,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<MediaOutput>>;
}

/// Real processes.
#[derive(Debug, Default, Clone)]
pub struct SystemRunner;

impl MediaRunner for SystemRunner {
    fn run<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [String],
        timeout: Duration,
        max_output: usize,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<MediaOutput>> {
        Box::pin(async move {
            let out = media::run_bounded(program, args, timeout, max_output, cancel).await?;
            Ok(MediaOutput { code: out.status.code().unwrap_or(-1), stdout: out.stdout, stderr: out.stderr })
        })
    }

    fn run_lines<'a>(
        &'a self,
        program: &'a Path,
        args: &'a [String],
        timeout: Duration,
        max_line: usize,
        on_line: LineSink<'a>,
        cancel: &'a CancellationToken,
    ) -> BoxFuture<'a, Result<MediaOutput>> {
        Box::pin(async move {
            let deadline = tokio::time::Instant::now() + timeout;
            let mut child = media::media_command(program)
                .args(args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
                return Err(EngineError::msg("media tool output is unavailable"));
            };
            let lines = async {
                let mut reader = BufReader::new(stdout);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    let n = (&mut reader).take(max_line as u64).read_until(b'\n', &mut line).await?;
                    if n == 0 {
                        return Ok(());
                    }
                    on_line(&line).map_err(EngineError::Failed)?;
                }
            };
            let errors = async {
                let mut buf = Vec::new();
                stderr.take(MAX_TOOL_OUTPUT_BYTES as u64 + 1).read_to_end(&mut buf).await?;
                if buf.len() > MAX_TOOL_OUTPUT_BYTES {
                    return Err(EngineError::msg(SIZE_LIMIT_ERROR));
                }
                Ok(buf)
            };
            let outcome = tokio::select! {
                r = async { tokio::try_join!(lines, errors) } => r,
                _ = tokio::time::sleep_until(deadline) => Err(EngineError::msg(TIME_LIMIT_ERROR)),
                _ = cancel.cancelled() => Err(EngineError::Cancelled),
            };
            let stderr = match outcome {
                Ok((_, stderr)) => stderr,
                Err(e) => {
                    let _ = child.kill().await;
                    return Err(e);
                }
            };
            let status = tokio::select! {
                s = child.wait() => s?,
                _ = tokio::time::sleep_until(deadline) => {
                    let _ = child.kill().await;
                    return Err(EngineError::msg(TIME_LIMIT_ERROR));
                }
                _ = cancel.cancelled() => {
                    let _ = child.kill().await;
                    return Err(EngineError::Cancelled);
                }
            };
            Ok(MediaOutput { code: status.code().unwrap_or(-1), stdout: Vec::new(), stderr })
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn limits_and_cancellation_kill_the_child() {
        let sh = Path::new("/bin/sh");
        let runner = SystemRunner;
        let cancel = CancellationToken::new();
        let args = vec!["-c".to_string(), "sleep 30".to_string()];
        let started = std::time::Instant::now();
        let err = runner.run(sh, &args, Duration::from_millis(200), 1024, &cancel).await.unwrap_err();
        assert!(err.to_string().contains("time limit"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10));

        let token = CancellationToken::new();
        let t2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            t2.cancel();
        });
        let mut sink = |_: &[u8]| Ok(());
        let err = runner.run_lines(sh, &args, Duration::from_secs(60), 512, &mut sink, &token).await.unwrap_err();
        assert!(matches!(err, EngineError::Cancelled));

        let noisy = vec!["-c".to_string(), "head -c 5000 /dev/zero >&2; sleep 30".to_string()];
        let err = runner.run(sh, &noisy, Duration::from_secs(60), 1024, &cancel).await.unwrap_err();
        assert!(err.to_string().contains("size limit"), "{err}");

        let ok = vec!["-c".to_string(), "printf 'a=1\\nb=2\\n'; exit 3".to_string()];
        let out = runner.run(sh, &ok, Duration::from_secs(10), 1024, &cancel).await.unwrap();
        assert_eq!(out.code, 3);
        assert_eq!(out.stdout, b"a=1\nb=2\n");
        let mut seen = Vec::new();
        let mut on_line = |l: &[u8]| {
            seen.push(l.to_vec());
            Ok(())
        };
        let out = runner.run_lines(sh, &ok, Duration::from_secs(10), 512, &mut on_line, &cancel).await.unwrap();
        assert_eq!(out.code, 3);
        assert_eq!(seen, vec![b"a=1\n".to_vec(), b"b=2\n".to_vec()]);

        let mut failing = |_: &[u8]| Err("bad line".to_string());
        let lots = vec!["-c".to_string(), "while true; do echo x; sleep 0.01; done".to_string()];
        let err = runner.run_lines(sh, &lots, Duration::from_secs(60), 512, &mut failing, &cancel).await.unwrap_err();
        assert!(err.to_string().contains("bad line"));
    }
}
