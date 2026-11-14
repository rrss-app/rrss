use super::*;
use rrss_core::ingest::feed_http::{FeedPolicy, FetchFailure};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, Weak};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, watch};
use tokio::task::JoinSet;
use tracing::Instrument;

const GLOBAL_REQUESTS: usize = 24;
const PER_HOST: usize = rrss_core::db::FEED_FETCH_PER_HOST_DEFAULT;
const PARSE_WORKERS: usize = 2;
const BUFFER_BYTES: usize = 32 * 1024 * 1024;
// Body-free results only need short bookkeeping writes, independently of parsing.
const RESULT_WORKERS: usize = 1;
pub(super) const PENDING_LIMIT: usize = 1024;
const WAITERS_PER_FEED: usize = 32;
const METADATA_BATCH: usize = 64;

#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub global: usize,
    pub host: usize,
    pub parsing: usize,
    pub buffer_bytes: usize,
    pub pending: usize,
    pub policy: FeedPolicy,
    pub offline: super::offline::Policy,
    /// 每站同时下载数随设置变化（生产入口）；测试传入的固定额度不受库里设置影响。
    pub host_from_settings: bool,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            global: GLOBAL_REQUESTS,
            host: PER_HOST,
            parsing: PARSE_WORKERS,
            buffer_bytes: BUFFER_BYTES,
            pending: PENDING_LIMIT,
            policy: FeedPolicy::default(),
            offline: super::offline::Policy::default(),
            host_from_settings: false,
        }
    }
}

pub(super) struct Entry {
    pub enqueued: tokio::time::Instant,
    pub dispatched: bool,
    pub generation: u64,
    pub slot: watch::Sender<Option<RefreshResult>>,
    pub added: bool,
    pub keep_due: bool,
    pub parked: Option<String>,
    /// A thawed claim reserves its host's free slot until metadata preparation
    /// either takes a download permit or parks it again.
    pub resuming: Option<String>,
}
pub(super) struct Queue {
    pub pending: VecDeque<i64>,
    pub added: VecDeque<i64>,
    /// Claims waiting for a full host, keyed by `host_key`. They return to the
    /// front of `pending` only when that host releases a permit, so a long
    /// same-host backlog is not re-read from the database after every download.
    parked: HashMap<String, VecDeque<(i64, u64)>>,
    pub entries: HashMap<i64, Entry>,
    /// Claims returned to the runnable queue reserve a free host slot until
    /// metadata preparation consumes or releases the reservation.
    resuming: HashMap<String, usize>,
    pub stopped: bool,
    pub generation: u64,
    pub operations: super::operations::Operations,
}

pub struct FetchTicket(watch::Receiver<Option<RefreshResult>>);
impl FetchTicket {
    pub async fn wait(mut self) -> Result<RefreshResult> {
        loop {
            if let Some(result) = self.0.borrow_and_update().clone() {
                return Ok(result);
            }
            self.0
                .changed()
                .await
                .map_err(|_| anyhow::anyhow!("抓取运行时已停止"))?;
        }
    }
}

pub(super) struct Shared {
    pub path: PathBuf,
    pub network: NetworkClients,
    pub connectivity: Mutex<super::offline::Connectivity>,
    pub probe_lock: tokio::sync::Mutex<()>,
    pub cooldowns: Mutex<super::cooldown::Cooldowns>,
    pub cooldown_store: Arc<super::cooldown::Store>,
    pub cooldown_ready: watch::Sender<bool>,
    pub clock: Arc<dyn super::cooldown::Clock>,
    recovery: Arc<Semaphore>,
    pub capacity: Arc<super::model_capacity::Capacity>,
    pub queue: Mutex<Queue>,
    pub wake: Arc<Notify>,
    space: Notify,
    pub(super) metrics: super::diagnostics::Metrics,
    pub(super) started: std::time::Instant,
    pub(super) background_diagnostics: Mutex<super::background::schedule::Diagnostics>,
    /// Feed rows read for dispatch; tests use it to bound same-host rescans.
    #[cfg(test)]
    lookups: AtomicUsize,
    pub refresh_admission: Arc<Semaphore>,
    pub run_id: String,
    pub revision: std::sync::atomic::AtomicU64,
    pub observer: std::sync::OnceLock<Arc<dyn Fn(views::SchedulerSnapshot) + Send + Sync>>,
    global: Arc<Semaphore>,
    /// A single added source may bypass byte backpressure. Its lease lasts
    /// through parsing/commit, bounding this exception to one extra body.
    added_body: Arc<Semaphore>,
    hosts: Mutex<HashMap<String, Weak<Semaphore>>>,
    pub manual: Arc<Semaphore>,
    pub manual_jobs: Mutex<HashMap<String, super::background::ManualResult>>,
    pub dirty: watch::Sender<u64>,
    pub automatic: watch::Sender<bool>,
    pub completed: watch::Sender<u64>,
    pub background_status: watch::Sender<Vec<views::BackgroundJobView>>,
    pub loop_running: std::sync::atomic::AtomicBool,
    icons: watch::Sender<u64>,
    pub stop: watch::Sender<bool>,
    sink: Arc<dyn Fn(views::FeedUpdated) + Send + Sync>,
    pub events: Notify,
    pub event_pending: Mutex<HashMap<Option<i64>, views::FeedUpdated>>,
    pub limits: Limits,
    // 当前每站同时下载数。新建（或空闲后重建）的主机名额按它分配。
    host_limit: AtomicUsize,
}

// The workers own Shared, not Lifetime. Dropping the last public handle stops them.
struct Lifetime(watch::Sender<bool>, Arc<super::model_capacity::Capacity>);
type Registry = Mutex<HashMap<PathBuf, (Weak<Shared>, Weak<Lifetime>)>>;
static RUNTIMES: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
impl Drop for Lifetime {
    fn drop(&mut self) {
        if self.0.send_if_modified(|stopped| {
            if *stopped {
                false
            } else {
                *stopped = true;
                true
            }
        }) {
            self.1.stop();
        }
    }
}
#[derive(Clone)]
pub struct Scheduler {
    pub(super) shared: Arc<Shared>,
    _lifetime: Arc<Lifetime>,
}

impl Scheduler {
    pub fn start(
        path: PathBuf,
        network: NetworkClients,
        sink: impl Fn(views::FeedUpdated) + Send + Sync + 'static,
    ) -> Self {
        let mut runtimes = RUNTIMES.get_or_init(Mutex::default).lock().unwrap();
        runtimes.retain(|_, (_, lifetime)| lifetime.strong_count() > 0);
        if let Some((shared, lifetime)) = runtimes.get(&path)
            && let (Some(shared), Some(lifetime)) = (shared.upgrade(), lifetime.upgrade())
            && !*shared.stop.borrow()
        {
            return Self {
                shared,
                _lifetime: lifetime,
            };
        }
        let limits = Limits {
            host_from_settings: true,
            ..Limits::default()
        };
        let runtime = Self::with_limits(path.clone(), network, sink, limits);
        runtimes.insert(
            path,
            (
                Arc::downgrade(&runtime.shared),
                Arc::downgrade(&runtime._lifetime),
            ),
        );
        runtime
    }
    pub(crate) fn diagnostics_for(path: &std::path::Path) -> Option<views::SchedulerDiagnostics> {
        let shared = RUNTIMES.get()?.lock().unwrap().get(path)?.0.upgrade()?;
        Some(shared.diagnostics())
    }
    pub(super) fn with_limits(
        path: PathBuf,
        network: NetworkClients,
        sink: impl Fn(views::FeedUpdated) + Send + Sync + 'static,
        limits: Limits,
    ) -> Self {
        Self::with_clock(
            path,
            network,
            sink,
            limits,
            Arc::new(super::cooldown::SystemClock),
        )
    }

