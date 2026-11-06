//! Planar YUV frames: plane layout and blending of RGBA overlay layers.
//!
//! The overlay video is produced from the decoder's native planar YUV
//! frames and encoded losslessly in the same pixel format, so every pixel
//! the overlay does not cover stays bit-identical to the decoded source.
//! Only covered pixels (and, with chroma subsampling, the chroma samples
//! they share) are modified.

use crate::probe::ColorTags;

/// Pixel format used when the source format is not supported directly.
pub const FALLBACK_PIX_FMT: &str = "yuv444p";
/// High bit depth variant of [`FALLBACK_PIX_FMT`].
pub const FALLBACK_PIX_FMT_HIGH: &str = "yuv444p10le";

/// Layout and color model of a planar YUV frame.
#[derive(Clone, Debug, PartialEq)]
pub struct YuvLayout {
    pub pix_fmt: String,
    pub width: usize,
    pub height: usize,
    /// Horizontal and vertical chroma subsampling (log2).
    shift_x: u32,
    shift_y: u32,
    /// 1 for 8-bit samples, 2 for little-endian 16-bit containers.
    bytes_per_sample: usize,
    depth: u32,
    full_range: bool,
    /// Luma coefficients of the red and blue channels.
    kr: f32,
    kb: f32,
}

impl YuvLayout {
    /// Layout for a supported planar YUV `pix_fmt`, or `None`.
    pub fn new(pix_fmt: &str, width: u32, height: u32, color: &ColorTags) -> Option<Self> {
        let (jpeg_range, rest) = match pix_fmt.strip_prefix("yuvj") {
            Some(rest) => (true, rest),
            None => (false, pix_fmt.strip_prefix("yuv")?),
        };
        let (shift_x, shift_y) = match rest.get(..3)? {
            "420" => (1, 1),
            "422" => (1, 0),
            "444" => (0, 0),
            _ => return None,
        };
        let (bytes_per_sample, depth) = match rest.get(3..)? {
            "p" => (1, 8),
            "p10le" if !jpeg_range => (2, 10),
            "p12le" if !jpeg_range => (2, 12),
            _ => return None,
        };
        let full_range = jpeg_range || color.range.as_deref() == Some("pc");
        let (kr, kb) = match color.space.as_deref() {
            Some("bt709") => (0.2126, 0.0722),
            Some("bt2020nc" | "bt2020c") => (0.2627, 0.0593),
            Some("smpte170m" | "bt470bg" | "fcc") => (0.299, 0.114),
            _ if height >= 720 => (0.2126, 0.0722),
            _ => (0.299, 0.114),
        };
        Some(Self {
            pix_fmt: pix_fmt.to_owned(),
            width: width as usize,
            height: height as usize,
            shift_x,
            shift_y,
            bytes_per_sample,
            depth,
            full_range,
            kr,
            kb,
        })
    }

    fn chroma_size(&self) -> (usize, usize) {
        let up = |size: usize, shift: u32| (size + (1 << shift) - 1) >> shift;
        (up(self.width, self.shift_x), up(self.height, self.shift_y))
    }

    /// Size of one frame in bytes.
    pub fn frame_len(&self) -> usize {
        let (cw, ch) = self.chroma_size();
        (self.width * self.height + 2 * cw * ch) * self.bytes_per_sample
    }

    /// Byte offset and width (in samples) of plane `index` (0 = Y, 1 = U, 2 = V).
    fn plane(&self, index: usize) -> (usize, usize) {
        let (cw, ch) = self.chroma_size();
        let luma = self.width * self.height;
        match index {
            0 => (0, self.width),
            1 => (luma * self.bytes_per_sample, cw),
            _ => ((luma + cw * ch) * self.bytes_per_sample, cw),
        }
    }

    fn read(&self, frame: &[u8], plane: usize, x: usize, y: usize) -> f32 {
        let (offset, stride) = self.plane(plane);
        let i = offset + (y * stride + x) * self.bytes_per_sample;
        if self.bytes_per_sample == 1 {
            f32::from(frame[i])
        } else {
            f32::from(u16::from_le_bytes([frame[i], frame[i + 1]]))
        }
    }

