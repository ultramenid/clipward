//! Export speed on the edited timeline; source analysis stays at original speed.
//! Port of video_speed.py.
use super::pyfmt;
use crate::error::{EngineError, Result};

pub const INVALID_SPEED: &str = "Video speed must be between 1x and 2x";

pub fn validate_video_speed(speed: f64) -> Result<f64> {
    if !speed.is_finite() || !(1.0..=2.0).contains(&speed) {
        return Err(EngineError::msg(format!("video render failed: {INVALID_SPEED}")));
    }
    Ok(speed)
}

pub fn scaled_duration_ms(duration_ms: i64, speed: f64) -> Result<i64> {
    Ok(pyfmt::round(duration_ms as f64 / validate_video_speed(speed)?))
}

pub fn speed_video_filter(speed: f64, fps: &str) -> Result<String> {
    validate_video_speed(speed)?;
    if speed == 1.0 {
        return Ok("null".into());
    }
    // Use a fine time base before scaling, then return to the export frame grid.
    Ok(format!(
        "settb=AVTB,setpts=(PTS-STARTPTS)/{},fps={fps}:start_time=0:round=near",
        pyfmt::fmt_g(speed, 8)
    ))
}

pub fn speed_audio_filter(speed: f64, duration_ms: i64) -> Result<String> {
    validate_video_speed(speed)?;
    if speed == 1.0 {
        return Ok(String::new());
    }
    // atempo preserves pitch. Its overlap windows can leave a short tail;
    // bound/pad samples to the requested duration before rebuilding the clock.
    let samples = pyfmt::round((duration_ms * 48) as f64 / speed);
    Ok(format!(
        "atempo={},apad=whole_len={samples},atrim=end_sample={samples},",
        pyfmt::fmt_g(speed, 8)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_speed() {
        for speed in [0.0, 0.5, 2.01, f64::NAN, f64::INFINITY, -1.0] {
            let err = validate_video_speed(speed).unwrap_err().to_string();
            assert!(err.contains("Video speed"), "{err}");
            assert!(err.contains("video render failed"));
        }
        for speed in [1.0, 1.1, 1.25, 1.5, 1.75, 2.0] {
            assert_eq!(validate_video_speed(speed).unwrap(), speed);
        }
    }

    #[test]
    fn filters() {
        assert_eq!(speed_video_filter(1.0, "30").unwrap(), "null");
        assert_eq!(
            speed_video_filter(1.5, "30000/1001").unwrap(),
            "settb=AVTB,setpts=(PTS-STARTPTS)/1.5,fps=30000/1001:start_time=0:round=near"
        );
        assert_eq!(speed_audio_filter(1.0, 1000).unwrap(), "");
        assert_eq!(
            speed_audio_filter(2.0, 11000).unwrap(),
            "atempo=2,apad=whole_len=264000,atrim=end_sample=264000,"
        );
        assert_eq!(scaled_duration_ms(11_000, 1.5).unwrap(), 7333);
        assert_eq!(scaled_duration_ms(5, 2.0).unwrap(), 2); // 2.5 rounds to even
    }
}
