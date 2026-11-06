//! Command-line interface.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use clap::{Parser, ValueEnum};
use video_fps_meter::{Lang, Options, OutputPaths, Stage, analyze, save_outputs, summarize};

/// Measures the real frame rate of a game or other rendered content from a video recording.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Recording to analyze.
    video: PathBuf,

    /// A frame is a new image when more than this many pixels changed.
    #[arg(long, default_value_t = 20)]
    threshold: u64,

    /// Start of the analyzed span, in seconds.
    #[arg(long, default_value_t = 0.0)]
    start: f64,

    /// End of the analyzed span, in seconds.
    #[arg(long)]
    end: Option<f64>,

    /// Target frame rate of the game; the overlay turns red below it.
    #[arg(long, default_value_t = 30.0)]
    target: f64,

    /// An image unchanged for at least this many milliseconds marks a static
    /// interval (menu, pause), excluded from the statistics. 0 disables it.
    #[arg(long, default_value_t = video_fps_meter::DEFAULT_STATIC_MS)]
    static_ms: f64,

    /// A frametime at least this many times the median of the previous ones
    /// is a stutter. The default (2.5) requires two or more consecutive late
    /// frames; 1.5 also counts single late frames.
    #[arg(long, default_value_t = video_fps_meter::DEFAULT_STUTTER_FACTOR)]
    stutter_factor: f64,

    /// Write <name>_overlay.<ext> with the FPS counter, statistics and graphs.
    /// Lossless: pixels outside the overlay and audio are unchanged.
    #[arg(long)]
    overlay: bool,

    /// Encode the overlay with lossless HEVC NVENC (falls back to lossless x264).
    #[arg(long)]
    nvenc: bool,

    /// Decode on the CPU only.
    #[arg(long)]
    no_hw_decode: bool,

    /// Skip <name>_fps.csv.
    #[arg(long)]
    no_csv: bool,

    /// Skip <name>_chart.png.
    #[arg(long)]
    no_chart: bool,

    /// Output language (default: system language).
    #[arg(long, value_enum)]
    lang: Option<LangArg>,
}

#[derive(Clone, Copy, ValueEnum)]
enum LangArg {
    En,
    Pt,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let lang = match cli.lang {
        Some(LangArg::En) => Lang::En,
        Some(LangArg::Pt) => Lang::Pt,
        None => Lang::detect(),
    };
    let overlay = cli.overlay.then(|| OutputPaths::for_video(&cli.video).overlay);
    let options = Options {
        threshold: cli.threshold,
        start_s: cli.start,
        end_s: cli.end,
        target_fps: cli.target,
        static_ms: cli.static_ms,
        stutter_factor: cli.stutter_factor,
        overlay: overlay.clone(),
        nvenc: cli.nvenc,
        hw_decode: !cli.no_hw_decode,
        lang,
    };

    let mut stage = Stage::Detection;
    let mut next_report = 10.0;
    let analysis = match analyze(&cli.video, &options, &AtomicBool::new(false), |progress| {
        if progress.stage != stage {
            stage = progress.stage;
            next_report = 10.0;
            eprintln!("{}", lang.strings().generating_overlay);
        }
        if progress.video_time_s >= next_report && progress.fraction < 1.0 {
            eprintln!("  {:.0} s", progress.video_time_s);
            next_report = (progress.video_time_s / 10.0).floor() * 10.0 + 10.0;
        }
    }) {
        Ok(analysis) => analysis,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    println!("\n{}\n", summarize(&analysis, lang));
    for message in save_outputs(&analysis, &cli.video, !cli.no_csv, !cli.no_chart, lang) {
        println!("{message}");
    }
    if let Some(path) = overlay {
        println!("Overlay: {}", path.display());
    }
    for warning in &analysis.warnings {
        println!("{warning}");
    }
    ExitCode::SUCCESS
}
