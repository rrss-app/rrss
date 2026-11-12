//! Per-category deadlines and pause recovery; finite request completion is
//! independent of later retries and embedding catch-up.
use super::*;
use tokio::time::Instant;

pub(super) const CATCH_UP: Duration = Duration::from_secs(5 * 60);
pub(super) const CHECK_INTERVAL: Duration = Duration::from_secs(60);
const PERIODIC: Duration = Duration::from_secs(15 * 60);
const MAX_RETRY: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct Environment {
    pub automatic: bool,
    pub day: String,
    pub budget: i64,
    pub embed_budget: i64,
    pub models: [String; 5],
    pub network: String,
    pub network_environment: Option<rrss_core::network::ProxyEnvironment>,
    pub space: String,
    pub embedding_pending: bool,
}

#[derive(Default, Clone, Copy)]
struct Slot {
    waiting_since: Option<Instant>,
    started: Option<Instant>,
    attempts: u64,
    retries: u64,
    failed: u64,
    pauses: u64,
    duration: rrss_core::db::metrics::Distribution,
    requested: u64,
    running: Option<u64>,
    completed: u64,
    outcome: Option<&'static str>,
    dirty: u64,
    dirty_running: u64,
    dirty_completed: u64,
    next: Option<Instant>,
    failures: u32,
    backlog: bool,
    attempted: bool,
}

#[derive(Default)]
pub(super) struct Pending {
    slots: [Slot; JOBS.len()],
    environment: Environment,
    derived: u64,
    observed: bool,
}

impl Pending {
    pub fn request(&mut self, version: u64) {
        for slot in &mut self.slots {
            if version > slot.requested && slot.waiting_since.is_none() {
                slot.waiting_since = Some(Instant::now());
            }
            slot.requested = slot.requested.max(version);
            // A finite refresh may finish with a paused/deferred category. It
            // must not bypass a pause or wait until tomorrow's budget reset.
            if slot.running.is_none() && slot.outcome.is_some() {
                slot.completed = slot.requested;
            }
        }
    }

    pub fn derived(&mut self, version: u64) {
        if version <= self.derived {
            return;
        }
        self.derived = version;
        for job in [Job::Clusters, Job::Interests] {
            let slot = &mut self.slots[job as usize];
            slot.dirty += 1;
            slot.waiting_since.get_or_insert_with(Instant::now);
        }
    }

    pub fn environment(&mut self, env: Environment, now: Instant) {
        let previous = &self.environment;
        let network_changed = previous.network != env.network
            || previous.network_environment != env.network_environment;
        for job in JOBS {
            let slot = &mut self.slots[job as usize];
            let model = job.model_index();
            let changed = model.is_some_and(|i| previous.models[i] != env.models[i]);
            let budget_changed = if job == Job::Embeddings {
                previous.embed_budget != env.embed_budget
            } else {
                previous.budget != env.budget
            };
            let resumed = match slot.outcome {
                Some("budget") => previous.day != env.day || budget_changed,
                Some("model_configuration" | "configuration") => changed,
                Some("network_configuration") => network_changed,
                Some("vector_space") => previous.space != env.space || changed,
                _ => false,
            };
            let changed_during_run = self.observed
                && slot.running.is_some()
                && (changed
                    || network_changed
                    || (matches!(job, Job::Embeddings | Job::Clusters | Job::Interests)
                        && previous.space != env.space)
                    || (model.is_some() && (previous.day != env.day || budget_changed)));
            if resumed
                || (changed && (slot.outcome.is_some() || (self.observed && env.automatic)))
                || changed_during_run
            {
                slot.outcome = None;
                slot.failures = 0;
                slot.dirty += 1;
                slot.next = None;
                slot.waiting_since = Some(now);
            }
            if env.automatic && !previous.automatic {
                // Object cooldowns not persisted by their domains are lost on
                // restart. Keep an initial model delay instead of retrying a
                // failing batch every time the application opens.
                slot.next = Some(
                    now + if model.is_some() {
                        CATCH_UP
                    } else {
                        Duration::ZERO
                    },
                );
            }
            if job == Job::Embeddings
                && env.automatic
                && slot.running.is_none()
                && slot.outcome.is_none()
            {
                if env.embedding_pending && slot.next.is_none() {
                    // The previous metadata snapshot may still say pending
                    // when a batch empties and new input arrives before the
                    // next scan. Do not require a false -> true transition.
                    slot.next = Some(now + CATCH_UP);
                } else if !env.embedding_pending && !slot.backlog && self.observed {
                    slot.next = None;
                }
            }
        }
        self.observed |= !env.day.is_empty();
        self.environment = env;
    }

