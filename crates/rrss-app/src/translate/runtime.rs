//! 应用层翻译作业内核；本模块不依赖 Tauri，宿主通过 ipc 投影接入。
pub use super::types::*;
use super::{
    cache::{self, Config, ProgressCache},
    window::{self, Backend, HttpBackend},
};
use crate::jobs::spawn_db;
use crate::views::{TranslationHint, TranslationInitialViewport, TranslationViewport};
use rrss_core::{articles, db};
use rrss_render::{Document, TranslationPatch, TranslationPlan};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    sync::{Mutex as AsyncMutex, Semaphore},
    time::Instant,
};

pub const WINDOW_TIMEOUT: Duration = Duration::from_secs(45);
pub const MAX_ACTIVE_JOBS: usize = 4;
pub const MAX_TERMINAL_JOBS: usize = 4;
pub const TERMINAL_TTL: Duration = Duration::from_secs(300);
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(1);
static EXECUTION_SLOT: OnceLock<Arc<Semaphore>> = OnceLock::new();
static RUNTIMES: OnceLock<Mutex<Vec<Weak<Runtime>>>> = OnceLock::new();

#[derive(Clone, Copy)]
struct Limits {
    timeout: Duration,
    chunk_chars: usize,
    terminal_ttl: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: WINDOW_TIMEOUT,
            chunk_chars: super::COMPLETE_CHUNK_CHARS,
            terminal_ttl: TERMINAL_TTL,
        }
    }
}

struct Source {
    title: String,
    document: Document,
    plan: TranslationPlan,
    config: Config,
    hash: String,
}
struct Work {
    source: Arc<Source>,
    title: String,
    document: Document,
    bypass: bool,
}
struct Job {
    snapshot: Snapshot,
    work: Option<Work>,
    revision: u64,
    ended: Option<Instant>,
}

impl Job {
    fn event(&mut self, kind: EventKind) -> Event {
        self.snapshot.seq += 1;
        Event {
            identity: self.snapshot.identity.clone(),
            seq: self.snapshot.seq,
            phase: self.snapshot.phase,
            save: self.snapshot.save,
            total: self.snapshot.units.len(),
            done: self
                .snapshot
                .units
                .iter()
                .filter(|s| **s == UnitState::Done)
                .count(),
            failed: self
                .snapshot
                .units
                .iter()
                .filter(|s| matches!(s, UnitState::Failed(_)))
                .count(),
            error: self.snapshot.error,
            kind,
        }
    }
    fn snapshot(&self) -> Snapshot {
        let mut snapshot = self.snapshot.clone();
        if let Some(work) = &self.work {
            fill_overlay(&mut snapshot, &work.title, &work.document);
        }
        snapshot
    }
    fn invalidate(&mut self) -> Event {
        self.snapshot.phase = Phase::Invalidated;
        self.snapshot.error = Some(Failure::Invalidated);
        for state in &mut self.snapshot.units {
            if matches!(state, UnitState::Pending | UnitState::Inflight) {
                *state = UnitState::Failed(Failure::Invalidated);
            }
        }
        self.snapshot.save = SaveState::Unsaved;
        self.work = None;
        self.ended = Some(Instant::now());
        self.event(EventKind::Error(Failure::Invalidated))
    }
}

fn fill_overlay(snapshot: &mut Snapshot, title: &str, document: &Document) {
    snapshot.title = None;
    snapshot.blocks.clear();
    for (state, index) in snapshot.units.iter().zip(&snapshot.unit_map) {
        if *state != UnitState::Done {
            continue;
        }
        match index {
            None => snapshot.title = Some(title.into()),
            Some(index) => {
                if let Some(block) = document.blocks.get(*index) {
                    snapshot.blocks.push(BlockPatch {
                        index: *index,
                        ir: block.clone(),
                    });
                }
            }
        }
    }
}

#[derive(Default)]
struct State {
    jobs: HashMap<u64, Job>,
    current: HashMap<i64, u64>,
    queue: VecDeque<u64>,
    worker: bool,
    foreground: Option<(u64, u64)>,
    reader_attachment: (u64, u64),
    reader_released: bool,
    viewport: Option<Priority>,
}

struct Dispatch {
    generation: u64,
    id: i64,
    source: Arc<Source>,
    ids: Vec<u32>,
    bypass_cache: bool,
}

