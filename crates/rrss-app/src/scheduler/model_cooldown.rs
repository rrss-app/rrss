//! Persisted API-origin cooldowns, shared by completion and embedding requests.
//! The serial state survives cancelled callers through the owning Capacity.
use super::model_capacity::Priority;
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tokio::time::Instant;

const SETTING: &str = "scheduler_model_cooldowns";
const VERSION: u8 = 2;
const LIMIT: usize = 1024;
const RECOVERY: Duration = Duration::from_secs(10);
const RECOVERY_LIMIT: Duration = Duration::from_secs(4 * 60 * 60);
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(60),
    Duration::from_secs(5 * 60),
    Duration::from_secs(15 * 60),
];

#[derive(Default)]
pub(super) struct Store {
    state: tokio::sync::Mutex<Cooldowns>,
    writes: std::sync::atomic::AtomicUsize,
    changed: tokio::sync::Notify,
}

pub(super) struct Recording(Arc<Store>);
impl Drop for Recording {
    fn drop(&mut self) {
        self.0
            .writes
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        self.0.changed.notify_waiters();
    }
}

impl Store {
    pub(super) fn diagnostics(&self) -> Option<views::CooldownDiagnostics> {
        // Do not wait behind SQLite persistence just to render diagnostics.
        let state = self.state.try_lock().ok()?;
        Some(views::CooldownDiagnostics {
            loaded: state.loaded,
            active_services: state
                .blocks
                .iter()
                .filter(|(key, block)| key.as_str() != "*" && block.remaining() > 0)
                .count(),
            global: state
                .blocks
                .get("*")
                .is_some_and(|block| block.remaining() > 0),
            recovery: state.recovery_end.is_some_and(|end| end > Instant::now()),
            storage_blocked: state.dirty,
        })
    }
    pub fn recording(self: &Arc<Self>) -> Recording {
        self.writes
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Recording(self.clone())
    }

    pub async fn admit(
        &self,
        path: &std::path::Path,
        key: &str,
        priority: Priority,
    ) -> rrss_ai::Result<()> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.writes.load(std::sync::atomic::Ordering::Acquire) == 0 {
                break;
            }
            changed.await;
        }
        self.state.lock().await.admit(path, key, priority).await
    }

    pub async fn response(
        &self,
        path: &std::path::Path,
        key: &str,
        status: u16,
        retry: Option<&str>,
    ) -> rrss_ai::Result<()> {
        self.state
            .lock()
            .await
            .response(path, key, status, retry)
            .await
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    until: i64,
    remaining_ms: u64,
    failures: u32,
    /// Explicit Retry-After must also gate interactive requests. None means
    /// only locally inferred backoff; v1 limits are migrated conservatively.
    #[serde(default)]
    server_until: Option<i64>,
}

