//! A separate observer can report stalled async workers before they resume.
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

struct State {
    stop: AtomicBool,
    heartbeat: AtomicU64,
    started: Instant,
}
pub struct Monitor(Arc<State>);
impl Drop for Monitor {
    fn drop(&mut self) {
        self.0.stop.store(true, Ordering::Relaxed);
    }
}

pub fn start(path: &Path) -> std::io::Result<Monitor> {
    let metrics = rrss_core::db::pool::metrics(path);
    let state = Arc::new(State {
        stop: AtomicBool::new(false),
        heartbeat: AtomicU64::new(0),
        started: Instant::now(),
    });
    let observer = state.clone();
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    std::thread::Builder::new()
        .name("rrss-latency".into())
        .spawn(move || {
            tracing::dispatcher::with_default(&dispatch, || {
                let mut last_report = Instant::now() - Duration::from_secs(1);
                while !observer.stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                    let gap = (observer.started.elapsed().as_millis() as u64)
                        .saturating_sub(observer.heartbeat.load(Ordering::Relaxed));
                    if let Some(metrics) = &metrics {
                        if gap >= 100 {
                            metrics
                                .runtime_stall_max_ms
                                .fetch_max(gap, Ordering::Relaxed);
                        }
                        if last_report.elapsed() >= Duration::from_secs(1) {
                            if gap >= 100 {
                                tracing::warn!(
                                    error_kind = "executor_stall",
                                    elapsed_ms = gap,
                                    "async executor heartbeat delayed"
                                );
                            }
                            for sample in metrics
                                .activity
                                .snapshot()
                                .active
                                .into_iter()
                                .filter(|s| s.elapsed_ms >= 100.0)
                            {
                                tracing::warn!(
                                    error_kind = "slow_stage",
                                    operation = sample.operation,
                                    stage = sample.stage,
                                    elapsed_ms = sample.elapsed_ms,
                                    "database stage still running"
                                );
                            }
                            last_report = Instant::now();
                        }
                    }
                }
            });
        })?;
    let pulse = state.clone();
    tokio::spawn(async move {
        while !pulse.stop.load(Ordering::Relaxed) {
            pulse.heartbeat.store(
                pulse.started.elapsed().as_millis() as u64,
                Ordering::Relaxed,
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    Ok(Monitor(state))
}

#[cfg(test)]
mod tests {
    #[tokio::test(flavor = "current_thread")]
    async fn independent_observer_sees_a_blocked_async_worker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fixture.db");
        drop(rrss_core::db::open_at(&path).unwrap());
        let _monitor = super::start(&path).unwrap();
        tokio::task::yield_now().await;
        std::thread::sleep(std::time::Duration::from_millis(350));
        assert!(
            rrss_core::db::pool::metrics(&path)
                .unwrap()
                .runtime_stall_max_ms
                .load(std::sync::atomic::Ordering::Relaxed)
                >= 100
        );
    }
}
