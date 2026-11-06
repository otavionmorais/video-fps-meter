//! Locating and spawning the FFmpeg command-line tools.

use std::io::Read;
use std::path::PathBuf;
use std::process::{ChildStderr, Command, Stdio};
use std::thread::{self, JoinHandle};

/// Resolves an FFmpeg tool, preferring a copy next to the running executable
/// over the one on `PATH`.
pub fn tool_path(name: &str) -> PathBuf {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    if let Some(local) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&file)))
        .filter(|path| path.is_file())
    {
        return local;
    }
    PathBuf::from(name)
}

/// Builds a command for an FFmpeg tool that never opens a console window.
pub fn command(name: &str) -> Command {
    let mut cmd = Command::new(tool_path(name));
    cmd.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Whether the tool can be executed.
pub fn is_available(name: &str) -> bool {
    command(name)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Drains a child's stderr on a background thread so the child never blocks
/// on a full pipe.
pub fn collect_stderr(mut stderr: ChildStderr) -> JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        String::from_utf8_lossy(&bytes).trim().to_owned()
    })
}

/// Last `max_chars` characters of a message.
pub fn tail(text: &str, max_chars: usize) -> &str {
    let count = text.chars().count();
    if count <= max_chars {
        return text;
    }
    let start = text.char_indices().nth(count - max_chars).map_or(0, |(i, _)| i);
    &text[start..]
}
