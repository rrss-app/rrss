//! Subscription connectivity only. Failures await a bounded observation window
//! before health writes; probes use the exact Content/feed client and no body.
use super::*;
use std::collections::{HashMap, VecDeque};
use tokio::time::Instant;

pub(super) const UNAVAILABLE: &str = "网络不可用；如使用代理，请检查代理设置";

#[derive(Clone, Copy)]
pub(super) struct Policy {
    pub window: Duration,
    pub hosts: usize,
    pub targets: usize,
    pub timeout: Duration,
    pub first_interval: Duration,
    pub max_interval: Duration,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            window: Duration::from_secs(2 * 60),
            // Without a recent success, one network-layer failure is enough to
            // probe. Subscriptions concentrated on one or two hosts (a single
            // RSSHub instance) could never reach a multi-host threshold, and
            // the HEAD probe of recent hosts already tells one dead site from
            // an outage.
            hosts: 1,
            targets: 3,
            timeout: Duration::from_secs(5),
            first_interval: Duration::from_secs(30),
            max_interval: Duration::from_secs(5 * 60),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Online,
    Checking,
    Offline,
}

pub(super) struct Connectivity {
    phase: Phase,
    last_reachable: Option<Instant>,
    failures: HashMap<String, Instant>,
    recent: VecDeque<reqwest::Url>,
    next_probe: Option<Instant>,
    interval: Duration,
    probe_revision: u64,
    policy: Policy,
}

fn host(url: &reqwest::Url) -> String {
    url.host_str().unwrap_or_default().to_ascii_lowercase()
}

impl Connectivity {
    pub fn new(policy: Policy) -> Self {
        Self {
            phase: Phase::Online,
            last_reachable: None,
            failures: HashMap::new(),
            recent: VecDeque::new(),
            next_probe: None,
            interval: Duration::ZERO,
            probe_revision: 0,
            policy,
        }
    }

    pub fn paused(&self) -> bool {
        self.phase != Phase::Online
    }

    pub fn unavailable(&self) -> bool {
        self.phase == Phase::Offline
    }

    pub fn reason(&self) -> Option<String> {
        self.unavailable().then(|| UNAVAILABLE.into())
    }

    pub fn has_success(&self, now: Instant) -> bool {
        self.last_reachable
            .is_some_and(|last| now.saturating_duration_since(last) <= self.policy.window)
    }

    /// The hostname (without port) prevents one site with several ports from
    /// being mistaken for several independent failing subscription hosts.
    pub fn failure(&mut self, url: &str, now: Instant) -> bool {
        if self.has_success(now) && !self.paused() {
            return false;
        }
        self.failures
            .retain(|_, at| now.saturating_duration_since(*at) <= self.policy.window);
        if let Ok(url) = reqwest::Url::parse(url) {
            self.failures.insert(host(&url), now);
        }
        if self.phase == Phase::Online && self.failures.len() >= self.policy.hosts {
            self.phase = Phase::Checking;
        }
        true
    }

    /// Any HTTP response is evidence of a working route. Only a committed
    /// 200/304 is added to the recent-success targets, separately below.
    pub fn reachable(&mut self, now: Instant) -> bool {
        if self.phase == Phase::Offline {
            tracing::info!("subscription network recovered");
        }
        let resumed = self.paused();
        self.phase = Phase::Online;
        self.last_reachable = Some(now);
        self.failures.clear();
        self.next_probe = None;
        self.interval = Duration::ZERO;
        self.probe_revision += 1;
        resumed
    }

    pub fn succeeded(&mut self, url: &str) {
        if let Ok(url) = reqwest::Url::parse(url) {
            let key = host(&url);
            self.recent.retain(|old| host(old) != key);
            self.recent.push_front(url);
            self.recent.truncate(self.policy.targets);
        }
    }

    fn failed_probe(&mut self, now: Instant) {
        if self.phase != Phase::Offline {
            tracing::warn!(error_kind = "offline", "subscription network unavailable");
        } else {
            tracing::debug!(
                error_kind = "offline",
                "subscription network still unavailable"
            );
        }
        self.phase = Phase::Offline;
        self.interval = if self.interval.is_zero() {
            self.policy.first_interval
        } else {
            self.interval
                .saturating_mul(2)
                .min(self.policy.max_interval)
        };
        self.next_probe = Some(now + self.interval);
        self.probe_revision += 1;
    }

    pub fn probe_due(&self, now: Instant) -> bool {
        self.phase == Phase::Checking || self.next_probe.is_some_and(|due| due <= now)
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.next_probe
    }
}

