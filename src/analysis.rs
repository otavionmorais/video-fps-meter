//! Frame-by-frame analysis of a recording.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel, sync_channel};
use std::thread;

use crate::decode::{DecodeParams, Decoder};
use crate::encode::{EncodeParams, Encoder, nvenc_available};
use crate::i18n::{Lang, Strings};
use crate::metric::gray_and_count;
use crate::overlay::Overlay;
use crate::probe::{ProbeError, VideoInfo, probe};
use crate::{Error, ffmpeg};

/// Frames buffered between the analysis and the overlay encoder.
const OVERLAY_QUEUE_DEPTH: usize = 2;

/// Analysis settings.
#[derive(Clone, Debug)]
pub struct Options {
    /// A frame is a new image when more than this many pixels changed.
    pub threshold: u64,
    /// Start of the analyzed span, in seconds.
    pub start_s: f64,
    /// End of the analyzed span, in seconds. `None` (or zero) means the end of the video.
    pub end_s: Option<f64>,
    /// Target frame rate of the game. The overlay turns red below it.
    pub target_fps: f64,
    /// Destination of the overlay video. `None` skips the overlay.
    pub overlay: Option<PathBuf>,
    /// Encode the overlay with NVIDIA NVENC, falling back to x264.
    pub nvenc: bool,
    /// Let FFmpeg decode on the GPU when possible.
    pub hw_decode: bool,
    pub lang: Lang,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            threshold: 20,
            start_s: 0.0,
            end_s: None,
            target_fps: 30.0,
            overlay: None,
            nvenc: false,
            hw_decode: true,
            lang: Lang::default(),
        }
    }
}

/// Result for one captured frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Index of the frame in the recording.
    pub frame: u64,
    pub time_s: f64,
    /// Pixels that changed relative to the previous frame (0 for the first frame).
    pub changed_pixels: u64,
    /// Whether this frame shows a new image rather than a repeat.
    pub new_image: bool,
    /// Rolling frame rate over the last second of capture.
    pub fps: f64,
    /// On a new image: how long the previous image stayed on screen.
    pub frametime_ms: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct Analysis {
    pub rows: Vec<Row>,
    /// Frame rate of the recording, in Hz.
    pub capture_rate: f64,
    /// Non-fatal messages for the user.
    pub warnings: Vec<String>,
    /// Whether the analysis stopped early because it was canceled.
    pub canceled: bool,
}

/// Progress report, sent once per analyzed frame.
#[derive(Clone, Copy, Debug)]
pub struct Progress {
    /// Completed fraction of the analyzed span (0 to 1).
    pub fraction: f64,
    /// Position in the recording, in seconds.
    pub video_time_s: f64,
}

struct OverlayJob {
    frame: Vec<u8>,
    fps: f64,
    frametime_ms: Option<f64>,
}

