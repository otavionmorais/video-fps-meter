//! Analysis of a recording.
//!
//! 1. **Detection pass:** every frame is compared with the previous one to
//!    find new images.
//! 2. **Timeline:** frametimes, static intervals, rolling FPS and stutters
//!    are derived from the whole sequence (see [`crate::timeline`]).
//! 3. **Overlay pass** (optional): the recording is decoded again and the
//!    overlay is drawn with the complete timeline, so static intervals are
//!    marked from their first frame.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, SyncSender, channel, sync_channel};
use std::thread::{self, JoinHandle};

use crate::decode::{DecodeParams, Decoder};
use crate::encode::{EncodeParams, Encoder, nvenc_available};
use crate::i18n::{Lang, Strings};
use crate::metric::gray_and_count;
use crate::overlay::Overlay;
use crate::probe::{ProbeError, VideoInfo, probe};
use crate::stats::Stats;
use crate::timeline::{self, DEFAULT_STATIC_MS, DEFAULT_STUTTER_FACTOR, StaticInterval, Stutter};
use crate::{Error, ffmpeg};

/// Frames buffered between overlay drawing and the encoder.
const ENCODER_QUEUE_DEPTH: usize = 2;
/// Share of the progress bar given to the detection pass when an overlay is
/// generated (the overlay pass is slower).
const DETECTION_SHARE: f64 = 0.4;

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
    /// An image unchanged for at least this long marks a static interval
    /// (menu, pause), excluded from the statistics. Zero disables it.
    pub static_ms: f64,
    /// A frametime at least this many times the recent median is a stutter.
    pub stutter_factor: f64,
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
            static_ms: DEFAULT_STATIC_MS,
            stutter_factor: DEFAULT_STUTTER_FACTOR,
            overlay: None,
            nvenc: false,
            hw_decode: true,
            lang: Lang::default(),
        }
    }
}

/// Result for one captured frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    /// Index of the frame in the recording.
    pub frame: u64,
    pub time_s: f64,
    /// Pixels that changed relative to the previous frame (0 for the first frame).
    pub changed_pixels: u64,
    /// Whether this frame shows a new image rather than a repeat.
    pub new_image: bool,
    /// Whether this frame belongs to a static interval (menu, pause).
    pub is_static: bool,
    /// Rolling frame rate over the last second of non-static frames.
    /// `None` inside static intervals and at the start of the recording.
    pub fps: Option<f64>,
    /// On a new image: how long the previous image stayed on screen.
    /// `None` when the previous image belonged to a static interval.
    pub frametime_ms: Option<f64>,
    /// Whether `frametime_ms` is a stutter.
    pub stutter: bool,
}

#[derive(Clone, Debug)]
pub struct Analysis {
    pub rows: Vec<Row>,
    /// Frame rate of the recording, in Hz.
    pub capture_rate: f64,
    pub static_intervals: Vec<StaticInterval>,
    pub stutters: Vec<Stutter>,
    /// Non-fatal messages for the user.
    pub warnings: Vec<String>,
    /// Whether the analysis stopped early because it was canceled.
    pub canceled: bool,
}

impl Analysis {
    pub fn stats(&self) -> Stats {
        Stats::from_rows(&self.rows)
    }
}

/// Current step of the analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Detection,
    Overlay,
}

