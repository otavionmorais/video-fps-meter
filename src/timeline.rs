//! Interpretation of the per-frame detection: frametimes, static intervals,
//! rolling FPS and stutters.
//!
//! A *run* is the stretch of captured frames during which one image stays on
//! screen. Its duration is the frametime of that image.
//!
//! - **Static intervals** (menus, pauses, loading screens) are runs that are
//!   too long, or too consistently slow, to be gameplay. They are excluded
//!   from every statistic. Three rules apply:
//!   1. a single run of at least `static_ms`;
//!   2. a chain of consecutive runs of at least [`IDLE_RUN_MS`] each, lasting
//!      [`IDLE_CHAIN_MS`] or more in total (for example a blinking cursor);
//!   3. an active span shorter than [`MENU_ISLAND_MS`] between two static
//!      spans (an animated transition between two menu screens).
//! - **Stutters** are frametimes of at least `stutter_factor` times the
//!   median of the preceding frametimes (default [`DEFAULT_STUTTER_FACTOR`]).
//!   With the default, a single late frame is not a stutter; at least two
//!   consecutive late frames are.

use std::collections::VecDeque;

use crate::analysis::Row;

/// Default minimum duration of a single static run, in milliseconds.
pub const DEFAULT_STATIC_MS: f64 = 500.0;
/// Minimum duration of a run that can be part of an idle chain.
pub const IDLE_RUN_MS: f64 = 200.0;
/// Minimum total duration of an idle chain.
pub const IDLE_CHAIN_MS: f64 = 1000.0;
/// Active spans shorter than this between two static spans are menu
/// transitions and become static too.
pub const MENU_ISLAND_MS: f64 = 2000.0;
/// Default ratio between a frametime and the recent median for a stutter.
pub const DEFAULT_STUTTER_FACTOR: f64 = 2.5;
/// Number of preceding frametimes used as the stutter reference.
const STUTTER_HISTORY: usize = 20;
/// Frametimes required before stutters are detected.
const STUTTER_MIN_HISTORY: usize = 5;
/// Absorbs floating-point error in frametime comparisons, in milliseconds.
const TOLERANCE_MS: f64 = 0.1;

/// A span excluded from the statistics.
#[derive(Clone, Debug, PartialEq)]
pub struct StaticInterval {
    pub start_s: f64,
    pub duration_s: f64,
}

