//! Output files written next to the analyzed recording.

use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use crate::analysis::Row;
use crate::chart;
use crate::i18n::Lang;

/// Output file locations for a recording `<dir>/<name>.<ext>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputPaths {
    /// `<dir>/<name>_fps.csv`
    pub csv: PathBuf,
    /// `<dir>/<name>_chart.png`
    pub chart: PathBuf,
    /// `<dir>/<name>_overlay.mp4`
    pub overlay: PathBuf,
}

impl OutputPaths {
    pub fn for_video(video: &Path) -> Self {
        let stem = video.file_stem().unwrap_or_default().to_string_lossy();
        let dir = video.parent().unwrap_or_else(|| Path::new(""));
        Self {
            csv: dir.join(format!("{stem}_fps.csv")),
            chart: dir.join(format!("{stem}_chart.png")),
            overlay: dir.join(format!("{stem}_overlay.mp4")),
        }
    }
}

/// Writes one line per captured frame.
pub fn write_csv(rows: &[Row], path: &Path) -> io::Result<()> {
    let mut out = BufWriter::new(File::create(path)?);
    writeln!(out, "frame,time_s,changed_pixels,new_image,fps,frametime_ms")?;
    for row in rows {
        write!(
            out,
            "{},{:.4},{},{},{:.2},",
            row.frame,
            row.time_s,
            row.changed_pixels,
            u8::from(row.new_image),
            row.fps
        )?;
        match row.frametime_ms {
            Some(frametime) => writeln!(out, "{frametime:.2}")?,
            None => writeln!(out)?,
        }
    }
    out.flush()
}

/// Saves the CSV and/or chart next to `video` and returns one message per
/// file describing the outcome.
pub fn save_outputs(rows: &[Row], video: &Path, csv: bool, chart: bool, lang: Lang) -> Vec<String> {
    if rows.is_empty() {
        return Vec::new();
    }
    let s = lang.strings();
    let paths = OutputPaths::for_video(video);
    let mut messages = Vec::new();
    let mut report = |name: &str, path: &Path, result: Result<(), String>| {
        messages.push(match result {
            Ok(()) => format!("{name}: {}", path.display()),
            Err(error) => format!("{name}: {}", s.save_failed.replace("{}", &error)),
        });
    };
    if csv {
        report("CSV", &paths.csv, write_csv(rows, &paths.csv).map_err(|e| e.to_string()));
    }
    if chart {
        report("PNG", &paths.chart, chart::save_chart(rows, &paths.chart, lang));
    }
    messages
}
