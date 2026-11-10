//! FPS counter, running statistics and FPS/frametime graphs drawn over the
//! video frames.
//!
//! Each element can be turned off with [`OverlayElements`]; the remaining
//! ones close up. Inside static intervals the counter shows a dash and the
//! graphs are interrupted. A detected stutter is not counted on screen
//! (false positives are common in recordings); its frametime is shown for a
//! moment with a "potential stutter" label instead. Layout and sizes are
//! defined for 1080p and scaled with the frame height.

use std::collections::VecDeque;

use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform,
};
use ttf_parser::{Face, OutlineBuilder};

use crate::analysis::Row;
use crate::i18n::{Lang, Strings};
use crate::stats::Stats;
use crate::yuv::YuvLayout;

type Rgb = [u8; 3];
const LINE_COLOR: Rgb = [255, 220, 0];
const TEXT_COLOR: Rgb = [255, 255, 255];
const GRID_COLOR: Rgb = [90, 90, 90];
const BAD_COLOR: Rgb = [255, 60, 60];
/// Opacity of the dark panels behind text and graphs.
const PANEL_ALPHA: u8 = 140;
/// Seconds of history shown in the graphs.
const GRAPH_SECONDS: f64 = 5.0;
/// Font size, in pixels at 1080p, for a text scale of 1.
const FONT_PX_PER_SCALE: f32 = 30.0;
/// Width, in pixels at 1080p, of the dark outline around text.
const TEXT_OUTLINE: f32 = 3.0;
/// How long the last stutter stays highlighted, in seconds.
const STUTTER_HIGHLIGHT_S: f64 = 1.5;
/// Shown instead of a value that is not available.
const NO_VALUE: &str = "—";
/// Top-left corner of the top panels, in pixels at 1080p.
const TOP_MARGIN: f32 = 30.0;
/// Left edge of text inside the top panels, in pixels at 1080p.
const TEXT_LEFT: f32 = 45.0;
/// Gap between the main top panel and the stutter alert, in pixels at 1080p.
const PANEL_GAP: f32 = 10.0;
/// Height of each graph, in pixels at 1080p.
const GRAPH_HEIGHT: f32 = 150.0;
/// Distance from the bottom of the frame to the top of the lower graph, and
/// between the tops of the two graphs, in pixels at 1080p.
const GRAPH_BOTTOM: f32 = 190.0;
const GRAPH_SPACING: f32 = 210.0;

/// Overlay elements to draw. All are on by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayElements {
    /// Large rolling FPS counter.
    pub fps_counter: bool,
    /// Running average FPS and 1% low.
    pub stats: bool,
    /// Frametime of the last stutter, labeled as a potential stutter.
    pub stutter_alert: bool,
    pub fps_graph: bool,
    pub frametime_graph: bool,
}

impl Default for OverlayElements {
    fn default() -> Self {
        Self { fps_counter: true, stats: true, stutter_alert: true, fps_graph: true, frametime_graph: true }
    }
}

impl OverlayElements {
    /// Whether at least one element is drawn.
    pub fn any(&self) -> bool {
        self.fps_counter || self.stats || self.stutter_alert || self.fps_graph || self.frametime_graph
    }
}

pub struct Overlay {
    strings: &'static Strings,
    elements: OverlayElements,
    width: i32,
    height: i32,
    /// Frame height relative to 1080p.
    scale: f32,
    capture_rate: f64,
    target_fps: f64,
    /// Number of samples visible in each graph.
    points: usize,
    /// `None` marks static frames, drawn as gaps.
    fps_history: VecDeque<Option<f64>>,
    frametime_history: VecDeque<Option<f64>>,
    current_frametime: f64,
    /// Last stutter and how many more frames it stays highlighted.
    stutter_highlight: Option<(f64, u32)>,
    font: Face<'static>,
    /// Counter, statistics and stutter alert.
    top: Option<Layer>,
    /// Graphs with their labels.
    bottom: Option<Layer>,
    /// Layout of the frames the overlay is blended into.
    frame_layout: YuvLayout,
}

