use super::StepVerdict;

/// The prefix that marks a log event line in provider output.
const BUGATTI_LOG_PREFIX: &str = "BUGATTI_LOG ";

/// A log event parsed from provider output during step execution.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEvent {
    /// The run this log event belongs to.
    pub run_id: String,
    /// The step that produced this log event.
    pub step_id: usize,
    /// The log message text.
    pub message: String,
}

/// Parse BUGATTI_LOG lines from text, returning extracted log events.
///
/// Lines matching 'BUGATTI_LOG <message>' are recognized.
/// Each matching line produces a LogEvent with the given run_id and step_id.
pub fn parse_log_events(text: &str, run_id: &str, step_id: usize) -> Vec<LogEvent> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            trimmed
                .strip_prefix(BUGATTI_LOG_PREFIX)
                .map(|msg| LogEvent {
                    run_id: run_id.to_string(),
                    step_id,
                    message: msg.to_string(),
                })
        })
        .collect()
}

/// Parse the RESULT contract marker from accumulated output text.
///
/// Scans from the end of the text for the last occurrence of a RESULT marker.
///
/// Matches:
///   RESULT OK
///   RESULT WARN: <message>
///   RESULT ERROR: <message>
///
/// The marker can appear on its own line or embedded at the end of a line
/// (agents sometimes omit the newline before the marker).
pub fn parse_result_marker(text: &str) -> Option<StepVerdict> {
    // First try line-based scan (most common case)
    for line in text.lines().rev() {
        let trimmed = line.trim();
        if let Some(verdict) = try_parse_result_line(trimmed) {
            return Some(verdict);
        }
    }

    // Fallback: scan for RESULT marker embedded anywhere in text.
    // Find the last occurrence and parse from there.
    let mut pos = text.len();
    while pos > 0 {
        if let Some(idx) = text[..pos].rfind("RESULT ") {
            let from_marker = &text[idx..];
            // Take up to the next newline (or end of string)
            let end = from_marker.find('\n').unwrap_or(from_marker.len());
            let candidate = from_marker[..end].trim();
            if let Some(verdict) = try_parse_result_line(candidate) {
                return Some(verdict);
            }
            pos = idx;
        } else {
            break;
        }
    }

    None
}

fn try_parse_result_line(line: &str) -> Option<StepVerdict> {
    let rest = line.strip_prefix("RESULT")?;

    if rest.is_empty() {
        return None;
    }

    let rest = rest.trim_start();

    if rest == "OK" {
        return Some(StepVerdict::Ok);
    }

    if let Some(msg) = rest.strip_prefix("WARN:") {
        return Some(StepVerdict::Warn(msg.trim().to_string()));
    }

    if let Some(msg) = rest.strip_prefix("ERROR:") {
        return Some(StepVerdict::Error(msg.trim().to_string()));
    }

    None
}
