//! Video stream properties read with `ffprobe`.

use std::path::Path;
use std::process::Stdio;

use crate::ffmpeg;

/// Properties of the first video stream.
#[derive(Clone, Debug)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    /// Frame rate as an exact rational (`num / den`).
    pub rate_num: u64,
    pub rate_den: u64,
    /// Frame count from the container, or estimated from the duration.
    pub frame_count: Option<u64>,
    /// Pixel format produced by the software decoder (e.g. `yuv420p`).
    pub pix_fmt: Option<String>,
}

impl VideoInfo {
    pub fn frame_rate(&self) -> f64 {
        self.rate_num as f64 / self.rate_den as f64
    }

    /// Frame rate formatted for FFmpeg's `-r` option.
    pub fn frame_rate_arg(&self) -> String {
        format!("{}/{}", self.rate_num, self.rate_den)
    }
}

/// Failure reasons for [`probe`].
#[derive(Debug)]
pub enum ProbeError {
    /// `ffprobe` could not be executed.
    Missing,
    /// The file is not a readable video. Holds ffprobe's message.
    Unreadable(String),
    /// The stream has no usable frame rate.
    NoFrameRate,
}

pub fn probe(path: &Path) -> Result<VideoInfo, ProbeError> {
    let output = ffmpeg::command("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .arg("stream=width,height,pix_fmt,r_frame_rate,avg_frame_rate,nb_frames:format=duration")
        .args(["-of", "default=noprint_wrappers=1"])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|_| ProbeError::Missing)?;
    let text = String::from_utf8_lossy(&output.stdout);
    let field = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(str::trim)
            .filter(|value| !value.is_empty() && *value != "N/A")
    };

    let dimension = |key| field(key).and_then(|v| v.parse::<u32>().ok()).filter(|&v| v > 0);
    let (Some(width), Some(height)) = (dimension("width"), dimension("height")) else {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(ProbeError::Unreadable(detail));
    };
    let (rate_num, rate_den) = field("r_frame_rate")
        .and_then(parse_rational)
        .or_else(|| field("avg_frame_rate").and_then(parse_rational))
        .ok_or(ProbeError::NoFrameRate)?;
    let rate = rate_num as f64 / rate_den as f64;
    let frame_count = field("nb_frames").and_then(|v| v.parse::<u64>().ok()).or_else(|| {
        let duration = field("duration")?.parse::<f64>().ok()?;
        Some((duration * rate + 0.5).floor() as u64)
    });

    let pix_fmt = field("pix_fmt").map(str::to_owned);

    Ok(VideoInfo { width, height, rate_num, rate_den, frame_count, pix_fmt })
}

fn parse_rational(text: &str) -> Option<(u64, u64)> {
    let (num, den) = text.split_once('/').unwrap_or((text, "1"));
    let num: u64 = num.trim().parse().ok()?;
    let den: u64 = den.trim().parse().ok()?;
    (num > 0 && den > 0).then_some((num, den))
}

#[cfg(test)]
mod tests {
    use super::parse_rational;

    #[test]
    fn parses_rationals() {
        assert_eq!(parse_rational("60000/1001"), Some((60000, 1001)));
        assert_eq!(parse_rational("30"), Some((30, 1)));
        assert_eq!(parse_rational("0/0"), None);
    }
}
