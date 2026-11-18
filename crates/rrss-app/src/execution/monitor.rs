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

/// A slowdown is reported once it has lasted this long, and again when it ends.
pub(crate) const STALL_AFTER: Duration = Duration::from_secs(2);

/// One warn when a measurement first reaches [`STALL_AFTER`], one info with the
/// observed peak when it drops back; nothing in between, so a long stall never repeats.
#[derive(Default)]
pub(crate) struct Sustained {
    peak: Option<Duration>,
}

pub(crate) enum Change {
    Began,
    Ended(Duration),
}

impl Sustained {
    pub(crate) const fn new() -> Self {
        Self { peak: None }
    }

    pub(crate) fn observe(&mut self, value: Duration) -> Option<Change> {
        if value < STALL_AFTER {
            return self.peak.take().map(Change::Ended);
        }
        match &mut self.peak {
            Some(peak) => {
                *peak = (*peak).max(value);
                None
            }
            None => {
                self.peak = Some(value);
                Some(Change::Began)
            }
        }
    }
}

fn heartbeat(watch: &mut Sustained, gap: Duration) {
    match watch.observe(gap) {
        Some(Change::Began) => tracing::warn!(
            error_kind = "executor_stall",
            elapsed_ms = gap.as_millis() as u64,
            "async executor heartbeat delayed"
        ),
        Some(Change::Ended(peak)) => tracing::info!(
            peak_gap_ms = peak.as_millis() as u64,
            "async executor heartbeat recovered"
        ),
        None => {}
    }
}

fn stages(watch: &mut Sustained, longest: Option<rrss_core::db::latency::Sample>) {
    let elapsed = longest.as_ref().map_or(Duration::ZERO, |sample| {
        Duration::from_secs_f64(sample.elapsed_ms / 1000.0)
    });
    match (watch.observe(elapsed), longest) {
        (Some(Change::Began), Some(sample)) => tracing::warn!(
            error_kind = "slow_stage",
            operation = sample.operation,
            stage = sample.stage,
            elapsed_ms = elapsed.as_millis() as u64,
            "database stage still running"
        ),
        (Some(Change::Ended(peak)), _) => tracing::info!(
            peak_ms = peak.as_millis() as u64,
            "long database stages finished"
        ),
        _ => {}
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
                let mut stalled = Sustained::default();
                let mut running = Sustained::default();
                loop {
                    std::thread::sleep(Duration::from_millis(100));
                    if observer.stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let gap = (observer.started.elapsed().as_millis() as u64)
                        .saturating_sub(observer.heartbeat.load(Ordering::Relaxed));
                    if let Some(metrics) = &metrics {
                        metrics
                            .runtime_stall_max_ms
                            .fetch_max(gap, Ordering::Relaxed);
                        stages(&mut running, metrics.activity.longest_active());
                    }
                    heartbeat(&mut stalled, Duration::from_millis(gap));
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
    #[test]
    fn logs_each_sustained_stall_and_recovery_once() {
        let dir = tempfile::tempdir().unwrap();
        let (subscriber, handle) = crate::log::subscriber(
            crate::log::Mode::Desktop {
                log_dir: dir.path().into(),
            },
            "",
        );
        tracing::subscriber::with_default(subscriber, || {
            let mut watch = super::Sustained::default();
            for millis in [350, 20, 1999, 2000, 3500, 5000, 20, 15, 2100, 10] {
                super::heartbeat(&mut watch, std::time::Duration::from_millis(millis));
            }
            let stage = |millis: f64| {
                Some(rrss_core::db::latency::Sample {
                    operation: "fixture_operation",
                    stage: "fixture_stage",
                    elapsed_ms: millis,
                })
            };
            let mut running = super::Sustained::default();
            for sample in [
                stage(150.0),
                stage(2500.0),
                stage(9000.0),
                None,
                stage(100.0),
            ] {
                super::stages(&mut running, sample);
            }
        });
        handle.end();
        let log = std::fs::read_to_string(dir.path().join("rrss.log")).unwrap();
        assert_eq!(log.matches("heartbeat delayed").count(), 2);
        assert_eq!(log.matches("heartbeat recovered").count(), 2);
        assert!(log.contains("peak_gap_ms=5000"));
        assert_eq!(log.matches("database stage still running").count(), 1);
        assert!(log.contains("fixture_operation") && log.contains("fixture_stage"));
        assert_eq!(log.matches("long database stages finished").count(), 1);
        assert!(log.contains("peak_ms=9000"));
    }

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
