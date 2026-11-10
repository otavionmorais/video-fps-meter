# Video FPS Meter

Measures the real frame rate of games and other rendered content from video recordings and renders an FPS/frametime overlay video for performance analysis. Works with any source recorded at a fixed rate: consoles, PCs, handhelds or emulators, through a capture card or a screen recorder.

## How it works

The recording runs at a fixed rate (for example 30 or 60 Hz). When the source misses a frame deadline, the previous image is shown again and appears as a repeated frame in the recording. Each captured frame is converted to grayscale and compared with the previous one at full resolution; a pixel counts as changed when its value differs by more than 8 (out of 255). A frame with more changed pixels than the threshold (default 20) is a new image.

From that, the tool reports:

- Real average FPS
- Frametime of each image (how long it stayed on screen)
- 1% low FPS
- Potential stutters: frametime spikes, with count, rate per minute and the worst one
- Static intervals (menus, pauses, loading screens), excluded from all statistics
- Optional overlay video with the FPS counter, running statistics (average, 1% low), a potential-stutter alert and FPS/frametime graphs, each of which can be turned off, at visually identical quality and about the size of the recording (see below)

### Static intervals

An image that does not change is either a game that stopped rendering or a scene where nothing moves. Gameplay almost always contains some motion (particles, foliage, HUD), so long runs of identical frames are treated as menus or pauses rather than drops:

1. A single image that stays on screen for at least **500 ms** (`--static-ms`) starts a static interval.
2. A sequence of slow images (each lasting at least 200 ms) that spans at least 1 s is also static. This covers menus with a blinking cursor.
3. A span with motion shorter than 2 s between two static spans is also static. This covers animated transitions between menu screens. Gameplay that resumes after a menu is not affected.

Static intervals are excluded from the average FPS, frametimes, 1% low and stutter count. They are listed in the summary (count, total and longest, with position), marked in the CSV and shaded in the chart. In the overlay the counter shows `FPS: —` and the graphs are interrupted. Shorter freezes are not affected and are reported as stutters.

### Stutters

A stutter is a frametime at least **2.5×** the median of the previous 20 frametimes (`--stutter-factor`). With the default, an image that stays on screen for two or more extra refreshes is a stutter (100 ms at a steady 30 fps, 50 ms at 60 fps), while a single late frame is not; it still counts toward the average FPS and 1% low. A factor of 1.5 also reports single late frames. Detection on recordings produces false positives, so the overlay shows no stutter count: each stutter's frametime appears for 1.5 s with a *potential stutter* label. Stutters are counted in the summary, flagged in the CSV and marked in red in the chart.

### Overlay video

The overlay is blended directly into the recording's YUV planes (no RGB round trip). Burning it in requires re-encoding every frame, so two quality levels are available:

| Quality | Encoding | Size | Fidelity outside the overlay |
|---|---|---|---|
| **High** (default) | 10-bit HEVC: NVENC QP 8, or x265 CRF 8 on the CPU | About 1–1.5× the recording | Visually identical |
| **Lossless** (`--lossless`) | Same pixel format: HEVC NVENC lossless, or x264 QP 0 on the CPU | About 6× the recording | Bit-identical |

In both, audio tracks are **copied unchanged** and the container (MKV, MP4 or MOV) and color tags of the recording are kept.

Measured on 1440p60 NVENC HEVC recordings (RTX 4060), outside the overlay area:

| Recording | High quality size | VMAF (identical copy) | PSNR | Lossless size |
|---|---|---|---|---|
| 8-bit, CQP 16, 30 s | 1.02× | 97.8 (98.6) | 53.2 dB | 6.4× |
| 10-bit, CQP 16, 99 s | 1.48× | 97.8 (98.6) | 55.7 dB | — |

The quantizer is fixed, so lighter recordings produce a larger relative size. Higher settings (QP 6, preset P7, `tune uhq`) were tested and did not improve the result: above this level the encoder only reproduces the compression noise of the recording. Encoding 8-bit sources at 10 bits gave a smaller and more accurate file than 8 bits at the same quantizer. Uploads to YouTube are re-encoded at a much lower bitrate, so the lossless mode is only useful when exact pixels are required.

Supported source formats are planar YUV 4:2:0, 4:2:2 and 4:4:4 at 8, 10 or 12 bits, which covers recordings from OBS and capture cards; other formats are converted to 4:4:4 and a warning is shown. NVENC is used for 4:2:0 sources; other chroma formats are encoded on the CPU.

### Known limitations

- **Static detection is a heuristic.** A real freeze longer than the static threshold is excluded instead of counted; the summary lists every excluded interval so it can be checked. Animated menus and loading screens render real frames and are measured as gameplay; use `--start` / `--end` to leave them out.
- **Precision is bounded by the recording rate.** A 30 Hz recording measures up to 30 fps with 33.3 ms frametime resolution; at 60 Hz the step is 16.7 ms.
- **No screen-tearing detection.** Designed for v-synced output.

## Installation

1. Download the executables from the [`release/`](release/) folder:
   - `video-fps-meter-gui.exe` — desktop interface
   - `video-fps-meter.exe` — command line
