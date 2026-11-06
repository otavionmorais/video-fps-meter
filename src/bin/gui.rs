//! Desktop interface.

#![windows_subsystem = "windows"]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use eframe::egui;
use video_fps_meter::{Lang, Options, OutputPaths, analyze, save_outputs, summarize};

const VIDEO_EXTENSIONS: [&str; 4] = ["mkv", "mp4", "avi", "mov"];

struct Texts {
    title: &'static str,
    language: &'static str,
    video: &'static str,
    choose: &'static str,
    no_video: &'static str,
    outputs: &'static str,
    overlay: &'static str,
    nvenc: &'static str,
    csv: &'static str,
    chart: &'static str,
    target: &'static str,
    start: &'static str,
    end: &'static str,
    static_ms: &'static str,
    stutter_factor: &'static str,
    optional: &'static str,
    analyze: &'static str,
    cancel: &'static str,
    open_folder: &'static str,
    done: &'static str,
    processing: &'static str,
    canceled: &'static str,
    error: &'static str,
    choose_video_first: &'static str,
    invalid_numbers: &'static str,
    unexpected_error: &'static str,
    videos: &'static str,
    all_files: &'static str,
}

static EN: Texts = Texts {
    title: "Video FPS Meter",
    language: "Language",
    video: "Video",
    choose: "Choose…",
    no_video: "No video selected",
    outputs: "Outputs",
    overlay: "Generate overlay video",
    nvenc: "Use GPU (NVENC)",
    csv: "Generate CSV",
    chart: "Generate chart (PNG)",
    target: "Target FPS",
    start: "Start (s)",
    end: "End (s)",
    static_ms: "Static scene (ms)",
    stutter_factor: "Stutter (×)",
    optional: "optional",
    analyze: "Analyze",
    cancel: "Cancel",
    open_folder: "Open folder",
    done: "Done.",
    processing: "Processing…",
    canceled: "Canceled. Partial results below.",
    error: "Error",
    choose_video_first: "Choose a video first.",
    invalid_numbers: "Invalid value in Target FPS, Start, End, Static scene or Stutter (must be at least 1).",
    unexpected_error: "Unexpected error",
    videos: "Videos",
    all_files: "All files",
};

static PT: Texts = Texts {
    title: "Medidor de FPS",
    language: "Idioma",
    video: "Vídeo",
    choose: "Escolher…",
    no_video: "Nenhum vídeo selecionado",
    outputs: "Saídas",
    overlay: "Gerar vídeo com overlay",
    nvenc: "Usar placa de vídeo (NVENC)",
    csv: "Gerar CSV",
    chart: "Gerar gráfico (PNG)",
    target: "FPS alvo",
    start: "Início (s)",
    end: "Fim (s)",
    static_ms: "Cena parada (ms)",
    stutter_factor: "Stutter (×)",
    optional: "opcional",
    analyze: "Analisar",
    cancel: "Cancelar",
    open_folder: "Abrir pasta",
    done: "Pronto.",
    processing: "Processando…",
    canceled: "Cancelado. Resultados parciais abaixo.",
    error: "Erro",
    choose_video_first: "Escolha um vídeo primeiro.",
    invalid_numbers: "Valores inválidos em FPS alvo, Início, Fim, Cena parada ou Stutter (mínimo 1).",
    unexpected_error: "Erro inesperado",
    videos: "Vídeos",
    all_files: "Todos os arquivos",
};

fn texts(lang: Lang) -> &'static Texts {
    match lang {
        Lang::En => &EN,
        Lang::Pt => &PT,
    }
}

/// Validated numeric inputs.
struct Inputs {
    target: f64,
    start: f64,
    end: Option<f64>,
    static_ms: f64,
    stutter_factor: f64,
}

enum Message {
    Progress(f32),
    Finished { report: String, canceled: bool },
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Idle,
    Working,
    Done,
    Canceled,
    Failed,
}

struct App {
    lang: Lang,
    video: Option<PathBuf>,
    overlay: bool,
    nvenc: bool,
    csv: bool,
    chart: bool,
    target: String,
    start: String,
    end: String,
    static_ms: String,
    stutter_factor: String,
    status: Status,
    progress: f32,
    report: String,
    cancel: Arc<AtomicBool>,
    messages: Option<Receiver<Message>>,
}

impl App {
    fn new() -> Self {
        Self {
            lang: Lang::detect(),
            video: None,
            overlay: true,
            nvenc: true,
            csv: true,
            chart: true,
            target: "30".into(),
            start: String::new(),
            end: String::new(),
            static_ms: format!("{}", video_fps_meter::DEFAULT_STATIC_MS),
            stutter_factor: format!("{}", video_fps_meter::DEFAULT_STUTTER_FACTOR),
            status: Status::Idle,
            progress: 0.0,
            report: String::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            messages: None,
        }
    }

