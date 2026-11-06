//! Cumulative statistics over the non-static frames, updated one frame at a
//! time. Used for the final summary and for the running values shown in the
//! overlay, so both always agree.

use std::collections::BTreeMap;

use crate::analysis::Row;

#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// Non-static frames seen.
    frames: u64,
    /// New images among the non-static frames.
    new_images: u64,
    /// Frametimes in hundredths of a millisecond, with their counts.
    frametimes: BTreeMap<u64, u64>,
    frametime_count: u64,
    stutters: u64,
}

impl Stats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_rows<'a>(rows: impl IntoIterator<Item = &'a Row>) -> Self {
        let mut stats = Self::new();
        for row in rows {
            stats.push(row);
        }
        stats
    }

    pub fn push(&mut self, row: &Row) {
        if row.is_static {
            return;
        }
        self.frames += 1;
        self.new_images += u64::from(row.new_image);
        if let Some(frametime) = row.frametime_ms {
            *self.frametimes.entry(to_key(frametime)).or_default() += 1;
            self.frametime_count += 1;
        }
        self.stutters += u64::from(row.stutter);
    }

    /// Time covered by the non-static frames, in seconds.
    pub fn active_duration_s(&self, capture_rate: f64) -> f64 {
        self.frames as f64 / capture_rate
    }

    /// New images per second over the non-static frames.
    pub fn average_fps(&self, capture_rate: f64) -> Option<f64> {
        (self.frames > 0).then(|| self.new_images as f64 * capture_rate / self.frames as f64)
    }

    /// Median frametime (upper median), in milliseconds.
    pub fn typical_frametime(&self) -> Option<f64> {
        self.nth_frametime(self.frametime_count / 2)
    }

    pub fn worst_frametime(&self) -> Option<f64> {
        self.frametimes.last_key_value().map(|(&key, _)| from_key(key))
    }

    /// 1000 divided by the mean of the slowest 1% of frametimes.
    pub fn one_percent_low(&self) -> Option<f64> {
        let n = self.frametime_count;
        if n == 0 {
            return None;
        }
        let slowest = n - (n as f64 * 0.99) as u64;
        let mut remaining = slowest;
        let mut sum = 0.0;
        for (&key, &count) in self.frametimes.iter().rev() {
            let taken = count.min(remaining);
            sum += from_key(key) * taken as f64;
            remaining -= taken;
            if remaining == 0 {
                break;
            }
        }
        Some(1000.0 / (sum / slowest as f64))
    }

    pub fn stutters(&self) -> u64 {
        self.stutters
    }

    /// Stutters per minute of non-static time.
    pub fn stutters_per_minute(&self, capture_rate: f64) -> Option<f64> {
        let minutes = self.active_duration_s(capture_rate) / 60.0;
        (minutes > 0.0).then(|| self.stutters as f64 / minutes)
    }

    fn nth_frametime(&self, index: u64) -> Option<f64> {
        let mut seen = 0;
        for (&key, &count) in &self.frametimes {
            seen += count;
            if seen > index {
                return Some(from_key(key));
            }
        }
        None
    }
}

/// Frametimes are compared at 0.01 ms resolution, as written to the CSV.
fn to_key(frametime_ms: f64) -> u64 {
    (frametime_ms * 100.0).round() as u64
}

fn from_key(key: u64) -> f64 {
    key as f64 / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(frametime: Option<f64>, is_static: bool) -> Row {
        Row { new_image: frametime.is_some(), frametime_ms: frametime, is_static, ..Row::default() }
    }

    #[test]
    fn one_percent_low_matches_sorted_definition() {
        let frametimes: Vec<f64> = (0..523).map(|i| 16.67 + f64::from(i % 37) * 1.11).collect();
        let stats = Stats::from_rows(&frametimes.iter().map(|&ft| row(Some(ft), false)).collect::<Vec<_>>());

        let mut sorted: Vec<f64> = frametimes.iter().map(|&ft| (ft * 100.0).round() / 100.0).collect();
        sorted.sort_by(f64::total_cmp);
        let slowest = &sorted[(sorted.len() as f64 * 0.99) as usize..];
        let expected = 1000.0 / (slowest.iter().sum::<f64>() / slowest.len() as f64);

        assert!((stats.one_percent_low().unwrap() - expected).abs() < 1e-9);
        assert_eq!(stats.typical_frametime(), Some(sorted[sorted.len() / 2]));
        assert_eq!(stats.worst_frametime(), sorted.last().copied());
    }

    #[test]
    fn static_rows_are_ignored() {
        let rows = [row(Some(33.33), false), row(None, true), row(None, true), row(None, false)];
        let stats = Stats::from_rows(&rows);
        assert!((stats.active_duration_s(60.0) - 2.0 / 60.0).abs() < 1e-12);
        assert!((stats.average_fps(60.0).unwrap() - 30.0).abs() < 1e-9);
    }
}