impl Overlay {
    pub fn new(
        frame_layout: YuvLayout,
        capture_rate: f64,
        target_fps: f64,
        lang: Lang,
        elements: OverlayElements,
    ) -> Self {
        let (width, height) = (frame_layout.width as u32, frame_layout.height as u32);
        let scale = height as f32 / 1080.0;
        let points = ((capture_rate * GRAPH_SECONDS).round_ties_even() as usize).max(2);
        let (w, h) = (width as i32, height as i32);
        let px = |v: f32| (v * scale) as i32;
        let graph_right = px(40.0) + (width as f64 * 0.45) as i32;
        let any_top = elements.fps_counter || elements.stats || elements.stutter_alert;
        let top = any_top.then(|| Layer::new(w, h, scale, (0, 0, px(560.0), px(250.0)))).flatten();
        let graphs = usize::from(elements.fps_graph) + usize::from(elements.frametime_graph);
        let bottom = (graphs > 0)
            .then(|| {
                let top_y = h - px(GRAPH_BOTTOM + (graphs - 1) as f32 * GRAPH_SPACING + 40.0);
                Layer::new(w, h, scale, (0, top_y, graph_right + px(50.0), h - px(20.0)))
            })
            .flatten();

        Self {
            strings: lang.strings(),
            elements,
            width: w,
            height: h,
            scale,
            capture_rate,
            target_fps,
            points,
            fps_history: VecDeque::with_capacity(points),
            frametime_history: VecDeque::with_capacity(points),
            current_frametime: 1000.0 / capture_rate,
            stutter_highlight: None,
            font: Face::parse(epaint_default_fonts::HACK_REGULAR, 0).expect("bundled font is valid"),
            top,
            bottom,
            frame_layout,
        }
    }

    /// Adds one captured frame to the graph history.
    pub fn record(&mut self, row: &Row) {
        if let Some((_, frames_left)) = &mut self.stutter_highlight {
            *frames_left = frames_left.saturating_sub(1);
        }
        if row.is_static {
            push_bounded(&mut self.fps_history, None, self.points);
            push_bounded(&mut self.frametime_history, None, self.points);
            return;
        }
        if let Some(frametime) = row.frametime_ms {
            self.current_frametime = frametime;
            if row.stutter {
                let frames = (self.capture_rate * STUTTER_HIGHLIGHT_S).round() as u32;
                self.stutter_highlight = Some((frametime, frames));
            }
        }
        push_bounded(&mut self.fps_history, row.fps, self.points);
        push_bounded(&mut self.frametime_history, Some(self.current_frametime), self.points);
    }

