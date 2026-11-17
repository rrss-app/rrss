//! Bounded, in-memory database activity; never retains SQL, values or database paths.
use std::{
    cell::Cell,
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const SLOW: Duration = Duration::from_millis(100);

pub fn stage(conn: &rusqlite::Connection, name: &'static str) -> Option<Guard> {
    super::metrics::for_connection(conn).map(|metrics| metrics.activity.start(name))
}

/// CPU/file stages share the caller's span, never record input data.
pub fn measure<T>(stage: &'static str, work: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    let result = work();
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    if started.elapsed() >= SLOW {
        tracing::warn!(
            stage,
            elapsed_ms,
            error_kind = "slow_stage",
            "slow backend stage"
        );
    } else {
        tracing::debug!(stage, elapsed_ms, "backend stage");
    }
    result
}
thread_local! { static OPERATION: Cell<&'static str> = const { Cell::new("database") }; }

pub fn scope<T>(name: &'static str, work: impl FnOnce() -> T) -> T {
    struct Restore(&'static str);
    impl Drop for Restore {
        fn drop(&mut self) {
            OPERATION.set(self.0);
        }
    }
    let _restore = Restore(OPERATION.replace(name));
    work()
}

#[derive(Clone, serde::Serialize)]
pub struct Sample {
    pub operation: &'static str,
    pub stage: &'static str,
    pub elapsed_ms: f64,
}

struct Active {
    operation: &'static str,
    stage: &'static str,
    started: Instant,
}
#[derive(Default)]
struct State {
    next: u64,
    active: BTreeMap<u64, Active>,
    recent: VecDeque<Sample>,
    slow_count: u64,
}
#[derive(Default)]
pub struct Activity(Mutex<State>);

#[derive(serde::Serialize)]
pub struct Snapshot {
    pub active: Vec<Sample>,
    pub recent_slow: Vec<Sample>,
    pub slow_stage_count: u64,
}

pub struct Guard {
    activity: Arc<Activity>,
    id: Option<u64>,
    operation: &'static str,
    stage: &'static str,
    started: Instant,
    span: tracing::Span,
}

impl Activity {
    pub fn start(self: &Arc<Self>, stage: &'static str) -> Guard {
        let operation = OPERATION.get();
        let started = Instant::now();
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let id = if state.active.len() < 128 {
            let id = state.next;
            state.next = state.next.wrapping_add(1);
            state.active.insert(
                id,
                Active {
                    operation,
                    stage,
                    started,
                },
            );
            Some(id)
        } else {
            None
        };
        Guard {
            activity: self.clone(),
            id,
            operation,
            stage,
            started,
            span: tracing::Span::current(),
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        Snapshot {
            active: state
                .active
                .values()
                .map(|item| Sample {
                    operation: item.operation,
                    stage: item.stage,
                    elapsed_ms: item.started.elapsed().as_secs_f64() * 1000.0,
                })
                .collect(),
            recent_slow: state.recent.iter().cloned().collect(),
            slow_stage_count: state.slow_count,
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed();
        let mut state = self.activity.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(id) = self.id {
            state.active.remove(&id);
        }
        if elapsed >= SLOW {
            state.slow_count += 1;
            if state.recent.len() == 128 {
                state.recent.pop_front();
            }
            let sample = Sample {
                operation: self.operation,
                stage: self.stage,
                elapsed_ms: elapsed.as_secs_f64() * 1000.0,
            };
            state.recent.push_back(sample.clone());
            drop(state);
            self.span.in_scope(|| {
                tracing::warn!(
                    error_kind = "slow_stage",
                    operation = sample.operation,
                    stage = sample.stage,
                    elapsed_ms = sample.elapsed_ms,
                    "slow database stage"
                )
            });
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn activity_tracks_inflight_and_rollback_without_sql_or_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.db");
        let conn = crate::db::open_at(&path).unwrap();
        let metrics = crate::db::pool::metrics(&path).unwrap();
        super::scope("fixture_write", || {
            let tx = crate::db::write_transaction(&conn).unwrap();
            crate::db::set_setting(&tx, "private_fixture_key", "private_fixture_value").unwrap();
            let snapshot = metrics.activity.snapshot();
            assert!(
                snapshot
                    .active
                    .iter()
                    .any(|item| item.operation == "fixture_write"
                        && item.stage == "write_transaction")
            );
            let encoded = serde_json::to_string(&snapshot).unwrap();
            assert!(!encoded.contains("private_fixture"));
            drop(tx);
        });
        assert!(
            !metrics
                .activity
                .snapshot()
                .active
                .iter()
                .any(|item| item.stage == "write_transaction")
        );
        assert!(
            crate::db::get_setting(&conn, "private_fixture_key")
                .unwrap()
                .is_none()
        );
    }
}
