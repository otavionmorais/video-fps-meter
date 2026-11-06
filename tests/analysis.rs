//! End-to-end tests on synthetic lossless videos whose real frame rate is
//! known in advance. Requires `ffmpeg` and `ffprobe` on `PATH`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;

use video_fps_meter::{Analysis, Lang, Options, OutputPaths, analyze, save_outputs, summarize};

const WIDTH: usize = 320;
const HEIGHT: usize = 180;

/// Creates an FFV1 video in which each new image lasts `hold(i)` captured
/// frames. A small circle moves on every new image, so small changes must
/// be detected.
fn synthetic_video(name: &str, rate: u32, total_frames: usize, hold: impl Fn(usize) -> usize) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("videos");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.mkv"));
    let mut ffmpeg = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "bgr24"])
        .args(["-s", &format!("{WIDTH}x{HEIGHT}"), "-r", &rate.to_string(), "-i", "-"])
        .args(["-c:v", "ffv1", "-pix_fmt", "bgr0"])
        .arg(&path)
        .stdin(Stdio::piped())
        .spawn()
        .expect("ffmpeg must be installed to run these tests");
    let stdin = ffmpeg.stdin.as_mut().unwrap();

    let (mut image, mut counter, mut remaining) = (Vec::new(), 0usize, 0usize);
    for i in 0..total_frames {
        if remaining == 0 {
            counter += 1;
            image = draw_frame(20 + (counter * 3) % (WIDTH - 40), HEIGHT / 2);
            remaining = hold(i);
        }
        stdin.write_all(&image).unwrap();
        remaining -= 1;
    }
    drop(ffmpeg.stdin.take());
    assert!(ffmpeg.wait().unwrap().success());
    path
}

fn draw_frame(cx: usize, cy: usize) -> Vec<u8> {
    let mut frame = Vec::with_capacity(WIDTH * HEIGHT * 3);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let (dx, dy) = (x as i64 - cx as i64, y as i64 - cy as i64);
            let bgr = if dx * dx + dy * dy <= 144 { [230, 230, 255] } else { [60, 120, 80] };
            frame.extend_from_slice(&bgr);
        }
    }
    frame
}

fn run(path: &Path, options: Options) -> Analysis {
    analyze(path, &options, &AtomicBool::new(false), |_| {}).expect("analysis succeeds")
}

fn average_fps(analysis: &Analysis) -> f64 {
    let new_images = analysis.rows.iter().filter(|r| r.new_image).count();
    new_images as f64 / (analysis.rows.len() as f64 / analysis.capture_rate)
}

fn frametimes(analysis: &Analysis) -> Vec<f64> {
    analysis.rows.iter().filter_map(|r| r.frametime_ms).collect()
}

#[test]
fn steady_30fps_in_60hz_capture() {
    let video = synthetic_video("steady", 60, 600, |_| 2);
    let analysis = run(&video, Options::default());
    assert!((analysis.capture_rate - 60.0).abs() < 1e-9);
    assert_eq!(analysis.rows.len(), 600);
    assert!((average_fps(&analysis) - 30.0).abs() < 0.2);
    assert!(frametimes(&analysis).iter().all(|ft| (ft - 33.33).abs() < 0.05));
}

#[test]
fn drop_is_detected() {
    // Between frames 300 and 360 each image lasts 3 frames (20 fps).
    let video = synthetic_video("drop", 60, 600, |i| if (300..360).contains(&i) { 3 } else { 2 });
    let analysis = run(&video, Options::default());
    let worst = frametimes(&analysis).into_iter().fold(0.0, f64::max);
    assert!((worst - 50.0).abs() < 0.1);
    assert!(average_fps(&analysis) < 30.0);
    // A sustained drop to 20 fps is a lower frame rate, not a stutter.
    assert!(analysis.stutters.is_empty());
    assert!(analysis.static_intervals.is_empty());
}

#[test]
fn static_screen_is_excluded() {
    let video = synthetic_video("static", 30, 120, |_| 10_000);
    let analysis = run(&video, Options::default());
    assert_eq!(analysis.rows.iter().filter(|r| r.new_image).count(), 1);
    assert_eq!(analysis.static_intervals.len(), 1);
    assert!(analysis.stutters.is_empty());
}