    pub fn claim(&mut self, job: Job, now: Instant) -> Option<u64> {
        let slot = self.slots[job as usize];
        let timed = slot.next.is_some_and(|next| next <= now);
        if slot.running.is_some()
            || (slot.outcome.is_some() && !timed)
            || (slot.completed >= slot.requested && slot.dirty <= slot.dirty_completed && !timed)
            || JOBS.iter().any(|other| {
                other.lane() == job.lane() && self.slots[*other as usize].running.is_some()
            })
            || job.dependency().is_some_and(|dependency| {
                let dep = self.slots[dependency as usize];
                dep.running.is_some()
                    || dep.completed < slot.requested
                    || (dep.dirty > dep.dirty_completed && dep.outcome.is_none())
            })
        {
            return None;
        }
        let current = &mut self.slots[job as usize];
        current.attempts += 1;
        current.retries += u64::from(slot.failures > 0);
        current.started = Some(now);
        current.waiting_since = None;
        current.dirty_running = slot.dirty;
        current.running = Some(slot.requested);
        current.next = None;
        Some(slot.requested)
    }

    pub fn finish(&mut self, job: Job, outcome: Option<&'static str>, more: bool, now: Instant) {
        let slot = &mut self.slots[job as usize];
        if let Some(started) = slot.started.take() {
            slot.duration.record(now.saturating_duration_since(started));
        }
        if outcome.is_some_and(|reason| matches!(reason, "job_failed" | "service_cooldown")) {
            slot.failed += 1;
        } else if outcome.is_some() {
            slot.pauses += 1;
        }
        slot.completed = slot.running.take().expect("only an owned job can finish");
        slot.outcome = outcome;
        slot.dirty_completed = slot.dirty_running;
        slot.backlog = more;
        slot.attempted = true;
        if slot.requested > slot.completed || slot.dirty > slot.dirty_completed {
            slot.waiting_since.get_or_insert(now);
        }
        // A repair committed while this owner was running must not be lost to
        // its stale pause result. One new attempt consumes the newer version.
        if slot.dirty > slot.dirty_running
            && outcome.is_some_and(|reason| !matches!(reason, "job_failed" | "service_cooldown"))
        {
            slot.outcome = None;
            slot.next = None;
            return;
        }
        slot.next = match outcome {
            Some("job_failed" | "service_cooldown") => {
                slot.failures = slot.failures.saturating_add(1);
                Some(
                    now + (CATCH_UP * (1 << slot.failures.saturating_sub(1).min(4))).min(MAX_RETRY),
                )
            }
            Some(_) => None,
            None => {
                slot.failures = 0;
                match job {
                    Job::Embeddings if more => Some(now + CATCH_UP),
                    Job::Daily | Job::Weekly if self.environment.automatic => {
                        Some(now + CHECK_INTERVAL)
                    }
                    Job::Summaries | Job::ClusterSummaries if self.environment.automatic => Some(
                        now + Duration::from_secs(rrss_core::ai_store::FOCUS_BATCH_MINUTES * 60),
                    ),
                    Job::Housekeeping | Job::Images if self.environment.automatic => {
                        Some(now + PERIODIC)
                    }
                    _ => None,
                }
            }
        };
        if slot.outcome.is_some() {
            slot.completed = slot.requested;
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        JOBS.iter()
            .filter_map(|job| {
                let slot = self.slots[*job as usize];
                let busy = JOBS.iter().any(|other| {
                    other.lane() == job.lane() && self.slots[*other as usize].running.is_some()
                });
                let waiting = job.dependency().is_some_and(|dependency| {
                    let dep = self.slots[dependency as usize];
                    dep.running.is_some()
                        || dep.completed < slot.requested
                        || (dep.dirty > dep.dirty_completed && dep.outcome.is_none())
                });
                (!busy && !waiting).then_some(slot.next).flatten()
            })
            .min()
    }

    pub fn completed(&self) -> u64 {
        self.slots[..Job::Images as usize]
            .iter()
            .map(|slot| slot.completed)
            .min()
            .unwrap_or(0)
    }

    pub fn snapshot(&self) -> Vec<views::BackgroundJobView> {
        JOBS.iter()
            .map(|job| {
                let slot = self.slots[*job as usize];
                let state = slot.state();
                views::BackgroundJobView {
                    kind: format!("{job:?}").to_lowercase(),
                    generation: slot.running.unwrap_or(slot.requested),
                    state: state.into(),
                    reason: slot.outcome.map(str::to_owned),
                }
            })
            .collect()
    }

    pub fn diagnostics(&self) -> Diagnostics {
        Diagnostics { slots: self.slots }
    }

    pub fn stop(&mut self, now: Instant) {
        for slot in &mut self.slots {
            if let Some(started) = slot.started.take() {
                slot.duration.record(now.saturating_duration_since(started));
            }
            slot.running = None;
            slot.next = None;
            slot.waiting_since = None;
            slot.outcome = Some("stopped");
        }
    }
}

impl Slot {
    fn state(&self) -> &'static str {
        if self.outcome == Some("stopped") {
            "stopped"
        } else if self.running.is_some() {
            "running"
        } else if self.outcome.is_some() {
            if self.next.is_some() {
                "cooldown"
            } else {
                "paused"
            }
        } else if self.requested > self.completed
            || self.dirty > self.dirty_completed
            || self.next.is_some()
        {
            "pending"
        } else if !self.attempted {
            "idle"
        } else {
            "completed"
        }
    }
}

