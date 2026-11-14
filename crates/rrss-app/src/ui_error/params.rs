//! One privacy boundary for string parameters. Never pass URLs or full paths.
use rrss_core::logsafe;
use std::path::Path;

pub(super) fn name(value: &str) -> String {
    logsafe::text(value, 200)
        .split_whitespace()
        .map(|part| {
            if part.contains("://") {
                logsafe::host_str(part)
            } else {
                part.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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
