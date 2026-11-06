//! Frame comparison metric.
//!
//! The metric is the number of pixels whose grayscale value changed by more
//! than [`SENSITIVITY`], computed at full resolution. Counting pixels instead
//! of averaging the difference keeps small moving objects from being diluted
//! by a static background.

use rayon::prelude::*;

/// Minimum difference (0–255) for a pixel to count as changed.
pub const SENSITIVITY: u8 = 8;

/// Pixels processed per parallel work unit.
const CHUNK_PIXELS: usize = 1 << 16;

/// Grayscale value of a BGR pixel using BT.601 weights in 15-bit fixed
/// point. Bit-exact with OpenCV's `cvtColor(..., COLOR_BGR2GRAY)` for 8-bit
/// images (verified over all 2^24 colors).
#[inline(always)]
pub fn luma(b: u8, g: u8, r: u8) -> u8 {
    ((u32::from(b) * 3735 + u32::from(g) * 19235 + u32::from(r) * 9798 + (1 << 14)) >> 15) as u8
}

/// Converts a BGR24 frame to grayscale into `gray` and, when `previous` is
/// given, returns how many pixels changed relative to it.
///
/// # Panics
/// Panics if the buffer sizes do not match (`bgr.len() == 3 * gray.len()`).
pub fn gray_and_count(bgr: &[u8], gray: &mut [u8], previous: Option<&[u8]>) -> u64 {
    let pixels = gray.len();
    assert_eq!(bgr.len(), pixels * 3, "BGR and gray buffers differ in size");
    let gray_chunks = gray.par_chunks_mut(CHUNK_PIXELS);
    let bgr_chunks = bgr.par_chunks(CHUNK_PIXELS * 3);
    match previous {
        Some(previous) => {
            assert_eq!(previous.len(), pixels, "frame sizes differ");
            gray_chunks
                .zip(bgr_chunks)
                .zip(previous.par_chunks(CHUNK_PIXELS))
                .map(|((gray, bgr), previous)| {
                    let mut changed = 0u32;
                    for ((out, px), &old) in gray.iter_mut().zip(bgr.chunks_exact(3)).zip(previous) {
                        let value = luma(px[0], px[1], px[2]);
                        *out = value;
                        changed += u32::from(value.abs_diff(old) > SENSITIVITY);
                    }
                    u64::from(changed)
                })
                .sum()
        }
        None => {
            gray_chunks.zip(bgr_chunks).for_each(|(gray, bgr)| {
                for (out, px) in gray.iter_mut().zip(bgr.chunks_exact(3)) {
                    *out = luma(px[0], px[1], px[2]);
                }
            });
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luma_matches_opencv_reference_values() {
        // Reference values from OpenCV's COLOR_BGR2GRAY on single pixels (BGR order).
        assert_eq!(luma(0, 0, 0), 0);
        assert_eq!(luma(255, 255, 255), 255);
        assert_eq!(luma(255, 0, 0), 29);
        assert_eq!(luma(0, 255, 0), 150);
        assert_eq!(luma(0, 0, 255), 76);
        assert_eq!(luma(80, 120, 60), 98);
    }

    #[test]
    fn counts_only_differences_above_sensitivity() {
        let width = 200_000; // spans several parallel chunks
        let first = vec![100u8; width * 3];
        let mut second = first.clone();
        // 5 pixels changed a lot, 7 pixels changed by exactly SENSITIVITY.
        for i in 0..5 {
            second[i * 3..i * 3 + 3].copy_from_slice(&[200, 200, 200]);
        }
        let s = SENSITIVITY;
        for i in 150_000..150_007 {
            second[i * 3..i * 3 + 3].copy_from_slice(&[100 + s, 100 + s, 100 + s]);
        }
        let mut gray_a = vec![0; width];
        let mut gray_b = vec![0; width];
        assert_eq!(gray_and_count(&first, &mut gray_a, None), 0);
        assert_eq!(gray_and_count(&second, &mut gray_b, Some(&gray_a)), 5);
    }
}