#[test]
fn menu_and_stutter_are_told_apart() {
    // 4 s of 30 fps, a 150 ms hitch, 2 s of 30 fps, a 3 s pause, 2 s of 30 fps.
    let video = synthetic_video("menu_and_stutter", 60, 669, |i| match i {
        240 => 9,
        369 => 180,
        _ => 2,
    });
    let destination = OutputPaths::for_video(&video).overlay;
    let _ = std::fs::remove_file(&destination);
    let analysis = run(&video, Options { overlay: Some(destination.clone()), ..Options::default() });

    assert_eq!(analysis.static_intervals.len(), 1);
    let pause = &analysis.static_intervals[0];
    assert!((pause.start_s - 370.0 / 60.0).abs() < 1e-9);
    assert!((pause.duration_s - 179.0 / 60.0).abs() < 1e-9);

    assert_eq!(analysis.stutters.len(), 1);
    assert!((analysis.stutters[0].frametime_ms - 150.0).abs() < 0.01);
    assert!((analysis.stutters[0].time_s - 249.0 / 60.0).abs() < 1e-9);

    let stats = analysis.stats();
    assert!((stats.worst_frametime().unwrap() - 150.0).abs() < 0.01);
    assert!(analysis.rows.iter().filter(|r| r.is_static).all(|r| r.fps.is_none()));
    assert_eq!(probe_frame_count(&destination), "669");
}

#[test]
fn start_and_end_select_a_span() {
    let video = synthetic_video("span", 60, 600, |_| 2);
    let analysis = run(&video, Options { start_s: 2.0, end_s: Some(4.0), ..Options::default() });
    assert_eq!(analysis.rows.len(), 120);
    assert_eq!(analysis.rows.first().map(|r| r.frame), Some(120));
    assert_eq!(analysis.rows.last().map(|r| r.frame), Some(239));
}

#[test]
fn hardware_and_software_decoding_agree() {
    let video = synthetic_video("hwdecode", 60, 240, |i| 2 + i % 3);
    let hw = run(&video, Options::default());
    let sw = run(&video, Options { hw_decode: false, ..Options::default() });
    assert_eq!(hw.rows, sw.rows);
}

#[test]
fn summary_in_both_languages() {
    let video = synthetic_video("languages", 60, 120, |_| 2);
    let analysis = run(&video, Options::default());
    assert!(summarize(&analysis, Lang::Pt).contains("FPS médio real"));
    assert!(summarize(&analysis, Lang::En).contains("Real average FPS"));
}

#[test]
fn optional_outputs() {
    let video = synthetic_video("outputs", 60, 120, |_| 2);
    let paths = OutputPaths::for_video(&video);
    let _ = std::fs::remove_file(&paths.csv);
    let _ = std::fs::remove_file(&paths.chart);
    let analysis = run(&video, Options::default());

    save_outputs(&analysis, &video, true, false, Lang::En);
    assert!(paths.csv.exists());
    assert!(!paths.chart.exists());
    let csv = std::fs::read_to_string(&paths.csv).unwrap();
    assert_eq!(csv.lines().count(), 121);
    assert_eq!(
        csv.lines().next(),
        Some("frame,time_s,changed_pixels,new_image,static,fps,frametime_ms,stutter")
    );

    save_outputs(&analysis, &video, false, true, Lang::En);
    assert!(paths.chart.exists());
}

#[test]
fn overlay_video_has_every_frame() {
    let video = synthetic_video("overlay", 60, 180, |_| 2);
    let destination = OutputPaths::for_video(&video).overlay;
    let _ = std::fs::remove_file(&destination);
    let analysis = run(&video, Options { overlay: Some(destination.clone()), ..Options::default() });
    assert_eq!(analysis.rows.len(), 180);
    assert_eq!(probe_frame_count(&destination), "180");
}