    /// Draws the overlay for `row` onto a planar YUV frame in place. `stats` holds
    /// the statistics up to and including `row`.
    pub fn draw(&mut self, frame: &mut [u8], row: &Row, stats: &Stats) {
        let s = self.scale;
        let px = |v: f32| (v * s) as i32;
        let target = self.target_fps;
        let strings = self.strings;
        let elements = self.elements;

        if let Some(top) = &mut self.top {
            top.pixmap.fill(Color::TRANSPARENT);
            // Baselines and panel bottom, in pixels at 1080p.
            let counter_y = TOP_MARGIN + 65.0;
            let stats_y = if elements.fps_counter { counter_y + 45.0 } else { TOP_MARGIN + 32.0 };
            let main_bottom = match (elements.fps_counter, elements.stats) {
                (_, true) => Some(stats_y + 22.0),
                (true, false) => Some(counter_y + 25.0),
                (false, false) => None,
            };

            if let Some(bottom) = main_bottom {
                let width = if elements.stats { 400.0 } else { 260.0 };
                top.panel(px(TOP_MARGIN), px(TOP_MARGIN), px(width), px(bottom - TOP_MARGIN));
            }
            if elements.fps_counter {
                let (counter, color) = match row.fps.filter(|_| !row.is_static) {
                    Some(fps) => {
                        (format!("FPS: {fps:.0}"), if fps < target - 0.5 { BAD_COLOR } else { TEXT_COLOR })
                    }
                    None => (format!("FPS: {NO_VALUE}"), TEXT_COLOR),
                };
                top.text(&self.font, &counter, px(TEXT_LEFT), px(counter_y), 1.6 * s, color);
            }
            if elements.stats {
                let value = |v: Option<f64>| v.map_or_else(|| NO_VALUE.to_owned(), |v| format!("{v:.1}"));
                let averages = format!(
                    "{} {}   1% low {}",
                    strings.overlay_average,
                    value(stats.average_fps(self.capture_rate)),
                    value(stats.one_percent_low())
                );
                top.text(&self.font, &averages, px(TEXT_LEFT), px(stats_y), 0.6 * s, TEXT_COLOR);
            }
            if elements.stutter_alert
                && let Some((frametime, frames_left)) = self.stutter_highlight
                && frames_left > 0
            {
                let alert = format!("+{frametime:.1} ms  {}", strings.overlay_potential_stutter);
                let panel_top = main_bottom.map_or(TOP_MARGIN, |bottom| bottom + PANEL_GAP);
                let padding = TEXT_LEFT - TOP_MARGIN;
                let width = text_width(&self.font, &alert, 0.6 * s) as i32 + 2 * px(padding);
                top.panel(px(TOP_MARGIN), px(panel_top), width, px(54.0));
                top.text(&self.font, &alert, px(TEXT_LEFT), px(panel_top + 32.0), 0.6 * s, BAD_COLOR);
            }
            blend(&self.frame_layout, frame, top);
        }

        if let Some(bottom) = &mut self.bottom {
            bottom.pixmap.fill(Color::TRANSPARENT);
            let (w, h, x) = ((self.width as f64 * 0.45) as i32, px(GRAPH_HEIGHT), px(40.0));
            // The lower graph sits at the bottom; the other one, if any, above it.
            let graph_y =
                |above: bool| self.height - px(GRAPH_BOTTOM + if above { GRAPH_SPACING } else { 0.0 });
            let fps_ceiling = if target <= 60.0 { 60.0 } else { 120.0 };
            let fps_marks = [0.0, fps_ceiling / 4.0, fps_ceiling / 2.0, fps_ceiling * 3.0 / 4.0, fps_ceiling];
            let mut graph = Graph { layer: bottom, font: &self.font, scale: s, points: self.points };
            if elements.fps_graph {
                graph.draw(
                    &self.fps_history,
                    (x, graph_y(elements.frametime_graph), w, h),
                    (0.0, fps_ceiling),
                    &fps_marks,
                    strings.overlay_fps,
                    |v| v < target - 0.5,
                );
            }
            if elements.frametime_graph {
                graph.draw(
                    &self.frametime_history,
                    (x, graph_y(false), w, h),
                    (0.0, 100.0),
                    &[16.7, 33.3, 50.0, 100.0],
                    strings.overlay_frametime,
                    |v| v > 1000.0 / target + 1.0,
                );
            }
            blend(&self.frame_layout, frame, bottom);
        }
    }
}

/// Blends a drawn layer into a planar YUV frame.
fn blend(frame_layout: &YuvLayout, frame: &mut [u8], layer: &Layer) {
    let (w, h) = (layer.pixmap.width() as usize, layer.pixmap.height() as usize);
    frame_layout.blend(frame, layer.x as usize, layer.y as usize, w, h, layer.pixmap.data());
}

fn push_bounded(history: &mut VecDeque<Option<f64>>, value: Option<f64>, capacity: usize) {
    if history.len() == capacity {
        history.pop_front();
    }
    history.push_back(value);
}

struct Graph<'a> {
    layer: &'a mut Layer,
    font: &'a Face<'static>,
    scale: f32,
    points: usize,
}