    pub(super) fn with_clock(
        path: PathBuf,
        network: NetworkClients,
        sink: impl Fn(views::FeedUpdated) + Send + Sync + 'static,
        limits: Limits,
        clock: Arc<dyn super::cooldown::Clock>,
    ) -> Self {
        let (dirty, _) = watch::channel(0);
        let (completed, _) = watch::channel(0);
        let (stop, _) = watch::channel(false);
        let (icons, _) = watch::channel(0);
        let cooldown_store = super::cooldown::Store::for_database(&path);
        let shared = Arc::new(Shared {
            capacity: super::model_capacity::Capacity::for_database(&path),
            path,
            network,
            connectivity: Mutex::new(super::offline::Connectivity::new(limits.offline)),
            probe_lock: tokio::sync::Mutex::new(()),
            cooldowns: Mutex::new(super::cooldown::Cooldowns::new(clock.now())),
            cooldown_store,
            cooldown_ready: watch::channel(false).0,
            clock,
            recovery: Arc::new(Semaphore::new(1)),
            queue: Mutex::new(Queue {
                pending: VecDeque::new(),
                added: VecDeque::new(),
                parked: HashMap::new(),
                entries: HashMap::new(),
                resuming: HashMap::new(),
                stopped: false,
                generation: 0,
                operations: super::operations::Operations::default(),
            }),
            refresh_admission: Arc::new(Semaphore::new(8)),
            revision: std::sync::atomic::AtomicU64::new(0),
            observer: std::sync::OnceLock::new(),
            run_id: format!(
                "{}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos(),
                next_run()
            ),
            wake: Arc::new(Notify::new()),
            space: Notify::new(),
            metrics: Default::default(),
            started: std::time::Instant::now(),
            background_diagnostics: Mutex::default(),
            #[cfg(test)]
            lookups: AtomicUsize::new(0),
            global: Arc::new(Semaphore::new(limits.global)),
            host_limit: AtomicUsize::new(limits.host),
            added_body: Arc::new(Semaphore::new(1)),
            hosts: Mutex::new(HashMap::new()),
            manual: Arc::new(Semaphore::new(32)),
            manual_jobs: Mutex::new(HashMap::new()),
            dirty,
            automatic: watch::channel(false).0,
            completed,
            background_status: watch::channel(Vec::new()).0,
            loop_running: std::sync::atomic::AtomicBool::new(false),
            icons,
            stop: stop.clone(),
            sink: Arc::new(sink),
            events: Notify::new(),
            event_pending: Mutex::default(),
            limits,
        });
        let work = shared.capacity.work();
        let events = shared.clone();
        tokio::spawn(async move {
            let _work = work;
            run_events(events).await;
        });
        let work = shared.capacity.work();
        let feeds = shared.clone();
        tokio::spawn(async move {
            let _work = work;
            drive(feeds).await;
        });
        let work = shared.capacity.work();
        let icons = shared.clone();
        tokio::spawn(async move {
            let _work = work;
            run_icons(icons).await;
        });
        let work = shared.capacity.work();
        let background = shared.clone();
        tokio::spawn(async move {
            let _work = work;
            super::background::drive(background).await;
        });
        let work = shared.capacity.work();
        let images = shared.clone();
        tokio::spawn(async move {
            let _work = work;
            let mut stop = images.stop.subscribe();
            if let Some(data_dir) = images.path.parent() {
                tokio::select! {
                    biased;
                    _ = stop.wait_for(|stopped| *stopped) => {},
                    _ = crate::image_preload::run(&images.path, data_dir, &images.network) => {},
                }
            }
        });
        Self {
            shared: shared.clone(),
            _lifetime: Arc::new(Lifetime(stop, shared.capacity.clone())),
        }
    }
    #[cfg(test)]
    pub(super) fn buffered_bytes(&self) -> usize {
        self.shared.metrics.bytes.current()
    }
    #[cfg(test)]
    pub(super) fn bypass_available(&self) -> usize {
        self.shared.added_body.available_permits()
    }
    #[cfg(test)]
    pub(super) fn lookups(&self) -> usize {
        self.shared.lookups.load(Ordering::Acquire)
    }
    pub fn stop(&self) {
        if self.shared.stop.send_if_modified(|stopped| {
            if *stopped {
                false
            } else {
                *stopped = true;
                true
            }
        }) {
            self.shared.capacity.stop();
        }
        self.shared.queue.lock().unwrap().operations.stop();
        self.shared.notify_status();
    }

    pub async fn shutdown(&self) -> bool {
        self.stop();
        let mut active = self.shared.capacity.active_work.subscribe();
        tokio::time::timeout(Duration::from_secs(5), active.wait_for(|count| *count == 0))
            .await
            .is_ok()
    }

    pub fn submit(&self, id: i64) -> Result<FetchTicket> {
        self.submit_inner(id, false, false)
    }

    pub(super) fn submit_added(&self, id: i64) -> Result<FetchTicket> {
        self.submit_inner(id, true, true)
    }

    fn submit_inner(&self, id: i64, added: bool, keep_due: bool) -> Result<FetchTicket> {
        let mut queue = self.shared.queue.lock().unwrap();
        if queue.stopped || *self.shared.stop.borrow() {
            anyhow::bail!("抓取运行时已停止");
        }
        if let Some(entry) = queue.entries.get_mut(&id) {
            if entry.slot.receiver_count() >= WAITERS_PER_FEED {
                anyhow::bail!("该订阅的等待请求过多，请稍后重试");
            }
            entry.keep_due |= keep_due;
            let ticket = FetchTicket(entry.slot.subscribe());
            if !keep_due {
                queue.wake_parked(id);
            }
            drop(queue);
            self.shared.wake.notify_one();
            return Ok(ticket);
        }
        // One reserved queue position keeps a single addition admissible even
        // when an import or refresh has filled the ordinary pending bound.
        if queue.entries.len()
            >= self
                .shared
                .limits
                .pending
                .saturating_add(usize::from(added))
        {
            anyhow::bail!("抓取队列已满，稍后会自动重试");
        }
        let (slot, receiver) = watch::channel(None);
        queue.generation += 1;
        let generation = queue.generation;
        queue.entries.insert(
            id,
            Entry {
                enqueued: tokio::time::Instant::now(),
                dispatched: false,
                generation,
                slot,
                added,
                keep_due,
                parked: None,
                resuming: None,
            },
        );
        queue.operations.attach(id, generation);
        if added {
            queue.added.push_back(id);
        } else {
            queue.pending.push_back(id);
        }
        self.shared.metrics.admitted.set(queue.entries.len());
        drop(queue);
        self.shared.wake.notify_one();
        Ok(FetchTicket(receiver))
    }

    /// Paged producers wait for bounded admission instead of dropping the rest
    /// of an import. This notification is separate from the dispatch wake.
    pub(super) async fn submit_wait(&self, id: i64, due: bool) -> Result<FetchTicket> {
        let mut stop = self.shared.stop.subscribe();
        loop {
            let space = self.shared.space.notified();
            tokio::pin!(space);
            space.as_mut().enable();
            let full = {
                let queue = self.shared.queue.lock().unwrap();
                !queue.entries.contains_key(&id)
                    && queue.entries.len() >= self.shared.limits.pending
            };
            if !full || *stop.borrow() {
                // Another producer may claim the freed slot between the check
                // and submit. Only a full queue is retryable here.
                match self.submit_inner(id, false, due) {
                    Ok(ticket) => return Ok(ticket),
                    Err(error) => {
                        let queue = self.shared.queue.lock().unwrap();
                        if queue.stopped
                            || *stop.borrow()
                            || queue.entries.len() < self.shared.limits.pending
                            || queue.entries.contains_key(&id)
                        {
                            return Err(error);
                        }
                    }
                }
            }
            tokio::select! {
                _ = stop.wait_for(|stopped| *stopped) => anyhow::bail!("抓取运行时已停止"),
                _ = &mut space => {},
            }
        }
    }
}

pub(super) fn host_key(url: &reqwest::Url) -> String {
    format!(
        "{}:{}",
        url.host_str().unwrap_or_default().to_ascii_lowercase(),
        url.port_or_known_default().unwrap_or(0)
    )
}
struct Permits {
    // Decrement usage before releasing semaphores, so a new owner can never
    // inflate the recorded peak by racing the previous owner's cleanup.
    _usage: super::diagnostics::Lease,
    _host: OwnedSemaphorePermit,
    _global: OwnedSemaphorePermit,
    bypass: Option<OwnedSemaphorePermit>,
    _recovery: Option<OwnedSemaphorePermit>,
    // Fields drop in declaration order: both permits are free before the release
    // hook returns parked same-host claims and wakes dispatch.
    _release: Release,
}
struct Release {
    shared: Arc<Shared>,
    host: String,
}
impl Drop for Release {
    fn drop(&mut self) {
        {
            let mut queue = self.shared.queue.lock().unwrap();
            self.shared.unpark(&mut queue, &self.host);
        }
        self.shared.wake.notify_one();
    }
}
enum Busy {
    /// The host is full: park the claim until that host releases a permit.
    Host(String),
    Cooling(super::cooldown::Deferred),
    Recovery(super::cooldown::Deferred),
    /// Global capacity or byte backpressure: retry on the next dispatch wake.
    Other,
}

#[derive(Clone, Copy)]
enum ParkReason {
    Capacity,
    Deferred,
}

/// Keep the accounting lease through parsing as well, while the worker still
/// owns the body. Errors, cancellation and discarded responses release it too.
struct BufferedBytes {
    shared: Arc<Shared>,
    bytes: usize,
}
impl BufferedBytes {
    fn add(&mut self, bytes: usize) {
        self.bytes += bytes;
        self.shared.metrics.bytes.add(bytes);
    }
    fn clear(&mut self) {
        if self.bytes > 0 {
            self.shared.metrics.bytes.remove(self.bytes);
            self.bytes = 0;
            self.shared.wake.notify_one();
        }
    }
}
impl Drop for BufferedBytes {
    fn drop(&mut self) {
        self.clear();
    }
}

struct Downloaded {
    feed: FeedRow,
    generation: u64,
    response: std::result::Result<FetchResponse, FetchFailure>,
    _bytes: BufferedBytes,
    _bypass: Option<OwnedSemaphorePermit>,
    finished_at: tokio::time::Instant,
    // 这个源本轮的调用链；解析与写库在它里面继续。
    span: tracing::Span,
}
impl Queue {
    fn set_resuming(&mut self, id: i64, host: Option<String>) {
        let Some(entry) = self.entries.get_mut(&id) else {
            return;
        };
        if entry.resuming == host {
            return;
        }
        if let Some(previous) = entry.resuming.take() {
            let count = self
                .resuming
                .get_mut(&previous)
                .expect("a resuming claim owns its host reservation");
            *count -= 1;
            if *count == 0 {
                self.resuming.remove(&previous);
            }
        }
        if let Some(host) = &host {
            *self.resuming.entry(host.clone()).or_default() += 1;
        }
        entry.resuming = host;
    }

