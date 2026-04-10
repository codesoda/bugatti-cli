use std::io::IsTerminal;
use std::sync::OnceLock;

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

fn detect_color_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

fn build_colors() -> Colors {
    let enabled = detect_color_enabled();
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
    COLORS.get_or_init(build_colors)
}

/// Returns whether ANSI color output should be enabled.
///
/// Color is disabled when:
/// - `NO_COLOR` is set to any value
/// - stdout is not a terminal (e.g. piped/redirected)
pub fn color_enabled() -> bool {
    colors().enabled
}

/// Returns `code` when color is enabled, otherwise an empty string.
pub fn ansi(code: &'static str) -> &'static str {
    if colors().enabled {
        code
    } else {
        ""
    }
}

pub mod prelude {
    pub use super::{ansi, color_enabled, colors, Colors};
}