/// A frametime spike.
#[derive(Clone, Debug, PartialEq)]
pub struct Stutter {
    /// Frame that completed the slow image.
    pub frame: u64,
    pub time_s: f64,
    pub frametime_ms: f64,
    /// Median of the preceding frametimes.
    pub reference_ms: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timeline {
    pub static_intervals: Vec<StaticInterval>,
    pub stutters: Vec<Stutter>,
}

/// Fills `frametime_ms`, `is_static`, `fps` and `stutter` in `rows`, whose
/// `new_image` flags are already set. A `static_ms` of zero disables static
/// detection.
pub fn build(rows: &mut [Row], capture_rate: f64, static_ms: f64, stutter_factor: f64) -> Timeline {
    let frame_ms = 1000.0 / capture_rate;
    let runs = find_runs(rows);
    let is_static_run = classify_runs(&runs, frame_ms, static_ms);

    for row in rows.iter_mut() {
        row.frametime_ms = None;
        row.is_static = false;
        row.fps = None;
        row.stutter = false;
    }
    for (j, run) in runs.iter().enumerate() {
        let run_static = is_static_run[j];
        if run_static {
            // The image that starts a static run is static only when the
            // previous run was static too (it is then inside the menu).
            rows[run.start].is_static = j > 0 && is_static_run[j - 1];
            for row in &mut rows[run.start + 1..run.start + run.len] {
                row.is_static = true;
            }
        }
        if let Some(next) = rows.get_mut(run.start + run.len)
            && !run_static
        {
            next.frametime_ms = Some(run.len as f64 * frame_ms);
        }
    }

    fill_fps(rows, capture_rate);
    Timeline {
        static_intervals: static_intervals(rows, capture_rate),
        stutters: detect_stutters(rows, stutter_factor),
    }
}

struct Run {
    /// Row where the image first appears.
    start: usize,
    /// Number of captured frames showing it.
    len: usize,
}

fn find_runs(rows: &[Row]) -> Vec<Run> {
    let starts: Vec<usize> = rows.iter().enumerate().filter(|(_, r)| r.new_image).map(|(i, _)| i).collect();
    starts
        .iter()
        .enumerate()
        .map(|(j, &start)| {
            let end = starts.get(j + 1).copied().unwrap_or(rows.len());
            Run { start, len: end - start }
        })
        .collect()
}

fn classify_runs(runs: &[Run], frame_ms: f64, static_ms: f64) -> Vec<bool> {
    let mut is_static = vec![false; runs.len()];
    if static_ms <= 0.0 {
        return is_static;
    }
    let duration = |run: &Run| run.len as f64 * frame_ms;

    for (flag, run) in is_static.iter_mut().zip(runs) {
        *flag = duration(run) >= static_ms - TOLERANCE_MS;
    }

    let mut chain_start = 0;
    while chain_start < runs.len() {
        if duration(&runs[chain_start]) < IDLE_RUN_MS - TOLERANCE_MS {
            chain_start += 1;
            continue;
        }
        let mut chain_end = chain_start;
        let mut total = 0.0;
        while chain_end < runs.len() && duration(&runs[chain_end]) >= IDLE_RUN_MS - TOLERANCE_MS {
            total += duration(&runs[chain_end]);
            chain_end += 1;
        }
        if total >= IDLE_CHAIN_MS - TOLERANCE_MS {
            is_static[chain_start..chain_end].fill(true);
        }
        chain_start = chain_end;
    }

    // Short active spans between two static spans (menu transitions).
    let mut i = 0;
    while i < runs.len() {
        if is_static[i] {
            i += 1;
            continue;
        }
        let island_start = i;
        let mut total = 0.0;
        while i < runs.len() && !is_static[i] {
            total += duration(&runs[i]);
            i += 1;
        }
        let between_static = island_start > 0 && i < runs.len();
        if between_static && total < MENU_ISLAND_MS - TOLERANCE_MS {
            is_static[island_start..i].fill(true);
        }
    }
    is_static
}

/// Rolling FPS over the last second of non-static frames. Static frames do
/// not enter the window, so the value continues across a pause instead of
/// ramping up from zero. Undefined until half a window is available.
fn fill_fps(rows: &mut [Row], capture_rate: f64) {
    let window_len = (capture_rate.round_ties_even() as usize).max(1);
    let mut window = VecDeque::with_capacity(window_len);
    let mut new_images = 0u32;
    for row in rows.iter_mut().filter(|r| !r.is_static) {
        if window.len() == window_len {
            new_images -= window.pop_front().unwrap_or(0);
        }
        window.push_back(u32::from(row.new_image));
        new_images += u32::from(row.new_image);
        if window.len() * 2 >= window_len {
            row.fps = Some(f64::from(new_images) * capture_rate / window.len() as f64);
        }
    }
}

fn static_intervals(rows: &[Row], capture_rate: f64) -> Vec<StaticInterval> {
    let mut intervals = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        if !rows[i].is_static {
            i += 1;
            continue;
        }
        let start = i;
        while i < rows.len() && rows[i].is_static {
            i += 1;
        }
        intervals.push(StaticInterval {
            start_s: rows[start].time_s,
            duration_s: (i - start) as f64 / capture_rate,
        });
    }
    intervals
}

fn detect_stutters(rows: &mut [Row], factor: f64) -> Vec<Stutter> {
    let mut stutters = Vec::new();
    let mut history: VecDeque<f64> = VecDeque::with_capacity(STUTTER_HISTORY);
    for row in rows.iter_mut() {
        let Some(frametime) = row.frametime_ms else { continue };
        if history.len() >= STUTTER_MIN_HISTORY {
            let reference = median(&history);
            if frametime >= factor * reference - TOLERANCE_MS {
                row.stutter = true;
                stutters.push(Stutter {
                    frame: row.frame,
                    time_s: row.time_s,
                    frametime_ms: frametime,
                    reference_ms: reference,
                });
            }
        }
        if history.len() == STUTTER_HISTORY {
            history.pop_front();
        }
        history.push_back(frametime);
    }
    stutters
}