struct Priority {
    intent: u64,
    visible: HashSet<usize>,
    nearby: HashSet<usize>,
    /// 当前阅读位置；其余内容先顺着它往下，再回到前文。
    anchor: Option<usize>,
}

impl State {
    /// 执行槽取得后才组窗；锁内只维护队列与单元状态，不执行数据库或网络操作。
    fn next_window(&mut self, limits: Limits) -> Option<Dispatch> {
        if let Some((_, foreground)) = self.foreground {
            if let Some(pos) = self.queue.iter().position(|g| *g == foreground) {
                self.queue.remove(pos);
                self.queue.push_front(foreground);
            }
        }
        let mut picked = None;
        while let Some(generation) = self.queue.pop_front() {
            let State {
                jobs,
                foreground,
                viewport,
                ..
            } = &mut *self;
            let priority = viewport
                .as_ref()
                .filter(|_| foreground.is_some_and(|(_, g)| g == generation));
            let Some(job) = jobs
                .get_mut(&generation)
                .filter(|job| job.snapshot.phase.active())
            else {
                continue;
            };
            let Some(work) = &job.work else {
                continue;
            };
            let mut ids = Vec::new();
            let mut chars = 0;
            let mut pending: Vec<_> = job
                .snapshot
                .units
                .iter()
                .enumerate()
                .filter_map(|(i, status)| (*status == UnitState::Pending).then_some(i))
                .collect();
            // 稳定排序保持每档文序；标题始终先于正文。取得执行槽后才读取最新意图。
            // 其余内容先接阅读位置往下，读者向下读时不会追上未译段落，同窗也尽量保持上下文连续。
            if let Some(priority) = priority {
                pending.sort_by_key(|i| match job.snapshot.unit_map[*i] {
                    None => 0,
                    Some(block) if priority.visible.contains(&block) => 1,
                    Some(block) if priority.nearby.contains(&block) => 2,
                    Some(block) if priority.anchor.is_some_and(|anchor| block < anchor) => 4,
                    _ => 3,
                });
            }
            for i in pending {
                let len = work.source.plan.units[i].t.chars().count();
                if !ids.is_empty()
                    && (chars + len > limits.chunk_chars
                        || matches!(
                            work.source.config.backend,
                            super::TranslateBackend::DeepLx { .. }
                        ))
                {
                    break;
                }
                job.snapshot.units[i] = UnitState::Inflight;
                ids.push(i as u32);
                chars += len;
            }
            if ids.is_empty() {
                continue;
            }
            job.snapshot.phase = Phase::Running;
            picked = Some(Dispatch {
                generation,
                id: job.snapshot.identity.article_id,
                source: Arc::clone(&work.source),
                ids,
                bypass_cache: work.bypass,
            });
            break;
        }
        if picked.is_none() {
            self.worker = false;
        }
        picked
    }

    fn attach(&mut self, generation: u64, ticket: u64) {
        let attachment = (ticket, generation);
        if attachment >= self.reader_attachment
            && !(attachment == self.reader_attachment && self.reader_released)
            && self.jobs.contains_key(&generation)
        {
            if self.foreground != Some((ticket, generation)) {
                self.viewport = None;
            }
            self.reader_attachment = attachment;
            self.reader_released = false;
            self.foreground = Some((ticket, generation));
        }
    }

    fn viewport(
        &mut self,
        generation: u64,
        ticket: u64,
        intent: u64,
        viewport: TranslationViewport,
    ) {
        if self.foreground != Some((ticket, generation))
            || self.viewport.as_ref().is_some_and(|v| intent <= v.intent)
        {
            return;
        }
        let Some(work) = self
            .jobs
            .get(&generation)
            .filter(|job| job.snapshot.phase.active())
            .and_then(|job| job.work.as_ref())
        else {
            return;
        };
        let eligible = |index: &usize| {
            // 表格与代码块一样保留原文，不进入翻译单元或可见区优先队列。
            matches!(
                work.source.document.blocks.get(*index),
                Some(
                    rrss_render::Block::Heading { .. }
                        | rrss_render::Block::Paragraph { .. }
                        | rrss_render::Block::ListItem { .. }
                        | rrss_render::Block::Quote { .. }
                )
            )
        };
        // 位置取原始可见块（含图片等不参与优先的块）；可见区为空时退到附近区的首块。
        let blocks = work.source.document.blocks.len();
        let first = |ids: &[usize]| ids.iter().copied().filter(|i| *i < blocks).min();
        let anchor = first(&viewport.visible).or_else(|| first(&viewport.nearby));
        let visible: HashSet<_> = viewport.visible.into_iter().filter(eligible).collect();
        let nearby = viewport
            .nearby
            .into_iter()
            .filter(eligible)
            .filter(|index| !visible.contains(index))
            .collect();
        self.viewport = Some(Priority {
            intent,
            visible,
            nearby,
            anchor,
        });
    }