/// Analyzes a recording.
///
/// Setting `cancel` stops the analysis and returns the rows analyzed so far.
pub fn analyze(
    path: &Path,
    options: &Options,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(Progress),
) -> Result<Analysis, Error> {
    let strings = options.lang.strings();
    let info = probe(path).map_err(|e| match e {
        ProbeError::Missing => Error(strings.ffmpeg_missing.to_owned()),
        ProbeError::Unreadable(detail) => open_error(strings, path, &detail),
        ProbeError::NoFrameRate => Error(strings.frame_rate_failed.to_owned()),
    })?;
    if !ffmpeg::is_available("ffmpeg") {
        return Err(Error(strings.ffmpeg_missing.to_owned()));
    }

    let rate = info.frame_rate();
    let frame_ms = 1000.0 / rate;
    let end_s = options.end_s.filter(|&end| end > 0.0);
    let first = (options.start_s * rate).round_ties_even() as u64;
    let last = end_s.map(|end| (end * rate).round_ties_even() as u64);
    let max_frames = last.map(|last| last.saturating_sub(first));
    let span_frames = last.or(info.frame_count).unwrap_or(0).saturating_sub(first).max(1);
    let pixels = info.width as usize * info.height as usize;

    let mut result = Analysis {
        rows: Vec::with_capacity(max_frames.unwrap_or(0) as usize),
        capture_rate: rate,
        warnings: Vec::new(),
        canceled: false,
    };
    if max_frames == Some(0) {
        return Ok(result);
    }

    // Seek half a frame early so container timestamp rounding cannot skip
    // the first requested frame.
    let start_s = if first == 0 { 0.0 } else { (first as f64 - 0.5) / rate };
    let mut params = DecodeParams {
        path,
        start_s,
        max_frames,
        hw_decode: options.hw_decode,
        pix_fmt: info.pix_fmt.as_deref(),
        frame_len: pixels * 3,
    };
    let (mut decoder, mut pending, mut recycle) = start_decoder(&params, strings)?;
    if pending.is_none() && params.hw_decode {
        // The hardware decoder produced nothing; retry on the CPU.
        drop(decoder);
        params.hw_decode = false;
        (decoder, pending, recycle) = start_decoder(&params, strings)?;
    }

    let mut encoder = None;
    if let Some(destination) = &options.overlay {
        let nvenc = options.nvenc && nvenc_available();
        if options.nvenc && !nvenc {
            result.warnings.push(strings.nvenc_unavailable.to_owned());
        }
        let params = EncodeParams {
            destination,
            original: path,
            info: &info,
            start_s: options.start_s,
            duration_s: end_s.map(|end| end - options.start_s),
            nvenc,
        };
        let spawned = Encoder::spawn(&params).map_err(|_| Error(strings.ffmpeg_missing.to_owned()))?;
        encoder = Some(spawn_overlay_thread(spawned, &info, options, recycle.clone()));
    }

    let window_len = (rate.round_ties_even() as usize).max(1);
    let mut window = VecDeque::with_capacity(window_len);
    let mut window_sum = 0u32;
    let mut gray = vec![0u8; pixels];
    let mut previous = vec![0u8; pixels];
    let mut has_previous = false;
    let mut repeats = 0u64;
    let mut index = first;
    let mut encoder_stopped = false;

    loop {
        if cancel.load(Ordering::Relaxed) {
            result.canceled = true;
            break;
        }
        let Some(frame) = pending.take().or_else(|| decoder.next_frame()) else { break };

        let changed = gray_and_count(&frame, &mut gray, has_previous.then_some(&previous[..]));
        let new_image = !has_previous || changed > options.threshold;
        let frametime_ms = if new_image {
            let frametime = (repeats > 0).then_some(repeats as f64 * frame_ms);
            repeats = 1;
            frametime
        } else {
            repeats += 1;
            None
        };

        if window.len() == window_len {
            window_sum -= window.pop_front().unwrap_or(0);
        }
        window.push_back(u32::from(new_image));
        window_sum += u32::from(new_image);
        let fps = f64::from(window_sum) * rate / window.len() as f64;

        result.rows.push(Row {
            frame: index,
            time_s: index as f64 / rate,
            changed_pixels: changed,
            new_image,
            fps,
            frametime_ms,
        });

        match &encoder {
            Some((jobs, _)) => {
                if jobs.send(OverlayJob { frame, fps, frametime_ms }).is_err() {
                    encoder_stopped = true;
                    break;
                }
            }
            None => {
                let _ = recycle.send(frame);
            }
        }

        std::mem::swap(&mut gray, &mut previous);
        has_previous = true;
        index += 1;
        on_progress(Progress {
            fraction: ((index - first) as f64 / span_frames as f64).min(1.0),
            video_time_s: index as f64 / rate,
        });
    }

    let (status, decoder_errors) = decoder.finish();
    if let Some((jobs, worker)) = encoder {
        drop(jobs);
        let (write_failed, encoder_errors) = worker.join().unwrap_or((true, String::new()));
        if write_failed || encoder_stopped {
            let detail = ffmpeg::tail(&encoder_errors, 800);
            return Err(Error(format!("{}\n{detail}", strings.encode_failed)));
        }
    }
    if result.rows.is_empty() && !result.canceled && status.is_some_and(|s| !s.success()) {
        return Err(open_error(strings, path, &decoder_errors));
    }
    on_progress(Progress { fraction: 1.0, video_time_s: index as f64 / rate });
    Ok(result)
}

/// A running decoder, its first frame (`None` if decoding produced nothing)
/// and the sender used to return frame buffers for reuse.
type StartedDecoder = (Decoder, Option<Vec<u8>>, Sender<Vec<u8>>);

/// Starts a decoder and waits for its first frame.
fn start_decoder(params: &DecodeParams, strings: &Strings) -> Result<StartedDecoder, Error> {
    let (recycle, recycled) = channel();
    let mut decoder =
        Decoder::spawn(params, recycled).map_err(|_| Error(strings.ffmpeg_missing.to_owned()))?;
    let first = decoder.next_frame();
    Ok((decoder, first, recycle))
}

type OverlayWorker = (std::sync::mpsc::SyncSender<OverlayJob>, thread::JoinHandle<(bool, String)>);

/// Draws the overlay and feeds the encoder on a separate thread. The worker
/// returns whether a write failed and FFmpeg's error output.
fn spawn_overlay_thread(
    mut encoder: Encoder,
    info: &VideoInfo,
    options: &Options,
    recycle: Sender<Vec<u8>>,
) -> OverlayWorker {
    let (jobs, queue) = sync_channel::<OverlayJob>(OVERLAY_QUEUE_DEPTH);
    let mut overlay =
        Overlay::new(info.width, info.height, info.frame_rate(), options.target_fps, options.lang);
    let worker = thread::spawn(move || {
        let mut write_failed = false;
        for mut job in queue {
            overlay.record(job.fps, job.frametime_ms);
            overlay.draw(&mut job.frame, job.fps);
            if encoder.write_frame(&job.frame).is_err() {
                write_failed = true;
                break;
            }
            let _ = recycle.send(job.frame);
        }
        (write_failed, encoder.finish())
    });
    (jobs, worker)
}

fn open_error(strings: &Strings, path: &Path, detail: &str) -> Error {
    let mut message = format!("{}: {}", strings.open_failed, path.display());
    if !detail.is_empty() {
        message.push('\n');
        message.push_str(ffmpeg::tail(detail, 800));
    }
    Error(message)
}
