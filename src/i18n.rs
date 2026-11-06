//! User-facing strings in English and Portuguese.

/// Interface language.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lang {
    #[default]
    En,
    Pt,
}

impl Lang {
    /// Picks Portuguese when the operating system locale is Portuguese,
    /// English otherwise.
    pub fn detect() -> Self {
        match sys_locale::get_locale() {
            Some(locale) if locale.to_ascii_lowercase().starts_with("pt") => Lang::Pt,
            _ => Lang::En,
        }
    }

    pub fn strings(self) -> &'static Strings {
        match self {
            Lang::En => &EN,
            Lang::Pt => &PT,
        }
    }
}

/// Strings used by the analysis engine, summaries and output files.
pub struct Strings {
    pub overlay_fps: &'static str,
    pub overlay_frametime: &'static str,
    pub chart_time: &'static str,
    pub no_frames: &'static str,
    pub capture_rate: &'static str,
    pub analyzed_span: &'static str,
    pub frames: &'static str,
    pub average_fps: &'static str,
    pub typical_frametime: &'static str,
    pub worst_frametime: &'static str,
    pub one_percent_low: &'static str,
    pub changed_pixels: &'static str,
    pub min: &'static str,
    pub median: &'static str,
    pub max: &'static str,
    pub open_failed: &'static str,
    pub frame_rate_failed: &'static str,
    pub ffmpeg_missing: &'static str,
    pub save_failed: &'static str,
    pub nvenc_unavailable: &'static str,
    pub encode_failed: &'static str,
    pub static_excluded: &'static str,
    pub interval: &'static str,
    pub intervals: &'static str,
    pub longest: &'static str,
    pub at: &'static str,
    pub stutters: &'static str,
    pub worst: &'static str,
    pub active_time: &'static str,
    pub overlay_average: &'static str,
    pub overlay_stutters: &'static str,
    pub generating_overlay: &'static str,
}

static EN: Strings = Strings {
    overlay_fps: "Framerate (fps)",
    overlay_frametime: "Frametime (ms)",
    chart_time: "Time (s)",
    no_frames: "No frames analyzed.",
    capture_rate: "Capture rate",
    analyzed_span: "Analyzed span",
    frames: "frames",
    average_fps: "Real average FPS",
    typical_frametime: "Typical frametime",
    worst_frametime: "Worst frametime",
    one_percent_low: "1% low (FPS)",
    changed_pixels: "Changed pixels between frames",
    min: "min",
    median: "median",
    max: "max",
    open_failed: "Could not open video",
    frame_rate_failed: "Could not read the video frame rate.",
    ffmpeg_missing: "FFmpeg not found. Install it (Windows: winget install --id Gyan.FFmpeg -e) \
                     or place ffmpeg and ffprobe next to this program.",
    save_failed: "ERROR ({}) — is the file open in another program?",
    nvenc_unavailable: "Warning: NVENC unavailable; the overlay was encoded on the CPU (x264).",
    encode_failed: "FFmpeg stopped accepting frames while generating the overlay.",
    static_excluded: "Excluded as static (menus, pauses)",
    interval: "interval",
    intervals: "intervals",
    longest: "longest",
    at: "at",
    stutters: "Stutters",
    worst: "worst",
    active_time: "active",
    overlay_average: "Avg",
    overlay_stutters: "Stutters",
    generating_overlay: "Generating overlay…",
};

static PT: Strings = Strings {
    overlay_fps: "Taxa de quadros (fps)",
    overlay_frametime: "Tempo de quadro (ms)",
    chart_time: "Tempo (s)",
    no_frames: "Nenhum quadro analisado.",
    capture_rate: "Taxa da captura",
    analyzed_span: "Trecho analisado",
    frames: "quadros",
    average_fps: "FPS médio real",
    typical_frametime: "Frametime típico",
    worst_frametime: "Pior frametime",
    one_percent_low: "1% low (FPS)",
    changed_pixels: "Pixels mudados entre quadros",
    min: "mín",
    median: "mediana",
    max: "máx",
    open_failed: "Não foi possível abrir o vídeo",
    frame_rate_failed: "Não foi possível ler a taxa de quadros do vídeo.",
    ffmpeg_missing: "FFmpeg não encontrado. Instale (Windows: winget install --id Gyan.FFmpeg -e) \
                     ou coloque ffmpeg e ffprobe na mesma pasta deste programa.",
    save_failed: "ERRO ({}) — o arquivo está aberto em outro programa?",
    nvenc_unavailable: "Aviso: NVENC indisponível; o overlay foi codificado no processador (x264).",
    encode_failed: "O FFmpeg parou de aceitar quadros ao gerar o overlay.",
    static_excluded: "Excluído como cena parada (menus, pausas)",
    interval: "intervalo",
    intervals: "intervalos",
    longest: "maior",
    at: "em",
    stutters: "Stutters",
    worst: "pior",
    active_time: "ativos",
    overlay_average: "Média",
    overlay_stutters: "Stutters",
    generating_overlay: "Gerando overlay…",
};