/// Progress report, sent once per processed frame.
#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub stage: Stage,
    /// Completed fraction of the whole job (0 to 1).
    pub fraction: f64,
    /// Position in the recording, in seconds.
    pub video_time_s: f64,
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
    let end_s = options.end_s.filter(|&end| end > 0.0);
    let first = (options.start_s * rate).round_ties_even() as u64;
    let last = end_s.map(|end| (end * rate).round_ties_even() as u64);
    let max_frames = last.map(|last| last.saturating_sub(first));
    let span_frames = last.or(info.frame_count).unwrap_or(0).saturating_sub(first).max(1);
    let detection_share = if options.overlay.is_some() { DETECTION_SHARE } else { 1.0 };

    let mut result = Analysis {
        rows: Vec::with_capacity(max_frames.unwrap_or(0) as usize),
        capture_rate: rate,
        static_intervals: Vec::new(),
        stutters: Vec::new(),
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
        frame_len: info.width as usize * info.height as usize * 3,
    };

    // Detection pass.
    let (mut decoder, mut pending, mut recycle) = start_decoder(&params, strings)?;
    if pending.is_none() && params.hw_decode {
        // The hardware decoder produced nothing; retry on the CPU.
        drop(decoder);
        params.hw_decode = false;
        (decoder, pending, recycle) = start_decoder(&params, strings)?;
    }
    let pixels = params.frame_len / 3;
    let mut gray = vec![0u8; pixels];
    let mut previous = vec![0u8; pixels];
    let mut index = first;
    loop {
        if cancel.load(Ordering::Relaxed) {
            result.canceled = true;
            break;
        }
        let Some(frame) = pending.take().or_else(|| decoder.next_frame()) else { break };
        let has_previous = !result.rows.is_empty();
        let changed = gray_and_count(&frame, &mut gray, has_previous.then_some(&previous[..]));
        let _ = recycle.send(frame);
        result.rows.push(Row {
            frame: index,
            time_s: index as f64 / rate,
            changed_pixels: changed,
            new_image: !has_previous || changed > options.threshold,
            ..Row::default()
        });
        std::mem::swap(&mut gray, &mut previous);
        index += 1;
        on_progress(Progress {
            stage: Stage::Detection,
            fraction: detection_share * ((index - first) as f64 / span_frames as f64).min(1.0),
            video_time_s: index as f64 / rate,
        });
    }
    let (status, decoder_errors) = decoder.finish();
    if result.rows.is_empty() && !result.canceled && status.is_some_and(|s| !s.success()) {
        return Err(open_error(strings, path, &decoder_errors));
    }

    let timeline = timeline::build(&mut result.rows, rate, options.static_ms, options.stutter_factor);
    result.static_intervals = timeline.static_intervals;
    result.stutters = timeline.stutters;

    // Overlay pass.
    if let Some(destination) = &options.overlay
        && !result.canceled
        && !result.rows.is_empty()
    {
        let nvenc = options.nvenc && nvenc_available();
        if options.nvenc && !nvenc {
            result.warnings.push(strings.nvenc_unavailable.to_owned());
        }
        let encode = EncodeParams {
            destination,
            original: path,
            info: &info,
            start_s: options.start_s,
            duration_s: end_s.map(|end| end - options.start_s),
            nvenc,
        };
        let total = result.rows.len() as f64;
        let completed = render_overlay(&params, &encode, &info, options, &result.rows, cancel, |done| {
            on_progress(Progress {
                stage: Stage::Overlay,
                fraction: detection_share + (1.0 - detection_share) * done as f64 / total,
                video_time_s: result.rows[done - 1].time_s,
            });
        })?;
        result.canceled = !completed;
    }

    if !result.canceled {
        let stage = if options.overlay.is_some() { Stage::Overlay } else { Stage::Detection };
        on_progress(Progress { stage, fraction: 1.0, video_time_s: index as f64 / rate });
    }
    Ok(result)
}

/// Decodes the span again, draws the overlay for each row and encodes the
/// result. Returns `false` if canceled.
fn render_overlay(
    decode: &DecodeParams,
    encode: &EncodeParams,
    info: &VideoInfo,
    options: &Options,
    rows: &[Row],
    cancel: &AtomicBool,
    mut on_frame: impl FnMut(usize),
) -> Result<bool, Error> {
    let strings = options.lang.strings();
    let (mut decoder, mut pending, recycle) = start_decoder(decode, strings)?;
    let encoder = Encoder::spawn(encode).map_err(|_| Error(strings.ffmpeg_missing.to_owned()))?;
    let (frames, writer) = spawn_writer(encoder, recycle);

    let mut overlay =
        Overlay::new(info.width, info.height, info.frame_rate(), options.target_fps, options.lang);
    let mut stats = Stats::new();
    let mut completed = true;
    let mut writer_stopped = false;
    for (i, row) in rows.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            completed = false;
            break;
        }
        let Some(mut frame) = pending.take().or_else(|| decoder.next_frame()) else { break };
        stats.push(row);
        overlay.record(row);
        overlay.draw(&mut frame, row, &stats);
        if frames.send(frame).is_err() {
            writer_stopped = true;
            break;
        }
        on_frame(i + 1);
    }

    drop(decoder);
    drop(frames);
    let (write_failed, encoder_errors) = writer.join().unwrap_or((true, String::new()));
    if write_failed || writer_stopped {
        let detail = ffmpeg::tail(&encoder_errors, 800);
        return Err(Error(format!("{}\n{detail}", strings.encode_failed)));
    }
    Ok(completed)
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

/// Writes frames to the encoder on a separate thread. The thread returns
/// whether a write failed and FFmpeg's error output.
fn spawn_writer(
    mut encoder: Encoder,
    recycle: Sender<Vec<u8>>,
) -> (SyncSender<Vec<u8>>, JoinHandle<(bool, String)>) {
    let (frames, queue) = sync_channel::<Vec<u8>>(ENCODER_QUEUE_DEPTH);
    let writer = thread::spawn(move || {
        let mut write_failed = false;
        for frame in queue {
            if encoder.write_frame(&frame).is_err() {
                write_failed = true;
                break;
            }
            let _ = recycle.send(frame);
        }
        (write_failed, encoder.finish())
    });
    (frames, writer)
}

fn open_error(strings: &Strings, path: &Path, detail: &str) -> Error {
    let mut message = format!("{}: {}", strings.open_failed, path.display());
    if !detail.is_empty() {
        message.push('\n');
        message.push_str(ffmpeg::tail(detail, 800));
    }
    Error(message)
}
