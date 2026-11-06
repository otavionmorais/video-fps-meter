//! FPS counter and FPS/frametime graphs drawn over the video frames.
//!
//! Layout and sizes are defined for 1080p and scaled with the frame height.

use std::collections::VecDeque;

use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform,
};
use ttf_parser::{Face, OutlineBuilder};

use crate::i18n::{Lang, Strings};

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

pub struct Overlay {
    strings: &'static Strings,
    width: i32,
    height: i32,
    /// Frame height relative to 1080p.
    scale: f32,
    target_fps: f64,
    /// Number of samples visible in each graph.
    points: usize,
    fps_history: VecDeque<f64>,
    frametime_history: VecDeque<f64>,
    current_frametime: f64,
    font: Face<'static>,
    layers: Vec<Layer>,
}

impl Overlay {
    pub fn new(width: u32, height: u32, capture_rate: f64, target_fps: f64, lang: Lang) -> Self {
        let scale = height as f32 / 1080.0;
        let points = ((capture_rate * GRAPH_SECONDS).round_ties_even() as usize).max(2);
        let (w, h) = (width as i32, height as i32);
        let px = |v: f32| (v * scale) as i32;
        let graph_right = px(40.0) + (width as f64 * 0.45) as i32;
        let layers = [
            // FPS counter.
            (0, 0, px(320.0), px(140.0)),
            // Both graphs with their labels.
            (0, h - px(440.0), graph_right + px(50.0), h - px(20.0)),
        ]
        .into_iter()
        .filter_map(|bounds| Layer::new(w, h, scale, bounds))
        .collect();

        Self {
            strings: lang.strings(),
            width: w,
            height: h,
            scale,
            target_fps,
            points,
            fps_history: VecDeque::with_capacity(points),
            frametime_history: VecDeque::with_capacity(points),
            current_frametime: 1000.0 / capture_rate,
            font: Face::parse(epaint_default_fonts::HACK_REGULAR, 0).expect("bundled font is valid"),
            layers,
        }
    }

    /// Adds one captured frame to the graph history. `frametime_ms` is set
    /// when the frame completed an image.
    pub fn record(&mut self, fps: f64, frametime_ms: Option<f64>) {
        if let Some(frametime) = frametime_ms {
            self.current_frametime = frametime;
        }
        push_bounded(&mut self.fps_history, fps, self.points);
        push_bounded(&mut self.frametime_history, self.current_frametime, self.points);
    }

    /// Draws the overlay onto a BGR24 frame in place.
    pub fn draw(&mut self, frame: &mut [u8], fps: f64) {
        for layer in &mut self.layers {
            layer.pixmap.fill(Color::TRANSPARENT);
        }
        let s = self.scale;
        let px = |v: f32| (v * s) as i32;
        let target = self.target_fps;

        if let Some(top) = self.layers.first_mut() {
            let color = if fps < target - 0.5 { BAD_COLOR } else { TEXT_COLOR };
            top.panel(px(30.0), px(30.0), px(260.0), px(90.0));
            top.text(&self.font, &format!("FPS: {fps:.0}"), px(45.0), px(95.0), 1.6 * s, color);
        }

        if let Some(bottom) = self.layers.get_mut(1) {
            let (w, h, x) = ((self.width as f64 * 0.45) as i32, px(150.0), px(40.0));
            let fps_ceiling = if target <= 60.0 { 60.0 } else { 120.0 };
            let fps_marks = [0.0, fps_ceiling / 4.0, fps_ceiling / 2.0, fps_ceiling * 3.0 / 4.0, fps_ceiling];
            let mut graph = Graph { layer: bottom, font: &self.font, scale: s, points: self.points };
            graph.draw(
                &self.fps_history,
                (x, self.height - px(400.0), w, h),
                (0.0, fps_ceiling),
                &fps_marks,
                self.strings.overlay_fps,
                |v| v < target - 0.5,
            );
            graph.draw(
                &self.frametime_history,
                (x, self.height - px(190.0), w, h),
                (0.0, 100.0),
                &[16.7, 33.3, 50.0, 100.0],
                self.strings.overlay_frametime,
                |v| v > 1000.0 / target + 1.0,
            );
        }

        for layer in &self.layers {
            layer.composite(frame, self.width as usize);
        }
    }
}

fn push_bounded(history: &mut VecDeque<f64>, value: f64, capacity: usize) {
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
    /// selects the red color for a segment from its end value.
    fn draw(
        &mut self,
        values: &VecDeque<f64>,
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
        let points: Vec<(f32, f32)> = values
            .iter()
            .enumerate()
            .map(|(i, &v)| ((f64::from(x) + i as f64 * step) as i32 as f32, to_y(v.clamp(min, max)) as f32))
            .collect();
        let width = (2.0 * s).floor().max(1.0);
        // Consecutive segments of the same color are stroked as one path.
        let mut start = 0;
        while start + 1 < points.len() {
            let bad = is_bad(values[start + 1]);
            let mut end = start + 1;
            while end + 1 < points.len() && is_bad(values[end + 1]) == bad {
                end += 1;
            }
            let color = if bad { BAD_COLOR } else { LINE_COLOR };
            self.layer.polyline(&points[start..=end], color, width);
            start = end;
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

    /// Blends the layer over a BGR24 frame.
    fn composite(&self, frame: &mut [u8], frame_w: usize) {
        let width = self.pixmap.width() as usize;
        let data = self.pixmap.data();
        for (row, src) in data.chunks_exact(width * 4).enumerate() {
            let start = ((self.y as usize + row) * frame_w + self.x as usize) * 3;
            let dst = &mut frame[start..start + width * 3];
            for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(3)) {
                let alpha = u32::from(s[3]);
                if alpha == 0 {
                    continue;
                }
                let inverse = 255 - alpha;
                // Source is premultiplied RGBA; destination is BGR.
                for (channel, premultiplied) in d.iter_mut().zip([s[2], s[1], s[0]]) {
                    let blended = u32::from(premultiplied) + (u32::from(*channel) * inverse + 127) / 255;
                    *channel = blended.min(255) as u8;
                }
            }
        }
    }
}

fn paint(color: Rgb, alpha: u8, anti_alias: bool) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color[0], color[1], color[2], alpha);
    paint.anti_alias = anti_alias;
    paint
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
            let mut overlay = Overlay::new(w, h, 60.0, 30.0, Lang::En);
            let mut frame = vec![128u8; (w * h * 3) as usize];
            for i in 0..60 {
                overlay.record(if i % 7 == 0 { 20.0 } else { 30.0 }, (i % 2 == 0).then_some(33.3));
                overlay.draw(&mut frame, 29.0);
            }
        }
    }
}