    fn initial_viewport(
        &mut self,
        snapshot: &Snapshot,
        initial: Option<TranslationInitialViewport>,
    ) {
        if let Some(initial) = initial.filter(|v| v.source_hash == snapshot.identity.source_hash) {
            self.attach(snapshot.identity.generation, initial.ticket);
            self.viewport(
                snapshot.identity.generation,
                initial.ticket,
                initial.intent,
                initial.viewport,
            );
        }
    }
}

pub struct Runtime {
    path: PathBuf,
    backend: Arc<dyn Backend>,
    sink: Arc<dyn EventSink>,
    state: Mutex<State>,
    gate: AsyncMutex<()>,
    limits: Limits,
    cleanup: AtomicBool,
    executor: OnceLock<tokio::runtime::Handle>,
}

impl Runtime {
    pub fn hint(&self, hint: TranslationHint) {
        let Some(generation) = hint.identity.generation else {
            return;
        };
        let mut state = self.state.lock().unwrap();
        if state.current.get(&hint.identity.article_id) != Some(&generation)
            || !state.jobs.get(&generation).is_some_and(|job| {
                job.snapshot.identity.source_hash == hint.identity.source_hash
                    && job.snapshot.identity.config_hash == hint.identity.config_hash
            })
        {
            return;
        }
        state.viewport(generation, hint.ticket, hint.intent, hint.viewport);
    }

    /// 一库一实例；所有实例仍共用进程级网络执行槽。
    pub fn new(path: &Path, sink: Arc<dyn EventSink>) -> anyhow::Result<Arc<Self>> {
        let _ = db::open_at(path)?;
        let path = path.canonicalize()?;
        let mut runtimes = RUNTIMES.get_or_init(Mutex::default).lock().unwrap();
        runtimes.retain(|weak| weak.strong_count() > 0);
        if let Some(existing) = runtimes
            .iter()
            .filter_map(Weak::upgrade)
            .find(|r| r.path == path)
        {
            return Ok(existing);
        }
        let network = crate::network::NetworkClients::new(&path);
        let runtime = Self::build(
            path,
            Arc::new(HttpBackend(network)),
            sink,
            Limits::default(),
        );
        runtimes.push(Arc::downgrade(&runtime));
        Ok(runtime)
    }

    fn build(
        path: PathBuf,
        backend: Arc<dyn Backend>,
        sink: Arc<dyn EventSink>,
        limits: Limits,
    ) -> Arc<Self> {
        Arc::new(Self {
            path,
            backend,
            sink,
            limits,
            state: Mutex::default(),
            gate: AsyncMutex::new(()),
            cleanup: AtomicBool::new(false),
            executor: OnceLock::new(),
        })
    }