async fn recent_targets(path: PathBuf, limit: usize) -> Result<Vec<reqwest::Url>> {
    crate::jobs::spawn_db(path, move |conn| {
        let mut targets = Vec::new();
        let mut cursor = (i64::MAX, i64::MAX);
        loop {
            let page = conn
                .prepare(
                    "SELECT last_success_at,id,url FROM feeds WHERE kind!='manual'
                AND last_success_at IS NOT NULL AND (last_success_at,id)<(?1,?2)
                ORDER BY last_success_at DESC,id DESC LIMIT 64",
                )?
                .query_map(rusqlite::params![cursor.0, cursor.1], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if page.is_empty() {
                break;
            }
            for (success, id, address) in page {
                cursor = (success, id);
                if let Ok(url) = reqwest::Url::parse(&address)
                    && matches!(url.scheme(), "http" | "https")
                    && url.host_str().is_some()
                    && targets.iter().all(|old| host(old) != host(&url))
                {
                    targets.push(url);
                    if targets.len() == limit {
                        return Ok(targets);
                    }
                }
            }
        }
        Ok(targets)
    })
    .await
}

impl super::runtime::Shared {
    pub(super) fn network_paused(&self) -> bool {
        self.connectivity.lock().unwrap().paused()
    }

    pub(super) fn network_response(&self) {
        if self.connectivity.lock().unwrap().reachable(Instant::now()) {
            self.wake.notify_one();
            self.notify_status();
        }
    }

    pub(super) async fn check_network(self: &Arc<Self>, force: bool) -> bool {
        let revision = self.connectivity.lock().unwrap().probe_revision;
        // A manual refresh shares an in-progress probe instead of adding HEAD
        // requests, but an idle offline state is probed immediately.
        let _owner = self.probe_lock.lock().await;
        {
            let state = self.connectivity.lock().unwrap();
            if !state.paused() {
                return true;
            }
            if state.probe_revision != revision {
                return !state.unavailable();
            }
            if !force && !state.probe_due(Instant::now()) {
                return false;
            }
        }
        let (policy, mut targets) = {
            let state = self.connectivity.lock().unwrap();
            (
                state.policy,
                state.recent.iter().cloned().collect::<Vec<_>>(),
            )
        };
        if targets.len() < policy.targets
            && let Ok(saved) = recent_targets(self.path.clone(), policy.targets).await
        {
            for target in saved {
                if targets.iter().all(|old| host(old) != host(&target)) {
                    targets.push(target);
                }
                if targets.len() == policy.targets {
                    break;
                }
            }
        }
        // One previously healthy host can itself be down. Keep at least two
        // independent probe destinations before concluding the network is down.
        let minimum = policy.targets.min(2);
        if targets.len() < minimum {
            for fallback in [
                "http://captive.apple.com/",
                "http://www.msftconnecttest.com/connecttest.txt",
            ] {
                let target = reqwest::Url::parse(fallback).unwrap();
                if targets.iter().all(|old| host(old) != host(&target)) {
                    targets.push(target);
                }
                if targets.len() >= minimum {
                    break;
                }
            }
        }
        let mut checks = tokio::task::JoinSet::new();
        if let Ok(client) = self.network.feed_client().await {
            for target in targets.into_iter().take(policy.targets) {
                let client = client.clone();
                checks.spawn(async move {
                    let reachable = client
                        .head(target.clone())
                        .timeout(policy.timeout)
                        .send()
                        .await
                        .is_ok();
                    tracing::debug!(
                        host = rrss_core::logsafe::host(&target),
                        reachable,
                        "subscription HEAD probe"
                    );
                    reachable
                });
            }
        }
        let mut reachable = false;
        while let Some(check) = checks.join_next().await {
            if check.unwrap_or(false) {
                reachable = true;
                break;
            }
        }
        checks.shutdown().await;
        let offline = {
            let mut state = self.connectivity.lock().unwrap();
            // A feed may have succeeded while the probe was waiting. Its HTTP
            // response wins over a late failed probe.
            if reachable || state.has_success(Instant::now()) {
                state.reachable(Instant::now());
                false
            } else {
                state.failed_probe(Instant::now());
                true
            }
        };
        if offline {
            self.pause_network_waiters();
        }
        self.wake.notify_one();
        self.notify_status();
        !offline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_confirmed_outages_and_recoveries_log_transitions() {
        let log = crate::scheduler::tests::capture_logs(|| {
            let mut state = Connectivity::new(Policy::default());
            let now = Instant::now();
            state.failure("https://user:secret@one.test/?token=canary", now);
            state.reachable(now);
            state.failed_probe(now);
            state.failed_probe(now);
            state.reachable(now);
            state.reachable(now);
        });
        assert_eq!(
            log.matches("subscription network unavailable").count(),
            1,
            "{log}"
        );
        assert_eq!(
            log.matches("subscription network still unavailable")
                .count(),
            1,
            "{log}"
        );
        assert_eq!(
            log.matches("subscription network recovered").count(),
            1,
            "{log}"
        );
        assert!(!log.contains("canary") && !log.contains("secret"));
    }

    #[test]
    fn default_probes_first_failure_without_recent_success() {
        let start = Instant::now();
        let mut state = Connectivity::new(Policy::default());
        assert!(state.failure("http://only.test/feed", start));
        assert!(state.paused() && !state.unavailable() && state.probe_due(start));
        state.reachable(start);
        assert!(!state.failure("http://only.test/feed", start));
        assert!(!state.paused());
    }

    #[test]
    fn distinct_hosts_and_success_window_gate_probes_and_interval_is_capped() {
        let mut state = Connectivity::new(Policy {
            hosts: 3,
            ..Policy::default()
        });
        let start = Instant::now();
        state.failure("http://one.test:80/a", start);
        state.failure("http://one.test:81/b", start);
        assert!(!state.paused());
        state.reachable(start);
        for host in ["a.test", "b.test", "c.test"] {
            assert!(!state.failure(&format!("http://{host}/"), start));
        }
        assert!(!state.paused());
        let later = start + Policy::default().window + Duration::from_secs(1);
        for host in ["a.test", "b.test", "c.test"] {
            state.failure(&format!("http://{host}/"), later);
        }
        assert!(state.paused());
        let mut now = later;
        for seconds in [30, 60, 120, 240, 300, 300] {
            state.failed_probe(now);
            assert_eq!(state.deadline(), Some(now + Duration::from_secs(seconds)));
            now += Duration::from_secs(seconds);
        }
        state.reachable(now);
        assert!(!state.paused());
        assert_eq!(state.deadline(), None);
    }
}