impl Graph<'_> {
    /// Draws a line graph. `area` is `(x, y, width, height)`; `is_bad`
    /// selects the red color for a segment from its end value. `None`
    /// values interrupt the line.
    fn draw(
        &mut self,
        values: &VecDeque<Option<f64>>,
        area: (i32, i32, i32, i32),
        (min, max): (f64, f64),
        marks: &[f64],
        label: &str,
        is_bad: impl Fn(f64) -> bool,
    ) {
        let (x, y, w, h) = area;
        let s = self.scale;
        let px = |v: f32| (v * s) as i32;
        let to_y = |v: f64| (f64::from(y) + f64::from(h) - (v - min) / (max - min) * f64::from(h)) as i32;

        self.layer.panel(x, y, w, h);
        for &mark in marks {
            let mark_y = to_y(mark);
            self.layer.grid_line(x, x + w, mark_y);
            self.layer.text(
                self.font,
                &format_number(mark),
                x + w + px(6.0),
                mark_y + px(6.0),
                0.5 * s,
                TEXT_COLOR,
            );
        }
        self.layer.text(self.font, label, x, y - px(10.0), 0.6 * s, TEXT_COLOR);
        if values.len() < 2 {
            return;
        }

        let step = f64::from(w) / (self.points - 1) as f64;
        let width = (2.0 * s).floor().max(1.0);
        let mut i = 0;
        while i < values.len() {
            // Contiguous run of defined values.
            if values[i].is_none() {
                i += 1;
                continue;
            }
            let run_start = i;
            while i < values.len() && values[i].is_some() {
                i += 1;
            }
            let run: Vec<f64> = values.range(run_start..i).flatten().copied().collect();
            let points: Vec<(f32, f32)> = run
                .iter()
                .enumerate()
                .map(|(k, &v)| {
                    let px_x = (f64::from(x) + (run_start + k) as f64 * step) as i32 as f32;
                    (px_x, to_y(v.clamp(min, max)) as f32)
                })
                .collect();
            // Consecutive segments of the same color are stroked as one path.
            let mut start = 0;
            while start + 1 < points.len() {
                let bad = is_bad(run[start + 1]);
                let mut end = start + 1;
                while end + 1 < points.len() && is_bad(run[end + 1]) == bad {
                    end += 1;
                }
                let color = if bad { BAD_COLOR } else { LINE_COLOR };
                self.layer.polyline(&points[start..=end], color, width);
                start = end;
            }
        }
    }
}

/// Formats a tick value without trailing zeros (`15`, `16.7`).
fn format_number(value: f64) -> String {
    if value.fract() == 0.0 { format!("{}", value as i64) } else { format!("{value}") }
}

/// Transparent drawing surface covering part of the frame.
struct Layer {
    x: i32,
    y: i32,
    /// Width of the dark outline around text, in pixels.
    outline: f32,
    pixmap: Pixmap,
}

impl Layer {
    fn new(frame_w: i32, frame_h: i32, scale: f32, bounds: (i32, i32, i32, i32)) -> Option<Self> {
        let (x0, y0, x1, y1) = bounds;
        let (x0, y0) = (x0.clamp(0, frame_w), y0.clamp(0, frame_h));
        let (x1, y1) = (x1.clamp(x0, frame_w), y1.clamp(y0, frame_h));
        let pixmap = Pixmap::new((x1 - x0) as u32, (y1 - y0) as u32)?;
        Some(Self { x: x0, y: y0, outline: (TEXT_OUTLINE * scale).max(1.0), pixmap })
    }

    fn transform(&self) -> Transform {
        Transform::from_translate(-self.x as f32, -self.y as f32)
    }

    fn panel(&mut self, x: i32, y: i32, w: i32, h: i32) {
        if let Some(rect) = Rect::from_xywh(x as f32, y as f32, w as f32, h as f32) {
            let paint = paint([0, 0, 0], PANEL_ALPHA, false);
            let transform = self.transform();
            self.pixmap.fill_rect(rect, &paint, transform, None);
        }
    }

    fn grid_line(&mut self, x0: i32, x1: i32, y: i32) {
        let mut builder = PathBuilder::new();
        builder.move_to(x0 as f32, y as f32 + 0.5);
        builder.line_to(x1 as f32, y as f32 + 0.5);
        if let Some(path) = builder.finish() {
            let stroke = Stroke { width: 1.0, ..Stroke::default() };
            let transform = self.transform();
            self.pixmap.stroke_path(&path, &paint(GRID_COLOR, 255, false), &stroke, transform, None);
        }
    }

    fn polyline(&mut self, points: &[(f32, f32)], color: Rgb, width: f32) {
        let mut builder = PathBuilder::new();
        let mut iter = points.iter();
        if let Some(&(x, y)) = iter.next() {
            builder.move_to(x, y);
        }
        for &(x, y) in iter {
            builder.line_to(x, y);
        }
        if let Some(path) = builder.finish() {
            let stroke =
                Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
            let transform = self.transform();
            self.pixmap.stroke_path(&path, &paint(color, 255, true), &stroke, transform, None);
        }
    }