    pub async fn start(
        self: &Arc<Self>,
        id: i64,
        mode: StartMode,
        initial: Option<TranslationInitialViewport>,
    ) -> Result<StartResult, Failure> {
        self.executor.get_or_init(tokio::runtime::Handle::current);
        let _gate = self.gate.lock().await;
        self.prune();
        let prepared = spawn_db(self.path.clone(), move |conn| {
            let tx = conn.unchecked_transaction()?;
            let article = articles::get_article(&tx, id)?;
            if !rrss_core::translations::is_enabled(&tx)? {
                return Ok(Err("翻译功能未开启"));
            }
            if article.pruned_at.is_some() {
                return Ok(Err("正文已清理，请先重新抓取正文"));
            }
            let document = crate::document::load_document(&article);
            let plan = TranslationPlan::new(&article.title, &document);
            if plan.units.is_empty() {
                return Ok(Err("没有可翻译的正文"));
            }
            let config = Config::read(&tx)?;
            if rrss_render::already_target_lang(
                &rrss_render::units_plain(&plan.units),
                &config.lang,
            ) {
                return Ok(Err("已是目标语言，无需翻译"));
            }
            let cached = cache::read_progress(&tx, &article, &document, &config)?;
            let hash = crate::document::source_hash(&article.title, &document);
            tx.commit()?;
            Ok(Ok((
                Arc::new(Source {
                    title: article.title,
                    document,
                    plan,
                    config,
                    hash,
                }),
                cached,
            )))
        })
        .await
        .map_err(|err| window::classify(&err))?;
        let (source, mut cached) = match prepared {
            Ok(value) => value,
            Err(note) => return Ok(StartResult::Noop(note)),
        };
        let mut events = Vec::new();
        let (snapshot, spawn) = {
            let mut state = self.state.lock().unwrap();
            let previous = state.current.get(&id).copied();
            let previous_job = previous.and_then(|generation| state.jobs.get(&generation));
            let same = previous_job.is_some_and(|job| {
                job.snapshot.identity.source_hash == source.hash
                    && job.snapshot.identity.config_hash == source.config.hash
                    && job.revision == source.config.revision
            });
            if let Some(job) = previous_job.filter(|_| same && mode != StartMode::Regenerate) {
                if job.snapshot.phase.active()
                    || (job.snapshot.save == SaveState::SaveFailed
                        && (mode == StartMode::Default
                            || job.snapshot.units.iter().all(|s| *s == UnitState::Done)))
                {
                    let snapshot = job.snapshot();
                    state.initial_viewport(&snapshot, initial);
                    return Ok(StartResult::Resumed(snapshot));
                }
                if let Some(work) = &job.work {
                    cached = Some(ProgressCache {
                        title: work.title.clone(),
                        document: work.document.clone(),
                        states: job.snapshot.units.clone(),
                    });
                }
            }
            if mode == StartMode::RetryFailed && cached.is_none() {
                return Err(Failure::RetryUnavailable);
            }
            if mode != StartMode::Regenerate {
                if let Some(cache) = &cached {
                    if cache.states.iter().all(|s| *s == UnitState::Done) {
                        return Ok(StartResult::Cached {
                            title: cache.title.clone(),
                            document: cache.document.clone(),
                            units: cache.states.clone(),
                            unit_map: source.plan.block_ids.clone(),
                            source_hash: source.hash.clone(),
                            config_hash: source.config.hash.clone(),
                        });
                    }
                }
            }
            if state
                .jobs
                .iter()
                .filter(|(g, job)| {
                    Some(**g) != previous
                        && (job.snapshot.phase.active() || job.snapshot.save == SaveState::Saving)
                })
                .count()
                >= MAX_ACTIVE_JOBS
            {
                return Err(Failure::Capacity);
            }
            if let Some(job) = previous.and_then(|generation| state.jobs.get_mut(&generation)) {
                if job.work.is_some() || job.snapshot.phase.active() {
                    events.push(job.invalidate());
                }
            }
            let bypass = mode != StartMode::Default || cached.is_some();
            let mut units = vec![UnitState::Pending; source.plan.units.len()];
            let mut title = source.title.clone();
            let mut document = source.document.clone();
            if mode != StartMode::Regenerate {
                if let Some(cache) = cached {
                    title = cache.title;
                    document = cache.document;
                    for (state, old) in units.iter_mut().zip(cache.states) {
                        if old == UnitState::Done {
                            *state = UnitState::Done;
                        }
                    }
                }
            }
            let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
            let snapshot = Snapshot {
                identity: Identity {
                    article_id: id,
                    generation,
                    source_hash: source.hash.clone(),
                    config_hash: source.config.hash.clone(),
                },
                seq: 0,
                phase: Phase::Queued,
                save: SaveState::Unsaved,
                units,
                unit_map: source.plan.block_ids.clone(),
                title: None,
                blocks: Vec::new(),
                error: None,
            };
            let mut job = Job {
                snapshot,
                revision: source.config.revision,
                work: Some(Work {
                    source,
                    title,
                    document,
                    bypass,
                }),
                ended: None,
            };
            events.push(job.event(EventKind::Start));
            let snapshot = job.snapshot();
            state.jobs.insert(generation, job);
            state.current.insert(id, generation);
            state.initial_viewport(&snapshot, initial);
            state.queue.push_back(generation);
            let spawn = !state.worker;
            state.worker = true;
            (snapshot, spawn)
        };
        for event in events {
            self.sink.emit(event);
        }
        self.prune();
        self.schedule_cleanup();
        if spawn {
            let runtime = Arc::clone(self);
            tokio::spawn(async move {
                runtime.run().await;
            });
        }
        Ok(StartResult::Started(snapshot))
    }