    pub(super) fn wake_parked(&mut self, id: i64) {
        let Some(host) = self
            .entries
            .get_mut(&id)
            .and_then(|entry| entry.parked.take())
        else {
            return;
        };
        if let Some(claims) = self.parked.get_mut(&host) {
            claims.retain(|(feed, _)| *feed != id);
            if claims.is_empty() {
                self.parked.remove(&host);
            }
        }
        self.set_resuming(id, Some(host));
        if self.entries.get(&id).is_some_and(|entry| entry.added) {
            self.added.push_front(id);
        } else {
            self.pending.push_front(id);
        }
    }
}

impl Shared {
    pub(super) fn cooldown_dispatch_ready(&self) -> bool {
        self.cooldown_store.writes.load(Ordering::Acquire) == 0
            && self
                .cooldowns
                .lock()
                .unwrap()
                .dispatch_ready(self.clock.now())
    }

    async fn deferred_result(
        &self,
        feed: &FeedRow,
        generation: u64,
        deferred: super::cooldown::Deferred,
    ) {
        self.deferred_results(vec![DeferredDownload {
            feed: feed.clone(),
            generation,
            deferred,
        }])
        .await;
    }

    async fn deferred_results(&self, entries: Vec<DeferredDownload>) {
        if entries.is_empty() {
            return;
        }
        let entries = Arc::new(entries);
        let snapshot = entries.clone();
        let marked = super::write_db(self.path.clone(), move |conn| {
            let requests = snapshot
                .iter()
                .map(|entry| {
                    (
                        &entry.feed,
                        entry.deferred.until,
                        entry.deferred.message.as_str(),
                    )
                })
                .collect::<Vec<_>>();
            rrss_core::ingest::record_fetch_deferred_batch(conn, &requests)
        })
        .await;
        match marked {
            Ok(results) => {
                for (entry, result) in entries.iter().zip(results) {
                    self.complete_deferred(entry, result);
                }
            }
            Err(_) => {
                for entry in entries.iter() {
                    self.finish(
                        RefreshResult {
                            feed_id: entry.feed.id,
                            status: RefreshStatus::Failed {
                                message: "无法记录订阅延期".into(),
                            },
                        },
                        entry.generation,
                    );
                }
            }
        }
    }

    fn complete_deferred(&self, entry: &DeferredDownload, result: RefreshResult) {
        let DeferredDownload {
            feed,
            generation,
            deferred,
        } = entry;
        let generation = *generation;
        if !matches!(result.status, RefreshStatus::Skipped { .. }) {
            self.finish(result, generation);
            return;
        }
        let keep = {
            let queue = self.queue.lock().unwrap();
            // Timed deferrals are already durable. Retain them for prompt thaw
            // only while there is spare admission capacity; otherwise a host
            // with enough cooled sources can block every later due page.
            queue
                .entries
                .get(&feed.id)
                .is_some_and(|entry| entry.keep_due)
                && (deferred.until.is_none() || queue.entries.len() < self.limits.pending)
        };
        if !keep {
            self.finish(result, generation);
            return;
        }
        let background = {
            let mut queue = self.queue.lock().unwrap();
            let background = queue.operations.finish(&result, generation);
            if let Some(entry) = queue.entries.get_mut(&feed.id) {
                entry.slot.send_replace(Some(result));
                entry.slot = watch::channel(None).0;
            }
            background
        };
        if background {
            self.dirty.send_modify(|version| *version += 1);
        }
        self.park(
            deferred.key.clone(),
            (feed.id, generation),
            ParkReason::Deferred,
        );
        self.wake.notify_one();
        self.notify_status();
    }

    fn unpark_ready(&self) -> bool {
        let mut queue = self.queue.lock().unwrap();
        let before = queue.pending.len() + queue.added.len();
        let hosts = queue.parked.keys().cloned().collect::<Vec<_>>();
        for host in hosts {
            self.unpark(&mut queue, &host);
        }
        queue.pending.len() + queue.added.len() > before
    }

    /// A newly received cooldown also applies to claims parked earlier for a
    /// full host. Re-read those once through the bounded metadata/deferral
    /// writer so manual operations settle now and source deadlines persist.
    /// This is driven by a received limit, never by every dispatch wake: due
    /// claims parked again after recording their deferral must not hot-loop.
    pub(super) fn recheck_cooling_hosts(&self) {
        let mut queue = self.queue.lock().unwrap();
        let cooling = self.cooldowns.lock().unwrap();
        let now = self.clock.now();
        let hosts = queue
            .parked
            .keys()
            .filter(|host| cooling.blocked_host(host, now).is_some())
            .cloned()
            .collect::<Vec<_>>();
        for host in hosts {
            let claims = queue.parked.remove(&host).unwrap_or_default();
            for (id, generation) in claims {
                let Some(entry) = queue.entries.get_mut(&id) else {
                    continue;
                };
                if entry.generation != generation || entry.parked.as_deref() != Some(host.as_str())
                {
                    continue;
                }
                entry.parked = None;
                let added = entry.added;
                queue.set_resuming(id, None);
                if added {
                    queue.added.push_back(id);
                } else {
                    queue.pending.push_back(id);
                }
            }
        }
        drop(cooling);
        drop(queue);
        self.wake.notify_one();
    }

    pub(super) fn pause_network_waiters(&self) {
        let mut queue = self.queue.lock().unwrap();
        let background = queue.operations.skip_pending(super::offline::UNAVAILABLE);
        // Finish existing observers with an explicit skip, but retain admitted
        // work. New observers attach to the eventual retry after connectivity
        // returns; no source deadline/health is modified while offline.
        for (&id, entry) in &mut queue.entries {
            entry.slot.send_replace(Some(RefreshResult {
                feed_id: id,
                status: RefreshStatus::Skipped {
                    message: super::offline::UNAVAILABLE.into(),
                    retry_at: None,
                },
            }));
            entry.slot = watch::channel(None).0;
        }
        drop(queue);
        if background && !*self.stop.borrow() {
            self.dirty.send_modify(|version| *version += 1);
        }
    }

    pub(super) fn retry_network(&self, claim: Claim) {
        requeue(self, claim);
        self.wake.notify_one();
    }