    /// Draws outlined text with its baseline starting at `(x, y)`. `size` is
    /// the text scale already multiplied by the frame scale.
    fn text(&mut self, font: &Face, text: &str, x: i32, y: i32, size: f32, color: Rgb) {
        let Some(path) = text_path(font, text, x as f32, y as f32, FONT_PX_PER_SCALE * size) else {
            return;
        };
        let transform = self.transform();
        let stroke = Stroke { width: self.outline, line_join: LineJoin::Round, ..Stroke::default() };
        self.pixmap.stroke_path(&path, &paint([0, 0, 0], 255, true), &stroke, transform, None);
        self.pixmap.fill_path(&path, &paint(color, 255, true), FillRule::Winding, transform, None);
    }
}

fn paint(color: Rgb, alpha: u8, anti_alias: bool) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], alpha);
    paint.anti_alias = anti_alias;
    paint
}

/// Horizontal advance of `text` at a text scale of `size`, in pixels.
fn text_width(font: &Face, text: &str, size: f32) -> f32 {
    let scale = FONT_PX_PER_SCALE * size / f32::from(font.units_per_em());
    text.chars()
        .filter_map(|ch| font.glyph_index(ch))
        .map(|glyph| f32::from(font.glyph_hor_advance(glyph).unwrap_or(0)) * scale)
        .sum()
}

/// Builds the outline of `text` at `size_px`, with the baseline at `(x, y)`.
fn text_path(font: &Face, text: &str, x: f32, y: f32, size_px: f32) -> Option<Path> {
    let scale = size_px / f32::from(font.units_per_em());
    let mut builder = PathBuilder::new();
    let mut pen = x;
    for ch in text.chars() {
        let Some(glyph) = font.glyph_index(ch) else { continue };
        let mut outline = GlyphOutline { builder: &mut builder, x: pen, y, scale };
        font.outline_glyph(glyph, &mut outline);
        pen += f32::from(font.glyph_hor_advance(glyph).unwrap_or(0)) * scale;
    }
    builder.finish()
}

struct GlyphOutline<'a> {
    builder: &'a mut PathBuilder,
    x: f32,
    y: f32,
    scale: f32,
}

impl GlyphOutline<'_> {
    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        (self.x + x * self.scale, self.y - y * self.scale)
    }
}

impl OutlineBuilder for GlyphOutline<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.point(x, y);
        self.builder.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.point(x, y);
        self.builder.line_to(x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (x1, y1) = self.point(x1, y1);
        let (x, y) = self.point(x, y);
        self.builder.quad_to(x1, y1, x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (x1, y1) = self.point(x1, y1);
        let (x2, y2) = self.point(x2, y2);
        let (x, y) = self.point(x, y);
        self.builder.cubic_to(x1, y1, x2, y2, x, y);
    }

    fn close(&mut self) {
        self.builder.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_tick_values_without_trailing_zeros() {
        assert_eq!(format_number(0.0), "0");
        assert_eq!(format_number(15.0), "15");
        assert_eq!(format_number(16.7), "16.7");
        assert_eq!(format_number(33.3), "33.3");
    }

    #[test]
    fn draws_without_panicking_on_small_and_large_frames() {
        for (w, h) in [(320u32, 180u32), (1920, 1080), (64, 36)] {
            let layout = YuvLayout::new("yuv420p", w, h, &crate::probe::ColorTags::default()).unwrap();
            let mut frame = vec![128u8; layout.frame_len()];
            let elements = [
                OverlayElements::default(),
                OverlayElements { fps_counter: false, fps_graph: false, ..OverlayElements::default() },
                OverlayElements { stats: false, frametime_graph: false, ..OverlayElements::default() },
                OverlayElements { fps_counter: false, stats: false, ..OverlayElements::default() },
            ];
            let mut overlays: Vec<Overlay> =
                elements.into_iter().map(|e| Overlay::new(layout.clone(), 60.0, 30.0, Lang::En, e)).collect();
            let mut stats = Stats::new();
            for i in 0..90 {
                let row = Row {
                    new_image: i % 2 == 0,
                    is_static: (40..60).contains(&i),
                    fps: Some(if i % 7 == 0 { 20.0 } else { 30.0 }),
                    frametime_ms: (i % 2 == 0).then_some(if i == 70 { 66.7 } else { 33.3 }),
                    stutter: i == 70,
                    ..Row::default()
                };
                stats.push(&row);
                for overlay in &mut overlays {
                    overlay.record(&row);
                    overlay.draw(&mut frame, &row, &stats);
                }
            }
        }
    }
}
