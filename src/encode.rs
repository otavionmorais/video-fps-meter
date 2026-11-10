//! Overlay video encoding through an `ffmpeg` subprocess fed with planar YUV
//! frames in the source pixel format.
//!
//! Two quality levels are available (see [`OverlayQuality`]). Audio streams
//! are copied without re-encoding and the source color tags are preserved
//! in both.

use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::thread::JoinHandle;

use crate::ffmpeg;
use crate::probe::VideoInfo;

/// How the overlay video is encoded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OverlayQuality {
    /// 10-bit HEVC at a fixed quantizer (NVENC QP 8, or x265 CRF 8 on the
    /// CPU). On a 1440p60 NVENC HEVC recording: about 1.1x the source size,
    /// VMAF 98.1 against 98.7 for an identical copy.
    #[default]
    High,
    /// Lossless in the source pixel format (HEVC NVENC or x264 with QP 0):
    /// pixels outside the overlay are bit-identical to the decoded source.
    /// About 6x the size of a compressed recording.
    Lossless,
}

/// Pixel format of the encoded video for frames in `source_pix_fmt` (a
/// planar YUV format accepted by [`crate::yuv::YuvLayout`]). High quality
/// raises 8-bit sources to 10 bits, which compresses more efficiently; NVENC
/// takes 4:2:0 10-bit as `p010le`.
pub fn output_pix_fmt(source_pix_fmt: &str, quality: OverlayQuality, nvenc: bool) -> String {
    if quality == OverlayQuality::Lossless {
        return source_pix_fmt.to_owned();
    }
    let rest = source_pix_fmt.strip_prefix("yuvj").or_else(|| source_pix_fmt.strip_prefix("yuv"));
    let chroma = rest.and_then(|rest| rest.get(..3)).unwrap_or("420");
    let depth = if source_pix_fmt.ends_with("p12le") { "12" } else { "10" };
    if nvenc && chroma == "420" && depth == "10" {
        "p010le".to_owned()
    } else {
        format!("yuv{chroma}p{depth}le")
    }
}

/// Video encoder arguments.
fn codec_args(quality: OverlayQuality, nvenc: bool) -> &'static [&'static str] {
    match (quality, nvenc) {
        (OverlayQuality::High, true) => {
            &["-c:v", "hevc_nvenc", "-preset", "p5", "-tune", "hq", "-rc", "constqp", "-qp", "8"]
        }
        (OverlayQuality::High, false) => {
            &["-c:v", "libx265", "-preset", "fast", "-crf", "8", "-x265-params", "log-level=error"]
        }
        (OverlayQuality::Lossless, true) => &["-c:v", "hevc_nvenc", "-preset", "p4", "-tune", "lossless"],
        (OverlayQuality::Lossless, false) => &["-c:v", "libx264", "-preset", "veryfast", "-qp", "0"],
    }
}

