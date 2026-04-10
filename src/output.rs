use std::io::IsTerminal;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Shared ANSI palette used by terminal output formatting.
#[derive(Debug)]
pub struct Colors {
    pub enabled: bool,
    pub bold: &'static str,
    pub dim: &'static str,
    pub light: &'static str,
    pub tool: &'static str,
    pub thinking: &'static str,
    pub result: &'static str,
    pub prompt: &'static str,
    pub cmd: &'static str,
    pub sep: &'static str,
    pub reset: &'static str,
}

static COLORS: OnceLock<Colors> = OnceLock::new();
static STDERR_COLORS: OnceLock<Colors> = OnceLock::new();

fn detect_color_enabled(stream: Stream) -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }

    match stream {
        Stream::Stdout => std::io::stdout().is_terminal(),
        Stream::Stderr => std::io::stderr().is_terminal(),
    }
}

fn build_colors(enabled: bool) -> Colors {
    let code = |value| if enabled { value } else { "" };

    Colors {
        enabled,
        bold: code("\x1b[1m"),
        dim: code("\x1b[38;5;243m"),
        light: code("\x1b[38;5;250m"),
        tool: code("\x1b[38;5;111m"),
        thinking: code("\x1b[38;5;183m"),
        result: code("\x1b[38;5;151m"),
        prompt: code("\x1b[38;5;223m"),
        cmd: code("\x1b[38;5;152m"),
        sep: code("\x1b[38;5;238m"),
        reset: code("\x1b[0m"),
    }
}

/// Returns a lazily initialized singleton color palette.
pub fn colors() -> &'static Colors {
    stdout_colors()
}

/// Returns a lazily initialized stdout color palette.
pub fn stdout_colors() -> &'static Colors {
    COLORS.get_or_init(|| build_colors(detect_color_enabled(Stream::Stdout)))
}

/// Returns a lazily initialized stderr color palette.
pub fn stderr_colors() -> &'static Colors {
    STDERR_COLORS.get_or_init(|| build_colors(detect_color_enabled(Stream::Stderr)))
}

/// Returns whether ANSI color output should be enabled.
///
/// Color is disabled when:
/// - `NO_COLOR` is set to any value
/// - stdout is not a terminal (e.g. piped/redirected)
pub fn color_enabled() -> bool {
    stdout_colors().enabled
}

/// Returns whether ANSI color output should be enabled for stderr.
pub fn color_enabled_stderr() -> bool {
    stderr_colors().enabled
}

/// Returns `code` when color is enabled, otherwise an empty string.
pub fn ansi(code: &'static str) -> &'static str {
    if stdout_colors().enabled {
        code
    } else {
        ""
    }
}

/// Returns `code` when stderr color is enabled, otherwise an empty string.
pub fn ansi_stderr(code: &'static str) -> &'static str {
    if stderr_colors().enabled {
        code
    } else {
        ""
    }
}

pub mod prelude {
    pub use super::{
        ansi, ansi_stderr, color_enabled, color_enabled_stderr, colors, stderr_colors,
        stdout_colors, Colors, Stream,
    };
}
