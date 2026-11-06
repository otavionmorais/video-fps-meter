# Project context

Tool that measures the real frame rate of games and other rendered content from fixed-rate video recordings (capture card or screen recorder) and renders FPS/frametime overlays. Generic by design: no assumptions about a specific game, platform or device. Open source; all code, comments and documentation are in English. The UI and output strings are available in English and Portuguese.

## Layout

| Path | Contents |
|---|---|
| `src/analysis.rs` | Analysis loop, options, rows, threading (decoder → analysis → overlay/encoder) |
| `src/metric.rs` | Grayscale conversion and changed-pixel count (Rayon) |
| `src/decode.rs` / `src/encode.rs` | `ffmpeg` subprocesses (raw BGR24 over pipes) |
| `src/probe.rs` | `ffprobe` stream info |
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
- **Overlay encoding:** NVENC is probed first; fallback to libx264 adds a warning. Original audio is mapped with `-map 1:a?`.
- **Subprocesses** use `CREATE_NO_WINDOW` on Windows. FFmpeg next to the executable takes precedence over `PATH`.
- **FFmpeg is not bundled or versioned.** Full builds are ~217 MB per executable, above GitHub's 100 MB file limit. Users install it separately (README, "Installation"). The GPLv3 builds are redistributable if a release archive is ever added.
- **`release/`** holds the prebuilt Windows executables and is versioned. After a change, rebuild with `cargo build --release` and copy both `.exe` files from the target directory into `release/`.
- **Licensing:** MIT. `THIRD_PARTY_NOTICES.md` lists the embedded fonts (license texts in `licenses/`) and the crates linked into the Windows executables. Regenerate the crate table from `cargo metadata --filter-platform x86_64-pc-windows-msvc` (normal dependencies reachable from the root) when dependencies change; the project license must stay compatible with all of them.
- **Out of scope by owner decision:** automatic static-scene detection and percentile-based pause filtering.

## Performance notes

Measured on a 15 s 4K30 HEVC clip (RTX 4060): analysis 7.7 s; analysis + NVENC overlay 11.5 s. The metric takes ~1.7 ms per 4K frame; the bottleneck is the Windows pipe carrying 25 MB BGR24 frames (~1.6 GB/s). Reducing it requires either a different transport (smaller pixel format with bit-exact BGR reconstruction, or parallel decoders over segments) — pending owner decision.

## Development

- Build output: set `CARGO_TARGET_DIR` outside OneDrive to avoid syncing `target/`.
- Checks: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
