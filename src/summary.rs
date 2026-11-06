//! Plain-text summary of an analysis.

use crate::analysis::Analysis;
use crate::i18n::Lang;

/// Summarizes an analysis: capture rate, span, excluded static intervals,
/// average FPS, frametimes, 1% low, stutters and the distribution of changed
/// pixels. Statistics cover only the non-static frames.
pub fn summarize(analysis: &Analysis, lang: Lang) -> String {
    let s = lang.strings();
    let rows = &analysis.rows;
    if rows.is_empty() {
        return s.no_frames.to_owned();
    }
    let rate = analysis.capture_rate;
    let stats = analysis.stats();

    let mut lines = vec![
        format!("{}: {rate:.2} Hz", s.capture_rate),
        format!(
            "{}: {:.1} s ({} {}), {:.1} s {}",
            s.analyzed_span,
            rows.len() as f64 / rate,
            rows.len(),
            s.frames,
            stats.active_duration_s(rate),
            s.active_time
        ),
    ];

    let intervals = &analysis.static_intervals;
    if let Some(longest) = intervals.iter().max_by(|a, b| a.duration_s.total_cmp(&b.duration_s)) {
        let total: f64 = intervals.iter().map(|i| i.duration_s).sum();
        lines.push(format!(
            "{}: {} {}, {total:.1} s ({} {:.1} s {} {})",
            s.static_excluded,
            intervals.len(),
            if intervals.len() == 1 { s.interval } else { s.intervals },
            s.longest,
            longest.duration_s,
            s.at,
            format_time(longest.start_s)
        ));
    }

    if let Some(fps) = stats.average_fps(rate) {
        lines.push(format!("{}: {fps:.2}", s.average_fps));
    }
    if let (Some(typical), Some(worst), Some(low)) =
        (stats.typical_frametime(), stats.worst_frametime(), stats.one_percent_low())
    {
        lines.push(format!("{}: {typical:.1} ms", s.typical_frametime));
        lines.push(format!("{}: {worst:.1} ms", s.worst_frametime));
        lines.push(format!("{}: {low:.1}", s.one_percent_low));
    }

    let mut stutter_line = format!(
        "{}: {} ({:.1}/min)",
        s.stutters,
        stats.stutters(),
        stats.stutters_per_minute(rate).unwrap_or(0.0)
    );
    if let Some(worst) = analysis.stutters.iter().max_by(|a, b| a.frametime_ms.total_cmp(&b.frametime_ms)) {
        stutter_line.push_str(&format!(
            ", {} {:.1} ms {} {}",
            s.worst,
            worst.frametime_ms,
            s.at,
            format_time(worst.time_s)
        ));
    }
    lines.push(stutter_line);

    let mut changed: Vec<u64> = rows.iter().skip(1).map(|r| r.changed_pixels).collect();
    changed.sort_unstable();
    if let (Some(min), Some(max)) = (changed.first(), changed.last()) {
        let median = changed[changed.len() / 2];
        lines.push(format!("{}: {} {min}, {} {median}, {} {max}", s.changed_pixels, s.min, s.median, s.max));
    }
    lines.join("\n")
}

/// Formats a position in the recording as `m:ss.s` (or `h:mm:ss.s`).
pub fn format_time(seconds: f64) -> String {
    let tenths = (seconds * 10.0).round() as u64;
    let (hours, rest) = (tenths / 36_000, tenths % 36_000);
    let (minutes, rest) = (rest / 600, rest % 600);
    let (secs, tenth) = (rest / 10, rest % 10);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}.{tenth}")
    } else {
        format!("{minutes}:{secs:02}.{tenth}")
    }
}

#[cfg(test)]
mod tests {
    use super::format_time;

    #[test]
    fn formats_positions() {
        assert_eq!(format_time(0.0), "0:00.0");
        assert_eq!(format_time(83.44), "1:23.4");
        assert_eq!(format_time(3725.0), "1:02:05.0");
    }
}
