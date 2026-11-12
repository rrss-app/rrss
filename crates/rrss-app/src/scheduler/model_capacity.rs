//! Per-database endpoint capacity. Weak registries discover shared ownership;
//! requests and the network runtime own the actual state, never a global worker.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};
use tokio::sync::Notify;

const MAX_WAITERS: usize = 64;
const BACKGROUND_REQUESTS: usize = 1;

// Space, endpoint and configuration fingerprint -> last failed probe.
type ProbeChecks = std::collections::BTreeMap<(Option<i64>, String, String), tokio::time::Instant>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Priority {
    Interactive,
    Background,
}

pub(crate) struct Capacity {
    pub(crate) metrics: super::diagnostics::ModelMetrics,
    // None means the finite summary candidate snapshot is being selected.
    pub(crate) summaries: tokio::sync::watch::Sender<Option<HashSet<i64>>>,
    pub(crate) derived: tokio::sync::watch::Sender<u64>,
    pub(crate) stopped: tokio::sync::watch::Sender<u64>,
    pub(crate) objects: Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>,
    pub(crate) digests: super::digest_jobs::Digests,
    pub(crate) active_work: tokio::sync::watch::Sender<usize>,
    endpoints: Mutex<HashMap<String, Weak<Endpoint>>>,
    cooldowns: Arc<super::model_cooldown::Store>,
    background: Arc<tokio::sync::Semaphore>,
    requests: Arc<tokio::sync::Semaphore>,
    pub(crate) content: Arc<Endpoint>,
    pub(crate) local: tokio::sync::Mutex<()>,
    // 入库后的增量排序：同一时刻只算一次，其间到达的文章并成下一批。
    pub(crate) rank: crate::jobs::rank::Batches,
    pub(crate) images: tokio::sync::Mutex<()>,
    pub(crate) checks: tokio::sync::Mutex<ProbeChecks>,
    pub(crate) embedding: Mutex<Option<EmbeddingRun>>,
    pub(crate) retries: Mutex<crate::jobs::retry::Retries>,
    pub(crate) retry_revision: std::sync::atomic::AtomicU64,
}

pub(crate) struct EmbeddingRun {
    pub epoch: u64,
    pub key: String,
    pub result:
        tokio::sync::watch::Sender<Option<std::result::Result<(), super::digest_jobs::Failure>>>,
}