    fn backpressured(&self) -> bool {
        self.metrics.bytes.current() >= self.limits.buffer_bytes
    }
    fn host(&self, url: &reqwest::Url) -> Arc<Semaphore> {
        let mut hosts = self.hosts.lock().unwrap();
        hosts.retain(|_, value| value.strong_count() > 0);
        let entry = hosts.entry(host_key(url)).or_default();
        if let Some(sem) = entry.upgrade() {
            return sem;
        }
        let sem = Arc::new(Semaphore::new(self.host_limit()));
        *entry = Arc::downgrade(&sem);
        sem
    }
    fn try_permits(
        self: &Arc<Self>,
        url: &reqwest::Url,
        added: bool,
    ) -> std::result::Result<Permits, Busy> {
        if !self.cooldown_dispatch_ready() {
            return Err(Busy::Other);
        }
        if let Some(deferred) = self
            .cooldowns
            .lock()
            .unwrap()
            .blocked_host(&host_key(url), self.clock.now())
        {
            return Err(Busy::Cooling(deferred));
        }
        let bypass = if self.backpressured() {
            if !added {
                return Err(Busy::Other);
            }
            Some(
                self.added_body
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| Busy::Other)?,
            )
        } else {
            None
        };
        let key = host_key(url);
        let Ok(host) = self.host(url).try_acquire_owned() else {
            return Err(Busy::Host(key));
        };
        let global = self
            .global
            .clone()
            .try_acquire_owned()
            .map_err(|_| Busy::Other)?;
        let recovery =
            if let Some(deferred) = self.cooldowns.lock().unwrap().recovery(self.clock.now()) {
                Some(
                    self.recovery
                        .clone()
                        .try_acquire_owned()
                        .map_err(|_| Busy::Recovery(deferred))?,
                )
            } else {
                None
            };
        Ok(Permits {
            _usage: self.http_usage(&host_key(url)),
            _host: host,
            _global: global,
            bypass,
            _recovery: recovery,
            _release: Release {
                shared: self.clone(),
                host: key,
            },
        })
    }
    fn http_usage(&self, key: &str) -> super::diagnostics::Lease {
        let used = self.host_limit().saturating_sub(self.host_free(key));
        self.metrics.host_peak.fetch_max(used, Ordering::Relaxed);
        self.metrics.http.enter()
    }
    fn host_free(&self, key: &str) -> usize {
        self.hosts
            .lock()
            .unwrap()
            .get(key)
            .and_then(Weak::upgrade)
            .map_or(self.host_limit(), |sem| sem.available_permits())
    }
    pub(super) fn host_limit(&self) -> usize {
        self.host_limit.load(Ordering::Relaxed)
    }
    /// Lock order: queue, then hosts. A releaser frees its permit before taking
    /// the queue lock and a parker re-checks under it, so no wakeup is lost.
    fn park(&self, key: String, claim: (i64, u64), reason: ParkReason) {
        let mut queue = self.queue.lock().unwrap();
        if *self.stop.borrow()
            || queue
                .entries
                .get(&claim.0)
                .is_none_or(|entry| entry.generation != claim.1)
        {
            return;
        }
        queue.set_resuming(claim.0, None);
        // Close the race where a received limit was committed and its recheck
        // completed after try_permits returned Host but before we parked here.
        if matches!(reason, ParkReason::Capacity)
            && self
                .cooldowns
                .lock()
                .unwrap()
                .blocked_host(&key, self.clock.now())
                .is_some()
        {
            let added = if let Some(entry) = queue.entries.get_mut(&claim.0)
                && entry.generation == claim.1
            {
                entry.parked = None;
                entry.added
            } else {
                return;
            };
            if added {
                queue.added.push_back(claim.0);
            } else {
                queue.pending.push_back(claim.0);
            }
            drop(queue);
            self.wake.notify_one();
            return;
        }
        if let Some(entry) = queue.entries.get_mut(&claim.0) {
            if entry.dispatched {
                entry.dispatched = false;
                entry.enqueued = tokio::time::Instant::now();
            }
            entry.parked = Some(key.clone());
        }
        queue
            .parked
            .entry(key.clone())
            .or_default()
            .push_back(claim);
        self.unpark(&mut queue, &key);
    }
    fn unpark(&self, queue: &mut Queue, key: &str) {
        if self.cooldown_store.writes.load(Ordering::Acquire) > 0 {
            return;
        }
        {
            let state = self.cooldowns.lock().unwrap();
            let now = self.clock.now();
            if state.blocked_host(key, now).is_some()
                || (key == super::cooldown::RECOVERY
                    && (!state.recovery_ready(now) || self.recovery.available_permits() == 0))
            {
                return;
            }
        }
        let Queue {
            parked,
            entries,
            resuming,
            ..
        } = queue;
        let Some(waiting) = parked.get_mut(key) else {
            return;
        };
        let reserved = resuming.get(key).copied().unwrap_or(0);
        let mut free = self.host_free(key).saturating_sub(reserved);
        let mut ready = Vec::new();
        while free > 0
            && let Some((id, generation)) = waiting.pop_front()
        {
            // Finished or re-submitted claims are dropped; a live one keeps its place.
            if entries.get(&id).is_some_and(|entry| {
                entry.generation == generation && entry.parked.as_deref() == Some(key)
            }) {
                ready.push(id);
                free -= 1;
            }
        }
        if waiting.is_empty() {
            parked.remove(key);
        }
        for id in ready.into_iter().rev() {
            if let Some(entry) = queue.entries.get_mut(&id) {
                entry.parked = None;
            }
            queue.set_resuming(id, Some(key.into()));
            if queue.entries.get(&id).is_some_and(|entry| entry.added) {
                queue.added.push_front(id);
            } else {
                queue.pending.push_front(id);
            }
        }
    }
    fn check_dispatch(&self, url: &reqwest::Url) -> rrss_core::Result<()> {
        {
            // Check the barrier and committed block under the same lock, so a
            // finishing write cannot slip between these two observations.
            let cooling = self.cooldowns.lock().unwrap();
            if self.cooldown_store.writes.load(Ordering::Acquire) > 0 {
                return Err(rrss_core::Error::FetchDeferred {
                    until: None,
                    reason: rrss_core::ingest::FetchDeferredReason::CooldownSaving,
                });
            }
            if let Some(deferred) = cooling.blocked_host(&host_key(url), self.clock.now()) {
                return Err(deferred.error());
            }
        }
        if self.network_paused() {
            return Err(rrss_core::Error::FetchPaused);
        }
        Ok(())
    }

    async fn permits(
        self: &Arc<Self>,
        url: reqwest::Url,
        reserved: Option<Permits>,
    ) -> rrss_core::Result<Permits> {
        self.check_dispatch(&url)?;
        if let Some(permit) = reserved {
            return Ok(permit);
        }
        let closed = |_| {
            rrss_core::Error::FeedRequest(
                rrss_core::ingest::feed_http::FeedRequestError::RuntimeStopped,
            )
        };
        // Never reserve global capacity while waiting for a busy host.
        let host = self.host(&url).acquire_owned().await.map_err(closed)?;
        let global = self.global.clone().acquire_owned().await.map_err(closed)?;
        // A response received while we awaited either permit may have imposed
        // a new limit. Every hop is checked at actual dispatch, including a
        // first-hop reservation held across asynchronous client construction.
        self.check_dispatch(&url)?;
        Ok(Permits {
            _usage: self.http_usage(&host_key(&url)),
            _host: host,
            _global: global,
            bypass: None,
            _recovery: None,
            _release: Release {
                shared: self.clone(),
                host: host_key(&url),
            },
        })
    }
    fn emit(&self, event: views::FeedUpdated) {
        let mut pending = self.event_pending.lock().unwrap();
        self.metrics.event_received.fetch_add(1, Ordering::Relaxed);
        // Once a burst overflows, retain one global invalidation for this batch.
        // Never flush dozens of redundant source events alongside it.
        if pending
            .get(&None)
            .is_some_and(|event| event.status == "invalidate")
            || (pending.len() >= 64 && !pending.contains_key(&event.feed_id))
        {
            self.metrics
                .event_coalesced
                .fetch_add(pending.len() as u64, Ordering::Relaxed);
            pending.clear();
            pending.insert(
                None,
                views::FeedUpdated {
                    feed_id: None,
                    generation: None,
                    status: "invalidate".into(),
                    inserted: 0,
                    error: None,
                    derived_degraded: false,
                    ..event
                },
            );
        } else if pending.insert(event.feed_id, event).is_some() {
            self.metrics.event_coalesced.fetch_add(1, Ordering::Relaxed);
        }
        drop(pending);
        self.events.notify_one();
    }
    pub(super) fn invalidate(&self) {
        self.emit(views::FeedUpdated {
            run_id: self.run_id.clone(),
            generation: None,
            status: "derived".into(),
            derived_degraded: false,
            feed_id: None,
            inserted: 0,
            error: None,
        });
        self.notify_status();
    }
    /// 新文章已提交、源事件已发出后再做增量排序：后台按批串行合并，不挡解析工作者。
    /// 失败时安排整表重算并补发该源的衍生降级事件；停止后只留待下次整表重算。
    /// 排序另起任务，仍属于这个源的调用链。
    fn rank_later(self: &Arc<Self>, feed_id: i64, ids: Vec<i64>, span: tracing::Span) {
        if ids.is_empty() {
            return;
        }
        let state = self.clone();
        let work = self.capacity.work();
        tokio::spawn(
            async move {
                let _work = work;
                let stopped = *state.stop.borrow();
                let failed = if stopped {
                    true
                } else {
                    match crate::jobs::rank::incremental(&state.path, ids).await {
                        Ok(()) => false,
                        Err(error) => {
                            tracing::warn!(
                                %error,
                                feed_id = rrss_core::logsafe::unless_in_span("feed_id", feed_id),
                                "incremental rank failed"
                            );
                            true
                        }
                    }
                };
                if !failed {
                    return;
                }
                let _ = crate::jobs::spawn_db(state.path.clone(), crate::jobs::rank::request).await;
                if !stopped && !*state.stop.borrow() {
                    state.emit(views::FeedUpdated {
                        run_id: state.run_id.clone(),
                        generation: None,
                        status: "derived".into(),
                        derived_degraded: true,
                        feed_id: Some(feed_id),
                        inserted: 0,
                        error: None,
                    });
                }
            }
            .instrument(span),
        );
    }
    fn finish(&self, result: RefreshResult, generation: u64) {
        let background;
        {
            let mut queue = self.queue.lock().unwrap();
            if *self.stop.borrow()
                || queue
                    .entries
                    .get(&result.feed_id)
                    .is_none_or(|entry| entry.generation != generation)
            {
                return;
            }
            queue.set_resuming(result.feed_id, None);
            let entry = queue.entries.remove(&result.feed_id).unwrap();
            queue.pending.retain(|id| *id != result.feed_id);
            queue.added.retain(|id| *id != result.feed_id);
            background = queue.operations.finish(&result, generation);
            entry.slot.send_replace(Some(result.clone()));
            queue.fill_operations(self.limits.pending);
            self.metrics.admitted.set(queue.entries.len());
        }
        self.wake.notify_one();
        self.space.notify_waiters();
        if background && !*self.stop.borrow() {
            self.dirty.send_modify(|version| *version += 1);
        }
        let degraded = matches!(
            &result.status,
            RefreshStatus::Updated {
                derived_degraded: true,
                ..
            }
        );
        let status = match &result.status {
            RefreshStatus::Updated { .. } => "updated",
            RefreshStatus::Failed { .. } => "failed",
            RefreshStatus::NotModified => "not_modified",
            RefreshStatus::Deleted => "deleted",
            RefreshStatus::Skipped { .. } => "skipped",
        };
        self.metrics.completed.fetch_add(1, Ordering::Relaxed);
        match &result.status {
            RefreshStatus::Updated { inserted, .. } => {
                self.metrics
                    .inserted
                    .fetch_add(u64::from(*inserted), Ordering::Relaxed);
            }
            RefreshStatus::Failed { .. } => {
                self.metrics.failed.fetch_add(1, Ordering::Relaxed);
            }
            RefreshStatus::Skipped { .. } => {
                self.metrics.skipped.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
        let (inserted, error) = match result.status {
            RefreshStatus::Updated { inserted, .. } => (inserted as i64, None),
            RefreshStatus::Failed { message } => (0, Some(message)),
            RefreshStatus::NotModified => (0, None),
            RefreshStatus::Deleted => (0, None),
            RefreshStatus::Skipped { message, .. } => (0, Some(message)),
        };
        if error.is_none() {
            self.icons.send_modify(|version| *version += 1);
        }
        if inserted > 0 || degraded {
            self.capacity.derived.send_modify(|version| *version += 1);
        }
        self.emit(views::FeedUpdated {
            run_id: self.run_id.clone(),
            generation: Some(generation),
            status: status.into(),
            derived_degraded: degraded,
            feed_id: Some(result.feed_id),
            inserted,
            error,
        });
        self.notify_status();
    }
}

type Claim = (i64, u64);
type ReadyDownload = (FeedRow, u64, Permits);
struct DeferredDownload {
    feed: FeedRow,
    generation: u64,
    deferred: super::cooldown::Deferred,
}
#[derive(Default)]
struct PreparedDownloads {
    ready: Vec<ReadyDownload>,
    deferred: Vec<DeferredDownload>,
}

fn requeue(shared: &Shared, (id, generation): Claim) {
    let mut queue = shared.queue.lock().unwrap();
    if !*shared.stop.borrow()
        && queue
            .entries
            .get(&id)
            .is_some_and(|entry| entry.generation == generation)
    {
        queue.set_resuming(id, None);
        if let Some(entry) = queue.entries.get_mut(&id)
            && entry.dispatched
        {
            entry.dispatched = false;
            entry.enqueued = tokio::time::Instant::now();
            shared.metrics.retried.fetch_add(1, Ordering::Relaxed);
        }
        if queue.entries.get(&id).is_some_and(|entry| entry.added) {
            queue.added.push_back(id);
        } else {
            queue.pending.push_back(id);
        }
    }
}

async fn prepare_downloads(state: Arc<Shared>, claims: Vec<Claim>) -> PreparedDownloads {
    #[cfg(test)]
    state.lookups.fetch_add(claims.len(), Ordering::AcqRel);
    let lookup = claims.clone();
    let from_settings = state.limits.host_from_settings;
    let rows = crate::jobs::spawn_db(state.path.clone(), move |conn| {
        // 每批派发前重读每站同时下载数：改设置后下一批生效，正在下载的主机空闲后切换。
        // 读不到设置时保持当前额度，不让整批订阅读取失败。
        let per_host = from_settings
            .then(|| rrss_core::db::feed_fetch_per_host(conn).ok())
            .flatten();
        let rows = lookup
            .iter()
            .map(|&(id, generation)| (id, generation, rrss_core::feeds::get_feed(conn, id)))
            .collect::<Vec<_>>();
        Ok((per_host, rows))
    })
    .await
    .map(|(per_host, rows)| {
        if let Some(per_host) = per_host {
            state.host_limit.store(per_host, Ordering::Relaxed);
        }
        rows
    });
    if *state.stop.borrow() {
        return PreparedDownloads::default();
    }
    let rows = rows.unwrap_or_else(|_| {
        claims
            .into_iter()
            .map(|(id, generation)| {
                (
                    id,
                    generation,
                    Err(rrss_core::Error::InvalidSetting("无法读取订阅".into())),
                )
            })
            .collect()
    });
    let mut prepared = PreparedDownloads::default();
    for (id, generation, row) in rows {
        let deferred = row
            .as_ref()
            .ok()
            .and_then(|feed| state.deferred(feed.id, &feed.url));
        let added = state
            .queue
            .lock()
            .unwrap()
            .entries
            .get(&id)
            .is_some_and(|entry| entry.added);
        match row {
            Ok(feed) if feed.kind == rrss_core::kind::FeedKind::Manual => state.finish(
                RefreshResult {
                    feed_id: id,
                    status: RefreshStatus::NotModified,
                },
                generation,
            ),
            Ok(feed) => match reqwest::Url::parse(&feed.url) {
                Ok(_) if state.network_paused() => requeue(&state, (id, generation)),
                Ok(_) if deferred.is_some() => {
                    let deferred = deferred.unwrap();
                    prepared.deferred.push(DeferredDownload {
                        feed,
                        generation,
                        deferred,
                    });
                }
                Ok(url) => match state.try_permits(&url, added) {
                    Ok(permit) => {
                        state.queue.lock().unwrap().set_resuming(id, None);
                        prepared.ready.push((feed, generation, permit));
                    }
                    Err(Busy::Host(key)) => state.park(key, (id, generation), ParkReason::Capacity),
                    Err(Busy::Other) => requeue(&state, (id, generation)),
                    Err(Busy::Recovery(deferred) | Busy::Cooling(deferred)) => {
                        prepared.deferred.push(DeferredDownload {
                            feed,
                            generation,
                            deferred,
                        });
                    }
                },
                Err(_) => state.finish(
                    RefreshResult {
                        feed_id: id,
                        status: RefreshStatus::Failed {
                            message: "无效的订阅地址".into(),
                        },
                    },
                    generation,
                ),
            },
            Err(error) => state.finish(
                RefreshResult {
                    feed_id: id,
                    status: if matches!(error, rrss_core::Error::FeedNotFound) {
                        RefreshStatus::Deleted
                    } else {
                        RefreshStatus::Failed {
                            message: "无法读取订阅".into(),
                        }
                    },
                },
                generation,
            ),
        }
    }
    prepared
}

fn worker_failed(
    shared: &Shared,
    active: &mut HashMap<tokio::task::Id, Claim>,
    error: tokio::task::JoinError,
) {
    if let Some((id, generation)) = active.remove(&error.id()) {
        shared.finish(
            RefreshResult {
                feed_id: id,
                status: RefreshStatus::Failed {
                    message: "抓取任务异常结束，请重试".into(),
                },
            },
            generation,
        );
    }
    tracing::error!(%error, "feed worker panicked");
}

async fn drive(shared: Arc<Shared>) {
    let mut stop = shared.stop.subscribe();
    tokio::select! {
        biased;
        _=stop.wait_for(|stopped|*stopped)=>{
            let mut queue=shared.queue.lock().unwrap();
            queue.stopped=true; queue.pending.clear(); queue.added.clear(); queue.parked.clear(); queue.resuming.clear(); queue.entries.clear(); queue.operations.stop();
            shared.space.notify_waiters();
            return;
        },
        _=super::cooldown::restore(shared.clone())=>{},
    }
    let mut downloads = JoinSet::new();
    let mut parsers = JoinSet::new();
    let mut recorders = JoinSet::new();
    let mut bodies: VecDeque<Downloaded> = VecDeque::new();
    let mut results: VecDeque<Downloaded> = VecDeque::new();
    let mut network_failures: VecDeque<Downloaded> = VecDeque::new();
    let mut probes = JoinSet::new();
    let mut cooldowns = JoinSet::new();
    let mut metadata = JoinSet::new();
    let mut metadata_ids = Vec::new();
    // Every deferred item still owns a bounded queue entry. One writer drains
    // small batches without holding up unrelated metadata or download dispatch.
    let mut deferred = VecDeque::new();
    let mut deferring = JoinSet::new();
    let mut deferral_ids = Vec::new();
    let mut active = HashMap::new();
    let mut remaining = 0;
    let mut activity_log = super::diagnostics::ActivityLog::new(tokio::time::Instant::now());
    loop {
        let wake = shared.wake.notified();
        tokio::pin!(wake);
        wake.as_mut().enable();
        if *stop.borrow() {
            break;
        }
        let clock = shared.clock.now();
        if shared.unpark_ready() {
            shared.wake.notify_one();
        }
        let cooldown_deadline = if cooldowns.is_empty() {
            let cooling = shared.cooldowns.lock().unwrap();
            if cooling.needs_checkpoint(clock) {
                cooldowns.spawn(super::cooldown::checkpoint(shared.clone()));
                None
            } else {
                cooling.deadline(clock)
            }
        } else {
            // An expired timer must not stay immediately ready while its
            // checkpoint is waiting for SQLite; the worker completion wakes us.
            None
        };
        let now = tokio::time::Instant::now();
        activity_log.tick(&shared.metrics, now);
        let (paused, offline, success, probe_due, next_probe) = {
            let network = shared.connectivity.lock().unwrap();
            (
                network.paused(),
                network.unavailable(),
                network.has_success(now),
                network.probe_due(now),
                network.deadline(),
            )
        };
        if offline {
            for failed in network_failures.drain(..) {
                shared.retry_network((failed.feed.id, failed.generation));
            }
        } else if !paused {
            while network_failures.front().is_some_and(|failed| {
                success || failed.finished_at + shared.limits.offline.window <= now
            }) {
                results.push_back(network_failures.pop_front().unwrap());
            }
        }
        if probe_due && probes.is_empty() {
            probes.spawn({
                let state = shared.clone();
                async move { state.check_network(false).await }
            });
        }
        let network_deadline = next_probe
            .into_iter()
            .chain(Some(activity_log.deadline()))
            .chain(cooldown_deadline)
            .chain(
                (!paused)
                    .then(|| {
                        network_failures
                            .front()
                            .map(|failed| failed.finished_at + shared.limits.offline.window)
                    })
                    .flatten(),
            )
            .min();
        // Parse bodies in completion order. Slow downloads never reserve these
        // workers; body-free responses bypass the byte buffer and parsing queue.
        while parsers.len() < shared.limits.parsing {
            let Some(body) = bodies.pop_front() else {
                break;
            };
            let claim = (body.feed.id, body.generation);
            let task = parsers.spawn(process_response(shared.clone(), body));
            active.insert(task.id(), claim);
        }
        while recorders.len() < RESULT_WORKERS {
            let Some(result) = results.pop_front() else {
                break;
            };
            let claim = (result.feed.id, result.generation);
            let task = recorders.spawn(process_response(shared.clone(), result));
            active.insert(task.id(), claim);
        }
        if deferring.is_empty() && !deferred.is_empty() {
            let batch = deferred
                .drain(..deferred.len().min(METADATA_BATCH))
                .collect::<Vec<DeferredDownload>>();
            deferral_ids = batch
                .iter()
                .map(|entry| (entry.feed.id, entry.generation))
                .collect();
            let state = shared.clone();
            deferring.spawn(async move {
                state.deferred_results(batch).await;
            });
        }
        shared.metrics.bodies.set(bodies.len());
        shared.metrics.results.set(results.len());
        shared.metrics.network_waiting.set(network_failures.len());
        shared
            .metrics
            .deferrals
            .set(deferred.len() + deferral_ids.len());
        if metadata.is_empty()
            && shared.cooldown_dispatch_ready()
            && !shared.network_paused()
            && shared.global.available_permits() > 0
            && (!shared.backpressured()
                || (shared.added_body.available_permits() > 0
                    && !shared.queue.lock().unwrap().added.is_empty()))
            && remaining > 0
        {
            let claims: Vec<_> = {
                let mut queue = shared.queue.lock().unwrap();
                let count = remaining
                    .min(queue.pending.len() + queue.added.len())
                    .min(METADATA_BATCH);
                remaining -= count;
                let mut ids = Vec::new();
                for _ in 0..count {
                    let id = queue.added.pop_front().or_else(|| {
                        if shared.backpressured() {
                            None
                        } else {
                            queue.pending.pop_front()
                        }
                    });
                    if let Some(id) = id {
                        ids.push(id);
                    } else {
                        break;
                    }
                }
                ids.into_iter()
                    .filter_map(|id| queue.entries.get(&id).map(|entry| (id, entry.generation)))
                    .collect()
            };
            if !claims.is_empty() {
                metadata_ids.clone_from(&claims);
                metadata.spawn(prepare_downloads(shared.clone(), claims));
            }
        }
        tokio::select! {
            biased;
            _ = stop.wait_for(|stopped| *stopped) => break,
            _ = &mut wake => { let queue = shared.queue.lock().unwrap(); remaining = queue.pending.len() + queue.added.len(); },
            Some(result) = metadata.join_next(), if !metadata.is_empty() => {
                match result {
                    Ok(prepared) => {
                        deferred.extend(prepared.deferred);
                        for (feed, generation, permit) in prepared.ready {
                            if shared.network_paused() || !shared.cooldown_dispatch_ready() || (shared.backpressured() && permit.bypass.is_none()) {
                                requeue(&shared, (feed.id, generation));
                                drop(permit);
                                continue;
                            }
                            let claim = (feed.id, generation);
                            // 级别只决定开关：warn 与默认的 warn 事件一样开启，stderr 只看 warn 时也带编号。
                            let span = tracing::span!(parent: None, tracing::Level::WARN, "feed", feed_id = feed.id);
                            let task = downloads.spawn(download(shared.clone(), feed, generation, permit).instrument(span));
                            active.insert(task.id(), claim);
                        }
                    },
                    Err(error) => {
                        tracing::error!(%error, "feed metadata worker failed");
                        for (id, generation) in metadata_ids.drain(..) {
                            shared.finish(RefreshResult { feed_id: id, status: RefreshStatus::Failed { message: "订阅读取任务异常结束，请重试".into() } }, generation);
                        }
                    }
                }
                metadata_ids.clear();
            },
            Some(result) = downloads.join_next_with_id(), if !downloads.is_empty() => {
                match result {
                    Ok((task, downloaded)) => {
                        active.remove(&task);
                        let hold = if downloaded.response.as_ref().is_err_and(|error| error.network) {
                            shared.connectivity.lock().unwrap().failure(&downloaded.feed.url, downloaded.finished_at)
                        } else {
                            if downloaded.response.is_ok() || downloaded.response.as_ref().is_err_and(|error| error.http_response) { shared.network_response(); }
                            false
                        };
                        if hold {
                            network_failures.push_back(downloaded);
                            shared.wake.notify_one();
                        } else if downloaded.response.as_ref().is_ok_and(|response| (200..300).contains(&response.status)) {
                            bodies.push_back(downloaded);
                            shared.metrics.bodies.set(bodies.len());
                        } else {
                            results.push_back(downloaded);
                        }
                    }
                    Err(error) => worker_failed(&shared, &mut active, error),
                }
                let queue = shared.queue.lock().unwrap();
                remaining = queue.pending.len() + queue.added.len();
            },
            Some(result) = parsers.join_next_with_id(), if !parsers.is_empty() => {
                match result {
                    Ok((task, ())) => { active.remove(&task); }
                    Err(error) => worker_failed(&shared, &mut active, error),
                }
            },
            Some(result) = recorders.join_next_with_id(), if !recorders.is_empty() => {
                match result {
                    Ok((task, ())) => { active.remove(&task); }
                    Err(error) => worker_failed(&shared, &mut active, error),
                }
            },
            Some(result) = deferring.join_next(), if !deferring.is_empty() => {
                if let Err(error) = result {
                    tracing::error!(%error, "feed deferral worker failed");
                    for &(id, generation) in &deferral_ids {
                        shared.finish(RefreshResult { feed_id: id, status: RefreshStatus::Failed {
                            message: "订阅延期任务异常结束，请重试".into(),
                        } }, generation);
                    }
                }
                deferral_ids.clear();
            },
            Some(_) = probes.join_next(), if !probes.is_empty() => {},
            Some(_) = cooldowns.join_next(), if !cooldowns.is_empty() => {},
            _ = async {
                if let Some(deadline) = network_deadline { tokio::time::sleep_until(deadline).await; }
                else { std::future::pending::<()>().await; }
            } => {},
        }
    }
    // Cancel downloads immediately, discard unparsed bodies, and let already
    // started short database transactions finish through the shutdown tracker.
    metadata.abort_all();
    deferring.abort_all();
    deferred.clear();
    downloads.abort_all();
    parsers.abort_all();
    recorders.abort_all();
    probes.abort_all();
    cooldowns.abort_all();
    bodies.clear();
    results.clear();
    network_failures.clear();
    metadata.shutdown().await;
    deferring.shutdown().await;
    downloads.shutdown().await;
    parsers.shutdown().await;
    recorders.shutdown().await;
    probes.shutdown().await;
    cooldowns.shutdown().await;
    let mut queue = shared.queue.lock().unwrap();
    queue.stopped = true;
    queue.pending.clear();
    queue.added.clear();
    queue.parked.clear();
    queue.resuming.clear();
    queue.entries.clear();
    queue.operations.stop();
    shared.metrics.admitted.set(0);
    shared.metrics.bodies.set(0);
    shared.metrics.results.set(0);
    shared.metrics.network_waiting.set(0);
    shared.metrics.deferrals.set(0);
    shared.space.notify_waiters();
}

fn next_run() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

async fn download(
    state: Arc<Shared>,
    feed: FeedRow,
    generation: u64,
    mut first: Permits,
) -> Downloaded {
    let _active = state.metrics.downloads.enter();
    let _duration = state.metrics.download.start();
    let started = tokio::time::Instant::now();
    if let Some(entry) = state.queue.lock().unwrap().entries.get_mut(&feed.id) {
        state.metrics.feed_wait.record(entry.enqueued.elapsed());
        entry.dispatched = true;
    }
    state
        .cooldowns
        .lock()
        .unwrap()
        .dispatched(state.clock.now());
    let bypass = first.bypass.take();
    // Recovery serializes the entire fetch, including redirects. Hop permits
    // are released between hosts, but this source-level lease must survive.
    let recovery = first._recovery.take();
    let mut first = Some(first);
    let request = FetchRequest {
        url: feed.url.clone(),
        etag: feed.etag.clone(),
        last_modified: feed.last_modified.clone(),
    };
    let mut bytes = BufferedBytes {
        shared: state.clone(),
        bytes: 0,
    };
    let mut response: std::result::Result<_, FetchFailure> = async {
        let client = state.network.feed_client().await?;
        rrss_core::ingest::feed_http::fetch_controlled(
            &client,
            request,
            state.limits.policy,
            |url| {
                let permit = first.take();
                let state = &state;
                async move { state.permits(url, permit).await }
            },
            |read| bytes.add(read),
            |head| {
                let state = state.clone();
                let feed = feed.clone();
                async move { state.response_head(feed, generation, head).await }
            },
        )
        .await
    }
    .await;
    if response
        .as_ref()
        .is_err_and(|error| error.class == rrss_core::ingest::FailureClass::Slow)
    {
        state.metrics.slow_failures.fetch_add(1, Ordering::Relaxed);
    }
    if response.as_ref().is_err_and(|error| error.timeout) {
        state.metrics.timeouts.fetch_add(1, Ordering::Relaxed);
    }
    drop(recovery);
    state.wake.notify_one();
    if response.is_err() {
        bytes.clear();
    }
    if response.as_ref().is_err_and(|error| error.network) {
        // A deleted/re-addressed source is not evidence about the current
        // subscriptions' connectivity. The write path still checks atomically.
        let id = feed.id;
        let old_url = feed.url.clone();
        let stale =
            crate::jobs::spawn_db(
                state.path.clone(),
                move |conn| match rrss_core::feeds::get_feed(conn, id) {
                    Ok(current) => Ok(current.url != old_url),
                    Err(rrss_core::Error::FeedNotFound) => Ok(true),
                    Err(error) => Err(error),
                },
            )
            .await
            .unwrap_or(false);
        if stale && let Err(error) = &mut response {
            error.network = false;
            error.http_response = false;
        }
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;
    match &response {
        Ok(response) => tracing::debug!(
            status = response.status,
            bytes = response.body.len(),
            elapsed_ms,
            "feed download finished"
        ),
        Err(error) => tracing::debug!(
            error_kind = error.kind.as_str(),
            status = error.status(),
            elapsed_ms,
            "feed download failed"
        ),
    }
    Downloaded {
        feed,
        generation,
        response,
        _bytes: bytes,
        _bypass: bypass,
        finished_at: tokio::time::Instant::now(),
        span: tracing::Span::current(),
    }
}

async fn process_response(state: Arc<Shared>, downloaded: Downloaded) {
    let parsing = downloaded
        .response
        .as_ref()
        .is_ok_and(|response| (200..300).contains(&response.status));
    let _active = parsing.then(|| state.metrics.parsers.enter());
    if parsing {
        state
            .metrics
            .parse_wait
            .record(downloaded.finished_at.elapsed());
    }
    let _duration = state.metrics.processing.start();
    let span = downloaded.span.clone();
    let Downloaded {
        feed,
        generation,
        response,
        _bytes,
        _bypass,
        finished_at: _,
        span: _,
    } = downloaded;
    let id = feed.id;
    let url = feed.url.clone();
    let schedule = state.fetch_schedule(&feed);
    let mut inserted_ids = Vec::new();
    // 这个源自己的处理在它的调用链里；刷新进度、网络状态与排序调度在链外，
    // 免得整次刷新的汇总挂到最后完成的那个源上。
    let result = async {
        let result = match response {
            Ok(response) => {
                let status = response.status;
                let deferred = matches!(status, 429 | 503)
                    .then(|| state.deferred(feed.id, &feed.url))
                    .flatten();
                apply_response(state.path.clone(), feed, response, schedule, state.clock.now().utc)
                    .await
                    .map(|(mut result, ids)| {
                        inserted_ids = ids;
                        if let Some(deferred) = deferred
                            && matches!(&result.status, RefreshStatus::Failed { message } if message == &format!("HTTP {status}"))
                        {
                            result.status = deferred.status();
                        }
                        result
                    })
            }
            // Dispatch was re-checked just before this hop and the network had
            // paused meanwhile. The source did not fail and is not deferred: keep
            // the claim for the retry after connectivity returns.
            Err(FetchFailure {
                error: rrss_core::Error::FetchPaused,
                ..
            }) => {
                state.retry_network((id, generation));
                return None;
            }
            Err(FetchFailure {
                error: rrss_core::Error::FetchDeferred { until, reason },
                ..
            }) => {
                let local_message = match reason {
                    rrss_core::ingest::FetchDeferredReason::CooldownSaving => "正在保存订阅限流记录，稍后继续抓取",
                    rrss_core::ingest::FetchDeferredReason::QueueBusy => "本地抓取队列繁忙，等待下载名额超时，请稍后重试",
                    _ => "",
                };
                let deferred = state.deferred(feed.id, &feed.url)
                    .unwrap_or_else(|| super::cooldown::Deferred::from_reason(until, reason, local_message));
                state.deferred_result(&feed, generation, deferred).await;
                return None;
            }
            Err(error) if error.network => match super::refresh_network_failure(
                state.clone(),
                feed,
                rrss_core::feeds::refresh_error_json(&error.error),
                schedule.with_failure(error.class).with_error(error.kind, error.status()),
            )
            .await
            {
                Ok(Some(result)) => Ok(result),
                Ok(None) => {
                    state.retry_network((id, generation));
                    return None;
                }
                Err(error) => Err(error),
            },
            Err(error) => {
                refresh_failed(
                    state.path.clone(),
                    feed,
                    rrss_core::feeds::refresh_error_json(&error.error),
                    schedule.with_failure(error.class).with_error(error.kind, error.status()),
                )
                .await
            }
        };
        Some(result.unwrap_or_else(|error| {
            tracing::warn!(
                feed_id = rrss_core::logsafe::unless_in_span("feed_id", id),
                error = %crate::log::error(error.as_ref()),
                "feed health write failed"
            );
            RefreshResult {
                feed_id: id,
                status: RefreshStatus::Failed {
                    message: "抓取失败".into(),
                },
            }
        }))
    }
    .instrument(span.clone())
    .await;
    let Some(result) = result else {
        drop(_bytes);
        drop(_bypass);
        state.wake.notify_one();
        return;
    };
    if matches!(
        result.status,
        RefreshStatus::Updated { .. } | RefreshStatus::NotModified
    ) {
        state.connectivity.lock().unwrap().succeeded(&url);
    }
    let feed_id = result.feed_id;
    state.finish(result, generation);
    state.rank_later(feed_id, inserted_ids, span);
    drop(_bytes);
    drop(_bypass);
    state.wake.notify_one();
}

async fn run_icons(shared: Arc<Shared>) {
    let mut dirty = shared.icons.subscribe();
    let mut stop = shared.stop.subscribe();
    tokio::select! {
        _ = stop.wait_for(|stopped| *stopped) => {},
        _ = async {
            let mut consumed = 0;
            loop {
                let version = *dirty.borrow_and_update();
                if version > consumed {
                    async {
                    match shared.network.client(rrss_core::network::NetworkScope::Content).await {
                        Ok(client) => crate::jobs::feed_icons::run(&shared.path, &client).await,
                        Err(error) => tracing::warn!(%error, "feed icons unavailable"),
                    }
                    }.instrument(crate::jobs::span("feed_icons")).await;
                    consumed = version;
                    shared.invalidate();
                } else if dirty.changed().await.is_err() { break; }
            }
        } => {},
    }
}

async fn run_events(shared: Arc<Shared>) {
    let mut stop = shared.stop.subscribe();
    loop {
        tokio::select! {
            biased;
            _ = stop.wait_for(|stopped| *stopped) => break,
            _ = shared.events.notified() => {},
        }
        tokio::select! {
            biased;
            _ = stop.wait_for(|stopped| *stopped) => break,
            _ = tokio::time::sleep(Duration::from_millis(100)) => {},
        }
        let events = std::mem::take(&mut *shared.event_pending.lock().unwrap());
        shared.metrics.event_batches.fetch_add(1, Ordering::Relaxed);
        shared
            .metrics
            .event_emitted
            .fetch_add(events.len() as u64, Ordering::Relaxed);
        for event in events.into_values() {
            (shared.sink)(event);
        }
        if let Some(observer) = shared.observer.get() {
            observer(shared.snapshot());
        }
    }
    if let Some(observer) = shared.observer.get() {
        observer(shared.snapshot());
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    use std::{future::Future, task::Poll};

    #[tokio::test]
    async fn paused_first_hop_requeues_even_when_connectivity_recovers_before_recording() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(304))
            .expect(1)
            .mount(&server)
            .await;
        let (_dir, path) = super::super::tests::fixture();
        let conn = db::open_at(&path).unwrap();
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,?1,0)",
            [server.uri()],
        )
        .unwrap();
        let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
        let state = &scheduler.shared;
        let global = state
            .global
            .clone()
            .try_acquire_many_owned(state.limits.global as u32)
            .unwrap();
        state.await_cooldown_ready().await.unwrap();
        let probe = state.probe_lock.lock().await;
        let ticket = scheduler.submit(1).unwrap();
        let generation = {
            let mut queue = state.queue.lock().unwrap();
            queue.pending.retain(|id| *id != 1);
            queue.entries[&1].generation
        };
        state
            .connectivity
            .lock()
            .unwrap()
            .failure(&server.uri(), tokio::time::Instant::now());
        let url = reqwest::Url::parse(&server.uri()).unwrap();
        let error = state.check_dispatch(&url).unwrap_err();
        assert!(matches!(error, rrss_core::Error::FetchPaused));
        state.network_response();
        process_response(
            state.clone(),
            Downloaded {
                feed: rrss_core::feeds::get_feed(&conn, 1).unwrap(),
                generation,
                response: Err(error.into()),
                _bytes: BufferedBytes {
                    shared: state.clone(),
                    bytes: 0,
                },
                _bypass: None,
                finished_at: tokio::time::Instant::now(),
                span: tracing::Span::none(),
            },
        )
        .await;
        assert!(state.queue.lock().unwrap().entries.contains_key(&1));
        drop((probe, global));
        state.wake.notify_one();
        let result = tokio::time::timeout(Duration::from_secs(10), ticket.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.status, RefreshStatus::NotModified);
        assert!(scheduler.shutdown().await);
    }

    #[tokio::test]
    async fn host_reservation_counts_survive_repark_requeue_finish_and_stop() {
        fn consistent(queue: &Queue) {
            let mut expected = HashMap::new();
            for host in queue
                .entries
                .values()
                .filter_map(|entry| entry.resuming.as_ref())
            {
                *expected.entry(host.clone()).or_insert(0_usize) += 1;
            }
            assert_eq!(queue.resuming, expected);
            assert!(queue.resuming.values().all(|count| *count > 0));
        }
        let (_dir, path) = super::super::tests::fixture();
        let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
        let state = &scheduler.shared;
        let global = state
            .global
            .clone()
            .try_acquire_many_owned(state.limits.global as u32)
            .unwrap();
        state.await_cooldown_ready().await.unwrap();
        let url = reqwest::Url::parse("https://parked.invalid/feed").unwrap();
        let key = host_key(&url);
        let host = state
            .host(&url)
            .try_acquire_many_owned(state.host_limit() as u32)
            .unwrap();
        let tickets = (1..=6)
            .map(|id| scheduler.submit(id).unwrap())
            .collect::<Vec<_>>();
        let claims = {
            let mut queue = state.queue.lock().unwrap();
            queue.pending.clear();
            (1..=6)
                .map(|id| (id, queue.entries[&id].generation))
                .collect::<Vec<_>>()
        };
        for claim in &claims {
            state.park(key.clone(), *claim, ParkReason::Capacity);
        }
        drop(host);
        assert!(state.unpark_ready());
        for _ in 0..100 {
            assert!(
                !state.unpark_ready(),
                "a pending reservation must prevent over-waking the host"
            );
        }
        {
            let queue = state.queue.lock().unwrap();
            consistent(&queue);
            assert_eq!(queue.resuming[&key], state.host_limit());
        }
        state.finish(
            RefreshResult {
                feed_id: 1,
                status: RefreshStatus::Deleted,
            },
            claims[0].1,
        );
        assert!(state.unpark_ready());
        {
            let mut queue = state.queue.lock().unwrap();
            queue.wake_parked(6);
            consistent(&queue);
        }
        state.park(key.clone(), claims[5], ParkReason::Capacity);
        {
            let mut queue = state.queue.lock().unwrap();
            consistent(&queue);
            // Simulate a prepared claim whose source now belongs to another host.
            queue.set_resuming(2, Some("replacement.invalid:443".into()));
            queue.pending.retain(|id| *id != 2);
            consistent(&queue);
        }
        requeue(state, claims[1]);
        {
            let queue = state.queue.lock().unwrap();
            consistent(&queue);
            assert!(!queue.resuming.contains_key("replacement.invalid:443"));
        }
        assert!(scheduler.shutdown().await);
        assert!(state.queue.lock().unwrap().resuming.is_empty());
        drop((global, tickets));
    }

    #[tokio::test]
    async fn redirect_admission_timeout_does_not_change_feed_health_or_backoff() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
        let target = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(304))
            .expect(0)
            .mount(&target)
            .await;
        let origin = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", target.uri()))
            .mount(&origin)
            .await;
        let (_dir, path) = super::super::tests::fixture();
        let conn = db::open_at(&path).unwrap();
        conn.execute(
            "INSERT INTO feeds(id,url,created_at,fail_count,slow_fail_count,failing_since,error,next_fetch_at)
            VALUES(1,?1,0,3,2,123,'previous failure',456)",
            [origin.uri()],
        )
        .unwrap();
        let scheduler = Scheduler::with_limits(
            path.clone(),
            NetworkClients::new(&path),
            |_| {},
            Limits {
                host: 1,
                policy: rrss_core::ingest::feed_http::FeedPolicy {
                    timeout: Duration::from_secs(60),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        scheduler.shared.await_cooldown_ready().await.unwrap();
        let target_url = reqwest::Url::parse(&target.uri()).unwrap();
        let held = scheduler
            .shared
            .host(&target_url)
            .acquire_owned()
            .await
            .unwrap();
        let ticket = scheduler.submit(1).unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if !origin.received_requests().await.unwrap().is_empty()
                    && scheduler.shared.global.available_permits() == scheduler.shared.limits.global
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(61)).await;
        tokio::time::resume();
        let result = tokio::time::timeout(Duration::from_secs(10), ticket.wait())
            .await
            .unwrap()
            .unwrap();
        let health: (i64, i64, i64, String, i64) = conn
            .query_row(
                "SELECT fail_count,slow_fail_count,failing_since,error,next_fetch_at FROM feeds WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        drop(held);
        assert!(scheduler.shutdown().await);
        assert!(
            matches!(result.status, RefreshStatus::Skipped { .. }),
            "{result:?}"
        );
        assert_eq!(health, (3, 2, 123, "previous failure".into(), 456));
    }

    #[tokio::test]
    async fn redirect_waiter_rechecks_cooldown_after_acquiring_capacity() {
        let (_dir, path) = super::super::tests::fixture();
        let conn = db::open_at(&path).unwrap();
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://feed.invalid/rss',0)",
            [],
        )
        .unwrap();
        let feed = rrss_core::feeds::get_feed(&conn, 1).unwrap();
        let url = reqwest::Url::parse(&feed.url).unwrap();
        let scheduler = Scheduler::with_limits(
            path.clone(),
            NetworkClients::new(&path),
            |_| {},
            Limits {
                host: 1,
                ..Default::default()
            },
        );
        let state = &scheduler.shared;
        state.await_cooldown_ready().await.unwrap();
        let held = state.host(&url).acquire_owned().await.unwrap();
        let waiting = state.permits(url.clone(), None);
        tokio::pin!(waiting);
        std::future::poll_fn(|cx| {
            assert!(waiting.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
        state
            .response_head(
                feed,
                1,
                rrss_core::ingest::feed_http::ResponseHead {
                    url: url.clone(),
                    visited: vec![url],
                    status: 429,
                    retry_after: Some("3600".into()),
                },
            )
            .await
            .unwrap();
        drop(held);
        assert!(
            matches!(waiting.await, Err(rrss_core::Error::FetchDeferred { .. })),
            "a redirect waiting for capacity must honor the newly committed limit"
        );
        assert!(scheduler.shutdown().await);
    }

    #[tokio::test]
    async fn prepared_first_hop_rechecks_cooldown_before_dispatch() {
        let (_dir, path) = super::super::tests::fixture();
        let conn = db::open_at(&path).unwrap();
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://feed.invalid/rss',0)",
            [],
        )
        .unwrap();
        let feed = rrss_core::feeds::get_feed(&conn, 1).unwrap();
        let url = reqwest::Url::parse(&feed.url).unwrap();
        let scheduler = Scheduler::with_limits(
            path.clone(),
            NetworkClients::new(&path),
            |_| {},
            Limits::default(),
        );
        let state = &scheduler.shared;
        state.await_cooldown_ready().await.unwrap();
        let reserved = state.try_permits(&url, false).ok().unwrap();
        state
            .response_head(
                feed,
                1,
                rrss_core::ingest::feed_http::ResponseHead {
                    url: url.clone(),
                    visited: vec![url.clone()],
                    status: 429,
                    retry_after: Some("3600".into()),
                },
            )
            .await
            .unwrap();
        assert!(matches!(
            state.permits(url, Some(reserved)).await,
            Err(rrss_core::Error::FetchDeferred { .. })
        ));
        assert!(scheduler.shutdown().await);
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    #[tokio::test]
    async fn burst_events_collapse_and_diagnostics_exclude_private_values() {
        let (_dir, path) = super::super::tests::fixture();
        let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
        let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), move |event| {
            events.send(event).ok();
        });
        for id in 1..=1000 {
            scheduler.shared.emit(views::FeedUpdated {
                run_id: scheduler.shared.run_id.clone(),
                feed_id: Some(id),
                generation: Some(id as u64),
                status: "failed".into(),
                inserted: 0,
                derived_degraded: false,
                error: Some("synthetic-private-error".into()),
            });
        }
        assert_eq!(scheduler.shared.event_pending.lock().unwrap().len(), 1);
        let event = tokio::time::timeout(Duration::from_secs(5), received.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(event.feed_id.is_none() && event.generation.is_none() && event.error.is_none());
        let snapshot = Scheduler::diagnostics_for(&path).unwrap();
        let received = snapshot
            .counters
            .iter()
            .find(|c| c.name == "events_received")
            .unwrap()
            .count;
        let coalesced = snapshot
            .counters
            .iter()
            .find(|c| c.name == "events_coalesced")
            .unwrap()
            .count;
        assert!(received >= 1000 && coalesced >= 999);
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("synthetic-private-error"));
        assert!(!json.contains(&path.to_string_lossy().to_string()));
        assert!(
            snapshot
                .timings
                .iter()
                .any(|t| t.stage == "db_connection_wait")
        );
        use rrss_ai::admission::Admission;
        let model = super::super::model_capacity::RequestAdmission::new(
            &path,
            "https://synthetic-user:fixture-secret@private-service.test/v1?key=private-query",
            super::super::model_capacity::Priority::Interactive,
            false,
        )
        .unwrap();
        let permit = model.acquire().await.unwrap();
        let conn = db::open_at(&path).unwrap();
        let queried = crate::queries::diagnostics(&conn, &path).scheduler.unwrap();
        assert_eq!(queried.run_id, snapshot.run_id);
        assert_eq!(queried.models.active, 1);
        let json = serde_json::to_string(&queried).unwrap();
        for private in [
            "fixture-secret",
            "private-service",
            "private-query",
            "synthetic-user",
        ] {
            assert!(!json.contains(private));
        }
        drop(permit);
        assert!(scheduler.shutdown().await);
        let stopped = scheduler.diagnostics();
        assert!(stopped.stopped);
        assert_eq!(
            stopped.feeds.admitted
                + stopped.feeds.http_active
                + stopped.feeds.parsing
                + stopped.feeds.buffered_bytes,
            0
        );
    }
}
