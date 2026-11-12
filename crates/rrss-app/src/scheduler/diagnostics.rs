//! On-demand numeric diagnostics. No service identity, URL, error text or input
//! is retained here; recording costs and memory do not grow with completed work.
use super::*;
use rrss_core::db::metrics::{Timing, TimingSnapshot};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

#[derive(Default)]
pub(super) struct Gauge {
    current: AtomicUsize,
    peak: AtomicUsize,
}
impl Gauge {
    pub fn enter(self: &Arc<Self>) -> Lease {
        self.add(1);
        Lease(self.clone())
    }
    pub fn add(&self, amount: usize) {
        let value = self.current.fetch_add(amount, Ordering::Relaxed) + amount;
        self.peak.fetch_max(value, Ordering::Relaxed);
    }
    pub fn remove(&self, amount: usize) {
        self.current.fetch_sub(amount, Ordering::Relaxed);
    }
    pub fn set(&self, value: usize) {
        self.current.store(value, Ordering::Relaxed);
        self.peak.fetch_max(value, Ordering::Relaxed);
    }
    pub fn current(&self) -> usize {
        self.current.load(Ordering::Relaxed)
    }
    pub fn peak(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }
}
pub(super) struct Lease(Arc<Gauge>);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.remove(1);
    }
}

#[derive(Default)]
pub(super) struct Metrics {
    pub http: Arc<Gauge>,
    pub downloads: Arc<Gauge>,
    pub parsers: Arc<Gauge>,
    pub bodies: Gauge,
    pub bytes: Gauge,
    pub results: Gauge,
    pub network_waiting: Gauge,
    pub deferrals: Gauge,
    pub admitted: Gauge,
    pub host_peak: AtomicUsize,
    pub feed_wait: Timing,
    pub download: Timing,
    pub parse_wait: Timing,
    pub processing: Timing,
    pub completed: AtomicU64,
    pub failed: AtomicU64,
    pub skipped: AtomicU64,
    pub inserted: AtomicU64,
    pub retried: AtomicU64,
    pub slow_failures: AtomicU64,
    pub timeouts: AtomicU64,
    pub rate_limited: AtomicU64,
    pub unavailable: AtomicU64,
    pub event_received: AtomicU64,
    pub event_emitted: AtomicU64,
    pub event_batches: AtomicU64,
    pub event_coalesced: AtomicU64,
}

const LOG_INTERVAL: Duration = Duration::from_secs(15 * 60);

#[cfg(test)]
mod log_tests {
    use super::*;
    #[test]
    fn periodic_summary_reports_deltas_and_is_silent_while_idle() {
        let log = crate::scheduler::tests::capture_logs(|| {
            let metrics = Metrics::default();
            let start = tokio::time::Instant::now();
            let mut log = ActivityLog::new(start);
            log.tick(&metrics, start + LOG_INTERVAL);
            metrics.completed.store(12, Ordering::Relaxed);
            metrics.failed.store(2, Ordering::Relaxed);
            log.tick(&metrics, start + LOG_INTERVAL);
            log.tick(&metrics, start + 2 * LOG_INTERVAL);
            log.tick(&metrics, start + 3 * LOG_INTERVAL);
            metrics.completed.store(15, Ordering::Relaxed);
            log.tick(&metrics, start + 4 * LOG_INTERVAL);
        });
        assert_eq!(log.matches("feed activity summary").count(), 2, "{log}");
        assert!(
            log.contains("completed=12 failed=2") && log.contains("completed=3 failed=0"),
            "{log}"
        );
    }
}

/// A fixed-size baseline of existing counters; idle intervals produce no event.
pub(super) struct ActivityLog {
    next: tokio::time::Instant,
    previous: [u64; 8],
}
impl ActivityLog {
    pub fn new(now: tokio::time::Instant) -> Self {
        Self {
            next: now + LOG_INTERVAL,
            previous: [0; 8],
        }
    }
    pub fn deadline(&self) -> tokio::time::Instant {
        self.next
    }
    pub fn tick(&mut self, metrics: &Metrics, now: tokio::time::Instant) {
        if now < self.next {
            return;
        }
        let elapsed_ms = now
            .saturating_duration_since(self.next - LOG_INTERVAL)
            .as_millis() as u64;
        self.next = now + LOG_INTERVAL;
        let current = [
            &metrics.completed,
            &metrics.failed,
            &metrics.skipped,
            &metrics.inserted,
            &metrics.retried,
            &metrics.timeouts,
            &metrics.rate_limited,
            &metrics.unavailable,
        ]
        .map(|counter| counter.load(Ordering::Relaxed));
        let delta = std::array::from_fn::<_, 8, _>(|i| current[i].saturating_sub(self.previous[i]));
        self.previous = current;
        if delta.iter().all(|count| *count == 0) {
            return;
        }
        let [
            completed,
            failed,
            skipped,
            inserted,
            retried,
            timeouts,
            rate_limited,
            unavailable,
        ] = delta;
        tracing::info!(
            completed,
            failed,
            skipped,
            inserted,
            retried,
            timeouts,
            rate_limited,
            unavailable,
            elapsed_ms,
            "feed activity summary"
        );
    }
}

#[derive(Default)]
pub(crate) struct ModelMetrics {
    pub(super) active: Arc<Gauge>,
    pub(super) interactive: Arc<Gauge>,
    pub(super) background: Arc<Gauge>,
    pub(super) wait: Timing,
    pub(super) held: Arc<Timing>,
    pub db_queue: Timing,
    pub db_work: Timing,
    pub(super) rate_limited: AtomicU64,
    pub(super) unavailable: AtomicU64,
    pub(super) cooldown: AtomicU64,
    pub(super) budget: AtomicU64,
}

