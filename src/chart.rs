//! PNG chart with the rolling FPS and the frametime of each image.

use std::error::Error;
use std::path::Path;
use std::sync::Once;

use plotters::prelude::*;
use plotters::style::{FontStyle, register_font};

use crate::analysis::Row;
use crate::i18n::Lang;

const SIZE: (u32, u32) = (1440, 720);
const SERIES_COLOR: RGBColor = RGBColor(31, 119, 180);

static REGISTER_FONT: Once = Once::new();

pub fn save_chart(rows: &[Row], path: &Path, lang: Lang) -> Result<(), String> {
    REGISTER_FONT.call_once(|| {
        let _ = register_font("sans-serif", FontStyle::Normal, epaint_default_fonts::HACK_REGULAR);
    });
    draw(rows, path, lang).map_err(|e| e.to_string())
}

fn draw(rows: &[Row], path: &Path, lang: Lang) -> Result<(), Box<dyn Error>> {
    let s = lang.strings();
    let root = BitMapBackend::new(path, SIZE).into_drawing_area();
    root.fill(&WHITE)?;
    let (upper, lower) = root.split_vertically(SIZE.1 / 2 - 20);

    let start = rows.first().map_or(0.0, |r| r.time_s);
    let end = rows.last().map_or(1.0, |r| r.time_s).max(start + 1e-3);
    let steps = frametime_steps(rows);
    let fps_top = padded_max(rows.iter().map(|r| r.fps));
    let frametime_top = padded_max(steps.iter().map(|&(_, v)| v));
    let grid = BLACK.mix(0.15);

    let mut fps_chart = ChartBuilder::on(&upper)
        .margin(12)
        .x_label_area_size(20)
        .y_label_area_size(60)
        .build_cartesian_2d(start..end, 0.0..fps_top)?;
    fps_chart
        .configure_mesh()
        .light_line_style(TRANSPARENT)
        .bold_line_style(grid)
        .y_desc(s.overlay_fps)
        .draw()?;
    fps_chart.draw_series(LineSeries::new(rows.iter().map(|r| (r.time_s, r.fps)), SERIES_COLOR))?;

    let mut frametime_chart = ChartBuilder::on(&lower)
        .margin(12)
        .x_label_area_size(45)
        .y_label_area_size(60)
        .build_cartesian_2d(start..end, 0.0..frametime_top)?;
    frametime_chart
        .configure_mesh()
        .light_line_style(TRANSPARENT)
        .bold_line_style(grid)
        .x_desc(s.chart_time)
        .y_desc(s.overlay_frametime)
        .draw()?;
    frametime_chart.draw_series(LineSeries::new(steps, SERIES_COLOR))?;

    root.present()?;
    Ok(())
}

/// Step line in which each frametime holds until the next one is recorded.
fn frametime_steps(rows: &[Row]) -> Vec<(f64, f64)> {
    let samples: Vec<(f64, f64)> =
        rows.iter().filter_map(|r| r.frametime_ms.map(|ft| (r.time_s, ft))).collect();
    let mut steps = Vec::with_capacity(samples.len() * 2);
    for (i, &(time, value)) in samples.iter().enumerate() {
        steps.push((time, value));
        if let Some(&(next_time, _)) = samples.get(i + 1) {
            steps.push((next_time, value));
        }
    }
    steps
}

fn padded_max(values: impl Iterator<Item = f64>) -> f64 {
    (values.fold(0.0, f64::max) * 1.1).max(1.0)
}
