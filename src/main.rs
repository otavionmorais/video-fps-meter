//! Command-line interface.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use clap::{Parser, ValueEnum};
use video_fps_meter::{Lang, Options, OutputPaths, analyze, save_outputs, summarize};

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

    /// Write <name>_overlay.mp4 with the FPS counter and graphs.
    #[arg(long)]
    overlay: bool,

    /// Encode the overlay with NVIDIA NVENC (falls back to x264).
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
        overlay: overlay.clone(),
        nvenc: cli.nvenc,
        hw_decode: !cli.no_hw_decode,
        lang,
    };

    let mut next_report = 10.0;
    let analysis = match analyze(&cli.video, &options, &AtomicBool::new(false), |progress| {
        if progress.video_time_s >= next_report {
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

    println!("\n{}\n", summarize(&analysis.rows, analysis.capture_rate, lang));
    for message in save_outputs(&analysis.rows, &cli.video, !cli.no_csv, !cli.no_chart, lang) {
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