    fn working(&self) -> bool {
        self.status == Status::Working
    }

    fn choose_video(&mut self) {
        let t = texts(self.lang);
        if let Some(path) = rfd::FileDialog::new()
            .add_filter(t.videos, &VIDEO_EXTENSIONS)
            .add_filter(t.all_files, &["*"])
            .pick_file()
        {
            self.video = Some(path);
        }
    }

    fn parse_inputs(&self) -> Option<Inputs> {
        let number = |text: &str| -> Result<Option<f64>, ()> {
            let text = text.trim().replace(',', ".");
            if text.is_empty() { Ok(None) } else { text.parse().map(Some).map_err(|_| ()) }
        };
        let target = number(&self.target).ok()?.unwrap_or(30.0);
        let start = number(&self.start).ok()?.unwrap_or(0.0);
        let end = number(&self.end).ok()?;
        let static_ms = number(&self.static_ms).ok()?.unwrap_or(0.0);
        let stutter_factor =
            number(&self.stutter_factor).ok()?.unwrap_or(video_fps_meter::DEFAULT_STUTTER_FACTOR);
        let valid = target > 0.0 && start >= 0.0 && static_ms >= 0.0 && end.is_none_or(|end| end > start);
        let valid = valid && stutter_factor >= 1.0;
        valid.then_some(Inputs { target, start, end, static_ms, stutter_factor })
    }

    fn start_analysis(&mut self, ctx: &egui::Context) {
        let t = texts(self.lang);
        let Some(video) = self.video.clone() else {
            warn(t.error, t.choose_video_first);
            return;
        };
        let Some(Inputs { target, start, end, static_ms, stutter_factor }) = self.parse_inputs() else {
            warn(t.error, t.invalid_numbers);
            return;
        };

        let options = Options {
            start_s: start,
            end_s: end,
            target_fps: target,
            static_ms,
            stutter_factor,
            overlay: self.overlay.then(|| OutputPaths::for_video(&video).overlay),
            nvenc: self.nvenc,
            lang: self.lang,
            ..Options::default()
        };
        let (csv, chart) = (self.csv, self.chart);
        let (sender, receiver) = channel();
        self.messages = Some(receiver);
        self.cancel.store(false, Ordering::Relaxed);
        self.status = Status::Working;
        self.progress = 0.0;
        self.report.clear();

        let cancel = Arc::clone(&self.cancel);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_analysis(&video, &options, csv, chart, &cancel, &sender, &ctx)
            }));
            if let Err(panic) = outcome {
                let detail = panic
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| panic.downcast_ref::<&str>().copied())
                    .unwrap_or_default();
                let _ = sender
                    .send(Message::Failed(format!("{}: {detail}", texts(options.lang).unexpected_error)));
            }
            ctx.request_repaint();
        });
    }

    fn poll_messages(&mut self) {
        let Some(receiver) = &self.messages else { return };
        let t = texts(self.lang);
        while let Ok(message) = receiver.try_recv() {
            match message {
                Message::Progress(fraction) => self.progress = fraction,
                Message::Finished { report, canceled } => {
                    self.report = report;
                    self.status = if canceled { Status::Canceled } else { Status::Done };
                    if !canceled {
                        self.progress = 1.0;
                    }
                }
                Message::Failed(error) => {
                    self.report = error.clone();
                    self.status = Status::Failed;
                    let shown: String = error.chars().take(600).collect();
                    rfd::MessageDialog::new()
                        .set_level(rfd::MessageLevel::Error)
                        .set_title(t.error)
                        .set_description(shown)
                        .show();
                }
            }
        }
        if !self.working() {
            self.messages = None;
        }
    }

    fn status_text(&self) -> String {
        let t = texts(self.lang);
        match self.status {
            Status::Idle => String::new(),
            Status::Working => format!("{} {:.0}%", t.processing, self.progress * 100.0),
            Status::Done => t.done.into(),
            Status::Canceled => t.canceled.into(),
            Status::Failed => t.error.into(),
        }
    }
}

fn run_analysis(
    video: &Path,
    options: &Options,
    csv: bool,
    chart: bool,
    cancel: &AtomicBool,
    sender: &Sender<Message>,
    ctx: &egui::Context,
) {
    let mut last_sent = -1.0f32;
    let result = analyze(video, options, cancel, |progress| {
        let fraction = progress.fraction as f32;
        if fraction - last_sent >= 0.005 || fraction >= 1.0 {
            last_sent = fraction;
            let _ = sender.send(Message::Progress(fraction));
            ctx.request_repaint();
        }
    });
    let message = match result {
        Ok(analysis) => {
            let mut lines = save_outputs(&analysis, video, csv, chart, options.lang);
            if let Some(overlay) = options.overlay.as_ref().filter(|p| p.exists()) {
                lines.push(format!("Overlay: {}", overlay.display()));
            }
            lines.extend(analysis.warnings.iter().cloned());
            let summary = summarize(&analysis, options.lang);
            Message::Finished {
                report: format!("{summary}\n\n{}", lines.join("\n")),
                canceled: analysis.canceled,
            }
        }
        Err(error) => Message::Failed(error.to_string()),
    };
    let _ = sender.send(message);
}

