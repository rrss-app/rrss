//! One privacy boundary for string parameters. Never pass URLs or full paths.
use rrss_core::logsafe;
use std::path::Path;

pub(super) fn name(value: &str) -> String {
    logsafe::text(value, 200)
        .split_whitespace()
        .map(|part| {
            if part.contains("://") {
                logsafe::host_str(part)
            } else if part.starts_with(['/', '\\']) || part.as_bytes().get(1) == Some(&b':') {
                part.rsplit(['/', '\\'])
                    .next()
                    .unwrap_or_default()
                    .to_string()
            } else {
                part.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
pub(super) fn options(values: &[String]) -> String {
    values
        .iter()
        .map(|value| name(value))
        .collect::<Vec<_>>()
        .join(" / ")
}
pub(super) fn host(value: &str) -> String {
    logsafe::host_str(value)
}
pub(super) fn file_name(value: &Path) -> Option<String> {
    value.file_name().map(|name| {
        // Also handle a foreign-platform path in diagnostic input.
        let name = name.to_string_lossy();
        self::name(name.rsplit(['/', '\\']).next().unwrap_or_default())
    })
}

// Reason content may deliberately contain whitespace. Preserve it, while using
// the same privacy projection for URLs and credentials as other UI parameters.
pub(crate) fn reason_text(value: &str) -> String {
    rrss_core::rank::reasons::safe_text(value)
}

pub(crate) fn reason_legacy_text(value: &str) -> String {
    // Migration content is already displayed data, never diagnostic input.
    // Query stripping would change the old text; keep this out of error parameters.
    value.to_string()
}
