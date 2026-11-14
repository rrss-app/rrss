use crate::views::UiError;
use std::time::{Duration, Instant};

/// Record the command name and result only; never its input arguments.
pub struct CommandLog {
    name: &'static str,
    started: Instant,
    span: tracing::Span,
}
impl CommandLog {
    pub fn start(name: &'static str) -> Self {
        tracing::debug!(command = name, "command started");
        Self {
            name,
            started: Instant::now(),
            // 级别只决定开关：与 warn 事件同样默认开启。结束事件在 span 外，自带命令名。
            span: tracing::span!(parent: None, tracing::Level::WARN, "command", command = name),
        }
    }
    /// 命令执行期间的调用链；跨任务或阻塞线程时由调用方带过去。
    pub fn span(&self) -> &tracing::Span {
        &self.span
    }
    pub fn finish<T>(self, result: &Result<T, UiError>) {
        let command = self.name;
        let elapsed = self.started.elapsed();
        let elapsed_ms = elapsed.as_millis() as u64;
        if let Err(error) = result {
            tracing::info!(command, elapsed_ms, code = error.code(), detail = %error.log_detail(), "command failed");
        }
        if elapsed > Duration::from_secs(1) {
            tracing::info!(command, elapsed_ms, "slow command");
        } else {
            tracing::debug!(command, elapsed_ms, "command finished");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn events_inside_a_command_carry_its_name_once() {
        let dir = tempfile::tempdir().unwrap();
        let (subscriber, handle) = crate::log::subscriber(
            crate::log::Mode::Desktop {
                log_dir: dir.path().into(),
            },
            "",
        );
        tracing::subscriber::with_default(subscriber, || {
            let log = CommandLog::start("fixture_command");
            log.span().in_scope(|| {
                tracing::warn!(error_kind = "fixture", "inner failure");
                tracing::warn!(
                    command = rrss_core::logsafe::unless_in_span("command", "fixture_command"),
                    error_kind = "fixture",
                    "inner with name"
                );
            });
            log.finish(&Err::<(), _>(UiError::from("failed".to_string())));
        });
        handle.end();
        let log = std::fs::read_to_string(dir.path().join("rrss.log")).unwrap();
        for needle in ["inner failure", "inner with name", "command failed"] {
            let line = log.lines().find(|line| line.contains(needle)).unwrap();
            assert_eq!(line.matches("command=fixture_command").count(), 1, "{line}");
        }
    }

    #[test]
    fn command_failures_are_scrubbed_and_slow_calls_are_visible() {
        let dir = tempfile::tempdir().unwrap();
        let (subscriber, handle) = crate::log::subscriber(
            crate::log::Mode::Desktop {
                log_dir: dir.path().into(),
            },
            "",
        );
        tracing::subscriber::with_default(subscriber, || {
            CommandLog {
                name: "fixture",
                started: Instant::now() - Duration::from_secs(2),
                span: tracing::Span::none(),
            }
            .finish(&Err::<(), _>(UiError::from(anyhow::anyhow!(
                "failed https://user:pass@example.test/a?token=canary-token sk-canary-key"
            ))));
        });
        handle.end();
        let log = std::fs::read_to_string(dir.path().join("rrss.log")).unwrap();
        assert!(log.contains("command failed") && log.contains("slow command"));
        assert!(!log.contains("canary-") && !log.contains("user:pass"));
        assert!(log.contains("code=legacy_text") && log.contains("detail="));
        assert!(!log.contains("WARN") && !log.contains("ERROR"));
    }
}