fn warn(title: &str, text: &str) {
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Warning)
        .set_title(title)
        .set_description(text)
        .show();
}

/// Lays out widgets right to left in a single row. Unlike `Ui::with_layout`,
/// the row takes only one line of height instead of all remaining space.
fn right_aligned_row<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(size, egui::Layout::right_to_left(egui::Align::Center), add_contents).inner
}

fn open_folder(dir: &Path) {
    #[cfg(windows)]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(any(windows, target_os = "macos")))]
    let program = "xdg-open";
    let _ = std::process::Command::new(program).arg(dir).spawn();
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_messages();
        let t = texts(self.lang);
        let working = self.working();
        let ctx = ui.ctx().clone();
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(t.title.into()));

        egui::CentralPanel::default().show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;

            ui.add_enabled_ui(!working, |ui| {
                right_aligned_row(ui, |ui| {
                    ui.selectable_value(&mut self.lang, Lang::En, "English");
                    ui.selectable_value(&mut self.lang, Lang::Pt, "Português");
                    ui.label(t.language);
                });
            });

            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.strong(t.video);
                    let name = self
                        .video
                        .as_ref()
                        .and_then(|p| p.file_name())
                        .map_or_else(|| t.no_video.to_owned(), |n| n.to_string_lossy().into_owned());
                    ui.weak(name);
                    right_aligned_row(ui, |ui| {
                        if ui.add_enabled(!working, egui::Button::new(t.choose)).clicked() {
                            self.choose_video();
                        }
                    });
                });
            });

            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add_enabled_ui(!working, |ui| {
                    ui.strong(t.outputs);
                    egui::Grid::new("outputs").num_columns(2).spacing([40.0, 6.0]).show(ui, |ui| {
                        ui.checkbox(&mut self.overlay, t.overlay);
                        ui.add_enabled(self.overlay, egui::Checkbox::new(&mut self.nvenc, t.nvenc));
                        ui.end_row();
                        ui.checkbox(&mut self.csv, t.csv);
                        ui.checkbox(&mut self.chart, t.chart);
                        ui.end_row();
                    });
                });
            });

            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.add_enabled_ui(!working, |ui| {
                    ui.columns(5, |columns| {
                        let fields = [
                            (t.target, &mut self.target, ""),
                            (t.start, &mut self.start, t.optional),
                            (t.end, &mut self.end, t.optional),
                            (t.static_ms, &mut self.static_ms, ""),
                            (t.stutter_factor, &mut self.stutter_factor, ""),
                        ];
                        for (column, (label, value, hint)) in columns.iter_mut().zip(fields) {
                            column.vertical_centered(|ui| {
                                ui.label(label);
                                ui.add(
                                    egui::TextEdit::singleline(value).hint_text(hint).desired_width(100.0),
                                );
                            });
                        }
                    });
                });
            });

            ui.horizontal(|ui| {
                let cancel_width = 110.0;
                let analyze_width = ui.available_width() - cancel_width - ui.spacing().item_spacing.x;
                let analyze = egui::Button::new(t.analyze).min_size(egui::vec2(analyze_width, 36.0));
                if ui.add_enabled(!working, analyze).clicked() {
                    self.start_analysis(&ctx);
                }
                let cancel = egui::Button::new(t.cancel).min_size(egui::vec2(cancel_width, 36.0));
                if ui.add_enabled(working, cancel).clicked() {
                    self.cancel.store(true, Ordering::Relaxed);
                }
            });

            ui.add(egui::ProgressBar::new(self.progress));
            ui.label(self.status_text());

            let can_open = matches!(self.status, Status::Done | Status::Canceled);
            let folder = self.video.as_ref().and_then(|p| p.parent()).map(Path::to_path_buf);
            let button_height = 36.0;
            let text_height = (ui.available_height() - button_height - 24.0).max(80.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(text_height).auto_shrink([false, false]).show(
                    ui,
                    |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.report.as_str())
                                .font(egui::TextStyle::Monospace)
                                .frame(egui::Frame::NONE)
                                .desired_width(f32::INFINITY),
                        );
                    },
                );
            });
            right_aligned_row(ui, |ui| {
                if ui.add_enabled(can_open, egui::Button::new(t.open_folder)).clicked()
                    && let Some(folder) = &folder
                {
                    open_folder(folder);
                }
            });
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([640.0, 620.0])
            .with_min_inner_size([560.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "video-fps-meter",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_theme(egui::Theme::Dark);
            Ok(Box::new(App::new()))
        }),
    )
}
