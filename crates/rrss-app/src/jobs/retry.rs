//! Bounded, process-local input failures. Chunk attempt limits remain in SQLite;
//! summary/text retries are reconstructed conservatively after startup.
use std::collections::HashMap;
use tokio::time::Instant;

pub(crate) const DELAY: std::time::Duration = std::time::Duration::from_secs(5 * 60);
const LIMIT: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Kind {
    Summary,
    Article,
    Interest,
    Memory,
    RebuildChunk,
    RebuildInterest,
    RebuildMemory,
}

impl Kind {
    pub(crate) fn rebuild(kind: &str) -> Self {
        match kind {
            "chunk" => Self::RebuildChunk,
            "interest" => Self::RebuildInterest,
            "memory" => Self::RebuildMemory,
            _ => unreachable!("known vector input kind"),
        }
    }
}

#[derive(Clone)]
struct Failure {
    input: String,
    attempts: u8,
    until: Instant,
}

/// Applied only after the vector transaction commits.
pub(crate) enum Update {
    Success {
        kind: Kind,
        id: i64,
    },
    Failed {
        kind: Kind,
        id: i64,
        input: String,
        terminal: bool,
    },
}

#[derive(Default, Clone)]
pub(crate) struct Retries {
    contexts: HashMap<Kind, String>,
    failures: HashMap<(Kind, i64), Failure>,
    overflow: Option<Instant>,
}

impl Retries {
    pub fn apply(&mut self, updates: Vec<Update>) {
        for update in updates {
            match update {
                Update::Success { kind, id } => self.success(kind, id),
                Update::Failed {
                    kind,
                    id,
                    input,
                    terminal,
                } => self.failed(kind, id, input, terminal),
            }
        }
    }
    pub fn reset_embeddings(&mut self) {
        self.failures.retain(|(kind, _), _| *kind == Kind::Summary);
        self.overflow = None;
    }
    pub fn context(&mut self, kind: Kind, context: &str) {
        self.failures
            .retain(|_, failure| failure.attempts > 0 || failure.until > Instant::now());
        if self.contexts.get(&kind).is_some_and(|old| old != context) {
            self.failures.retain(|(stored, _), _| *stored != kind);
        }
        self.contexts.insert(kind, context.into());
    }

    pub fn allowed(&self, kind: Kind, id: i64, input: &str) -> bool {
        if self.overflow.is_some_and(|until| until > Instant::now()) {
            return false;
        }
        self.failures.get(&(kind, id)).is_none_or(|failure| {
            failure.input != input || (failure.attempts < 3 && failure.until <= Instant::now())
        })
    }

    pub fn excluded(&self, kind: Kind) -> Vec<(i64, String)> {
        self.failures
            .iter()
            .filter(|((stored, id), failure)| {
                *stored == kind && !self.allowed(kind, *id, &failure.input)
            })
            .map(|((_, id), failure)| (*id, failure.input.clone()))
            .collect()
    }

    pub fn failed(&mut self, kind: Kind, id: i64, input: String, terminal: bool) {
        let now = Instant::now();
        if !self.failures.contains_key(&(kind, id)) && self.failures.len() >= LIMIT {
            // Do not evict a still-cooled input to make it eligible immediately.
            self.overflow = Some(now + DELAY);
            return;
        }
        let failure = self.failures.entry((kind, id)).or_insert(Failure {
            input: input.clone(),
            attempts: 0,
            until: now,
        });
        if failure.input != input {
            failure.attempts = 0;
        }
        failure.input = input;
        failure.attempts = if terminal {
            failure.attempts.saturating_add(1)
        } else {
            0
        };
        failure.until = now + DELAY;
    }

    pub fn success(&mut self, kind: Kind, id: i64) {
        self.failures.remove(&(kind, id));
    }

    /// Check a current input, not historical failures: deleted/disabled input
    /// must not keep the scheduler alive after the real backlog is empty.
    pub fn retryable(&self, kind: Kind, id: i64, input: &str) -> bool {
        self.failures
            .get(&(kind, id))
            .is_none_or(|failure| failure.input != input || failure.attempts < 3)
    }
}

/// Service/configuration failures are not evidence of a bad article or chunk.
pub(crate) fn input_failure(error: &anyhow::Error) -> bool {
    fn invalid(error: &rrss_ai::Error) -> bool {
        match error {
            rrss_ai::Error::Http { status, .. } => matches!(status, 400 | 413 | 422),
            rrss_ai::Error::EmptyContent
            | rrss_ai::Error::EmptyEmbedding
            | rrss_ai::Error::InvalidJson(_) => true,
            rrss_ai::Error::Content { source, .. } => invalid(source),
            _ => false,
        }
    }
    error.downcast_ref::<rrss_ai::Error>().is_some_and(invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn deterministic_input_retries_are_bounded_and_new_versions_are_eligible() {
        let mut retries = Retries::default();
        retries.context(Kind::Summary, "model-a");
        for _ in 0..3 {
            retries.failed(Kind::Summary, 1, "input-a".into(), true);
            assert!(!retries.allowed(Kind::Summary, 1, "input-a"));
            assert!(retries.allowed(Kind::Summary, 2, "input-a"));
            tokio::time::advance(DELAY).await;
        }
        assert!(!retries.allowed(Kind::Summary, 1, "input-a"));
        assert!(retries.allowed(Kind::Summary, 1, "input-b"));
        assert!(!retries.retryable(Kind::Summary, 1, "input-a"));
        retries.context(Kind::Summary, "model-b");
        assert!(retries.allowed(Kind::Summary, 1, "input-a"));
    }

    #[test]
    fn transient_and_auth_failures_do_not_consume_input_attempts() {
        for error in [
            rrss_ai::Error::ServiceCooldown,
            rrss_ai::Error::BudgetExhausted,
            rrss_ai::Error::Http {
                status: 500,
                message: "test".into(),
            },
            rrss_ai::Error::Http {
                status: 401,
                message: "test".into(),
            },
            rrss_ai::Error::Request {
                kind: rrss_ai::complete::RequestKind::Timeout,
            },
        ] {
            assert!(!input_failure(&error.into()));
        }
    }
}
