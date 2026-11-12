//! Local frontend diagnostics. Do not route logging failures back to the UI.
use crate::views::ClientLogLevel;
use std::{
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(60);
const LIMIT: usize = 30;
const ENTRY_BYTES: usize = 2048;

struct Limiter {
    since: Instant,
    accepted: usize,
    dropped: u64,
}
impl Limiter {
    fn new(now: Instant) -> Self {
        Self {
            since: now,
            accepted: 0,
            dropped: 0,
        }
    }
    fn accept(&mut self, now: Instant) -> bool {
        if now.saturating_duration_since(self.since) >= WINDOW {
            self.since = now;
            self.accepted = 0;
        }
        if self.accepted >= LIMIT {
            self.dropped += 1;
            false
        } else {
            self.accepted += 1;
            true
        }
    }
}

fn bounded(value: &str, remaining: &mut usize) -> String {
    // Limit work before sanitizing. An oversized field is omitted whole so a
    // credential crossing the limit cannot leave an unredacted prefix.
    let safe = if value.len() > 16 * 1024 {
        "<oversized field omitted>".into()
    } else {
        rrss_core::logsafe::text(value, ENTRY_BYTES)
    };
    let mut end = safe.len().min(*remaining);
    while !safe.is_char_boundary(end) {
        end -= 1;
    }
    *remaining -= end;
    safe[..end].to_owned()
}

fn fields(scope: &str, message: &str, stack: Option<&str>) -> (String, String, String) {
    // Leave room for timestamp, target and field names in the formatted event.
    let mut remaining = ENTRY_BYTES - 256;
    let scope = bounded(scope, &mut 128);
    remaining -= scope.len();
    let message = bounded(message, &mut remaining);
    let stack = bounded(stack.unwrap_or(""), &mut remaining);
    (scope, message, stack)
}

pub fn record(level: ClientLogLevel, scope: &str, message: &str, stack: Option<&str>) {
    static LIMITER: OnceLock<Mutex<Limiter>> = OnceLock::new();
    let now = Instant::now();
    let mut limiter = LIMITER
        .get_or_init(|| Mutex::new(Limiter::new(now)))
        .lock()
        .unwrap();
    if !limiter.accept(now) {
        return;
    }
    let dropped = limiter.dropped;
    drop(limiter);
    let (scope, message, stack) = fields(scope, message, stack);
    match level {
        ClientLogLevel::Warn => {
            tracing::warn!(target: "rrss_ui", scope, error = %message, stack, dropped, "frontend warning")
        }
        ClientLogLevel::Error => {
            tracing::error!(target: "rrss_ui", scope, error = %message, stack, dropped, "frontend error")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frontend_limit_resets_and_counts_drops() {
        let now = Instant::now();
        let mut limiter = Limiter::new(now);
        for _ in 0..LIMIT {
            assert!(limiter.accept(now));
        }
        assert!(!limiter.accept(now));
        assert!(!limiter.accept(now));
        assert!(limiter.accept(now + WINDOW));
        assert_eq!(limiter.dropped, 2);
    }
    #[test]
    fn frontend_fields_are_bounded_and_scrubbed_before_truncation() {
        let (scope, message, stack) = fields(
            "reader",
            "error sk-canary-token https://user:pass@example.test/a?token=canary-query",
            Some(&"汉".repeat(3000)),
        );
        assert!(scope.len() + message.len() + stack.len() <= ENTRY_BYTES);
        assert!(!message.contains("canary") && !message.contains("user:pass"));
        assert_eq!(
            fields("x", &"secret".repeat(10000), None).1,
            "<oversized field omitted>"
        );
    }
}