fn probe_frame_count(path: &Path) -> String {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-count_frames", "-select_streams", "v:0"])
        .args(["-show_entries", "stream=nb_read_frames", "-of", "csv=p=0"])
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

#[test]
fn missing_file_is_reported() {
    let result =
        analyze(Path::new("does-not-exist.mkv"), &Options::default(), &AtomicBool::new(false), |_| {});
    assert!(result.is_err());
}

/// Creates a 3 s source (90 frames) with moving content, BT.709 limited-range
/// color tags and an AAC audio track slightly shorter than the video.
fn source_with_audio(name: &str, pix_fmt: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("videos");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.mkv"));
    let status = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(["-f", "lavfi", "-i", &format!("testsrc2=size={WIDTH}x{HEIGHT}:rate=30")])
        .args(["-f", "lavfi", "-t", "2.9", "-i", "sine=frequency=440:sample_rate=48000"])
        .args(["-frames:v", "90", "-c:v", "ffv1", "-pix_fmt", pix_fmt, "-c:a", "aac"])
        .args([
            "-color_range",
            "tv",
            "-colorspace",
            "bt709",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
        ])
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success());
    path
}

/// Decoded frames of `path`, cropped to `crop` (FFmpeg `w:h:x:y`), in `pix_fmt`.
fn decoded(path: &Path, pix_fmt: &str, crop: Option<&str>) -> Vec<u8> {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-i"]).arg(path).args(["-map", "0:v"]);
    if let Some(crop) = crop {
        cmd.args(["-vf", &format!("crop={crop}")]);
    }
    let output = cmd.args(["-f", "rawvideo", "-pix_fmt", pix_fmt, "-"]).output().unwrap();
    assert!(output.status.success());
    output.stdout
}

/// MD5 of every audio packet payload, in order.
fn audio_packet_hashes(path: &Path) -> String {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a:0", "-show_entries", "packet=data_hash"])
        .args(["-show_data_hash", "md5", "-of", "default=nw=1:nk=1"])
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stream_field(path: &Path, stream: &str, field: &str) -> String {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", stream, "-show_entries", &format!("stream={field}")])
        .args(["-of", "csv=p=0"])
        .arg(path)
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn assert_lossless_overlay(name: &str, pix_fmt: &str, nvenc: bool) {
    let source = source_with_audio(name, pix_fmt);
    let destination = OutputPaths::for_video(&source).overlay;
    assert_eq!(destination.extension().unwrap(), "mkv");
    let _ = std::fs::remove_file(&destination);
    let analysis = run(&source, Options { overlay: Some(destination.clone()), nvenc, ..Options::default() });
    assert_eq!(analysis.rows.len(), 90);

    // The overlay covers the left part of the frame; the right part must be
    // bit-identical to the source.
    let untouched = "152:180:168:0";
    let original = decoded(&source, pix_fmt, Some(untouched));
    let rendered = decoded(&destination, pix_fmt, Some(untouched));
    assert!(!original.is_empty());
    assert!(original == rendered, "pixels outside the overlay changed");
    assert_ne!(decoded(&source, pix_fmt, None), decoded(&destination, pix_fmt, None));

    assert_eq!(stream_field(&destination, "v:0", "pix_fmt"), pix_fmt);
    assert_eq!(stream_field(&destination, "v:0", "color_range"), stream_field(&source, "v:0", "color_range"));
    assert_eq!(stream_field(&destination, "v:0", "color_space"), "bt709");
    assert_eq!(stream_field(&destination, "a:0", "codec_name"), "aac");
    let audio = audio_packet_hashes(&source);
    assert!(audio.lines().count() > 100);
    assert_eq!(audio_packet_hashes(&destination), audio, "audio was not copied unchanged");
    assert_eq!(probe_frame_count(&destination), "90");
}

#[test]
fn overlay_is_lossless_outside_the_overlay_8bit() {
    assert_lossless_overlay("lossless_8bit", "yuv420p", false);
}

#[test]
fn overlay_is_lossless_outside_the_overlay_10bit() {
    assert_lossless_overlay("lossless_10bit", "yuv420p10le", false);
}

#[test]
fn overlay_is_lossless_with_nvenc_or_fallback() {
    assert_lossless_overlay("lossless_nvenc", "yuv420p", true);
}