    fn write(&self, frame: &mut [u8], plane: usize, x: usize, y: usize, value: f32) {
        let max = ((1u32 << self.depth) - 1) as f32;
        let value = (value + 0.5).floor().clamp(0.0, max);
        let (offset, stride) = self.plane(plane);
        let i = offset + (y * stride + x) * self.bytes_per_sample;
        if self.bytes_per_sample == 1 {
            frame[i] = value as u8;
        } else {
            frame[i..i + 2].copy_from_slice(&(value as u16).to_le_bytes());
        }
    }

    /// Code-value contribution `(alpha, y, cb, cr)` of a premultiplied RGBA
    /// pixel: the blended sample is `sample * (1 - alpha) + contribution`.
    fn contribution(&self, rgba: &[u8]) -> (f32, f32, f32, f32) {
        let [r, g, b, a] = [rgba[0], rgba[1], rgba[2], rgba[3]].map(|v| f32::from(v) / 255.0);
        let (kr, kb) = (self.kr, self.kb);
        let luma = kr * r + (1.0 - kr - kb) * g + kb * b;
        let cb = (b - luma) / (2.0 * (1.0 - kb));
        let cr = (r - luma) / (2.0 * (1.0 - kr));
        let max = ((1u32 << self.depth) - 1) as f32;
        let mid = (1u32 << (self.depth - 1)) as f32;
        let unit = (1u32 << (self.depth - 8)) as f32;
        let (y_offset, y_scale, c_scale) =
            if self.full_range { (0.0, max, max) } else { (16.0 * unit, 219.0 * unit, 224.0 * unit) };
        (a, a * y_offset + y_scale * luma, a * mid + c_scale * cb, a * mid + c_scale * cr)
    }

