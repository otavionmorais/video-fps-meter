//! Plain-text summary of an analysis.

use crate::analysis::Row;
use crate::i18n::Lang;

/// Summarizes the analyzed rows: capture rate, span, average FPS, frametimes,
/// 1% low and the distribution of changed pixels.
pub fn summarize(rows: &[Row], capture_rate: f64, lang: Lang) -> String {
    let s = lang.strings();
    if rows.is_empty() {
        return s.no_frames.to_owned();
    }
    let duration = rows.len() as f64 / capture_rate;
    let new_images = rows.iter().filter(|r| r.new_image).count();
    let mut frametimes: Vec<f64> = rows.iter().filter_map(|r| r.frametime_ms).map(round2).collect();
    frametimes.sort_by(f64::total_cmp);

    let mut lines = vec![
        format!("{}: {capture_rate:.2} Hz", s.capture_rate),
        format!("{}: {duration:.1} s ({} {})", s.analyzed_span, rows.len(), s.frames),
        format!("{}: {:.2}", s.average_fps, new_images as f64 / duration),
    ];
    if let Some(&worst) = frametimes.last() {
        let slowest = &frametimes[(frametimes.len() as f64 * 0.99) as usize..];
        let slowest_mean = slowest.iter().sum::<f64>() / slowest.len() as f64;
        lines.push(format!("{}: {:.1} ms", s.typical_frametime, frametimes[frametimes.len() / 2]));
        lines.push(format!("{}: {worst:.1} ms", s.worst_frametime));
        lines.push(format!("{}: {:.1}", s.one_percent_low, 1000.0 / slowest_mean));
    }

    let mut changed: Vec<u64> = rows.iter().skip(1).map(|r| r.changed_pixels).collect();
    changed.sort_unstable();
    if let (Some(min), Some(max)) = (changed.first(), changed.last()) {
        let median = changed[changed.len() / 2];
        lines.push(format!("{}: {} {min}, {} {median}, {} {max}", s.changed_pixels, s.min, s.median, s.max));
    }
    lines.join("\n")
}

/// Frametimes are compared at 0.01 ms resolution, as stored in the CSV.
fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}
