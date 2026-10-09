//! Builds the editor's voiceover recorder on its own and, as a test, records two
//! seconds from the real microphone and checks the .m4a with ffprobe:
//! `cargo test -p clipward_app --example record_voiceover -- --ignored --nocapture`
//! (the terminal needs microphone access).
#[path = "../src/recorder.rs"]
#[allow(dead_code, unused_imports)]
mod recorder;

fn main() {}