#[derive(Default)]
pub(in crate::scheduler) struct Diagnostics {
    slots: [Slot; JOBS.len()],
}
impl Diagnostics {
    pub(in crate::scheduler) fn snapshot(&self) -> Vec<views::BackgroundDiagnostics> {
        let now = Instant::now();
        JOBS.iter()
            .map(|job| {
                let slot = self.slots[*job as usize];
                let state = slot.state();
                let waiting = match state {
                    "pending" => slot.waiting_since.or(slot.next),
                    "cooldown" => slot.next,
                    _ => None,
                };
                views::BackgroundDiagnostics {
                    kind: format!("{job:?}").to_lowercase(),
                    state: state.into(),
                    reason: slot.outcome.map(str::to_owned),
                    pending_batches: usize::from(
                        matches!(state, "pending" | "cooldown" | "paused")
                            || (slot.running.is_some()
                                && (slot.requested > slot.running.unwrap()
                                    || slot.dirty > slot.dirty_running)),
                    ),
                    running_ms: slot
                        .started
                        .map_or(0, |at| now.saturating_duration_since(at).as_millis() as u64),
                    oldest_eligible_wait_ms: waiting
                        .map_or(0, |at| now.saturating_duration_since(at).as_millis() as u64),
                    next_attempt_in_ms: slot
                        .next
                        .map(|at| at.saturating_duration_since(now).as_millis() as u64),
                    attempts: slot.attempts,
                    retries: slot.retries,
                    failures: slot.failed,
                    pauses: slot.pauses,
                    duration: slot.duration.snapshot().into(),
                }
            })
            .collect()
    }
}

impl Job {
    pub(super) fn model_index(self) -> Option<usize> {
        match self {
            Self::Summaries => Some(0),
            Self::Embeddings => Some(1),
            Self::Daily => Some(2),
            Self::Weekly => Some(3),
            Self::ClusterSummaries => Some(4),
            _ => None,
        }
    }
}
