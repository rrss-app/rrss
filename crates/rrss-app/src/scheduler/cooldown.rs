//! RSS host politeness. Only committed snapshots authorize dispatch. Within a
//! run, elapsed cooldown time comes from a monotonic clock; a restart resumes
//! from the saved UTC deadline, so time spent stopped counts toward the wait.
use super::*;
use rrss_core::ingest::{feed_http::ResponseHead, retry_after_deadline, slow_retry_interval};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, Weak};
use tokio::time::Instant;

pub(super) const SETTING: &str = "scheduler_feed_cooldowns";
const VERSION: u32 = 2;
const LIMIT: usize = super::runtime::PENDING_LIMIT;
const MAX_SNAPSHOT_BYTES: usize = 4 * 1024 * 1024;
const CHECKPOINT: Duration = Duration::from_secs(30);
/// Damaged-snapshot throttling ends after the longest local cooldown; any limit
/// the lost snapshot could still have held without Retry-After has lapsed by then.
const RECOVERY_LIMIT: Duration = Duration::from_secs(4 * 60 * 60);
pub(super) const GLOBAL: &str = "*";
pub(super) const RECOVERY: &str = "recovery";
const DAMAGED: &str = "订阅限流记录损坏或版本不兼容，已保守限速";
const SAVE_FAILED: &str = "无法保存订阅限流记录，已暂停抓取";