    /// Blends a premultiplied RGBA layer of `width` × `height` pixels whose
    /// top-left corner is at `(x0, y0)`. The layer must lie inside the frame.
    pub fn blend(&self, frame: &mut [u8], x0: usize, y0: usize, width: usize, height: usize, rgba: &[u8]) {
        debug_assert!(x0 + width <= self.width && y0 + height <= self.height);
        let pixel = |x: usize, y: usize| -> Option<&[u8]> {
            let inside = (x0..x0 + width).contains(&x) && (y0..y0 + height).contains(&y);
            if !inside {
                return None;
            }
            let i = ((y - y0) * width + (x - x0)) * 4;
            Some(&rgba[i..i + 4]).filter(|p| p[3] > 0)
        };

        for y in y0..y0 + height {
            for x in x0..x0 + width {
                if let Some(p) = pixel(x, y) {
                    let (alpha, luma, _, _) = self.contribution(p);
                    let value = self.read(frame, 0, x, y) * (1.0 - alpha) + luma;
                    self.write(frame, 0, x, y, value);
                }
            }
        }

        let (block_w, block_h) = (1usize << self.shift_x, 1usize << self.shift_y);
        for cy in (y0 >> self.shift_y)..=((y0 + height - 1) >> self.shift_y) {
            for cx in (x0 >> self.shift_x)..=((x0 + width - 1) >> self.shift_x) {
                let xs = (cx * block_w)..((cx + 1) * block_w).min(self.width);
                let ys = (cy * block_h)..((cy + 1) * block_h).min(self.height);
                let count = (xs.len() * ys.len()) as f32;
                let (mut alpha, mut cb, mut cr) = (0.0, 0.0, 0.0);
                for y in ys {
                    for x in xs.clone() {
                        if let Some(p) = pixel(x, y) {
                            let (a, _, b, r) = self.contribution(p);
                            alpha += a;
                            cb += b;
                            cr += r;
                        }
                    }
                }
                if alpha == 0.0 {
                    continue;
                }
                let keep = 1.0 - alpha / count;
                let u = self.read(frame, 1, cx, cy) * keep + cb / count;
                let v = self.read(frame, 2, cx, cy) * keep + cr / count;
                self.write(frame, 1, cx, cy, u);
                self.write(frame, 2, cx, cy, v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(pix_fmt: &str, w: u32, h: u32) -> YuvLayout {
        YuvLayout::new(pix_fmt, w, h, &ColorTags::default()).unwrap()
    }

    fn frame(layout: &YuvLayout, y: u16, c: u16) -> Vec<u8> {
        let mut frame = vec![0; layout.frame_len()];
        let (cw, ch) = layout.chroma_size();
        for py in 0..layout.height {
            for px in 0..layout.width {
                layout.write(&mut frame, 0, px, py, f32::from(y));
            }
        }
        for plane in 1..3 {
            for py in 0..ch {
                for px in 0..cw {
                    layout.write(&mut frame, plane, px, py, f32::from(c));
                }
            }
        }
        frame
    }

    #[test]
    fn parses_supported_formats() {
        assert_eq!(layout("yuv420p", 1920, 1080).frame_len(), 1920 * 1080 * 3 / 2);
        assert_eq!(layout("yuv420p10le", 1920, 1080).frame_len(), 1920 * 1080 * 3);
        assert_eq!(layout("yuv444p", 4, 2).frame_len(), 24);
        assert_eq!(layout("yuv420p", 5, 3).frame_len(), 15 + 2 * 3 * 2);
        assert!(layout("yuvj420p", 64, 64).full_range);
        assert!(YuvLayout::new("nv12", 64, 64, &ColorTags::default()).is_none());
        assert!(YuvLayout::new("gbrp", 64, 64, &ColorTags::default()).is_none());
    }

    #[test]
    fn transparent_layer_changes_nothing() {
        let l = layout("yuv420p", 16, 16);
        let original = frame(&l, 100, 140);
        let mut blended = original.clone();
        l.blend(&mut blended, 2, 2, 8, 8, &[0; 8 * 8 * 4]);
        assert_eq!(blended, original);
    }

    #[test]
    fn opaque_white_maps_to_limited_range_white() {
        for (pix_fmt, white, mid) in [("yuv420p", 235.0, 128.0), ("yuv420p10le", 940.0, 512.0)] {
            let l = layout(pix_fmt, 8, 8);
            let mut f = frame(&l, 50, 60);
            l.blend(&mut f, 0, 0, 8, 8, &[255; 8 * 8 * 4]);
            assert_eq!(l.read(&f, 0, 3, 3), white);
            assert_eq!(l.read(&f, 1, 1, 1), mid);
            assert_eq!(l.read(&f, 2, 1, 1), mid);
        }
    }

    #[test]
    fn only_covered_samples_change() {
        let l = layout("yuv420p", 16, 16);
        let original = frame(&l, 100, 140);
        let mut f = original.clone();
        // 3×3 opaque black square at (5, 5).
        let layer: Vec<u8> = [0, 0, 0, 255].repeat(9);
        l.blend(&mut f, 5, 5, 3, 3, &layer);
        for y in 0..16 {
            for x in 0..16 {
                let covered = (5..8).contains(&x) && (5..8).contains(&y);
                assert_eq!(l.read(&f, 0, x, y) != 100.0, covered, "luma at ({x}, {y})");
            }
        }
        // Chroma blocks 2..=3 in each direction overlap the square.
        for cy in 0..8 {
            for cx in 0..8 {
                let touched = (2..4).contains(&cx) && (2..4).contains(&cy);
                assert_eq!(l.read(&f, 1, cx, cy) != 140.0, touched, "chroma at ({cx}, {cy})");
            }
        }
    }

    #[test]
    fn translucent_black_darkens_luma_and_desaturates_chroma() {
        let l = layout("yuv444p", 4, 4);
        let mut f = frame(&l, 200, 180);
        l.blend(&mut f, 0, 0, 4, 4, &[0, 0, 0, 140].repeat(16));
        let keep = 1.0 - 140.0 / 255.0;
        let expected_y = 200.0 * keep + 16.0 * (140.0 / 255.0);
        let expected_c = 180.0 * keep + 128.0 * (140.0 / 255.0);
        assert!((l.read(&f, 0, 0, 0) - expected_y).abs() <= 0.5);
        assert!((l.read(&f, 1, 0, 0) - expected_c).abs() <= 0.5);
    }
}
