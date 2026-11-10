//! Measures the real frame rate of a game or other rendered content from a
//! video recording.
//!
//! The recording runs at a fixed rate (for example 30 or 60 Hz). When the
//! source fails to deliver a new image in time, the previous one is shown
//! again and appears as a repeated frame. Each captured frame is compared
//! with the previous one; frames whose number of changed pixels exceeds a
//! threshold count as new images. Static intervals (menus, pauses) are
//! excluded from the statistics and frametime spikes are reported as stutters.

pub mod analysis;
pub mod chart;
pub mod decode;
pub mod encode;
pub mod ffmpeg;
pub mod i18n;
pub mod metric;
pub mod output;
pub mod overlay;
pub mod probe;
pub mod stats;
pub mod summary;
pub mod timeline;
pub mod yuv;

pub use analysis::{Analysis, Options, Progress, Row, Stage, analyze};
pub use encode::OverlayQuality;
pub use i18n::Lang;
pub use output::{OutputPaths, save_outputs};
pub use overlay::OverlayElements;
pub use stats::Stats;
pub use summary::summarize;
pub use timeline::{DEFAULT_STATIC_MS, DEFAULT_STUTTER_FACTOR, StaticInterval, Stutter};

/// Error meant to be shown to the user. The message is already localized.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Error(pub String);
