//! Frame decoding through an `ffmpeg` subprocess that emits raw BGR24 frames.

use std::io::{self, Read};
use std::path::Path;
use std::process::{Child, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread::{self, JoinHandle};

use crate::ffmpeg;

/// Decoded frames buffered between the reader thread and the consumer.
const QUEUE_DEPTH: usize = 3;

/// Hardware decoder requested from FFmpeg (NVIDIA NVDEC). When it is not
/// available the analysis retries on the CPU.
///
/// Other accelerators are deliberately not used: in measurements with 4K
/// HEVC, D3D11VA was slower than CPU decoding because of the copy back to
/// system memory, and `-hwaccel auto` may select the Vulkan decoder, which
/// crashes on some codecs (e.g. FFV1 in FFmpeg 9).
const HWACCEL: &str = "cuda";

pub struct DecodeParams<'a> {
    pub path: &'a Path,
    /// Position of the first frame, in seconds.
    pub start_s: f64,
    /// Maximum number of frames to decode.
    pub max_frames: Option<u64>,
    /// Decode on the GPU.
    pub hw_decode: bool,
    /// Pixel format of the software decoder. Hardware frames are converted
    /// to it before the BGR conversion, which makes the output bit-identical
    /// to software decoding (a direct NV12 → BGR conversion is not).
    pub pix_fmt: Option<&'a str>,
    /// Size of one BGR24 frame in bytes.
    pub frame_len: usize,
}

/// Running decoder. Frames arrive in order through [`Decoder::next`].
pub struct Decoder {
    child: Child,
    frames: Option<Receiver<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
    stderr: Option<JoinHandle<String>>,
    reached_end: bool,
}

impl Decoder {
    /// Starts FFmpeg. Buffers sent to `recycled` are reused for later frames.
    pub fn spawn(params: &DecodeParams, recycled: Receiver<Vec<u8>>) -> io::Result<Self> {
        let mut cmd = ffmpeg::command("ffmpeg");
        cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
        if params.hw_decode {
            cmd.args(["-hwaccel", HWACCEL]);
        }
        cmd.args(["-ss", &params.start_s.to_string()]).arg("-i").arg(params.path).args([
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
        ]);
        if let Some(max) = params.max_frames {
            cmd.args(["-frames:v", &max.to_string()]);
        }
        if let (true, Some(pix_fmt)) = (params.hw_decode, params.pix_fmt) {
            cmd.args(["-vf", &format!("format={pix_fmt}")]);
        }
        cmd.args(["-f", "rawvideo", "-pix_fmt", "bgr24", "-"]).stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd.spawn()?;
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = ffmpeg::collect_stderr(child.stderr.take().expect("stderr is piped"));
        let (sender, frames) = sync_channel(QUEUE_DEPTH);
        let frame_len = params.frame_len;
        let reader = thread::spawn(move || read_frames(stdout, frame_len, &sender, &recycled));

        Ok(Self {
            child,
            frames: Some(frames),
            reader: Some(reader),
            stderr: Some(stderr),
            reached_end: false,
        })
    }

    /// Next frame, or `None` at the end of the stream.
    pub fn next_frame(&mut self) -> Option<Vec<u8>> {
        let frame = self.frames.as_ref()?.recv().ok();
        self.reached_end |= frame.is_none();
        frame
    }

    /// Stops the decoder (killing FFmpeg if it has not finished) and returns
    /// its exit status and error output.
    pub fn finish(mut self) -> (Option<ExitStatus>, String) {
        self.shutdown()
    }

    fn shutdown(&mut self) -> (Option<ExitStatus>, String) {
        // Dropping the receiver unblocks a reader waiting on a full queue.
        self.frames = None;
        if !self.reached_end {
            let _ = self.child.kill();
        }
        let status = self.child.wait().ok();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        let stderr = self.stderr.take().map(|h| h.join().unwrap_or_default()).unwrap_or_default();
        (status, stderr)
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        if self.reader.is_some() {
            self.shutdown();
        }
    }
}

fn read_frames(
    mut stdout: impl Read,
    frame_len: usize,
    sender: &SyncSender<Vec<u8>>,
    recycled: &Receiver<Vec<u8>>,
) {
    loop {
        let mut buffer =
            recycled.try_recv().ok().filter(|b| b.len() == frame_len).unwrap_or_else(|| vec![0; frame_len]);
        // A partial frame at the end of the stream is discarded.
        if stdout.read_exact(&mut buffer).is_err() || sender.send(buffer).is_err() {
            return;
        }
    }
}