impl Default for Capacity {
    fn default() -> Self {
        Self {
            metrics: Default::default(),
            summaries: tokio::sync::watch::channel(Some(HashSet::new())).0,
            derived: tokio::sync::watch::channel(0).0,
            stopped: tokio::sync::watch::channel(0).0,
            objects: Mutex::default(),
            digests: super::digest_jobs::Digests::default(),
            active_work: tokio::sync::watch::channel(0).0,
            endpoints: Mutex::default(),
            cooldowns: Arc::default(),
            background: Arc::new(tokio::sync::Semaphore::new(BACKGROUND_REQUESTS)),
            requests: Arc::new(tokio::sync::Semaphore::new(MAX_WAITERS + 1)),
            content: Arc::new(Endpoint {
                limit: 8,
                ..Endpoint::default()
            }),
            local: tokio::sync::Mutex::new(()),
            rank: Default::default(),
            images: tokio::sync::Mutex::new(()),
            checks: tokio::sync::Mutex::new(std::collections::BTreeMap::new()),
            embedding: Mutex::new(None),
            retries: Mutex::default(),
            retry_revision: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl Capacity {
    pub(super) fn diagnostics(&self) -> crate::views::ModelDiagnostics {
        use std::sync::atomic::Ordering;
        let mut active_endpoints = 0;
        let mut busiest_endpoint = 0;
        for endpoint in self
            .endpoints
            .lock()
            .unwrap()
            .values()
            .filter_map(Weak::upgrade)
        {
            let active = endpoint.queue.lock().unwrap().active.len();
            active_endpoints += usize::from(active > 0);
            busiest_endpoint = busiest_endpoint.max(active);
        }
        let content = self.content.queue.lock().unwrap();
        let m = &self.metrics;
        crate::views::ModelDiagnostics {
            cooldowns: self.cooldowns.diagnostics(),
            active: m.active.current(),
            peak_active: m.active.peak(),
            waiting_interactive: m.interactive.current(),
            waiting_background: m.background.current(),
            active_endpoints,
            busiest_endpoint,
            background_active: BACKGROUND_REQUESTS
                .saturating_sub(self.background.available_permits()),
            content_active: content.active.len(),
            content_waiting: content.waiting.len(),
            wait: m.wait.snapshot().into(),
            held: m.held.snapshot().into(),
            http_429: m.rate_limited.load(Ordering::Relaxed),
            http_503: m.unavailable.load(Ordering::Relaxed),
            cooldown_rejections: m.cooldown.load(Ordering::Relaxed),
            budget_rejections: m.budget.load(Ordering::Relaxed),
        }
    }
    pub(crate) fn stop(&self) {
        self.stopped.send_modify(|epoch| *epoch += 1);
        self.summaries.send_replace(Some(HashSet::new()));
    }
    pub(crate) fn object(&self, key: String) -> Arc<tokio::sync::Mutex<()>> {
        let mut objects = self.objects.lock().unwrap();
        objects.retain(|_, lock| lock.strong_count() > 0);
        let entry = objects.entry(key).or_default();
        if let Some(lock) = entry.upgrade() {
            return lock;
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        *entry = Arc::downgrade(&lock);
        lock
    }

    pub(crate) fn work(self: &Arc<Self>) -> Work {
        self.active_work.send_modify(|count| *count += 1);
        Work(self.clone())
    }
    pub(crate) fn for_database(path: &Path) -> Arc<Self> {
        static DATABASES: OnceLock<Mutex<HashMap<PathBuf, Weak<Capacity>>>> = OnceLock::new();
        let mut databases = DATABASES.get_or_init(Mutex::default).lock().unwrap();
        databases.retain(|_, capacity| capacity.strong_count() > 0);
        let entry = databases.entry(path.to_owned()).or_default();
        if let Some(capacity) = entry.upgrade() {
            return capacity;
        }
        let capacity = Arc::new(Self::default());
        *entry = Arc::downgrade(&capacity);
        capacity
    }

    fn endpoint(&self, base: &str) -> rrss_ai::Result<Arc<Endpoint>> {
        let url = reqwest::Url::parse(base.trim())
            .map_err(|_| rrss_ai::Error::Configuration("模型端点地址无效"))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(rrss_ai::Error::Configuration("模型端点地址无效"));
        }
        // The service origin unifies /v1, completions and embedding paths and
        // excludes credentials, query strings, model names and API keys.
        let key = url.origin().ascii_serialization();
        let mut endpoints = self.endpoints.lock().unwrap();
        endpoints.retain(|_, endpoint| endpoint.strong_count() > 0);
        let entry = endpoints.entry(key).or_default();
        if let Some(endpoint) = entry.upgrade() {
            return Ok(endpoint);
        }
        let endpoint = Arc::new(Endpoint::default());
        *entry = Arc::downgrade(&endpoint);
        Ok(endpoint)
    }
}

pub(crate) struct Work(Arc<Capacity>);
impl Drop for Work {
    fn drop(&mut self) {
        self.0.active_work.send_modify(|count| *count -= 1);
    }
}

#[derive(Default)]
struct Queue {
    sequence: u64,
    active: HashSet<u64>,
    waiting: VecDeque<(u64, Priority)>,
}

pub(crate) struct Endpoint {
    limit: usize,
    queue: Mutex<Queue>,
    pub(crate) changed: Notify,
}

pub(crate) struct Permit {
    endpoint: Arc<Endpoint>,
    id: u64,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut queue = self.endpoint.queue.lock().unwrap();
        queue.waiting.retain(|(id, _)| *id != self.id);
        queue.active.remove(&self.id);
        drop(queue);
        self.endpoint.changed.notify_waiters();
    }
}

impl Default for Endpoint {
    fn default() -> Self {
        Self {
            limit: 1,
            queue: Mutex::default(),
            changed: Notify::new(),
        }
    }
}

impl Endpoint {
    pub(crate) async fn acquire(self: Arc<Self>, priority: Priority) -> rrss_ai::Result<Permit> {
        let id = {
            let mut queue = self.queue.lock().unwrap();
            if queue.waiting.len() >= MAX_WAITERS {
                return Err(rrss_ai::Error::Capacity);
            }
            queue.sequence += 1;
            let id = queue.sequence;
            queue.waiting.push_back((id, priority));
            id
        };
        // The same guard removes a cancelled waiter or releases an active request.
        let permit = Permit {
            endpoint: self.clone(),
            id,
        };
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut queue = self.queue.lock().unwrap();
                let next = queue
                    .waiting
                    .iter()
                    .find(|(_, priority)| *priority == Priority::Interactive)
                    .or_else(|| queue.waiting.front())
                    .map(|(id, _)| *id);
                if queue.active.len() < self.limit && next == Some(id) {
                    queue.waiting.retain(|(waiting, _)| *waiting != id);
                    queue.active.insert(id);
                    self.changed.notify_waiters();
                    return Ok(permit);
                }
            }
            changed.await;
        }
    }
}

pub(crate) struct RequestAdmission {
    _capacity: Arc<Capacity>,
    endpoint: Arc<Endpoint>,
    priority: Priority,
    budget_path: Option<PathBuf>,
    pub(crate) budget_kind: rrss_core::ai_store::BudgetKind,
    path: PathBuf,
    service: String,
    pub(crate) space: Option<(
        PathBuf,
        Option<i64>,
        Option<rrss_core::ai_store::EmbedEndpoint>,
    )>,
}