/// Shared across stopping/restarting runtimes for this database, so startup
/// cannot read an older snapshot while a received limit is still being saved.
pub(super) struct Store {
    serial: Arc<tokio::sync::Mutex<()>>,
    pub writes: std::sync::atomic::AtomicUsize,
    changed: tokio::sync::Notify,
}
impl Store {
    pub fn for_database(path: &std::path::Path) -> Arc<Self> {
        static STORES: std::sync::OnceLock<Mutex<std::collections::HashMap<PathBuf, Weak<Store>>>> =
            std::sync::OnceLock::new();
        let mut stores = STORES.get_or_init(Mutex::default).lock().unwrap();
        stores.retain(|_, store| store.strong_count() > 0);
        if let Some(store) = stores.get(path).and_then(Weak::upgrade) {
            return store;
        }
        let store = Arc::new(Self {
            serial: Arc::new(tokio::sync::Mutex::new(())),
            writes: std::sync::atomic::AtomicUsize::new(0),
            changed: tokio::sync::Notify::new(),
        });
        stores.insert(path.to_path_buf(), Arc::downgrade(&store));
        store
    }
    async fn idle(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.writes.load(std::sync::atomic::Ordering::Acquire) == 0 {
                return;
            }
            changed.await;
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Time {
    pub utc: i64,
    pub mono: Instant,
}
pub(super) trait Clock: Send + Sync {
    fn now(&self) -> Time;
}
pub(super) struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> Time {
        Time {
            utc: now_ms(),
            mono: Instant::now(),
        }
    }
}

#[derive(Clone)]
struct Block {
    until: i64,
    remaining_ms: u64,
    anchored: Instant,
}
impl Block {
    fn new(until: i64, now: Time) -> Self {
        Self {
            until,
            remaining_ms: until.saturating_sub(now.utc).max(0) as u64,
            anchored: now.mono,
        }
    }
    fn remaining(&self, now: Time) -> u64 {
        self.remaining_ms.saturating_sub(
            now.mono
                .saturating_duration_since(self.anchored)
                .as_millis()
                .min(u64::MAX as u128) as u64,
        )
    }
    fn until(&self, now: Time) -> i64 {
        self.until.max(
            now.utc
                .saturating_add(self.remaining(now).min(i64::MAX as u64) as i64),
        )
    }
    fn merge(&mut self, other: Self, now: Time) {
        let until = self.until(now).max(other.until(now));
        if other.remaining(now) > self.remaining(now) {
            *self = other;
        }
        self.until = until;
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Route {
    feed: i64,
    source: String,
    hosts: Vec<String>,
}
fn fingerprint(url: &str) -> String {
    format!("{:x}", Sha256::digest(url.as_bytes()))
}
fn key(url: &reqwest::Url) -> String {
    super::runtime::host_key(url)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedBlock {
    host: String,
    blocked_until: i64,
    // Read v1 snapshots without treating an existing server limit as damage.
    // UTC is authoritative after restart; new snapshots omit this legacy field.
    #[serde(default, skip_serializing)]
    remaining_ms: Option<u64>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u32,
    blocks: Vec<SavedBlock>,
    overflow: Option<SavedBlock>,
    penalties: BTreeMap<String, u8>,
    routes: Vec<Route>,
    recovery: bool,
    #[serde(default)]
    recovery_until: Option<i64>,
}

fn valid_host(host: &str) -> bool {
    reqwest::Url::parse(&format!("http://{host}/")).is_ok_and(|url| {
        url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && key(&url) == host
    })
}

#[derive(Clone)]
pub(super) struct Cooldowns {
    pub ready: bool,
    blocks: BTreeMap<String, Block>,
    overflow: Option<Block>,
    penalties: BTreeMap<String, u8>,
    routes: VecDeque<Route>,
    events: VecDeque<(i64, u64, String, u16)>,
    recovery: bool,
    recovery_next: Instant,
    /// Throttling end: monotonic within this run, UTC across restarts.
    recovery_end: Instant,
    persist_error: bool,
    load_error: bool,
    last_saved: Instant,
}

#[derive(Clone)]
pub(super) struct Deferred {
    pub key: String,
    pub until: Option<i64>,
    pub message: String,
}
impl Deferred {
    fn block(host: String, until: i64) -> Self {
        let date = chrono::DateTime::from_timestamp_millis(until)
            .map(|date| date.to_rfc3339())
            .unwrap_or_else(|| "远期".into());
        Self {
            key: host,
            until: Some(until),
            message: format!("订阅主机限流，已延期至 {date}"),
        }
    }
    pub fn status(&self) -> RefreshStatus {
        RefreshStatus::Skipped {
            message: self.message.clone(),
            retry_at: self.until,
        }
    }
    pub fn error(&self) -> rrss_core::Error {
        rrss_core::Error::FetchDeferred {
            until: self.until,
            message: self.message.clone(),
        }
    }
}

impl Cooldowns {
    /// Compare existing scheduler state only after persistence succeeds.
    fn log_changes(&self, previous: &Self, now: Time) {
        let blocks = |state: &Self| {
            state
                .blocks
                .iter()
                .map(|(host, block)| (host.as_str(), block))
                .chain(state.overflow.as_ref().map(|block| (GLOBAL, block)))
                .map(|(host, block)| (host.to_owned(), block.until(now)))
                .collect::<BTreeMap<_, _>>()
        };
        let before = blocks(previous);
        let after = blocks(self);
        for (key, until) in &after {
            let host = if key == GLOBAL {
                "all".into()
            } else {
                rrss_core::logsafe::host_str(&format!("http://{key}/"))
            };
            let status = self
                .events
                .iter()
                .rev()
                .find(|event| &event.2 == key && matches!(event.3, 429 | 503))
                .map(|event| event.3);
            let reason = if status.is_some() { "http" } else { "restored" };
            if !before.contains_key(key) || previous.persist_error || previous.load_error {
                tracing::warn!(
                    host,
                    error_kind = reason,
                    status,
                    until,
                    "feed host cooldown entered"
                );
            } else if before.get(key) != Some(until) {
                tracing::debug!(
                    host,
                    error_kind = reason,
                    status,
                    until,
                    "feed host cooldown extended"
                );
            }
        }
        for (key, until) in &before {
            if !after.contains_key(key) {
                let host = if key == GLOBAL {
                    "all".into()
                } else {
                    rrss_core::logsafe::host_str(&format!("http://{key}/"))
                };
                tracing::info!(host, until, "feed host cooldown ended");
            }
        }
        if previous.persist_error || previous.load_error {
            tracing::info!("feed cooldown storage recovered");
        }
        if self.recovery && !previous.recovery {
            tracing::warn!(
                error_kind = "invalid_snapshot",
                "feed cooldown snapshot invalid; throttling enabled"
            );
        } else if previous.recovery && !self.recovery {
            tracing::info!("feed cooldown snapshot recovery ended");
        }
    }

    pub fn new(now: Time) -> Self {
        Self {
            ready: false,
            blocks: BTreeMap::new(),
            overflow: None,
            penalties: BTreeMap::new(),
            routes: VecDeque::new(),
            events: VecDeque::new(),
            recovery: false,
            recovery_next: now.mono,
            recovery_end: now.mono,
            persist_error: false,
            load_error: false,
            last_saved: now.mono,
        }
    }

    fn load(raw: Option<&str>, now: Time) -> Result<Self> {
        let mut state = Self::new(now);
        state.ready = true;
        let Some(raw) = raw else {
            return Ok(state);
        };
        anyhow::ensure!(raw.len() <= MAX_SNAPSHOT_BYTES, "invalid cooldown snapshot");
        let saved: Snapshot =
            serde_json::from_str(raw).map_err(|_| anyhow::anyhow!("invalid cooldown snapshot"))?;
        anyhow::ensure!(
            matches!(saved.version, 1 | VERSION)
                && saved.blocks.len() <= LIMIT
                && saved.routes.len() <= LIMIT
                && saved.penalties.len() <= LIMIT,
            "invalid cooldown snapshot"
        );
        // Resume from the UTC deadline: a stop does not extend the wait. A
        // forward clock change while stopped may end it early; normal restarts
        // preserve the deadline instead of repeating the original whole wait.
        let block = |saved: SavedBlock| -> Result<Block> {
            anyhow::ensure!(
                saved.blocked_until >= 0
                    && saved
                        .remaining_ms
                        .is_none_or(|left| left <= i64::MAX as u64),
                "invalid cooldown snapshot"
            );
            Ok(Block::new(saved.blocked_until, now))
        };
        for saved in saved.blocks {
            anyhow::ensure!(
                valid_host(&saved.host) && !state.blocks.contains_key(&saved.host),
                "invalid cooldown snapshot"
            );
            let host = saved.host.clone();
            let value = block(saved)?;
            if value.remaining_ms > 0 {
                state.blocks.insert(host, value);
            }
        }
        if let Some(saved) = saved.overflow {
            anyhow::ensure!(saved.host == GLOBAL, "invalid cooldown snapshot");
            let value = block(saved)?;
            if value.remaining_ms > 0 {
                state.overflow = Some(value);
            }
        }
        for (host, tier) in saved.penalties {
            anyhow::ensure!(
                valid_host(&host) && (1..=3).contains(&tier),
                "invalid cooldown snapshot"
            );
            state.penalties.insert(host, tier);
        }
        for route in saved.routes {
            anyhow::ensure!(
                route.feed > 0
                    && route.source.len() == 64
                    && route.source.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && !route.hosts.is_empty()
                    && route.hosts.len() <= 11
                    && route.hosts.iter().all(|host| valid_host(host))
                    && !state
                        .routes
                        .iter()
                        .any(|old| old.feed == route.feed && old.source == route.source),
                "invalid cooldown snapshot"
            );
            state.routes.push_back(route);
        }
        if saved.recovery {
            let left = match saved.recovery_until {
                Some(until) if until >= 0 => until.saturating_sub(now.utc),
                None if saved.version == 1 => RECOVERY_LIMIT.as_millis() as i64,
                _ => anyhow::bail!("invalid cooldown snapshot"),
            };
            state.start_recovery(now, left);
        }
        Ok(state)
    }

    /// Bounded by `RECOVERY_LIMIT`, also when a saved deadline is too far
    /// ahead because the clock was rolled back.
    fn start_recovery(&mut self, now: Time, left_ms: i64) {
        let left = Duration::from_millis(left_ms.max(0) as u64).min(RECOVERY_LIMIT);
        self.recovery = !left.is_zero();
        self.recovery_end = now.mono + left;
    }
    fn recovering(&self, now: Time) -> bool {
        self.recovery && now.mono < self.recovery_end
    }

    fn snapshot(&mut self, now: Time) -> Snapshot {
        let save = |host: String, value: &mut Block| {
            value.until = value.until(now);
            SavedBlock {
                host,
                blocked_until: value.until,
                remaining_ms: None,
            }
        };
        Snapshot {
            version: VERSION,
            blocks: self
                .blocks
                .iter_mut()
                .map(|(host, value)| save(host.clone(), value))
                .collect(),
            overflow: self
                .overflow
                .as_mut()
                .map(|value| save(GLOBAL.into(), value)),
            penalties: self.penalties.clone(),
            routes: self
                .routes
                .iter()
                .filter(|route| {
                    self.overflow.is_some()
                        || route
                            .hosts
                            .iter()
                            .any(|host| self.blocks.contains_key(host))
                })
                .cloned()
                .collect(),
            recovery: self.recovery,
            recovery_until: self.recovery.then(|| {
                // Re-anchor when saving so a clock change during this run does
                // not silently shorten or restart the remaining recovery window.
                now.utc.saturating_add(
                    self.recovery_end
                        .saturating_duration_since(now.mono)
                        .as_millis() as i64,
                )
            }),
        }
    }

    pub fn reason(&self) -> Option<String> {
        if self.load_error {
            Some("无法读取订阅限流记录，已暂停抓取".into())
        } else if self.persist_error {
            Some(SAVE_FAILED.into())
        } else {
            self.recovery.then(|| DAMAGED.into())
        }
    }

    pub(super) fn diagnostics(&self, now: Time) -> views::CooldownDiagnostics {
        views::CooldownDiagnostics {
            loaded: self.ready,
            active_services: self
                .blocks
                .values()
                .filter(|block| block.remaining(now) > 0)
                .count(),
            global: self
                .overflow
                .as_ref()
                .is_some_and(|block| block.remaining(now) > 0),
            recovery: self.recovering(now),
            storage_blocked: self.persist_error || self.load_error,
        }
    }

    pub fn blocked_host(&self, host: &str, now: Time) -> Option<Deferred> {
        if self.persist_error || self.load_error {
            return Some(Deferred {
                key: GLOBAL.into(),
                until: None,
                message: self.reason().unwrap(),
            });
        }
        if let Some(value) = &self.overflow
            && value.remaining(now) > 0
        {
            return Some(Deferred::block(GLOBAL.into(), value.until(now)));
        }
        self.blocks
            .get(host)
            .filter(|value| value.remaining(now) > 0)
            .map(|value| Deferred::block(host.into(), value.until(now)))
    }

    pub fn blocked_source(&self, feed: i64, url: &str, now: Time) -> Option<Deferred> {
        let direct = reqwest::Url::parse(url)
            .ok()
            .and_then(|url| self.blocked_host(&key(&url), now));
        let source = fingerprint(url);
        let linked = self
            .routes
            .iter()
            .find(|route| route.feed == feed && route.source == source)
            .into_iter()
            .flat_map(|route| route.hosts.iter())
            .filter_map(|host| self.blocked_host(host, now));
        direct
            .into_iter()
            .chain(linked)
            .max_by_key(|deferred| deferred.until.unwrap_or(i64::MAX))
    }

    pub fn recovery(&self, now: Time) -> Option<Deferred> {
        if !self.recovering(now) {
            return None;
        }
        let delay = self
            .recovery_next
            .saturating_duration_since(now.mono)
            .max(rrss_core::ingest::feed_http::CONNECT_TIMEOUT);
        Some(Deferred {
            key: RECOVERY.into(),
            until: Some(now.utc.saturating_add(delay.as_millis() as i64)),
            message: DAMAGED.into(),
        })
    }

    pub fn dispatch_ready(&self, now: Time) -> bool {
        self.ready
            && !self.persist_error
            && !self.load_error
            && (!self.recovering(now) || now.mono >= self.recovery_next)
    }
    pub fn recovery_ready(&self, now: Time) -> bool {
        !self.recovering(now) || now.mono >= self.recovery_next
    }
    pub fn dispatched(&mut self, now: Time) {
        if self.recovering(now) {
            self.recovery_next = now.mono + rrss_core::ingest::feed_http::CONNECT_TIMEOUT;
        }
    }

    fn expire(&mut self, now: Time) {
        if self.recovery && !self.recovering(now) {
            self.recovery = false;
        }
        self.blocks.retain(|_, block| block.remaining(now) > 0);
        if self
            .overflow
            .as_ref()
            .is_some_and(|block| block.remaining(now) == 0)
        {
            self.overflow = None;
        }
    }
    pub fn needs_checkpoint(&self, now: Time) -> bool {
        if self.persist_error || self.load_error {
            return now.mono.saturating_duration_since(self.last_saved) >= CHECKPOINT;
        }
        // Saved deadlines are UTC, so a live cooldown needs no periodic write.
        // Only expiry (a block or recovery ending) is persisted.
        self.ready
            && (self.blocks.values().any(|block| block.remaining(now) == 0)
                || self
                    .overflow
                    .as_ref()
                    .is_some_and(|block| block.remaining(now) == 0)
                || (self.recovery && !self.recovering(now)))
    }
    pub fn deadline(&self, now: Time) -> Option<Instant> {
        if !self.ready {
            return None;
        }
        if self.persist_error || self.load_error {
            // Retry storage failures on the checkpoint cadence. An expired
            // block must not turn a failed write into an always-ready timer.
            return Some(self.last_saved + CHECKPOINT);
        }
        let expiry = self
            .blocks
            .values()
            .chain(self.overflow.iter())
            .map(|block| {
                now.mono
                    + Duration::from_millis(block.remaining(now).min(CHECKPOINT.as_millis() as u64))
            });
        // Long blocks wake at most every CHECKPOINT; the expiry check is cheap.
        expiry
            .chain(
                (self.recovering(now) && self.recovery_next > now.mono)
                    .then_some(self.recovery_next),
            )
            .chain(self.recovery.then_some(self.recovery_end))
            .min()
    }

    /// The event guard makes retries of a failed settings write idempotent. It
    /// is bounded and process-local; the persisted host tier survives expiry.
    fn header(
        &mut self,
        feed: &FeedRow,
        generation: u64,
        head: &ResponseHead,
        received: Time,
        now: Time,
    ) -> bool {
        let event = (feed.id, generation, key(&head.url), head.status);
        if self.events.contains(&event) {
            return self.persist_error;
        }
        self.events.push_back(event);
        if self.events.len() > LIMIT {
            self.events.pop_front();
        }
        let host = key(&head.url);
        let mut hosts = head.visited.iter().map(key).collect::<Vec<_>>();
        hosts.sort();
        hosts.dedup();
        let mut changed = false;
        if hosts.len() > 1 {
            let route = Route {
                feed: feed.id,
                source: fingerprint(&feed.url),
                hosts,
            };
            if !self.routes.contains(&route) {
                self.routes
                    .retain(|old| old.feed != route.feed || old.source != route.source);
                self.routes.push_front(route);
                self.routes.truncate(LIMIT);
                changed = self.overflow.is_some()
                    || self
                        .routes
                        .front()
                        .unwrap()
                        .hosts
                        .iter()
                        .any(|host| self.blocks.contains_key(host));
            }
        }
        if matches!(head.status, 429 | 503) {
            let tier = if self.penalties.len() >= LIMIT && !self.penalties.contains_key(&host) {
                3
            } else {
                self.penalties
                    .get(&host)
                    .copied()
                    .unwrap_or(0)
                    .saturating_add(1)
                    .min(3)
            };
            if self.penalties.len() < LIMIT || self.penalties.contains_key(&host) {
                self.penalties.insert(host.clone(), tier);
            }
            let until = head
                .retry_after
                .as_deref()
                .and_then(|after| retry_after_deadline(after, received.utc))
                .unwrap_or_else(|| {
                    received.utc.saturating_add(
                        slow_retry_interval(feed.interval_secs, i64::from(tier))
                            .saturating_mul(1000),
                    )
                });
            let block = Block::new(until, received);
            if let Some(current) = self.blocks.get_mut(&host) {
                current.merge(block, now);
            } else if self.blocks.len() < LIMIT {
                self.blocks.insert(host, block);
            } else if let Some(current) = &mut self.overflow {
                current.merge(block, now);
            } else {
                self.overflow = Some(block);
            }
            // A freshly committed valid limit replaces damaged-snapshot recovery.
            self.recovery = false;
            changed = true;
        } else if self.penalties.remove(&host).is_some() {
            changed = true;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooldown_transition_logs_follow_snapshot_changes() {
        let log = crate::scheduler::tests::capture_logs(|| {
            let now = Time {
                utc: 1000,
                mono: Instant::now(),
            };
            let before = Cooldowns::new(now);
            let mut state = before.clone();
            let host = "feed.test:443".to_owned();
            state.blocks.insert(host.clone(), Block::new(2000, now));
            state.events.push_back((1, 1, host.clone(), 429));
            state.log_changes(&before, now);
            state.log_changes(&state, now);
            let before = state.clone();
            state
                .blocks
                .get_mut(&host)
                .unwrap()
                .merge(Block::new(3000, now), now);
            state.log_changes(&before, now);
            let before = state.clone();
            let later = Time {
                utc: 4000,
                mono: now.mono + Duration::from_secs(3),
            };
            state.expire(later);
            state.log_changes(&before, later);
        });
        assert_eq!(
            log.matches("feed host cooldown entered").count(),
            1,
            "{log}"
        );
        assert_eq!(
            log.matches("feed host cooldown extended").count(),
            1,
            "{log}"
        );
        assert_eq!(log.matches("feed host cooldown ended").count(), 1, "{log}");
        assert!(
            log.contains("host=feed.test")
                && log.contains("status=429")
                && log.contains("until=2000"),
            "{log}"
        );
    }

    #[test]
    fn legacy_snapshots_keep_server_deadlines_and_bound_recovery() {
        let now = Time {
            utc: 1_000_000,
            mono: Instant::now(),
        };
        let until = now.utc + 86_400_000;
        let raw = serde_json::json!({
            "version": 1,
            "blocks": [{"host": "feed.invalid:443", "blocked_until": until, "remaining_ms": 86_400_000}],
            "overflow": null, "penalties": {}, "routes": [], "recovery": false
        }).to_string();
        let state = Cooldowns::load(Some(&raw), now).expect("valid v1 snapshots must migrate");
        assert_eq!(
            state.blocked_host("feed.invalid:443", now).unwrap().until,
            Some(until)
        );
        let after_downtime = Time {
            utc: until + 1,
            ..now
        };
        assert!(
            Cooldowns::load(Some(&raw), after_downtime)
                .unwrap()
                .blocked_host("feed.invalid:443", after_downtime)
                .is_none()
        );
        let raw = r#"{"version":1,"blocks":[],"overflow":null,"penalties":{},"routes":[],"recovery":true}"#;
        let state = Cooldowns::load(Some(raw), now).unwrap();
        assert!(
            state.recovering(now),
            "legacy recovery cannot silently disappear"
        );
        assert!(!state.recovering(Time {
            mono: now.mono + RECOVERY_LIMIT,
            ..now
        }));
    }

    #[test]
    fn recovery_snapshot_preserves_monotonic_remaining_after_clock_changes() {
        let now = Time {
            utc: 100_000_000,
            mono: Instant::now(),
        };
        for jump in [-86_400_000, 86_400_000] {
            let mut state = Cooldowns::new(now);
            state.start_recovery(now, RECOVERY_LIMIT.as_millis() as i64);
            let later = Time {
                utc: now.utc + 3_600_000 + jump,
                mono: now.mono + Duration::from_secs(3600),
            };
            let raw = serde_json::to_string(&state.snapshot(later)).unwrap();
            let restarted = Cooldowns::load(Some(&raw), later).unwrap();
            assert!(restarted.recovering(later));
            assert!(!restarted.recovering(Time {
                mono: later.mono + Duration::from_secs(3 * 3600),
                ..later
            }));
            assert!(restarted.recovering(Time {
                mono: later.mono + Duration::from_secs(3 * 3600 - 1),
                ..later
            }));
        }
    }

    #[test]
    fn live_cooldowns_do_not_checkpoint_and_recovery_is_capped_on_clock_rollback() {
        let now = Time {
            utc: 100_000_000,
            mono: Instant::now(),
        };
        let mut state = Cooldowns::new(now);
        state.ready = true;
        state.blocks.insert(
            "feed.invalid:443".into(),
            Block::new(now.utc + 3_600_000, now),
        );
        assert!(!state.needs_checkpoint(Time {
            mono: now.mono + CHECKPOINT,
            ..now
        }));
        assert!(state.needs_checkpoint(Time {
            mono: now.mono + Duration::from_secs(3600),
            ..now
        }));
        state.start_recovery(now, RECOVERY_LIMIT.as_millis() as i64);
        let raw = serde_json::to_string(&state.snapshot(now)).unwrap();
        let rolled_back = Time {
            utc: now.utc - 86_400_000,
            ..now
        };
        let restarted = Cooldowns::load(Some(&raw), rolled_back).unwrap();
        assert!(restarted.recovering(rolled_back));
        assert!(!restarted.recovering(Time {
            mono: now.mono + RECOVERY_LIMIT,
            ..rolled_back
        }));
    }

    #[test]
    fn failed_snapshot_write_retries_on_cadence_even_after_a_block_expires() {
        let now = Time {
            utc: 1000,
            mono: Instant::now(),
        };
        let mut state = Cooldowns::new(now);
        state.ready = true;
        state.persist_error = true;
        state
            .blocks
            .insert("feed.invalid:443".into(), Block::new(now.utc, now));
        assert!(!state.needs_checkpoint(now));
        assert_eq!(state.deadline(now), Some(now.mono + CHECKPOINT));
        assert!(state.needs_checkpoint(Time {
            mono: now.mono + CHECKPOINT,
            ..now
        }));
    }
}

struct Writing(Arc<super::runtime::Shared>);
impl Writing {
    fn new(state: Arc<super::runtime::Shared>) -> Arc<Self> {
        state
            .cooldown_store
            .writes
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Arc::new(Self(state))
    }
}
impl Drop for Writing {
    fn drop(&mut self) {
        self.0
            .cooldown_store
            .writes
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        self.0.wake.notify_one();
        self.0.cooldown_store.changed.notify_waiters();
    }
}

async fn update(
    state: Arc<super::runtime::Shared>,
    edit: impl Fn(&mut Cooldowns, Time) -> bool + Send + Sync + 'static,
) -> Result<()> {
    let writing = Writing::new(state.clone());
    let work = state.capacity.work();
    // A received limit owns its persistence intent even if the RSS deadline
    // cancels the caller. Requests remain bounded by RSS admission. Serialization
    // waits before borrowing a SQLite connection, including across restarts.
    tokio::spawn(async move {
        let _work = work;
        let owner = Arc::new(state.cooldown_store.serial.clone().lock_owned().await);
        let now = state.clock.now();
        let previous = state.cooldowns.lock().unwrap().clone();
        let mut candidate = previous.clone();
        // A due expiry is saved here too. Clearing it only in memory would keep
        // the expired block in the stored snapshot and never log its end.
        let save = edit(&mut candidate, now)
            || candidate.persist_error
            || candidate.needs_checkpoint(now);
        candidate.expire(now);
        if !save {
            *state.cooldowns.lock().unwrap() = candidate;
            state.wake.notify_one();
            return Ok(());
        }
        let raw = serde_json::to_string(&candidate.snapshot(now))?;
        let target = state.clone();
        let intended = candidate.clone();
        // Keep serialization and the dispatch barrier until error handling also
        // finishes. The blocking worker needs its own leases if we are cancelled.
        let writer_owner = owner.clone();
        let writer_intent = writing.clone();
        let result = write_db(state.path.clone(), move |conn| {
            let _owner = &writer_owner;
            let _writing = &writer_intent;
            let mut committed = intended.clone();
            let result = db::atomic_write(conn, |conn| db::set_setting(conn, SETTING, &raw));
            committed.persist_error = result.is_err();
            committed.last_saved = target.clock.now().mono;
            *target.cooldowns.lock().unwrap() = committed;
            target.notify_status();
            result
        })
        .await;
        if let Err(error) = &result {
            if !previous.persist_error {
                tracing::warn!(error = %crate::log::error(error.as_ref()), "feed cooldown write failed; fetch paused");
            }
            candidate.persist_error = true;
            candidate.last_saved = state.clock.now().mono;
            *state.cooldowns.lock().unwrap() = candidate;
            state.notify_status();
        } else {
            state.cooldowns.lock().unwrap().log_changes(&previous, state.clock.now());
        }
        result
    })
    .await?
}

pub(super) async fn restore(state: Arc<super::runtime::Shared>) {
    state.cooldown_store.idle().await;
    let raw =
        crate::jobs::spawn_db(state.path.clone(), |conn| db::get_setting(conn, SETTING)).await;
    let now = state.clock.now();
    let (candidate, save) = match raw {
        Ok(raw) => match Cooldowns::load(raw.as_deref(), now) {
            Ok(candidate) => (candidate, raw.is_some()),
            Err(_) => {
                let mut candidate = Cooldowns::new(now);
                candidate.ready = true;
                candidate.start_recovery(now, RECOVERY_LIMIT.as_millis() as i64);
                (candidate, true)
            }
        },
        Err(_) => {
            let mut candidate = Cooldowns::new(now);
            candidate.ready = true;
            candidate.load_error = true;
            (candidate, false)
        }
    };
    let previous = state.cooldowns.lock().unwrap().clone();
    if candidate.load_error && !previous.load_error {
        tracing::warn!(
            error_kind = "storage",
            "feed cooldown read failed; fetch paused"
        );
    }
    *state.cooldowns.lock().unwrap() = candidate;
    let restored = if save {
        update(state.clone(), |_, _| true).await.is_ok()
    } else {
        !state.cooldowns.lock().unwrap().load_error
    };
    if restored {
        state
            .cooldowns
            .lock()
            .unwrap()
            .log_changes(&previous, state.clock.now());
    }
    state.cooldown_ready.send_replace(true);
    state.notify_status();
    state.wake.notify_one();
}

pub(super) async fn checkpoint(state: Arc<super::runtime::Shared>) {
    if state.cooldowns.lock().unwrap().load_error {
        restore(state).await;
    } else {
        let _ = update(state, |_, _| true).await;
    }
}

impl super::runtime::Shared {
    pub(super) async fn await_cooldown_ready(&self) -> Result<()> {
        let mut ready = self.cooldown_ready.subscribe();
        let mut stop = self.stop.subscribe();
        tokio::select! {
            _=stop.wait_for(|stopped|*stopped)=>anyhow::bail!("抓取运行时已停止"),
            result=ready.wait_for(|ready|*ready)=>{result?;Ok(())},
        }
    }

    pub(super) async fn response_head(
        self: &Arc<Self>,
        feed: FeedRow,
        generation: u64,
        head: ResponseHead,
    ) -> rrss_core::Result<()> {
        match head.status {
            429 => {
                self.metrics
                    .rate_limited
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            503 => {
                self.metrics
                    .unavailable
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            _ => {}
        }
        let received = self.clock.now();
        let host = key(&head.url);
        let limited = matches!(head.status, 429 | 503);
        let needed = limited
            || head.visited.iter().map(key).any(|other| other != host)
            || self.cooldowns.lock().unwrap().penalties.contains_key(&host);
        if !needed {
            return Ok(());
        }
        if update(self.clone(), move |state, now| {
            state.header(&feed, generation, &head, received, now)
        })
        .await
        .is_err()
        {
            return Err(rrss_core::Error::FetchDeferred {
                until: None,
                message: SAVE_FAILED.into(),
            });
        }
        if limited {
            self.recheck_cooling_hosts();
        }
        Ok(())
    }

    pub(super) fn deferred(&self, id: i64, url: &str) -> Option<Deferred> {
        self.cooldowns
            .lock()
            .unwrap()
            .blocked_source(id, url, self.clock.now())
    }
    pub(super) fn fetch_schedule(&self, feed: &FeedRow) -> FetchSchedule {
        let schedule = FetchSchedule::default();
        self.deferred(feed.id, &feed.url)
            .and_then(|deferred| deferred.until)
            .map_or(schedule, |until| schedule.not_before(until))
    }
}
