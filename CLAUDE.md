# Project context

Tool that measures the real frame rate of games and other rendered content from fixed-rate video recordings (capture card or screen recorder) and renders FPS/frametime overlays. Generic by design: no assumptions about a specific game, platform or device. Open source; all code, comments and documentation are in English. The UI and output strings are available in English and Portuguese.

## Layout

| Path | Contents |
|---|---|
| `src/analysis.rs` | Options, rows, detection pass and overlay pass (decoder → drawing → encoder threads) |
| `src/timeline.rs` | Frametimes, static intervals, rolling FPS and stutters, derived from the `new_image` flags |
| `src/stats.rs` | Cumulative statistics over non-static frames (shared by summary and overlay) |
| `src/metric.rs` | Grayscale conversion and changed-pixel count (Rayon) |
| `src/decode.rs` / `src/encode.rs` | `ffmpeg` subprocesses (raw BGR24 over pipes) |
| `src/probe.rs` | `ffprobe` stream info |
| `src/yuv.rs` | Planar YUV layout and blending of RGBA layers into YUV frames |
| `src/overlay.rs` | FPS counter and graphs (tiny-skia + ttf-parser, Hack font from `epaint_default_fonts`) |
| `src/chart.rs` | PNG chart (plotters) |
| `src/summary.rs`, `src/output.rs`, `src/i18n.rs` | Text summary, CSV/paths, localized strings |
| `src/main.rs` | CLI (clap) |
| `src/bin/gui.rs` | Desktop UI (eframe/egui, rfd dialogs) |
| `tests/analysis.rs` | End-to-end tests on synthetic FFV1 videos (require FFmpeg) |

## Technical decisions (do not revert without a reason)

- **Metric:** count of pixels with grayscale `absdiff > 8`, full resolution; default threshold 20. Averaging the difference was rejected because it misses small moving objects. Validated on a 4K30 NVENC HEVC CQP ~18 recording: repeated frames 0–2 changed pixels, new frames hundreds to millions.
- **Metric stability:** results must not change with the decoding path. Grayscale uses BT.601 weights in 15-bit fixed point (`3735, 19235, 9798`, shift 15), bit-exact with OpenCV's `COLOR_BGR2GRAY` over all 2^24 colors. CPU and GPU decoding produce identical rows.
- **Hardware decoding:** `-hwaccel cuda` only, followed by `-vf format=<native pix_fmt>`. Without the format step, NV12 → BGR conversion differs from software decoding. D3D11VA was slower than CPU decoding for 4K HEVC (copy-back cost), and `-hwaccel auto` selected Vulkan, which crashes FFmpeg 9 on FFV1. Falls back to CPU when the first frame cannot be decoded.
- **Seeking:** `-ss` is placed half a frame before the first requested frame to avoid losing it to container timestamp rounding; `-frames:v` limits the span; `-fps_mode passthrough` keeps every decoded frame.
- **Lossless overlay (owner requirement: never reduce quality):** pass 2 decodes the native planar YUV format and blends the RGBA layers into the YUV planes, touching only covered pixels and the chroma samples they share; the video is encoded losslessly in the same pixel format (`hevc_nvenc -tune lossless`, or `libx264 -qp 0`), audio streams are copied, container kept (MKV/MP4/MOV, else MKV). Verified on a real 4K HEVC recording: PSNR inf outside the overlay over all frames and identical audio packets. Pitfalls found: an RGB round trip changes every pixel even with lossless encoding; color tags given as *output* options make FFmpeg convert the frames (they must be *input* options on the raw stream); `-shortest` drops the last frames when audio is shorter; `-ss 0` on the audio input drops the AAC priming packet (seek only for spans). Unsupported source formats fall back to `yuv444p`/`yuv444p10le` with a warning.
- **Subprocesses** use `CREATE_NO_WINDOW` on Windows. FFmpeg next to the executable takes precedence over `PATH`.
- **FFmpeg is not bundled or versioned.** Full builds are ~217 MB per executable, above GitHub's 100 MB file limit. Users install it separately (README, "Installation"). The GPLv3 builds are redistributable if a release archive is ever added.
- **`release/`** holds the prebuilt Windows executables and is versioned. After a change, rebuild with `cargo build --release` and copy both `.exe` files from the target directory into `release/`.
- **Licensing:** MIT. `THIRD_PARTY_NOTICES.md` lists the embedded fonts (license texts in `licenses/`) and the crates linked into the Windows executables. Regenerate the crate table from `cargo metadata --filter-platform x86_64-pc-windows-msvc` (normal dependencies reachable from the root) when dependencies change; the project license must stay compatible with all of them.
- **Static intervals (owner priority: never report menus or pauses as drops):** a run of identical frames ≥ `static_ms` (default 500 ms), or a chain of runs each ≥ 200 ms totaling ≥ 1 s (blinking cursors), or an active span < 2 s between two static spans (menu transitions), is static. Static frames are excluded from all statistics, listed in the summary, `fps = None`, and shown as `FPS: —` with interrupted graphs. Classification is by timing only; classifying by low pixel activity was rejected because it would hide real low-motion gameplay.
- **Stutters (owner priority):** frametime ≥ `stutter_factor` × median of the previous 20 counted frametimes (needs ≥ 5). Default 2.5: on a real 30 Hz recording, 1.5 flagged a single repeated frame (66.7 ms, possibly an OBS duplicate) that the owner did not see as a stutter. Frametimes that close a static run are not counted.
- **Rolling FPS** uses the last second of non-static frames, so it continues across pauses instead of ramping up from zero; undefined for the first half second.
- **Two-pass overlay:** static detection needs up to ~1 s of look-ahead, so the overlay is drawn in a second decode pass with the complete timeline. Chosen over a frame buffer, which would need up to ~1.5 GB at 4K60.
- **Overlay statistics** (average, 1% low) are cumulative up to the current frame and computed by `Stats`, the same code as the final summary.
- **Overlay elements** (counter, statistics, stutter alert, FPS graph, frametime graph) are toggled individually (`OverlayElements`; GUI checkboxes, `--overlay-hide`); the remaining ones close up.
- **No stutter count in the overlay (owner decision):** false positives are common on recordings, so the overlay only shows the stutter's frametime for 1.5 s labeled `POTENTIALLY STUTTER` / `POTENCIALMENTE STUTTER`. The summary, CSV and chart still report stutters (labeled "potential" in the summary).
- **Out of scope by owner decision:** tearing detection, percentile-based pause filtering.

## Performance notes

Measured on a 15 s 4K30 HEVC clip (RTX 4060): analysis 6.4 s. Lossless NVENC overlay on a real 30.8 s 4K30 recording: 36 s total, 1.3 GB output (source 205 MB). The metric takes ~1.7 ms per 4K frame; the bottleneck is the Windows pipe carrying 25 MB BGR24 frames (~1.6 GB/s). Reducing it requires either a different transport (smaller pixel format with bit-exact BGR reconstruction, or parallel decoders over segments) — pending owner decision.

## Development

- Build output: set `CARGO_TARGET_DIR` outside OneDrive to avoid syncing `target/`.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