struct Block {
    saved: Saved,
    anchor: Instant,
    server_remaining_ms: u64,
}
impl Block {
    fn remaining(&self) -> u64 {
        self.remaining_at(Instant::now())
    }
    fn remaining_at(&self, now: Instant) -> u64 {
        self.saved.remaining_ms.saturating_sub(
            now.saturating_duration_since(self.anchor)
                .as_millis()
                .min(u64::MAX as u128) as u64,
        )
    }
    fn server_remaining(&self) -> u64 {
        self.server_remaining_at(Instant::now())
    }
    fn server_remaining_at(&self, now: Instant) -> u64 {
        self.server_remaining_ms.saturating_sub(
            now.saturating_duration_since(self.anchor)
                .as_millis()
                .min(u64::MAX as u128) as u64,
        )
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u8,
    blocks: BTreeMap<String, Saved>,
    damaged: bool,
    #[serde(default)]
    recovery_until: Option<i64>,
}

#[derive(Default)]
struct Cooldowns {
    loaded: bool,
    dirty: bool,
    blocks: BTreeMap<String, Block>,
    damaged: bool,
    recovery_next: Option<Instant>,
    recovery_end: Option<Instant>,
}

impl Cooldowns {
    async fn load(&mut self, path: &std::path::Path) -> rrss_ai::Result<()> {
        if self.loaded {
            return Ok(());
        }
        let raw = crate::jobs::spawn_db(path.to_owned(), |conn| db::get_setting(conn, SETTING))
            .await
            .map_err(|_| rrss_ai::Error::Admission("无法读取模型限流记录"))?;
        if let Some(raw) = raw {
            let now = now_ms();
            let anchor = Instant::now();
            let snapshot = (raw.len() <= 256 * 1024)
                .then(|| serde_json::from_str::<Snapshot>(&raw).ok())
                .flatten();
            match snapshot.filter(|s| {
                matches!(s.version, 1 | VERSION)
                    && s.blocks.len() <= LIMIT + 1
                    && (!s.damaged || s.version == 1 || s.recovery_until.is_some_and(|at| at >= 0))
                    && s.blocks.iter().all(|(key, block)| {
                        (key == "*"
                            || (key.len() == 64 && key.bytes().all(|b| b.is_ascii_hexdigit())))
                            && block.remaining_ms <= i64::MAX as u64
                            && block.until >= 0
                            && block
                                .server_until
                                .is_none_or(|at| at >= 0 && at <= block.until)
                    })
            }) {
                Some(saved) => {
                    let legacy = saved.version == 1;
                    self.dirty = legacy;
                    if saved.damaged {
                        let left = saved.recovery_until.map_or(RECOVERY_LIMIT, |until| {
                            Duration::from_millis(until.saturating_sub(now).max(0) as u64)
                        });
                        self.start_recovery(left);
                        self.dirty |= !self.damaged || left > RECOVERY_LIMIT;
                    }
                    for (key, mut saved) in saved.blocks {
                        // v1 did not distinguish a server deadline from local
                        // backoff. Never shorten a potentially explicit limit.
                        if legacy {
                            saved.server_until = Some(saved.until);
                        }
                        // Match RSS cooldown recovery: downtime counts toward
                        // the saved UTC deadline, then use monotonic time again.
                        saved.remaining_ms = saved.until.saturating_sub(now).max(0) as u64;
                        self.blocks.insert(
                            key,
                            Block {
                                server_remaining_ms: saved
                                    .server_until
                                    .map_or(0, |until| until.saturating_sub(now).max(0) as u64),
                                saved,
                                anchor,
                            },
                        );
                    }
                }
                None => {
                    tracing::warn!(
                        error_kind = "invalid_cooldown_snapshot",
                        "model cooldown snapshot invalid; conservative limits enabled"
                    );
                    self.start_recovery(RECOVERY_LIMIT);
                    self.dirty = true;
                }
            }
        }
        self.loaded = true;
        Ok(())
    }

    fn start_recovery(&mut self, left: Duration) {
        let left = left.min(RECOVERY_LIMIT);
        self.damaged = !left.is_zero();
        self.recovery_end = self.damaged.then(|| Instant::now() + left);
        self.recovery_next = self.damaged.then(|| Instant::now() + RECOVERY);
    }

    fn expire_recovery(&mut self) {
        if self.damaged && self.recovery_end.is_some_and(|end| end <= Instant::now()) {
            self.damaged = false;
            self.recovery_end = None;
            self.recovery_next = None;
            self.dirty = true;
        }
    }

    async fn save(&mut self, path: &std::path::Path) -> rrss_ai::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let now = now_ms();
        let anchor = Instant::now();
        let snapshot = Snapshot {
            version: VERSION,
            damaged: self.damaged,
            recovery_until: self.recovery_end.map(|end| {
                now.saturating_add(end.saturating_duration_since(anchor).as_millis() as i64)
            }),
            blocks: self
                .blocks
                .iter()
                .map(|(key, block)| {
                    let mut saved = block.saved.clone();
                    saved.remaining_ms = block.remaining_at(anchor);
                    saved.until = saved
                        .until
                        .max(now.saturating_add(saved.remaining_ms as i64));
                    saved.server_until = (block.server_remaining_at(anchor) > 0).then(|| {
                        saved
                            .server_until
                            .unwrap_or(0)
                            .max(now.saturating_add(block.server_remaining_at(anchor) as i64))
                    });
                    (key.clone(), saved)
                })
                .collect(),
        };
        let raw = serde_json::to_string(&snapshot).expect("model cooldown snapshot");
        crate::jobs::spawn_db(path.to_owned(), move |conn| {
            db::set_setting(conn, SETTING, &raw)
        })
        .await
        .map_err(|_| rrss_ai::Error::Admission("无法保存模型限流记录，已暂停派发"))?;
        self.dirty = false;
        Ok(())
    }