/// Whether FFmpeg can encode `pix_fmt` at `quality` with NVIDIA NVENC (HEVC).
pub fn nvenc_available(pix_fmt: &str, quality: OverlayQuality) -> bool {
    ffmpeg::command("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi"])
        .args(["-i", "color=c=black:s=256x256:d=0.1", "-pix_fmt", pix_fmt])
        .args(codec_args(quality, true))
        .args(["-f", "null", "-"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub struct EncodeParams<'a> {
    pub destination: &'a Path,
    /// Source recording; its audio streams (if any) are copied into the output.
    pub original: &'a Path,
    pub info: &'a VideoInfo,
    /// Pixel format of the frames written to the encoder.
    pub pix_fmt: &'a str,
    /// Pixel format of the encoded video (see [`output_pix_fmt`]).
    pub output_pix_fmt: &'a str,
    pub quality: OverlayQuality,
    pub start_s: f64,
    pub duration_s: Option<f64>,
    pub nvenc: bool,
}

pub struct Encoder {
    child: Child,
    stdin: Option<ChildStdin>,
    stderr: Option<JoinHandle<String>>,
}

impl Encoder {
    pub fn spawn(params: &EncodeParams) -> io::Result<Self> {
        let info = params.info;
        let mut cmd = ffmpeg::command("ffmpeg");
        cmd.args(["-y", "-hide_banner", "-loglevel", "error"])
            .args(["-f", "rawvideo", "-pix_fmt", params.pix_fmt])
            .args(["-s", &format!("{}x{}", info.width, info.height)])
            .args(["-r", &info.frame_rate_arg()]);
        // Color tags go on the raw input: the output inherits them. Given as
        // output options instead, FFmpeg converts the frames to match them,
        // which breaks losslessness.
        let color = &info.color;
        // An RGB source ("gbr" matrix) is converted to YUV, so its matrix tag does not apply.
        let space = color.space.as_ref().filter(|space| *space != "gbr");
        for (option, value) in [
            ("-color_range", color.range.as_ref()),
            ("-colorspace", space),
            ("-color_primaries", color.primaries.as_ref()),
            ("-color_trc", color.transfer.as_ref()),
        ] {
            if let Some(value) = value {
                cmd.args([option, value.as_str()]);
            }
        }
        cmd.args(["-i", "-"]);
        // Seek the audio only for a span: `-ss 0` would still drop packets
        // with negative timestamps, such as the AAC priming packet.
        if params.start_s > 0.0 {
            cmd.args(["-ss", &params.start_s.to_string()]);
        }
        if let Some(duration) = params.duration_s {
            cmd.args(["-t", &duration.to_string()]);
        }
        cmd.arg("-i").arg(params.original).args(["-map", "0:v", "-map", "1:a?"]);
        cmd.args(codec_args(params.quality, params.nvenc));
        let hevc = params.nvenc || params.quality == OverlayQuality::High;
        if hevc && is_quicktime(params.destination) {
            // Lets QuickTime-based players recognize HEVC in MP4/MOV.
            cmd.args(["-tag:v", "hvc1"]);
        }
        // Every frame is kept even when the audio track ends earlier.
        cmd.args(["-pix_fmt", params.output_pix_fmt, "-c:a", "copy"])
            .arg(params.destination)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take();
        let stderr = child.stderr.take().map(ffmpeg::collect_stderr);
        Ok(Self { child, stdin, stderr })
    }

    pub fn write_frame(&mut self, frame: &[u8]) -> io::Result<()> {
        match self.stdin.as_mut() {
            Some(stdin) => stdin.write_all(frame),
            None => Err(io::ErrorKind::BrokenPipe.into()),
        }
    }

    /// Closes the input, waits for FFmpeg to finalize the file and returns
    /// its error output.
    pub fn finish(mut self) -> String {
        drop(self.stdin.take());
        let _ = self.child.wait();
        self.stderr.take().map(|h| h.join().unwrap_or_default()).unwrap_or_default()
    }
}

fn is_quicktime(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mp4") || e.eq_ignore_ascii_case("mov"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_quality_raises_to_10_bits_and_keeps_chroma() {
        let high = OverlayQuality::High;
        assert_eq!(output_pix_fmt("yuv420p", high, false), "yuv420p10le");
        assert_eq!(output_pix_fmt("yuv420p", high, true), "p010le");
        assert_eq!(output_pix_fmt("yuv420p10le", high, true), "p010le");
        assert_eq!(output_pix_fmt("yuvj420p", high, false), "yuv420p10le");
        assert_eq!(output_pix_fmt("yuv422p", high, true), "yuv422p10le");
        assert_eq!(output_pix_fmt("yuv444p12le", high, false), "yuv444p12le");
    }

    #[test]
    fn lossless_keeps_the_source_format() {
        assert_eq!(output_pix_fmt("yuv420p", OverlayQuality::Lossless, true), "yuv420p");
        assert_eq!(output_pix_fmt("yuv422p10le", OverlayQuality::Lossless, false), "yuv422p10le");
    }
}
