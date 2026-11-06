# Video FPS Meter

Measures the real frame rate of games and other rendered content from video recordings and renders an FPS/frametime overlay video for performance analysis. Works with any source recorded at a fixed rate: consoles, PCs, handhelds or emulators, through a capture card or a screen recorder.

## How it works

The recording runs at a fixed rate (for example 30 or 60 Hz). When the source misses a frame deadline, the previous image is shown again and appears as a repeated frame in the recording. Each captured frame is converted to grayscale and compared with the previous one at full resolution; a pixel counts as changed when its value differs by more than 8 (out of 255). A frame with more changed pixels than the threshold (default 20) is a new image.

From that, the tool reports:

- Real average FPS
- Frametime of each image (how long it stayed on screen)
- 1% low FPS
- Optional overlay video with an FPS counter and FPS/frametime graphs, keeping the original audio

### Known limitations

- **A static scene counts as a drop.** From video alone, an image that did not change because nothing moved is indistinguishable from an image the game failed to render. Analyze spans with motion and use the start/end options to exclude menus and pauses.
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

With an NVIDIA GPU and an FFmpeg build that includes NVDEC/NVENC (the Windows builds above do), decoding runs on the GPU automatically and the overlay can be encoded with `--nvenc` / the **Use GPU (NVENC)** option. Without one, decoding and encoding run on the CPU.

## Usage

### Desktop interface

Run `video-fps-meter-gui`, choose a recording, select the outputs and click **Analyze**.

### Command line

```bash
video-fps-meter recording.mkv
video-fps-meter recording.mkv --overlay --nvenc --target 30
video-fps-meter recording.mkv --start 10 --end 70 --no-csv --lang pt
```

| Option | Description |
|---|---|
| `--overlay` | Write `<name>_overlay.mp4` |
| `--nvenc` | Encode the overlay with NVENC (falls back to x264 when unavailable) |
| `--target` | Target frame rate of the game; the overlay turns red below it (default 30) |
| `--start` / `--end` | Span to analyze, in seconds |
| `--threshold` | Changed pixels above this count mark a new image (default 20) |
| `--no-hw-decode` | Decode on the CPU only |
| `--no-csv` / `--no-chart` | Skip `<name>_fps.csv` / `<name>_chart.png` |
| `--lang` | `en` or `pt` (default: system language) |

Output files are written next to the recording.

## Recording guidelines (OBS)

Repeated frames must remain practically identical after compression.

- **Lossless (most reliable):** *Simple* output mode, *Lossless* quality (UtVideo). Produces several GB per minute.
- **Compressed (validated):** *Advanced* output mode, **NVENC HEVC**, **CQP 16–18**, MKV container. In a 4K30 capture test, repeated frames showed 0–2 changed pixels and new frames hundreds to millions, far from the default threshold of 20.
- Capture source with **Limited** color range, Rec. 709, and HDR disabled on the source device.

Validate each setup with a control clip: a few seconds of gameplay running above the capture rate (should report a constant FPS equal to the capture rate) and a few seconds of a fully static screen (should report close to 0).

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
- Decoding, analysis and overlay rendering/encoding run on separate threads; the per-frame metric is parallelized with Rayon.

## License

Released under the [MIT License](LICENSE). Bundled fonts and linked crates are listed in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
