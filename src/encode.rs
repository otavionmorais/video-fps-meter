//! Overlay video encoding through an `ffmpeg` subprocess fed with BGR24 frames.

use std::io::{self, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::thread::JoinHandle;

use crate::ffmpeg;
use crate::probe::VideoInfo;

/// Whether FFmpeg can encode with NVIDIA NVENC on this machine.
pub fn nvenc_available() -> bool {
    ffmpeg::command("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi"])
        .args(["-i", "color=c=black:s=256x256:d=0.1", "-c:v", "h264_nvenc", "-f", "null", "-"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub struct EncodeParams<'a> {
    pub destination: &'a Path,
    /// Source recording; its audio track (if any) is copied into the output.
    pub original: &'a Path,
    pub info: &'a VideoInfo,
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
            .args(["-f", "rawvideo", "-pix_fmt", "bgr24"])
            .args(["-s", &format!("{}x{}", info.width, info.height)])
            .args(["-r", &info.frame_rate_arg(), "-i", "-"])
            .args(["-ss", &params.start_s.to_string()]);
        if let Some(duration) = params.duration_s {
            cmd.args(["-t", &duration.to_string()]);
        }
        cmd.arg("-i").arg(params.original).args(["-map", "0:v", "-map", "1:a?"]);
        if params.nvenc {
            cmd.args(["-c:v", "h264_nvenc", "-preset", "p5", "-rc", "vbr", "-cq", "19", "-b:v", "0"]);
        } else {
            cmd.args(["-c:v", "libx264", "-preset", "fast", "-crf", "18"]);
        }
        cmd.args(["-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "192k", "-shortest"])
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