    /// 阅读附着只改变下一窗的前台优先级；不启动任务。
    pub fn attach(&self, generation: u64, ticket: u64) {
        self.state.lock().unwrap().attach(generation, ticket);
    }
    pub fn detach(&self, generation: u64, ticket: u64) {
        let mut state = self.state.lock().unwrap();
        if (ticket, generation) >= state.reader_attachment {
            state.reader_attachment = (ticket, generation);
            state.reader_released = true;
            state.foreground = None;
            state.viewport = None;
        }
    }

    pub async fn snapshot(
        self: &Arc<Self>,
        id: i64,
        generation: Option<u64>,
    ) -> Result<Option<Snapshot>, Failure> {
        let _gate = self.gate.lock().await;
        self.prune();
        let (generation, identity, revision, saved) = {
            let state = self.state.lock().unwrap();
            let Some(generation) = generation.or_else(|| state.current.get(&id).copied()) else {
                return Ok(None);
            };
            if state.current.get(&id) != Some(&generation) {
                return Ok(None);
            }
            let Some(job) = state
                .jobs
                .get(&generation)
                .filter(|job| job.snapshot.identity.article_id == id)
            else {
                return Ok(None);
            };
            (
                generation,
                job.snapshot.identity.clone(),
                job.revision,
                job.snapshot.save == SaveState::Saved,
            )
        };
        let (valid, cached) = spawn_db(self.path.clone(), move |conn| {
            let tx = conn.unchecked_transaction()?;
            let article = match articles::get_article(&tx, id) {
                Ok(article) => article,
                Err(rrss_core::Error::ArticleNotFound) => return Ok((false, None)),
                Err(err) => return Err(err),
            };
            let source = crate::document::load_document(&article);
            let Some(config) = Config::optional(&tx)? else {
                return Ok((false, None));
            };
            if !rrss_core::translations::is_enabled(&tx)?
                || article.pruned_at.is_some()
                || (!saved && config.revision != revision)
                || config.hash != identity.config_hash
                || crate::document::source_hash(&article.title, &source) != identity.source_hash
            {
                return Ok((false, None));
            }
            Ok((true, cache::read_progress(&tx, &article, &source, &config)?))
        })
        .await
        .map_err(|_| Failure::Storage)?;
        if !valid {
            self.invalidate(generation);
        }
        let state = self.state.lock().unwrap();
        let Some(job) = state.jobs.get(&generation) else {
            return Ok(None);
        };
        let mut snapshot = job.snapshot();
        if valid && job.work.is_none() && snapshot.save == SaveState::Saved {
            if let Some(cache) = cached {
                fill_overlay(&mut snapshot, &cache.title, &cache.document);
            } else {
                snapshot.phase = Phase::Invalidated;
                snapshot.error = Some(Failure::Invalidated);
            }
        }
        Ok(Some(snapshot))
    }

    pub async fn recover(self: &Arc<Self>, id: i64) -> Result<Recovery, Failure> {
        if let Some(snapshot) = self.snapshot(id, None).await? {
            return Ok(Recovery::Session(snapshot));
        }
        spawn_db(self.path.clone(), move |conn| {
            let tx = conn.unchecked_transaction()?;
            let article = articles::get_article(&tx, id)?;
            let source = crate::document::load_document(&article);
            let Some(config) = Config::optional(&tx)? else {
                return Ok(Recovery::Missing);
            };
            if !rrss_core::translations::is_enabled(&tx)? {
                return Ok(Recovery::Missing);
            }
            Ok(
                match cache::read_progress(&tx, &article, &source, &config)? {
                    Some(cache) => Recovery::Cached {
                        title: cache.title,
                        document: cache.document,
                        units: cache.states,
                        unit_map: TranslationPlan::new(&article.title, &source).block_ids,
                        source_hash: crate::document::source_hash(&article.title, &source),
                        config_hash: config.hash.clone(),
                    },
                    None => Recovery::Missing,
                },
            )
        })
        .await
        .map_err(|_| Failure::Storage)
    }

    pub async fn retry_save(
        self: &Arc<Self>,
        id: i64,
        generation: u64,
    ) -> Result<Snapshot, Failure> {
        {
            let state = self.state.lock().unwrap();
            let job = state.jobs.get(&generation).ok_or(Failure::Invalidated)?;
            if state.current.get(&id) != Some(&generation)
                || job.snapshot.save != SaveState::SaveFailed
                || job.work.is_none()
            {
                return Err(Failure::Invalidated);
            }
        }
        self.save(generation).await;
        self.schedule_cleanup();
        self.snapshot(id, Some(generation))
            .await?
            .ok_or(Failure::Invalidated)
    }

