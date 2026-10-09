//! Voiceover takes from the microphone (the React editor's `MediaRecorder`), on
//! AVFoundation: `AVAudioRecorder` encodes mono AAC straight into an .m4a.
//!
//! Microphone access goes through `AVCaptureDevice`'s authorization, so the first
//! take shows the macOS prompt (the bundle needs `NSMicrophoneUsageDescription`).
//! No echo cancellation or noise suppression: on macOS those come only from the
//! voice-processing I/O unit (`AVAudioEngine`), which AVAudioRecorder can't use.
//! A `Recorder` stays on the thread that made it (the editor keeps it on the main thread).

#[cfg(target_os = "macos")]
pub use mac::Recorder;
#[cfg(not(target_os = "macos"))]
pub use stub::Recorder;

#[cfg(target_os = "macos")]
mod mac {
    use objc2::AnyThread;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool};
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
    use objc2_avf_audio::{
        AVAudioRecorder, AVEncoderBitRateKey, AVFormatIDKey, AVNumberOfChannelsKey, AVSampleRateKey,
    };
    use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// `kAudioFormatMPEG4AAC` ('aac ').
    const AAC: u32 = u32::from_be_bytes(*b"aac ");
    const SAMPLE_RATE: f64 = 48_000.0;
    const BIT_RATE: i32 = 128_000;
    /// What `start` says when the microphone is off-limits.
    const DENIED: &str = "Could not use the microphone. Allow Clipward to use it in System Settings → Privacy & Security → Microphone.";

    /// A microphone take in progress, written to `path` as AAC in an .m4a file.
    pub struct Recorder {
        recorder: Retained<AVAudioRecorder>,
        path: PathBuf,
        stopped: bool,
    }

    impl Recorder {
        /// Ask for microphone access if needed (waiting on the macOS prompt), then
        /// start recording to `path`. Errors are sentences to show as they are.
        pub async fn start(path: PathBuf) -> Result<Recorder, String> {
            if !microphone_allowed().await {
                return Err(DENIED.into());
            }
            Self::begin(path)
        }

        /// Start recording to `path`, access already granted.
        fn begin(path: PathBuf) -> Result<Recorder, String> {
            let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
                return Err("Could not use the microphone on this Mac.".into());
            };
            if unsafe { AVCaptureDevice::defaultDeviceWithMediaType(audio) }.is_none() {
                return Err("No microphone is connected. Plug one in and try again.".into());
            }
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|err| {
                    format!("Could not create the folder for the recording: {err}.")
                })?;
            }

            let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
            let settings = settings().ok_or("Could not set up the recording on this Mac.")?;
            let recorder = unsafe {
                AVAudioRecorder::initWithURL_settings_error(
                    AVAudioRecorder::alloc(),
                    &url,
                    &settings,
                )
            }
            .map_err(|err| format!("Could not start recording: {}.", err.localizedDescription()))?;
            // Opens the file and the input now, so `record` starts at once.
            if !unsafe { recorder.prepareToRecord() } || !unsafe { recorder.record() } {
                unsafe { recorder.stop() };
                let _ = std::fs::remove_file(&path);
                return Err(
                    "Could not start recording. Check that the microphone isn't in use by another app and try again."
                        .into(),
                );
            }
            Ok(Recorder {
                recorder,
                path,
                stopped: false,
            })
        }

        /// Stop and finish the file; returns its path.
        pub fn stop(mut self) -> Result<PathBuf, String> {
            self.stopped = true;
            // Writes the AAC tail and the moov atom before returning.
            unsafe { self.recorder.stop() };
            let size = std::fs::metadata(&self.path)
                .map(|meta| meta.len())
                .unwrap_or(0);
            if size == 0 {
                return Err("The recording came out empty. Try again.".into());
            }
            Ok(self.path.clone())
        }
    }

    impl Drop for Recorder {
        /// Leaving mid-take: stop so the mic is released (the partial file stays valid).
        fn drop(&mut self) {
            if !self.stopped {
                unsafe { self.recorder.stop() };
            }
        }
    }

    /// Mono AAC at 48 kHz, 128 kbps. The container comes from the .m4a extension.
    fn settings() -> Option<Retained<NSDictionary<NSString, AnyObject>>> {
        let (format, rate, channels, bit_rate) = unsafe {
            (
                AVFormatIDKey?,
                AVSampleRateKey?,
                AVNumberOfChannelsKey?,
                AVEncoderBitRateKey?,
            )
        };
        let values: [Retained<AnyObject>; 4] = [
            NSNumber::new_u32(AAC),
            NSNumber::new_f64(SAMPLE_RATE),
            NSNumber::new_i32(1),
            NSNumber::new_i32(BIT_RATE),
        ]
        .map(|number| Retained::into_super(Retained::into_super(number)).into());
        Some(NSDictionary::from_retained_objects(
            &[format, rate, channels, bit_rate],
            &values,
        ))
    }

    /// Microphone access, asking with the system prompt the first time.
    async fn microphone_allowed() -> bool {
        let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
            return false;
        };
        match unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) } {
            AVAuthorizationStatus::Authorized => return true,
            AVAuthorizationStatus::NotDetermined => {}
            _ => return false,
        }
        // The answer arrives on an arbitrary dispatch queue; hand it over a channel
        // so the caller's foreground task simply awaits it.
        let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
        let tx = Mutex::new(Some(tx));
        let answered = block2::RcBlock::new(move |granted: Bool| {
            if let Some(tx) = tx.lock().ok().and_then(|mut tx| tx.take()) {
                let _ = tx.send(granted.as_bool());
            }
        });
        unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &answered) };
        rx.await.unwrap_or(false)
    }

    #[cfg(test)]
    mod tests {
        use super::Recorder;
        use objc2_av_foundation::{AVCaptureDevice, AVMediaTypeAudio};
        use std::path::{Path, PathBuf};
        use std::process::Command;
        use std::time::Duration;

        /// The engine's bundled ffprobe, else one on PATH.
        fn ffprobe() -> Option<PathBuf> {
            let bundled = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../engine-bin/ffprobe");
            if bundled.is_file() {
                return Some(bundled);
            }
            Command::new("ffprobe")
                .arg("-version")
                .output()
                .ok()
                .map(|_| PathBuf::from("ffprobe"))
        }

        /// Records two seconds from the real microphone, so it needs mic permission
        /// for the terminal: `cargo test -p clipward_app recorder -- --ignored`.
        #[test]
        #[ignore]
        fn records_two_seconds_of_aac() {
            let path = std::env::temp_dir().join("clipward-recorder-test.m4a");
            let _ = std::fs::remove_file(&path);
            eprintln!("microphone access: {:?}", unsafe {
                AVCaptureDevice::authorizationStatusForMediaType(AVMediaTypeAudio.unwrap())
            });
            // `RECORDER_SKIP_PERMISSION=1` checks the encoder and the file without
            // asking (macOS hands an unauthorized process silence, not an error).
            let recorder = if std::env::var_os("RECORDER_SKIP_PERMISSION").is_some() {
                Recorder::begin(path.clone()).unwrap()
            } else {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(Recorder::start(path.clone())).unwrap()
            };
            std::thread::sleep(Duration::from_secs(2));
            let written = recorder.stop().unwrap();
            assert_eq!(written, path);

            let Some(ffprobe) = ffprobe() else {
                eprintln!(
                    "no ffprobe; only checked that {} is non-empty",
                    path.display()
                );
                return;
            };
            let out = Command::new(ffprobe)
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "stream=codec_name,channels,sample_rate:format=duration",
                ])
                .args(["-of", "default=noprint_wrappers=1"])
                .arg(&path)
                .output()
                .unwrap();
            let report = String::from_utf8_lossy(&out.stdout);
            eprintln!("{report}");
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(report.contains("codec_name=aac"), "{report}");
            assert!(report.contains("channels=1"), "{report}");
            let duration: f64 = report
                .lines()
                .find_map(|line| line.strip_prefix("duration="))
                .and_then(|value| value.parse().ok())
                .unwrap_or(0.0);
            assert!((1.5..3.0).contains(&duration), "duration {duration}");
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod stub {
    use std::path::PathBuf;

    const UNAVAILABLE: &str = "Voiceover recording isn't available on this system yet.";

    /// A microphone take in progress (macOS only for now).
    pub struct Recorder {
        _not_send: std::marker::PhantomData<*const ()>,
    }

    impl Recorder {
        pub async fn start(_path: PathBuf) -> Result<Recorder, String> {
            Err(UNAVAILABLE.into())
        }

        pub fn stop(self) -> Result<PathBuf, String> {
            Err(UNAVAILABLE.into())
        }
    }
}