2. Install [FFmpeg](https://ffmpeg.org/). It is required and not bundled. Both `ffmpeg` and `ffprobe` are used.
   - **Windows:**
     ```bash
     winget install --id Gyan.FFmpeg -e
     ```
     Alternatively, download a build from [gyan.dev](https://www.gyan.dev/ffmpeg/builds/) or [BtbN](https://github.com/BtbN/FFmpeg-Builds/releases) and either add its `bin` folder to `PATH` or copy `ffmpeg.exe` and `ffprobe.exe` next to the executables above.
   - **Linux:** install the `ffmpeg` package from the distribution (e.g. `sudo apt install ffmpeg`).
   - **macOS:** `brew install ffmpeg`.
3. Verify the installation:
   ```bash
   ffmpeg -version
   ```

A copy of FFmpeg next to the executables takes precedence over the one on `PATH`.

### GPU acceleration (optional)

With an NVIDIA GPU and an FFmpeg build that includes NVDEC/NVENC (the Windows builds above do), decoding and overlay encoding run on the GPU automatically (`--no-hw-decode` and `--no-nvenc` / the **Use GPU (NVENC)** option turn them off). Without one, decoding and encoding run on the CPU; high-quality encoding with x265 is then about 3× slower than real time at 1440p60.

## Usage

### Desktop interface

Run `video-fps-meter-gui`, choose a recording, select the outputs and click **Analyze**.

### Command line

```bash
video-fps-meter recording.mkv
video-fps-meter recording.mkv --overlay --target 30
video-fps-meter recording.mkv --start 10 --end 70 --no-csv --lang pt
```

| Option | Description |
|---|---|
| `--overlay` | Write `<name>_overlay.<ext>` (high-quality 10-bit HEVC, same container as the recording) |
| `--lossless` | Encode the overlay losslessly (bit-identical outside the overlay, about 6× larger) |
| `--no-nvenc` | Encode the overlay on the CPU (x265, or x264 when lossless) instead of NVENC |
| `--overlay-hide` | Overlay elements to leave out, comma-separated: `fps-counter`, `stats`, `stutter-alert`, `fps-graph`, `frametime-graph` |
| `--target` | Target frame rate of the game; the overlay turns red below it (default 30) |
| `--start` / `--end` | Span to analyze, in seconds |
| `--threshold` | Changed pixels above this count mark a new image (default 20) |
| `--static-ms` | An image unchanged for at least this long starts a static interval (default 500; 0 disables) |
| `--stutter-factor` | A frametime at least this many times the recent median is a stutter (default 2.5) |
| `--no-hw-decode` | Decode on the CPU only |
| `--no-csv` / `--no-chart` | Skip `<name>_fps.csv` / `<name>_chart.png` |
| `--lang` | `en` or `pt` (default: system language) |

Output files are written next to the recording. The CSV has one line per captured frame with the columns `frame, time_s, changed_pixels, new_image, static, fps, frametime_ms, stutter`.

## Recording guidelines (OBS)

Repeated frames must remain practically identical after compression.

- **Lossless (most reliable):** *Simple* output mode, *Lossless* quality (UtVideo). Produces several GB per minute.
- **Compressed (validated):** *Advanced* output mode, MKV container, **NVENC HEVC**, rate control **CQP 16**, preset **P5**, tuning **High Quality**, **single pass**, **look-ahead off**, **adaptive quantization off**, profile **main10** with color format **P010** (*Advanced → Video*). On a 1440p60 recording with these settings, repeated frames showed 0–1 changed pixels and new frames 47 or more. With adaptive quantization, look-ahead, two-pass and 8-bit enabled, repeated frames reached 18 changed pixels and new frames started at 21, leaving almost no margin around the default threshold of 20.
- OBS frame rate equal to the source output (for example 60, not 59.94, for a 60 Hz console), capture source set to a fixed resolution and frame rate, buffering off, uncompressed (NV12/YUY2) rather than MJPEG when available.
- Source device at a fixed refresh rate with **VRR off**, and HDR off.
- Capture source with **Limited** color range and Rec. 709.

Validate each setup with a control clip: a few seconds of gameplay running above the capture rate (should report a constant FPS equal to the capture rate) and a few seconds of a fully static screen (should be reported as a static interval).

## Building from source

Requires a stable [Rust toolchain](https://rustup.rs/) (1.85 or newer).

```bash
cargo build --release
```

Binaries are written to `target/release/`: `video-fps-meter` (command line) and `video-fps-meter-gui` (desktop interface, no console window on Windows).

### Tests

```bash
cargo test
```

The integration tests generate synthetic lossless videos with a known frame rate, so no real recording is needed. FFmpeg must be installed.

## Design notes

- Decoding is delegated to an `ffmpeg` subprocess that outputs raw BGR24 frames; no FFmpeg libraries are linked.
- The grayscale conversion uses BT.601 weights in 15-bit fixed point, bit-exact with OpenCV's `COLOR_BGR2GRAY`.
- Hardware decoding uses NVDEC (`-hwaccel cuda`). Decoded frames are converted to the stream's native pixel format before the BGR conversion, which makes the output bit-identical to software decoding. If NVDEC is unavailable, decoding falls back to the CPU.
- Analysis runs in two passes when an overlay is requested: the first detects new images (BGR24 frames), the second decodes the recording again in its native YUV format and blends the overlay with the complete timeline, so static intervals are marked from their first frame.
- Decoding, analysis, overlay drawing and encoding run on separate threads; the per-frame metric is parallelized with Rayon.

## License

Released under the [MIT License](LICENSE). Bundled fonts and linked crates are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