impl RequestAdmission {
    pub(crate) fn new(
        path: &Path,
        base: &str,
        priority: Priority,
        auto: bool,
    ) -> rrss_ai::Result<Self> {
        let capacity = Capacity::for_database(path);
        Ok(Self {
            endpoint: capacity.endpoint(base)?,
            path: path.to_owned(),
            service: rrss_core::ai_store::hash_input(
                &reqwest::Url::parse(base.trim())
                    .map_err(|_| rrss_ai::Error::Configuration("模型端点地址无效"))?
                    .origin()
                    .ascii_serialization(),
            ),
            _capacity: capacity,
            priority,
            budget_path: auto.then(|| path.to_owned()),
            budget_kind: rrss_core::ai_store::BudgetKind::Generation,
            space: None,
        })
    }
}

impl rrss_ai::admission::Admission for RequestAdmission {
    fn response(
        &self,
        status: u16,
        retry_after: Option<String>,
    ) -> rrss_ai::admission::Response<'_> {
        // Persist received limits even if the request's deadline/IPC waiter is
        // cancelled. Subsequent admissions share this serial persistence barrier.
        let capacity = self._capacity.clone();
        match status {
            429 => {
                capacity
                    .metrics
                    .rate_limited
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            503 => {
                capacity
                    .metrics
                    .unavailable
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            _ => {}
        }
        let recording = capacity.cooldowns.recording();
        let work = capacity.work();
        let path = self.path.clone();
        let service = self.service.clone();
        let task = tokio::spawn(async move {
            let _work = work;
            let _recording = recording;
            capacity
                .cooldowns
                .response(&path, &service, status, retry_after.as_deref())
                .await
        });
        Box::pin(async move {
            task.await
                .map_err(|_| rrss_ai::Error::Admission("模型限流记录写入中断"))?
        })
    }

    fn acquire(&self) -> rrss_ai::admission::Acquisition<'_> {
        Box::pin(async move {
            let waiting = self
                ._capacity
                .requests
                .clone()
                .try_acquire_owned()
                .map_err(|_| rrss_ai::Error::Capacity)?;
            let queue_timer = self._capacity.metrics.wait.start();
            let queued = match self.priority {
                Priority::Interactive => self._capacity.metrics.interactive.enter(),
                Priority::Background => self._capacity.metrics.background.enter(),
            };
            let background = if self.priority == Priority::Background {
                Some(
                    self._capacity
                        .background
                        .clone()
                        .acquire_owned()
                        .await
                        .map_err(|_| rrss_ai::Error::Admission("模型运行时已停止"))?,
                )
            } else {
                None
            };
            let permit = self.endpoint.clone().acquire(self.priority).await?;
            // Wait without a database connection, then recheck immediately before
            // each HTTP attempt, including fallback and format-repair requests.
            if let Some(path) = &self.budget_path {
                let kind = self.budget_kind;
                super::super::jobs::spawn_db(path.clone(), move |conn| {
                    rrss_core::ai_store::ensure_auto_job_budget(conn, kind)
                })
                .await
                .map_err(|error| {
                    if error
                        .downcast_ref::<rrss_core::Error>()
                        .is_some_and(|error| {
                            matches!(error, rrss_core::Error::DailyBudgetExhausted)
                        })
                    {
                        self._capacity
                            .metrics
                            .budget
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        rrss_ai::Error::BudgetExhausted
                    } else {
                        rrss_ai::Error::Admission("无法核对自动模型预算")
                    }
                })?;
            }
            if let Some((path, id, endpoint)) = &self.space {
                let id = *id;
                let endpoint = endpoint.clone();
                let current = crate::jobs::spawn_db(path.clone(), move |conn| {
                    let space = rrss_core::vector_space::building(conn)?
                        .or(rrss_core::vector_space::active(conn)?);
                    let binding = rrss_core::ai_store::resolve_embed(conn)?;
                    Ok(space.map(|space| space.id) == id
                        && match (binding, endpoint) {
                            (None, None) => true,
                            (Some(binding), Some(endpoint)) => {
                                binding.base_url == endpoint.base_url
                                    && binding.model == endpoint.model
                                    && binding.api_key == endpoint.api_key
                            }
                            _ => false,
                        })
                })
                .await
                .map_err(|_| rrss_ai::Error::Admission("无法核对向量空间"))?;
                if !current {
                    return Err(rrss_ai::Error::Admission(
                        "向量空间或模型已改变，已停止旧批次",
                    ));
                }
            }
            self._capacity
                .cooldowns
                .admit(&self.path, &self.service, self.priority)
                .await
                .map_err(|error| {
                    if matches!(error, rrss_ai::Error::ServiceCooldown) {
                        self._capacity
                            .metrics
                            .cooldown
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    error
                })?;
            drop(queue_timer);
            drop(queued);
            let measured = super::diagnostics::ModelLease {
                _active: self._capacity.metrics.active.enter(),
                held: self._capacity.metrics.held.clone(),
                started: std::time::Instant::now(),
            };
            Ok(Box::new((measured, permit, background, waiting)) as rrss_ai::admission::Permit)
        })
    }
}

#[cfg(test)]
mod tests;
