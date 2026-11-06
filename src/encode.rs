//! Lossless overlay video encoding through an `ffmpeg` subprocess fed with
//! planar YUV frames in the source pixel format.
//!
//! The video is encoded losslessly (HEVC NVENC or x264 with QP 0), so the
//! output decodes to exactly the frames that were written. Audio streams are
//! copied without re-encoding and the source color tags are preserved.

use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::thread::JoinHandle;

use crate::ffmpeg;
use crate::probe::VideoInfo;

/// Whether FFmpeg can encode `pix_fmt` losslessly with NVIDIA NVENC (HEVC).
pub fn nvenc_lossless_available(pix_fmt: &str) -> bool {
    ffmpeg::command("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi"])
        .args(["-i", "color=c=black:s=256x256:d=0.1", "-pix_fmt", pix_fmt])
        .args(["-c:v", "hevc_nvenc", "-tune", "lossless", "-f", "null", "-"])
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
        if params.nvenc {
            cmd.args(["-c:v", "hevc_nvenc", "-preset", "p4", "-tune", "lossless"]);
            if is_quicktime(params.destination) {
                // Lets QuickTime-based players recognize HEVC in MP4/MOV.
                cmd.args(["-tag:v", "hvc1"]);
            }
        } else {
            cmd.args(["-c:v", "libx264", "-preset", "veryfast", "-qp", "0"]);
        }
        // Every frame is kept even when the audio track ends earlier.
        cmd.args(["-pix_fmt", params.pix_fmt, "-c:a", "copy"])
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
