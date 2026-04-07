use std::io::IsTerminal;

/// Returns whether ANSI color output should be enabled.
///
/// Color is disabled when:
/// - `NO_COLOR` is set to any value
/// - stdout is not a terminal (e.g. piped/redirected)
pub fn color_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal()
}

/// Returns `code` when color is enabled, otherwise an empty string.
pub fn ansi(code: &'static str) -> &'static str {
    if color_enabled() {
        code
    } else {
        ""
    }
}