    async fn is_current(&self, id: i64, source: Arc<Source>) -> Result<bool, Failure> {
        spawn_db(self.path.clone(), move |conn| {
            let tx = conn.unchecked_transaction()?;
            cache::matches_current(&tx, id, &source.hash, &source.config)
        })
        .await
        .map_err(|err| {
            if matches!(
                err.downcast_ref::<rrss_core::Error>(),
                Some(rrss_core::Error::ArticleNotFound)
            ) {
                Failure::Invalidated
            } else {
                Failure::Storage
            }
        })
    }

    async fn run(self: Arc<Self>) {
        loop {
            let permit = EXECUTION_SLOT
                .get_or_init(|| Arc::new(Semaphore::new(1)))
                .clone()
                .acquire_owned()
                .await
                .unwrap();
            let picked = self.state.lock().unwrap().next_window(self.limits);
            let Some(Dispatch {
                generation,
                id,
                source,
                ids,
                bypass_cache,
            }) = picked
            else {
                break;
            };
            if let Err(error) = self
                .is_current(id, Arc::clone(&source))
                .await
                .and_then(|current| {
                    if current {
                        Ok(())
                    } else {
                        Err(Failure::Invalidated)
                    }
                })
            {
                drop(permit);
                self.stop(generation, error);
                self.prune();
                self.schedule_cleanup();
                continue;
            }
            if !self
                .state
                .lock()
                .unwrap()
                .jobs
                .get(&generation)
                .is_some_and(|job| job.snapshot.phase.active())
            {
                continue;
            }
            let started_at = Instant::now();
            let dispatched = {
                let mut state = self.state.lock().unwrap();
                state
                    .jobs
                    .get_mut(&generation)
                    .filter(|job| job.snapshot.phase.active())
                    .map(|job| {
                        job.event(EventKind::Delta {
                            title: None,
                            blocks: Vec::new(),
                            units: ids.iter().map(|id| (*id, UnitState::Inflight)).collect(),
                        })
                    })
            };
            if let Some(event) = dispatched {
                self.sink.emit(event);
            } else {
                continue;
            }
            tracing::debug!(
                article_id = id,
                generation,
                units = ids.len(),
                "translation window started"
            );
            let result = tokio::time::timeout(
                self.limits.timeout,
                self.backend.execute(window::Request {
                    path: &self.path,
                    config: &source.config,
                    plan: &source.plan,
                    source: &source.document,
                    ids: &ids,
                    bypass_cache,
                    article_id: Some(id),
                }),
            )
            .await
            .unwrap_or(Err(Failure::Timeout));
            window::log_finished(
                Some(id),
                Some(generation),
                ids.len(),
                started_at.elapsed(),
                &result,
            );
            drop(permit);
            if let Err(error) = self
                .is_current(id, Arc::clone(&source))
                .await
                .and_then(|current| {
                    if current {
                        Ok(())
                    } else {
                        Err(Failure::Invalidated)
                    }
                })
            {
                self.stop(generation, error);
                self.prune();
                self.schedule_cleanup();
                continue;
            }
            let (event, finished) = {
                let mut state = self.state.lock().unwrap();
                let Some(job) = state
                    .jobs
                    .get_mut(&generation)
                    .filter(|job| job.snapshot.phase.active())
                else {
                    continue;
                };
                let Some(work) = job.work.as_mut() else {
                    continue;
                };
                let mut title = None;
                let mut blocks = Vec::new();
                let mut updates = Vec::new();
                let stop = result
                    .as_ref()
                    .err()
                    .copied()
                    .filter(|error| error.stops_job());
                let output =
                    result.unwrap_or_else(|error| ids.iter().map(|id| (*id, Err(error))).collect());
                for (id, result) in output {
                    let value = match result {
                        Ok(TranslationPatch::Title(text)) => {
                            work.title = text.clone();
                            title = Some(text);
                            UnitState::Done
                        }
                        Ok(TranslationPatch::Block { index, block }) => {
                            work.document.blocks[index] = block.clone();
                            blocks.push(BlockPatch { index, ir: block });
                            UnitState::Done
                        }
                        Err(error) => UnitState::Failed(error),
                    };
                    job.snapshot.units[id as usize] = value;
                    updates.push((id, value));
                }
                if let Some(error) = stop {
                    job.snapshot.error = Some(error);
                    for (i, status) in job.snapshot.units.iter_mut().enumerate() {
                        if *status == UnitState::Pending {
                            *status = UnitState::Failed(error);
                            updates.push((i as u32, *status));
                        }
                    }
                }
                let finished = !job
                    .snapshot
                    .units
                    .iter()
                    .any(|s| matches!(s, UnitState::Pending | UnitState::Inflight));
                if finished {
                    let any_done = job.snapshot.units.contains(&UnitState::Done);
                    let all_done = job.snapshot.units.iter().all(|s| *s == UnitState::Done);
                    job.snapshot.phase = if all_done {
                        Phase::Completed
                    } else if any_done {
                        Phase::Partial
                    } else {
                        Phase::Failed
                    };
                    job.ended = Some(Instant::now());
                    if !any_done {
                        job.snapshot.error = job.snapshot.units.iter().find_map(|s| match s {
                            UnitState::Failed(error) => Some(*error),
                            _ => None,
                        });
                    }
                }
                let event = job.event(EventKind::Delta {
                    title,
                    blocks,
                    units: updates,
                });
                if !finished {
                    state.queue.push_back(generation);
                }
                (event, finished)
            };
            self.sink.emit(event);
            if finished {
                self.save(generation).await;
                self.prune();
                self.schedule_cleanup();
            }
        }
    }