    pub async fn admit(
        &mut self,
        path: &std::path::Path,
        key: &str,
        priority: Priority,
    ) -> rrss_ai::Result<()> {
        self.load(path).await?;
        self.expire_recovery();
        self.save(path).await?;
        let remaining = [key, "*"]
            .iter()
            .filter_map(|key| self.blocks.get(*key))
            .map(|block| {
                if priority == Priority::Interactive {
                    block.server_remaining()
                } else {
                    block.remaining()
                }
            })
            .max()
            .unwrap_or(0);
        if remaining > 0 {
            return Err(rrss_ai::Error::ServiceCooldown);
        }
        if self.damaged && priority == Priority::Background {
            if self.recovery_next.is_some_and(|at| at > Instant::now()) {
                return Err(rrss_ai::Error::ServiceCooldown);
            }
            self.recovery_next = Some(Instant::now() + RECOVERY);
        }
        Ok(())
    }

    pub async fn response(
        &mut self,
        path: &std::path::Path,
        key: &str,
        status: u16,
        retry: Option<&str>,
    ) -> rrss_ai::Result<()> {
        self.load(path).await?;
        self.expire_recovery();
        if !matches!(status, 429 | 503) {
            if self
                .blocks
                .get(key)
                .is_some_and(|block| block.remaining() == 0)
            {
                self.blocks.remove(key);
                self.dirty = true;
            }
            return self.save(path).await;
        }
        let now = now_ms();
        let anchor = Instant::now();
        // Capacity overflow remains conservative without creating unbounded keys.
        let key = if self.blocks.contains_key(key) || self.blocks.len() < LIMIT {
            key
        } else {
            "*"
        };
        let failures = self
            .blocks
            .get(key)
            .map_or(1, |b| b.saved.failures.saturating_add(1));
        let server_until =
            retry.and_then(|value| rrss_core::ingest::retry_after_deadline(value, now));
        let delay = RETRY_DELAYS[(failures.saturating_sub(1) as usize).min(RETRY_DELAYS.len() - 1)];
        let mut until =
            server_until.unwrap_or_else(|| now.saturating_add(delay.as_millis() as i64));
        let mut remaining_ms = until.saturating_sub(now).max(0) as u64;
        let mut server_remaining_ms =
            server_until.map_or(0, |until| until.saturating_sub(now).max(0) as u64);
        if let Some(block) = self.blocks.get(key) {
            remaining_ms = remaining_ms.max(block.remaining_at(anchor));
            until = until.max(block.saved.until);
            server_remaining_ms = server_remaining_ms.max(block.server_remaining_at(anchor));
        }
        let server_until =
            (server_remaining_ms > 0).then(|| now.saturating_add(server_remaining_ms as i64));
        until = until.max(server_until.unwrap_or(until));
        self.blocks.insert(
            key.into(),
            Block {
                saved: Saved {
                    until,
                    remaining_ms,
                    failures,
                    server_until,
                },
                anchor,
                server_remaining_ms,
            },
        );
        self.damaged = false;
        self.recovery_end = None;
        self.recovery_next = None;
        self.dirty = true;
        // Observing headers persists the limit, but must not replace the
        // original HTTP status seen by translation and other downstream users.
        self.save(path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn inferred_backoff_uses_short_tiers_and_only_blocks_background() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cooldowns.db");
        let key = rrss_core::ai_store::hash_input("https://model.test");
        let mut state = Cooldowns::default();
        for seconds in [60, 300, 900, 900] {
            state.response(&path, &key, 503, None).await.unwrap();
            assert!(
                state
                    .admit(&path, &key, Priority::Interactive)
                    .await
                    .is_ok()
            );
            assert!(matches!(
                state.admit(&path, &key, Priority::Background).await,
                Err(rrss_ai::Error::ServiceCooldown)
            ));
            tokio::time::advance(Duration::from_secs(seconds - 1)).await;
            assert!(
                state
                    .admit(&path, &key, Priority::Background)
                    .await
                    .is_err()
            );
            tokio::time::advance(Duration::from_secs(1)).await;
            assert!(state.admit(&path, &key, Priority::Background).await.is_ok());
        }
        state.response(&path, &key, 200, None).await.unwrap();
        state
            .response(&path, &key, 429, Some("invalid"))
            .await
            .unwrap();
        tokio::time::advance(Duration::from_secs(60)).await;
        assert!(state.admit(&path, &key, Priority::Background).await.is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn explicit_deadlines_survive_inferred_limits_and_expire_independently() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cooldowns.db");
        let key = rrss_core::ai_store::hash_input("https://model.test");
        let mut state = Cooldowns::default();
        state.response(&path, &key, 429, Some("600")).await.unwrap();
        state.response(&path, &key, 503, None).await.unwrap();
        tokio::time::advance(Duration::from_secs(300)).await;
        assert!(
            state
                .admit(&path, &key, Priority::Interactive)
                .await
                .is_err()
        );
        state.response(&path, &key, 503, None).await.unwrap();
        tokio::time::advance(Duration::from_secs(300)).await;
        assert!(
            state
                .admit(&path, &key, Priority::Interactive)
                .await
                .is_ok()
        );
        assert!(
            state
                .admit(&path, &key, Priority::Background)
                .await
                .is_err()
        );
        tokio::time::advance(Duration::from_secs(600)).await;
        assert!(state.admit(&path, &key, Priority::Background).await.is_ok());
    }

    #[tokio::test]
    async fn restart_preserves_local_and_server_deadline_distinction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cooldowns.db");
        let local = rrss_core::ai_store::hash_input("https://local.test");
        let server = rrss_core::ai_store::hash_input("https://server.test");
        let mut state = Cooldowns::default();
        state.response(&path, &local, 429, None).await.unwrap();
        state
            .response(&path, &server, 429, Some("600"))
            .await
            .unwrap();
        let mut restarted = Cooldowns::default();
        assert!(
            restarted
                .admit(&path, &local, Priority::Interactive)
                .await
                .is_ok()
        );
        assert!(
            restarted
                .admit(&path, &local, Priority::Background)
                .await
                .is_err()
        );
        assert!(
            restarted
                .admit(&path, &server, Priority::Interactive)
                .await
                .is_err()
        );
        assert!(
            restarted
                .admit(&path, &server, Priority::Background)
                .await
                .is_err()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn corrupt_recovery_is_background_only_expires_and_stays_expired_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cooldowns.db");
        let conn = db::open_at(&path).unwrap();
        db::set_setting(&conn, SETTING, "broken").unwrap();
        let key = rrss_core::ai_store::hash_input("https://model.test");
        let mut state = Cooldowns::default();
        assert!(
            state
                .admit(&path, &key, Priority::Background)
                .await
                .is_err()
        );
        assert!(
            state
                .admit(&path, &key, Priority::Interactive)
                .await
                .is_ok()
        );
        assert!(
            state
                .admit(&path, &key, Priority::Background)
                .await
                .is_err()
        );
        tokio::time::advance(RECOVERY_LIMIT).await;
        for _ in 0..2 {
            assert!(state.admit(&path, &key, Priority::Background).await.is_ok());
        }
        let mut restarted = Cooldowns::default();
        assert!(
            restarted
                .admit(&path, &key, Priority::Background)
                .await
                .is_ok()
        );
        assert!(!restarted.damaged);
    }

    #[tokio::test(start_paused = true)]
    async fn recovery_restart_retains_remaining_deadline_and_bounds_legacy_or_rolled_back_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cooldowns.db");
        let conn = db::open_at(&path).unwrap();
        let key = rrss_core::ai_store::hash_input("https://model.test");
        for (version, deadline, wait) in [
            (1, None, RECOVERY_LIMIT),
            (
                VERSION,
                Some(now_ms() + 3_600_000),
                Duration::from_secs(3600),
            ),
            (VERSION, Some(now_ms() + 86_400_000), RECOVERY_LIMIT),
        ] {
            let raw = serde_json::json!({"version": version, "blocks": {}, "damaged": true, "recovery_until": deadline}).to_string();
            db::set_setting(&conn, SETTING, &raw).unwrap();
            let mut restarted = Cooldowns::default();
            assert!(
                restarted
                    .admit(&path, &key, Priority::Background)
                    .await
                    .is_err()
            );
            tokio::time::advance(wait + Duration::from_secs(1)).await;
            assert!(
                restarted
                    .admit(&path, &key, Priority::Background)
                    .await
                    .is_ok()
            );
            assert!(!restarted.damaged);
        }
    }

    #[tokio::test]
    async fn restart_counts_downtime_without_discarding_unexpired_model_limits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cooldowns.db");
        let conn = db::open_at(&path).unwrap();
        let expired = rrss_core::ai_store::hash_input("https://expired.test");
        let active = rrss_core::ai_store::hash_input("https://active.test");
        let raw = serde_json::json!({
            "version": 1, "damaged": false, "blocks": {
                &expired: {"until": now_ms() - 1_000, "remaining_ms": 60_000, "failures": 1},
                &active: {"until": now_ms() + 3_600_000, "remaining_ms": 60_000, "failures": 1}
            }
        })
        .to_string();
        db::set_setting(&conn, SETTING, &raw).unwrap();
        let mut restarted = Cooldowns::default();
        assert!(
            restarted
                .admit(&path, &expired, Priority::Background)
                .await
                .is_ok()
        );
        assert!(matches!(
            restarted.admit(&path, &active, Priority::Background).await,
            Err(rrss_ai::Error::ServiceCooldown)
        ));
        assert!(
            matches!(
                restarted.admit(&path, &active, Priority::Interactive).await,
                Err(rrss_ai::Error::ServiceCooldown)
            ),
            "v1 deadlines may have come from Retry-After"
        );
        let upgraded: Snapshot =
            serde_json::from_str(&db::get_setting(&conn, SETTING).unwrap().unwrap()).unwrap();
        assert_eq!(upgraded.version, VERSION);
        assert!(upgraded.blocks[&active].server_until.is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn expired_cooldowns_and_corrupt_recovery_use_monotonic_time() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cooldowns.db");
        let key = rrss_core::ai_store::hash_input("https://model.test");
        let mut state = Cooldowns::default();
        state.response(&path, &key, 429, Some("60")).await.unwrap();
        assert!(
            state
                .admit(&path, &key, Priority::Background)
                .await
                .is_err()
        );
        tokio::time::advance(Duration::from_secs(60)).await;
        assert!(state.admit(&path, &key, Priority::Background).await.is_ok());
        let conn = db::open_at(&path).unwrap();
        db::set_setting(&conn, SETTING, "broken").unwrap();
        let mut damaged = Cooldowns::default();
        assert!(
            damaged
                .admit(&path, &key, Priority::Background)
                .await
                .is_err()
        );
        assert!(damaged.damaged);
        tokio::time::advance(RECOVERY).await;
        assert!(
            damaged
                .admit(&path, &key, Priority::Background)
                .await
                .is_ok()
        );
        assert!(
            damaged
                .admit(&path, &key, Priority::Background)
                .await
                .is_err()
        );
        let mut restarted = Cooldowns::default();
        assert!(
            restarted
                .admit(&path, &key, Priority::Background)
                .await
                .is_err()
        );
        assert!(restarted.damaged, "recovery marker survives restart");
    }
}