fn median(values: &VecDeque<f64>) -> f64 {
    let mut sorted: Vec<f64> = values.iter().copied().collect();
    sorted.sort_by(f64::total_cmp);
    sorted[sorted.len() / 2]
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 60.0;

    /// Rows for a 60 Hz capture where image `k` lasts `holds[k]` frames.
    fn rows_from_holds(holds: &[usize]) -> Vec<Row> {
        let mut rows = Vec::new();
        for &hold in holds {
            for i in 0..hold {
                let frame = rows.len() as u64;
                rows.push(Row {
                    frame,
                    time_s: frame as f64 / RATE,
                    changed_pixels: if i == 0 { 1000 } else { 0 },
                    new_image: i == 0,
                    ..Row::default()
                });
            }
        }
        rows
    }

    fn steady(images: usize, hold: usize) -> Vec<usize> {
        vec![hold; images]
    }

    fn concat(parts: &[Vec<usize>]) -> Vec<usize> {
        parts.concat()
    }

    #[test]
    fn steady_gameplay_has_no_static_or_stutter() {
        let mut rows = rows_from_holds(&steady(300, 2));
        let timeline = build(&mut rows, RATE, DEFAULT_STATIC_MS, DEFAULT_STUTTER_FACTOR);
        assert!(timeline.static_intervals.is_empty());
        assert!(timeline.stutters.is_empty());
        assert!(rows.iter().all(|r| !r.is_static));
        let fps = rows.last().unwrap().fps.unwrap();
        assert!((fps - 30.0).abs() < 1e-9);
    }

    #[test]
    fn short_hitch_is_a_stutter_not_static() {
        // 150 ms hitch in 30 fps gameplay.
        let mut rows = rows_from_holds(&concat(&[steady(60, 2), vec![9], steady(60, 2)]));
        let timeline = build(&mut rows, RATE, DEFAULT_STATIC_MS, DEFAULT_STUTTER_FACTOR);
        assert!(timeline.static_intervals.is_empty());
        assert_eq!(timeline.stutters.len(), 1);
        assert!((timeline.stutters[0].frametime_ms - 150.0).abs() < 1e-6);
        assert!((timeline.stutters[0].reference_ms - 100.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn single_late_frame_is_not_a_stutter_by_default() {
        // 30 fps on a 30 Hz-equivalent cadence: one repeated frame doubles the frametime.
        let mut rows = rows_from_holds(&concat(&[steady(30, 2), vec![4], steady(30, 2)]));
        assert!(build(&mut rows, RATE, DEFAULT_STATIC_MS, DEFAULT_STUTTER_FACTOR).stutters.is_empty());
    }

    #[test]
    fn two_late_frames_are_a_stutter() {
        let mut rows = rows_from_holds(&concat(&[steady(30, 2), vec![6], steady(30, 2)]));
        let timeline = build(&mut rows, RATE, DEFAULT_STATIC_MS, DEFAULT_STUTTER_FACTOR);
        assert_eq!(timeline.stutters.len(), 1);
        assert!((timeline.stutters[0].frametime_ms - 100.0).abs() < 1e-6);
    }

    #[test]
    fn lower_factor_detects_single_late_frames() {
        let mut rows = rows_from_holds(&concat(&[steady(30, 2), vec![3], steady(30, 2)]));
        let timeline = build(&mut rows, RATE, DEFAULT_STATIC_MS, 1.5);
        assert_eq!(timeline.stutters.len(), 1);
        assert!((timeline.stutters[0].frametime_ms - 50.0).abs() < 1e-6);
    }

    #[test]
    fn long_pause_is_static_and_excluded() {
        // 2 s pause between two gameplay spans.
        let mut rows = rows_from_holds(&concat(&[steady(60, 2), vec![120], steady(60, 2)]));
        let timeline = build(&mut rows, RATE, DEFAULT_STATIC_MS, DEFAULT_STUTTER_FACTOR);
        assert_eq!(timeline.static_intervals.len(), 1);
        assert!((timeline.static_intervals[0].duration_s - 119.0 / RATE).abs() < 1e-9);
        assert!(timeline.stutters.is_empty());
        assert!(rows.iter().filter_map(|r| r.frametime_ms).all(|ft| ft < 40.0));
        assert!(rows.iter().filter(|r| r.is_static).all(|r| r.fps.is_none()));
    }

    #[test]
    fn fps_continues_across_a_pause() {
        let mut rows = rows_from_holds(&concat(&[steady(60, 2), vec![120], steady(60, 2)]));
        build(&mut rows, RATE, DEFAULT_STATIC_MS, DEFAULT_STUTTER_FACTOR);
        let resumed = rows.iter().skip_while(|r| !r.is_static).find(|r| !r.is_static).unwrap();
        assert!((resumed.fps.unwrap() - 30.0).abs() < 1.5);
    }

    #[test]
    fn blinking_menu_is_static() {
        // Cursor blinking every 250 ms for 2 s.
        let mut rows = rows_from_holds(&concat(&[steady(60, 2), steady(8, 15), steady(60, 2)]));
        let timeline = build(&mut rows, RATE, DEFAULT_STATIC_MS, DEFAULT_STUTTER_FACTOR);
        assert_eq!(timeline.static_intervals.len(), 1);
        assert!(timeline.stutters.is_empty());
    }

    #[test]
    fn short_chain_of_hitches_stays_stutters() {
        // Three 250 ms hitches in a row (750 ms) are below the idle chain length.
        let mut rows = rows_from_holds(&concat(&[steady(60, 2), steady(3, 15), steady(60, 2)]));
        let timeline = build(&mut rows, RATE, DEFAULT_STATIC_MS, DEFAULT_STUTTER_FACTOR);
        assert!(timeline.static_intervals.is_empty());
        assert!(!timeline.stutters.is_empty());
    }

    #[test]
    fn video_ending_on_a_menu_is_static() {
        let mut rows = rows_from_holds(&concat(&[steady(60, 2), vec![90]]));
        let timeline = build(&mut rows, RATE, DEFAULT_STATIC_MS, DEFAULT_STUTTER_FACTOR);
        assert_eq!(timeline.static_intervals.len(), 1);
    }

    #[test]
    fn video_starting_on_a_menu_is_static() {
        let mut rows = rows_from_holds(&concat(&[vec![120], steady(60, 2)]));
        let timeline = build(&mut rows, RATE, DEFAULT_STATIC_MS, DEFAULT_STUTTER_FACTOR);
        assert_eq!(timeline.static_intervals.len(), 1);
        assert!(timeline.stutters.is_empty());
    }

    #[test]
    fn menu_transition_between_two_menus_is_static() {
        // Menu, 0.8 s animated transition with a 300 ms pause inside, menu.
        let transition = concat(&[steady(4, 2), vec![18], steady(8, 2)]);
        let mut rows =
            rows_from_holds(&concat(&[steady(60, 2), vec![120], transition, vec![120], steady(60, 2)]));
        let timeline = build(&mut rows, RATE, DEFAULT_STATIC_MS, DEFAULT_STUTTER_FACTOR);
        assert_eq!(timeline.static_intervals.len(), 1);
        assert!(timeline.stutters.is_empty());
    }

    #[test]
    fn gameplay_after_a_menu_is_not_absorbed() {
        // Menu, then a hitch right after resuming: still a stutter.
        let mut rows =
            rows_from_holds(&concat(&[steady(60, 2), vec![120], steady(10, 2), vec![9], steady(60, 2)]));
        let timeline = build(&mut rows, RATE, DEFAULT_STATIC_MS, DEFAULT_STUTTER_FACTOR);
        assert_eq!(timeline.static_intervals.len(), 1);
        assert_eq!(timeline.stutters.len(), 1);
    }

    #[test]
    fn long_gameplay_between_menus_is_kept() {
        let mut rows = rows_from_holds(&concat(&[vec![120], steady(90, 2), vec![120]]));
        let timeline = build(&mut rows, RATE, DEFAULT_STATIC_MS, DEFAULT_STUTTER_FACTOR);
        assert_eq!(timeline.static_intervals.len(), 2);
    }

    #[test]
    fn zero_disables_static_detection() {
        let mut rows = rows_from_holds(&concat(&[steady(60, 2), vec![120], steady(60, 2)]));
        let timeline = build(&mut rows, RATE, 0.0, DEFAULT_STUTTER_FACTOR);
        assert!(timeline.static_intervals.is_empty());
        assert_eq!(timeline.stutters.len(), 1);
    }
}