    async fn save(self: &Arc<Self>, generation: u64) {
        let _gate = self.gate.lock().await;
        let prepared = {
            let mut state = self.state.lock().unwrap();
            let Some(id) = state
                .jobs
                .get(&generation)
                .map(|job| job.snapshot.identity.article_id)
            else {
                return;
            };
            if state.current.get(&id) != Some(&generation) {
                return;
            }
            let job = state.jobs.get_mut(&generation).unwrap();
            if job.snapshot.phase.active()
                || job.snapshot.phase == Phase::Invalidated
                || job.snapshot.save == SaveState::Saved
            {
                return;
            }
            if !job.snapshot.units.contains(&UnitState::Done) {
                let event = job.event(EventKind::Error(
                    job.snapshot.error.unwrap_or(Failure::InvalidOutput),
                ));
                drop(state);
                self.sink.emit(event);
                return;
            }
            let Some(work) = job.work.as_mut() else {
                return;
            };
            // 混合缓存中失败处始终是原文；已有完整译文时提交会改为保留旧缓存。
            for (status, index) in job.snapshot.units.iter().zip(&job.snapshot.unit_map) {
                if matches!(status, UnitState::Failed(_)) {
                    match index {
                        Some(index) => {
                            work.document.blocks[*index] =
                                work.source.document.blocks[*index].clone()
                        }
                        None => work.title = work.source.title.clone(),
                    }
                }
            }
            job.snapshot.save = SaveState::Saving;
            (
                job.snapshot.identity.article_id,
                Arc::clone(&work.source),
                ProgressCache {
                    title: work.title.clone(),
                    document: work.document.clone(),
                    states: job.snapshot.units.clone(),
                },
            )
        };
        let (id, source, cache) = prepared;
        let result = spawn_db(self.path.clone(), move |conn| {
            cache::commit_progress(conn, id, &source.hash, &source.config, cache)
        })
        .await;
        let event = {
            let mut state = self.state.lock().unwrap();
            let Some(job) = state
                .jobs
                .get_mut(&generation)
                .filter(|job| job.snapshot.phase != Phase::Invalidated)
            else {
                return;
            };
            match result {
                Ok(commit) => {
                    match commit {
                        cache::Commit::Saved => {
                            job.snapshot.save = SaveState::Saved;
                            job.work = None;
                        }
                        // 工作副本留给「重试未译部分」；旧的完整缓存不被部分结果替换。
                        cache::Commit::Kept => job.snapshot.save = SaveState::Kept,
                    }
                    if job.snapshot.error == Some(Failure::Storage) {
                        job.snapshot.error =
                            job.snapshot.units.iter().find_map(|state| match state {
                                UnitState::Failed(error) if error.stops_job() => Some(*error),
                                _ => None,
                            });
                    }
                    job.event(EventKind::Done)
                }
                Err(error)
                    if matches!(
                        error.downcast_ref::<rrss_core::Error>(),
                        Some(
                            rrss_core::Error::InvalidSetting(_) | rrss_core::Error::ArticleNotFound
                        )
                    ) =>
                {
                    job.invalidate()
                }
                Err(_) => {
                    job.snapshot.save = SaveState::SaveFailed;
                    job.snapshot.error = Some(Failure::Storage);
                    job.event(EventKind::Error(Failure::Storage))
                }
            }
        };
        self.sink.emit(event);
    }