pub(super) struct ModelLease {
    pub _active: Lease,
    pub held: Arc<Timing>,
    pub started: Instant,
}
impl Drop for ModelLease {
    fn drop(&mut self) {
        self.held.record(self.started.elapsed());
    }
}

impl From<TimingSnapshot> for views::TimingView {
    fn from(value: TimingSnapshot) -> Self {
        Self {
            count: value.count,
            total_ms: value.total_ms,
            max_ms: value.max_ms,
            p50_ms: value.p50_ms,
            p95_ms: value.p95_ms,
        }
    }
}

pub(super) fn named(name: &str, timing: &Timing) -> views::SchedulerTiming {
    views::SchedulerTiming {
        stage: name.into(),
        timing: timing.snapshot().into(),
    }
}

impl Scheduler {
    /// A cheap in-memory snapshot, deliberately separate from invalidation
    /// events. The existing diagnostics query supplies persisted backlog/usage.
    pub fn diagnostics(&self) -> views::SchedulerDiagnostics {
        self.shared.diagnostics()
    }
}

impl runtime::Shared {
    pub(super) fn diagnostics(&self) -> views::SchedulerDiagnostics {
        let m = &self.metrics;
        let queue = self.queue.lock().unwrap();
        let now = tokio::time::Instant::now();
        let network_paused = self.network_paused();
        let eligible = !network_paused && self.cooldown_dispatch_ready();
        let clock = self.clock.now();
        let cooling = self.cooldowns.lock().unwrap();
        let mut queued = 0;
        let mut parked = 0;
        let mut oldest: u64 = 0;
        let mut oldest_eligible: u64 = 0;
        for entry in queue.entries.values().filter(|entry| !entry.dispatched) {
            queued += 1;
            parked += usize::from(entry.parked.is_some());
            let elapsed = now.saturating_duration_since(entry.enqueued).as_millis() as u64;
            oldest = oldest.max(elapsed);
            let waiting_for_cooldown = entry.parked.as_ref().is_some_and(|key| {
                key == super::cooldown::GLOBAL
                    || key == super::cooldown::RECOVERY
                    || cooling.blocked_host(key, clock).is_some()
            });
            if eligible && !waiting_for_cooldown {
                oldest_eligible = oldest_eligible.max(elapsed);
            }
        }
        let feeds = views::FeedDiagnostics {
            cooldowns: cooling.diagnostics(clock),
            network_paused,
            admitted: queue.entries.len(),
            admission_limit: self.limits.pending,
            peak_admitted: m.admitted.peak(),
            queued,
            parked,
            operation_waiting: queue.operations.pending_count(),
            oldest_wait_ms: oldest,
            oldest_eligible_wait_ms: oldest_eligible,
            downloads: m.downloads.current(),
            http_active: m.http.current(),
            http_peak: m.http.peak(),
            http_limit: self.limits.global,
            host_peak: m.host_peak.load(Ordering::Relaxed),
            host_limit: self.host_limit(),
            parsing: m.parsers.current(),
            parsing_peak: m.parsers.peak(),
            parsing_limit: self.limits.parsing,
            buffered_bodies: m.bodies.current(),
            buffered_bodies_peak: m.bodies.peak(),
            buffered_bytes: m.bytes.current(),
            buffered_bytes_peak: m.bytes.peak(),
            buffer_high_water: self.limits.buffer_bytes,
            // High water pauses dispatch, not reads already in flight. One
            // reserved addition can own another body until parsing completes.
            buffer_envelope: self.limits.buffer_bytes.saturating_add(
                (self.limits.global + 1).saturating_mul(self.limits.policy.max_body_bytes),
            ),
            result_waiting: m.results.current(),
            network_waiting: m.network_waiting.current(),
            deferral_waiting: m.deferrals.current(),
        };
        drop(cooling);
        drop(queue);
        let mut timings = vec![
            named("feed_queue", &m.feed_wait),
            named("download", &m.download),
            named("parse_queue", &m.parse_wait),
            named("processing", &m.processing),
            named("db_worker_queue", &self.capacity.metrics.db_queue),
            named("db_work", &self.capacity.metrics.db_work),
        ];
        if let Some(db) = db::pool::metrics(&self.path) {
            timings.extend([
                named("db_connection_wait", &db.connection_wait),
                named("db_write_wait", &db.write_wait),
                named("db_write_hold", &db.write_hold),
                named("feed_parse", &db.feed_parse),
                named("feed_ingest", &db.feed_ingest),
            ]);
        }
        views::SchedulerDiagnostics {
            run_id: self.run_id.clone(),
            elapsed_ms: self.started.elapsed().as_millis() as u64,
            stopped: *self.stop.borrow(),
            feeds,
            models: self.capacity.diagnostics(),
            background: self.background_diagnostics.lock().unwrap().snapshot(),
            timings,
            counters: [
                ("feeds_completed", &m.completed),
                ("feeds_failed", &m.failed),
                ("feeds_skipped", &m.skipped),
                ("articles_inserted", &m.inserted),
                ("feed_requeues", &m.retried),
                ("slow_transport_failures", &m.slow_failures),
                ("feed_timeouts", &m.timeouts),
                ("feed_429", &m.rate_limited),
                ("feed_503", &m.unavailable),
                ("events_received", &m.event_received),
                ("events_emitted", &m.event_emitted),
                ("event_batches", &m.event_batches),
                ("events_coalesced", &m.event_coalesced),
            ]
            .into_iter()
            .map(|(name, count)| views::SchedulerCounter {
                name: name.into(),
                count: count.load(Ordering::Relaxed),
            })
            .collect(),
        }
    }
}