    fn invalidate(self: &Arc<Self>, generation: u64) {
        let event = {
            let mut state = self.state.lock().unwrap();
            state
                .jobs
                .get_mut(&generation)
                .filter(|job| job.snapshot.phase != Phase::Invalidated)
                .map(Job::invalidate)
        };
        if let Some(event) = event {
            self.sink.emit(event);
            self.prune();
            self.schedule_cleanup();
        }
    }

    fn stop(self: &Arc<Self>, generation: u64, error: Failure) {
        if error == Failure::Invalidated {
            self.invalidate(generation);
            return;
        }
        let event = {
            let mut state = self.state.lock().unwrap();
            let Some(job) = state
                .jobs
                .get_mut(&generation)
                .filter(|job| job.snapshot.phase.active())
            else {
                return;
            };
            for status in &mut job.snapshot.units {
                if matches!(status, UnitState::Pending | UnitState::Inflight) {
                    *status = UnitState::Failed(error);
                }
            }
            let any_done = job.snapshot.units.contains(&UnitState::Done);
            job.snapshot.phase = if any_done {
                Phase::Partial
            } else {
                Phase::Failed
            };
            job.snapshot.save = if any_done {
                SaveState::SaveFailed
            } else {
                SaveState::Unsaved
            };
            job.snapshot.error = Some(error);
            job.ended = Some(Instant::now());
            job.event(EventKind::Error(error))
        };
        self.sink.emit(event);
    }

    fn prune(&self) {
        let mut state = self.state.lock().unwrap();
        let mut terminal: Vec<_> = state
            .jobs
            .iter()
            .filter_map(|(generation, job)| {
                job.ended
                    .filter(|_| job.snapshot.save != SaveState::Saving)
                    .map(|time| (*generation, time))
            })
            .collect();
        terminal.sort_by_key(|(_, time)| *time);
        let excess = terminal.len().saturating_sub(MAX_TERMINAL_JOBS);
        for (pos, (generation, time)) in terminal.into_iter().enumerate() {
            if pos < excess || time.elapsed() >= self.limits.terminal_ttl {
                state.jobs.remove(&generation);
                state.current.retain(|_, current| *current != generation);
                state.queue.retain(|current| *current != generation);
            }
        }
    }

    fn schedule_cleanup(self: &Arc<Self>) {
        let Some(executor) = self.executor.get() else {
            return;
        };
        if self.cleanup.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        executor.spawn(async move {
            loop {
                let Some(runtime) = weak.upgrade() else {
                    break;
                };
                runtime.prune();
                let deadline = {
                    let state = runtime.state.lock().unwrap();
                    let deadline = state
                        .jobs
                        .values()
                        .filter(|job| job.snapshot.save != SaveState::Saving)
                        .filter_map(|job| job.ended.map(|at| at + runtime.limits.terminal_ttl))
                        .min();
                    if deadline.is_none() {
                        runtime.cleanup.store(false, Ordering::SeqCst);
                    }
                    deadline
                };
                let Some(deadline) = deadline else {
                    break;
                };
                drop(runtime);
                tokio::time::sleep_until(deadline).await;
            }
        });
    }
}

pub(crate) fn configuration_changed(path: &Path, revision: u64) {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let runtimes: Vec<_> = RUNTIMES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap()
        .iter()
        .filter_map(Weak::upgrade)
        .filter(|r| r.path == path)
        .collect();
    for runtime in runtimes {
        let events: Vec<_> = {
            let mut state = runtime.state.lock().unwrap();
            state
                .jobs
                .values_mut()
                .filter(|job| {
                    job.revision < revision
                        && (job.snapshot.phase.active()
                            || job.snapshot.save == SaveState::Saving
                            || job.work.is_some())
                })
                .map(Job::invalidate)
                .collect()
        };
        for event in events {
            runtime.sink.emit(event);
        }
        runtime.prune();
        runtime.schedule_cleanup();
    }
}

#[cfg(test)]
mod tests;
