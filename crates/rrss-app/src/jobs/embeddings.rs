use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{BuildHasherDefault, DefaultHasher};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::scheduler::model_capacity::{Priority, RequestAdmission};
use anyhow::Result;
use rrss_ai::{EmbedBatchRequest, embed::embed_batch_admitted_until, retry_smaller};
use rrss_core::ai_store;
use rrss_core::rag;
use rusqlite::Connection;

use super::retry::{Kind, Update};
use super::{EMBED_BATCH, is_skip, spawn_db};

/// 一次嵌入请求的起始条数。常见兼容服务的条数上限：百炼 text-embedding-v4 为 10，
/// 硅基流动 bge-m3 为 32，智谱 embedding-3 为 64。初值取各家最小上限，成功后倍增到 64。
/// 只把本队列第一次拒绝记入进程记忆，且仅当该组大于 10；记下减半后的大小，下限也是 10。
/// 被拒也可能是单条输入的问题（超长等），继续拆小只影响当前队列。
/// 因此每次启动对云服务最多多一次被拒请求。
pub(crate) const EMBED_INPUTS_PER_REQUEST: usize = 10;
pub(crate) const EMBED_INPUTS_MAX: usize = 64;
const EMBED_RUN_BUDGET: Duration = Duration::from_secs(20);

#[derive(Clone, Copy)]
struct CapState {
    cap: usize,
    grow: bool,
}

/// 按 (base_url, model) 记住本进程学到的批大小。默认 `RandomState` 不能常量构造，
/// 所以用 `BuildHasherDefault` 才能放进静态 `Mutex`。锁只在这张表的读写期间持有。
type CapMap = HashMap<(String, String), CapState, BuildHasherDefault<DefaultHasher>>;
static EMBED_CAPS: Mutex<CapMap> = Mutex::new(HashMap::with_hasher(BuildHasherDefault::new()));

fn with_caps<T>(f: impl FnOnce(&mut CapMap) -> T) -> T {
    let mut caps = EMBED_CAPS.lock().unwrap_or_else(|err| err.into_inner());
    f(&mut caps)
}

fn remembered_cap(endpoint: &ai_store::EmbedEndpoint) -> CapState {
    let key = (endpoint.base_url.clone(), endpoint.model.clone());
    with_caps(|caps| {
        caps.get(&key).copied().unwrap_or(CapState {
            cap: EMBED_INPUTS_PER_REQUEST,
            grow: true,
        })
    })
}

fn note_cap(endpoint: &ai_store::EmbedEndpoint, cap: usize, grow: bool) {
    let key = (endpoint.base_url.clone(), endpoint.model.clone());
    with_caps(|caps| {
        caps.insert(key, CapState { cap, grow });
    });
}

/// 单条输入的结论：向量，或写进 `last_error` / 日志的失败原因。
pub(crate) type EmbedResult = std::result::Result<Vec<f32>, String>;

/// Only eligible input (including locally cooling input) schedules another
/// finite batch. No model calls or vector materialization are needed here.
pub(crate) async fn has_pending(db_path: &Path) -> Result<bool> {
    let capacity = crate::scheduler::model_capacity::Capacity::for_database(db_path);
    spawn_db(db_path.to_owned(), move |conn| {
        ai_store::resolve_embed(conn)?
            .ok_or_else(|| rrss_core::Error::TaskModelUnset(ai_store::TASK_EMBED.into()))?;
        let pending = pending_inputs(conn, &capacity)?;
        if pending {
            ai_store::ensure_auto_job_budget(conn, ai_store::BudgetKind::Embedding)?;
        }
        Ok(pending)
    })
    .await
}

pub(crate) fn pending_inputs(
    conn: &Connection,
    capacity: &crate::scheduler::model_capacity::Capacity,
) -> rrss_core::Result<bool> {
    let retries = capacity.retries.lock().unwrap().clone();
    if let Some(space) = rrss_core::vector_space::building(conn)? {
        let inputs =
            rrss_core::vector_space::pending_rebuild_batch(conn, space.id, 1, |kind, id, text| {
                retries.retryable(Kind::rebuild(kind), id, &ai_store::hash_input(text))
            })?;
        return Ok(!inputs.is_empty());
    }
    if rag::has_due_embed_articles(conn)? {
        return Ok(true);
    }
    for (kind, retry_kind) in [
        (rag::TextEmbedding::Interest, Kind::Interest),
        (rag::TextEmbedding::Memory, Kind::Memory),
    ] {
        if !kind
            .pending_batch(conn, 1, |id, text| {
                retries.retryable(retry_kind, id, &ai_store::hash_input(text))
            })?
            .is_empty()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub async fn run_embed_queue(db_path: &Path, client: &reqwest::Client) -> Result<()> {
    use crate::scheduler::{
        digest_jobs::Failure,
        model_capacity::{Capacity, EmbeddingRun},
    };
    let capacity = Capacity::for_database(db_path);
    let epoch = *capacity.stopped.borrow();
    let mut stop = capacity.stopped.subscribe();
    loop {
        let key = spawn_db(db_path.to_owned(), |conn| {
            let endpoint = ai_store::resolve_embed(conn)?;
            let space =
                rrss_core::vector_space::building(conn)?.or(rrss_core::vector_space::active(conn)?);
            Ok(ai_store::hash_input(
                &serde_json::json!([
                    endpoint.map(|ep| (ep.base_url, ep.model, ep.api_key)),
                    space.map(|s| (s.id, s.state)),
                    rrss_core::vector_space::dims_setting(conn)?
                ])
                .to_string(),
            ))
        })
        .await?;
        let (mut result, owns, same) = {
            let mut running = capacity.embedding.lock().unwrap();
            if let Some(running) = &*running
                && running.epoch == epoch
            {
                (running.result.subscribe(), false, running.key == key)
            } else {
                let (send, receive) = tokio::sync::watch::channel(None);
                *running = Some(EmbeddingRun {
                    epoch,
                    key: key.clone(),
                    result: send,
                });
                (receive, true, true)
            }
        };
        if !owns {
            loop {
                if let Some(result) = result.borrow_and_update().clone() {
                    if same {
                        return result.map_err(anyhow::Error::new);
                    }
                    break;
                }
                tokio::select! {
                    biased;
                    _ = stop.wait_for(|current| *current != epoch) => anyhow::bail!("嵌入任务已停止"),
                    changed = result.changed() => { changed.map_err(|_| anyhow::anyhow!("嵌入任务已停止"))?; }
                }
            }
            // A changed space/configuration needs its own finite attempt after
            // the previous owner, never the previous owner's result.
            continue;
        }
        struct Owner {
            capacity: std::sync::Arc<Capacity>,
            epoch: u64,
            finished: bool,
        }
        impl Drop for Owner {
            fn drop(&mut self) {
                if self.finished {
                    return;
                }
                let mut running = self.capacity.embedding.lock().unwrap();
                if running.as_ref().is_some_and(|run| run.epoch == self.epoch) {
                    let run = running.take().unwrap();
                    if run.result.borrow().is_none() {
                        run.result.send_replace(Some(Err(Failure {
                            reason: "stopped",
                            message: "嵌入任务已停止".into(),
                        })));
                    }
                }
            }
        }
        let mut owner = Owner {
            capacity: capacity.clone(),
            epoch,
            finished: false,
        };
        let result = tokio::select! {
            biased;
            _ = stop.wait_for(|current| *current != epoch) => Err(anyhow::anyhow!("嵌入任务已停止")),
            result = run_embed_queue_inner(db_path, client) => result,
        };
        // Publish and remove while holding the same mutex; a different-key waiter
        // must not spin on an already completed owner or lose the replacement.
        let mut running = capacity.embedding.lock().unwrap();
        if running.as_ref().is_some_and(|run| run.epoch == epoch) {
            let run = running.take().unwrap();
            let shared = result.as_ref().map(|_| ()).map_err(|error| Failure {
                reason: crate::scheduler::background::pause_reason(error),
                message: error.to_string(),
            });
            run.result.send_replace(Some(shared));
        }
        owner.finished = true;
        drop(running);
        // The entry was already removed; disarm Drop before a new same-epoch owner.
        std::mem::drop(owner);
        return result;
    }
}

async fn run_embed_queue_inner(db_path: &Path, client: &reqwest::Client) -> Result<()> {
    let path = db_path.to_path_buf();
    let Some(endpoint) = spawn_db(path.clone(), |conn| {
        ai_store::ensure_auto_job_budget(conn, ai_store::BudgetKind::Embedding)?;
        ai_store::resolve_embed(conn)
    })
    .await?
    else {
        return Ok(());
    };
    super::vector_space::check(db_path, client, false).await?;
    let writable = spawn_db(path.clone(), |conn| {
        Ok(rrss_core::vector_space::active(conn)?.is_none()
            || rrss_core::vector_space::write_space(conn)?.is_some())
    })
    .await?;
    if !writable {
        return Err(crate::scheduler::background::SpacePaused.into());
    }
    if spawn_db(path.clone(), rrss_core::vector_space::building)
        .await?
        .is_some()
    {
        let result = super::vector_space::rebuild(db_path, client).await;
        crate::scheduler::model_capacity::Capacity::for_database(db_path)
            .derived
            .send_modify(|version| *version += 1);
        return result;
    }
    let snapshot = endpoint.clone();
    if !spawn_db(path.clone(), move |conn| {
        rrss_core::vector_space::binding_matches(conn, &snapshot)
    })
    .await?
    {
        return Err(crate::scheduler::background::SpacePaused.into());
    }
    let capacity = crate::scheduler::model_capacity::Capacity::for_database(db_path);
    let context = ai_store::hash_input(
        &serde_json::json!([endpoint.base_url, endpoint.model, endpoint.api_key]).to_string(),
    );
    {
        let mut retries = capacity.retries.lock().unwrap();
        for kind in [Kind::Article, Kind::Interest, Kind::Memory] {
            retries.context(kind, &context);
        }
    }
    let mut summaries = capacity.summaries.subscribe();
    let mut elapsed = Duration::ZERO;
    let mut seen = HashSet::new();
    loop {
        // 小轮之间检查；正在进行的最后一轮允许略超预算。
        if elapsed > EMBED_RUN_BUDGET {
            break;
        }
        let waiting = summaries.borrow_and_update().clone();
        let Some(waiting) = waiting else {
            if summaries.changed().await.is_err() {
                break;
            }
            continue;
        };
        let started = Instant::now();
        let mut excluded = capacity
            .retries
            .lock()
            .unwrap()
            .excluded(Kind::Article)
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        excluded.extend(waiting.iter().copied());
        excluded.extend(seen.iter().copied());
        let articles = spawn_db(path.clone(), move |conn| {
            rag::due_embed_articles_excluding(conn, EMBED_BATCH, &excluded)
        })
        .await?;
        let mut fresh = Vec::new();
        for article in articles {
            if fresh.len() < EMBED_BATCH as usize
                && !waiting.contains(&article.id)
                && seen.insert(article.id)
            {
                fresh.push(article);
            }
        }
        fresh.truncate(EMBED_BATCH as usize);
        if fresh.is_empty() {
            elapsed += started.elapsed();
            if !waiting.is_empty() {
                if summaries.changed().await.is_err() {
                    break;
                }
                continue;
            }
            break;
        }
        match embed_articles(db_path, client, &endpoint, fresh).await {
            Ok(0) => break,
            Ok(_) => {}
            Err(err) => {
                if is_skip(&err) {
                    tracing::debug!(error = %crate::log::error(err.as_ref()), "skip remaining auto embeds");
                } else {
                    tracing::warn!(error = %crate::log::error(err.as_ref()), "embed articles failed");
                }
                return Err(err);
            }
        }
        elapsed += started.elapsed();
    }
    embed_pending_interests(db_path, client).await?;
    embed_pending_text(db_path, client, rag::TextEmbedding::Memory).await
}

pub async fn embed_pending_interests(db_path: &Path, client: &reqwest::Client) -> Result<()> {
    embed_pending_text(db_path, client, rag::TextEmbedding::Interest).await?;
    // Vector writes wake the local lane; embedding never starts an independent
    // full interest recomputation outside that lane.
    Ok(())
}

async fn embed_pending_text(
    db_path: &Path,
    client: &reqwest::Client,
    kind: rag::TextEmbedding,
) -> Result<()> {
    let path = db_path.to_path_buf();
    let capacity = crate::scheduler::model_capacity::Capacity::for_database(db_path);
    let retry_kind = match kind {
        rag::TextEmbedding::Interest => Kind::Interest,
        rag::TextEmbedding::Memory => Kind::Memory,
    };
    let Some((endpoint, pending)) = spawn_db(path, move |conn| {
        let Some(endpoint) = ai_store::resolve_embed(conn)? else {
            return Ok(None);
        };
        let retries = capacity.retries.lock().unwrap().clone();
        let pending = kind.pending_batch(conn, EMBED_BATCH as usize, |id, input| {
            retries.allowed(retry_kind, id, &ai_store::hash_input(input))
        })?;
        Ok(Some((endpoint, pending)))
    })
    .await?
    else {
        return Ok(());
    };
    let expected: std::sync::Arc<HashMap<_, _>> =
        std::sync::Arc::new(pending.iter().cloned().collect());
    let (ids, texts) = pending.into_iter().unzip();
    let saved = embed_and_save(db_path, client, &endpoint, ids, texts, move |conn, done| {
        let mut updates = Vec::new();
        let current: HashMap<_, _> = kind
            .pending_batch(conn, expected.len(), |id, _| expected.contains_key(&id))?
            .into_iter()
            .collect();
        for (id, result) in done {
            if current.get(id) != expected.get(id) {
                continue;
            }
            match result {
                Ok(vec) => {
                    kind.set(conn, *id, vec)?;
                    updates.push(Update::Success {
                        kind: retry_kind,
                        id: *id,
                    });
                }
                Err(_err) => {
                    updates.push(Update::Failed {
                        kind: retry_kind,
                        id: *id,
                        input: ai_store::hash_input(&expected[id]),
                        terminal: true,
                    });
                    tracing::warn!(?kind, id, error_kind = "embedding", "embed text failed");
                }
            }
        }
        Ok(updates)
    })
    .await;
    saved.map(|_| ())
}

/// 先按当前正文同步这几篇的分块，再把各篇缺向量的分块合起来批量请求。
async fn embed_articles(
    db_path: &Path,
    client: &reqwest::Client,
    endpoint: &ai_store::EmbedEndpoint,
    articles: Vec<rag::DueEmbedArticle>,
) -> Result<usize> {
    let path = db_path.to_path_buf();
    let expected: std::sync::Arc<HashMap<_, _>> = std::sync::Arc::new(
        articles
            .iter()
            .cloned()
            .map(|article| (article.id, article))
            .collect(),
    );
    let pending = spawn_db(path, move |conn| {
        let mut pending = Vec::new();
        for article in &articles {
            let current = rrss_core::articles::get_article(conn, article.id)?;
            if current.title != article.title || current.summary != article.summary || current.blocks_json != article.blocks_json || current.content_html != article.content_html { continue; }
            match rag::sync_article_chunks(conn, article, article.tier) {
                Ok(chunks) => pending.extend(chunks),
                Err(err) => tracing::error!(article_id = article.id, error = %err, "sync article chunks failed"),
            }
        }
        Ok(pending)
    })
    .await?;
    let inputs: std::sync::Arc<HashMap<_, _>> =
        std::sync::Arc::new(pending.iter().map(|c| (c.id, c.text.clone())).collect());
    let (ids, texts) = pending.into_iter().map(|c| (c.id, c.text)).unzip();
    let model = endpoint.model.clone();
    embed_and_save(db_path, client, endpoint, ids, texts, move |conn, done| {
        let mut updates = Vec::new();
        for (id, result) in done {
            use rusqlite::OptionalExtension;
            let chunk: Option<(i64, String)> = conn
                .query_row(
                    "SELECT article_id,text FROM chunks WHERE id=?1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let Some((article_id, text)) = chunk else {
                continue;
            };
            let Some(before) = expected.get(&article_id) else {
                continue;
            };
            let current = rrss_core::articles::get_article(conn, article_id)?;
            if inputs.get(id) != Some(&text)
                || current.title != before.title
                || current.summary != before.summary
                || current.blocks_json != before.blocks_json
                || current.content_html != before.content_html
            {
                continue;
            }
            match result {
                Ok(vec) => rag::set_chunk_embedding(conn, *id, &model, vec)?,
                Err(err) => {
                    tracing::warn!(
                        chunk_id = id,
                        error_kind = "embedding",
                        "embed chunk failed"
                    );
                    rag::record_chunk_error(conn, *id, err)?;
                    updates.push(Update::Failed {
                        kind: Kind::Article,
                        id: article_id,
                        input: String::new(),
                        terminal: false,
                    });
                }
            }
        }
        Ok(updates)
    })
    .await
}

/// 批量嵌入 `texts`，每次请求的结论交给 `save`，在一个写事务里落库（`ids[i]` 对应 `texts[i]`）。
/// 返回成功落库的向量条数。`Err` 表示本轮应停下（预算用尽、数据库失败）；已落库的结果保留。
pub(super) async fn embed_and_save<F>(
    db_path: &Path,
    client: &reqwest::Client,
    endpoint: &ai_store::EmbedEndpoint,
    ids: Vec<i64>,
    texts: Vec<String>,
    save: F,
) -> Result<usize>
where
    F: Fn(&Connection, &[(i64, EmbedResult)]) -> rrss_core::Result<Vec<Update>>
        + Clone
        + Send
        + 'static,
{
    if !texts.is_empty() {
        super::vector_space::check(db_path, client, false).await?;
    }
    let snapshot = endpoint.clone();
    if !spawn_db(db_path.to_path_buf(), move |conn| {
        rrss_core::vector_space::binding_matches(conn, &snapshot)
    })
    .await?
    {
        return Ok(0);
    }
    let space_id = spawn_db(db_path.to_path_buf(), |conn| {
        Ok(rrss_core::vector_space::write_space(conn)?.map(|s| s.id))
    })
    .await?;
    let mut queue = EmbedQueue::new(db_path, client, endpoint, texts);
    let mut saved = 0;
    loop {
        let snapshot = endpoint.clone();
        let current = spawn_db(db_path.to_path_buf(), move |conn| {
            Ok(
                rrss_core::vector_space::write_space(conn)?.map(|space| space.id) == space_id
                    && rrss_core::vector_space::binding_matches(conn, &snapshot)?,
            )
        })
        .await?;
        if !current {
            break;
        }
        let Some(step) = queue.next().await else {
            break;
        };
        let mut done: Vec<(i64, EmbedResult)> = step?
            .into_iter()
            .map(|(i, result)| (ids[i], result))
            .collect();
        let ok = done.iter().filter(|(_, result)| result.is_ok()).count();
        let save = save.clone();
        let snapshot = endpoint.clone();
        let wrote = spawn_db(db_path.to_path_buf(), move |conn| {
            let tx = rrss_core::db::write_transaction(conn)?;
            if rrss_core::vector_space::write_space(&tx)?.map(|s| s.id) != space_id {
                return Ok(None);
            }
            for (_, result) in &mut done {
                if let Ok(vector) = result {
                    let Some(space) = rrss_core::vector_space::ensure_for_embedding(
                        &tx,
                        &snapshot,
                        vector.len(),
                    )?
                    else {
                        return Ok(None);
                    };
                    *vector =
                        rrss_core::vector_space::fit(std::mem::take(vector), space.dims as usize);
                }
            }
            let updates = save(&tx, &done)?;
            tx.commit()?;
            Ok(Some(updates))
        })
        .await?;
        let Some(updates) = wrote else {
            break;
        };
        crate::scheduler::model_capacity::Capacity::for_database(db_path)
            .retries
            .lock()
            .unwrap()
            .apply(updates);
        saved += ok;
        if ok > 0 {
            crate::scheduler::model_capacity::Capacity::for_database(db_path)
                .derived
                .send_modify(|version| *version += 1);
        }
    }
    Ok(saved)
}

/// 一组待嵌入文本的请求队列。服务以条数 / 体积拒绝一批时，对半拆开重试，
/// 并把这组剩下的请求也降到同样大小；只有单条仍被拒才记为该条失败。
pub(crate) struct EmbedQueue<'a> {
    db_path: &'a Path,
    client: &'a reqwest::Client,
    endpoint: &'a ai_store::EmbedEndpoint,
    texts: Vec<String>,
    blank: Vec<usize>,
    todo: VecDeque<(Vec<usize>, Option<tokio::time::Instant>)>,
    cap: usize,
    grow: bool,
    /// 本队列是否已因条数被拒过。进程记忆只记第一次。
    rejected: bool,
    started: Instant,
    succeeded: usize,
    failed: usize,
    requests: usize,
    outcome: &'static str,
    error: Option<String>,
}

impl Drop for EmbedQueue<'_> {
    fn drop(&mut self) {
        if self.texts.is_empty() {
            return;
        }
        tracing::info!(model = %self.endpoint.model, count = self.texts.len(),
            succeeded = self.succeeded, failed = self.failed,
            pending = self.texts.len().saturating_sub(self.succeeded + self.failed),
            requests = self.requests, outcome = self.outcome, error = ?self.error,
            elapsed_ms = self.started.elapsed().as_millis() as u64, "embedding batch finished");
    }
}

enum Failure {
    /// 预算用尽或数据库失败：停下本轮。
    Stop(anyhow::Error),
    /// 服务返回的错误：可能拆小重试，否则记到这批输入上。
    Api(rrss_ai::Error),
}

impl<'a> EmbedQueue<'a> {
    pub(crate) fn new(
        db_path: &'a Path,
        client: &'a reqwest::Client,
        endpoint: &'a ai_store::EmbedEndpoint,
        texts: Vec<String>,
    ) -> Self {
        let (blank, filled): (Vec<usize>, Vec<usize>) =
            (0..texts.len()).partition(|&i| texts[i].trim().is_empty());
        let CapState { cap, grow } = remembered_cap(endpoint);
        Self {
            db_path,
            client,
            endpoint,
            texts,
            blank,
            todo: VecDeque::from([(filled, None)]),
            cap,
            grow,
            rejected: false,
            started: Instant::now(),
            succeeded: 0,
            failed: 0,
            requests: 0,
            outcome: "cancelled",
            error: None,
        }
    }

    /// 发出下一次请求，返回这次有结论的输入下标与结果；没有剩余时返回 `None`。
    /// `Err` 表示应停下，之前返回的结果仍有效。
    pub(crate) async fn next(&mut self) -> Option<Result<Vec<(usize, EmbedResult)>>> {
        let result = self.next_inner().await;
        match &result {
            None => self.outcome = "completed",
            Some(Err(error)) => {
                self.outcome = "stopped";
                self.error = Some(crate::log::error(error.as_ref()));
            }
            Some(Ok(done)) => {
                self.succeeded += done.iter().filter(|(_, result)| result.is_ok()).count();
                self.failed += done.iter().filter(|(_, result)| result.is_err()).count();
                if self.succeeded + self.failed == self.texts.len() {
                    self.outcome = "completed";
                }
            }
        }
        result
    }

    async fn next_inner(&mut self) -> Option<Result<Vec<(usize, EmbedResult)>>> {
        if !self.blank.is_empty() {
            let blank = std::mem::take(&mut self.blank);
            return Some(Ok(blank
                .into_iter()
                .map(|i| (i, Err("empty embedding input".to_string())))
                .collect()));
        }
        loop {
            let (mut group, deadline) = self.todo.pop_front()?;
            if group.is_empty() {
                continue;
            }
            if group.len() > self.cap {
                let rest = group.split_off(self.cap);
                self.todo.push_front((rest, deadline));
            }
            let deadline = deadline
                .unwrap_or_else(|| tokio::time::Instant::now() + rrss_ai::deadline::ITEM_TIMEOUT);
            match self.request(&group, deadline).await {
                Ok(vectors) => {
                    if group.len() == self.cap && self.grow {
                        self.cap = self.cap.saturating_mul(2).min(EMBED_INPUTS_MAX);
                        note_cap(self.endpoint, self.cap, true);
                    }
                    return Some(Ok(group
                        .into_iter()
                        .zip(vectors.into_iter().map(Ok))
                        .collect()));
                }
                Err(Failure::Stop(err)) => return Some(Err(err)),
                Err(Failure::Api(err)) if group.len() > 1 && retry_smaller(&err) => {
                    let count = group.len();
                    let first = !self.rejected;
                    self.rejected = true;
                    self.cap = (count / 2).max(1);
                    self.grow = false;
                    if first && count > EMBED_INPUTS_PER_REQUEST {
                        note_cap(
                            self.endpoint,
                            (count / 2).max(EMBED_INPUTS_PER_REQUEST),
                            false,
                        );
                    }
                    tracing::debug!(
                        model = %self.endpoint.model,
                        rejected = group.len(),
                        next = self.cap,
                        error = %err.log_message(),
                        "embedding batch rejected; retrying smaller"
                    );
                    self.todo.push_front((group, Some(deadline)));
                }
                Err(Failure::Api(err)) if !retry_smaller(&err) => return Some(Err(err.into())),
                Err(Failure::Api(err)) => {
                    let msg = err.to_string();
                    return Some(Ok(group
                        .into_iter()
                        .map(|i| (i, Err(msg.clone())))
                        .collect()));
                }
            }
        }
    }

    async fn request(
        &mut self,
        group: &[usize],
        deadline: tokio::time::Instant,
    ) -> std::result::Result<Vec<Vec<f32>>, Failure> {
        let path = self.db_path.to_path_buf();
        let (space, binding) = spawn_db(path.clone(), |conn| {
            ai_store::ensure_auto_job_budget(conn, ai_store::BudgetKind::Embedding)?;
            Ok((
                rrss_core::vector_space::building(conn)?
                    .or(rrss_core::vector_space::active(conn)?)
                    .map(|space| space.id),
                ai_store::resolve_embed(conn)?,
            ))
        })
        .await
        .map_err(Failure::Stop)?;
        let mut admission = RequestAdmission::new(
            self.db_path,
            &self.endpoint.base_url,
            Priority::Background,
            true,
        )
        .map_err(|error| Failure::Stop(error.into()))?;
        if binding.as_ref().is_some_and(|bound| {
            bound.base_url != self.endpoint.base_url
                || bound.model != self.endpoint.model
                || bound.api_key != self.endpoint.api_key
        }) {
            return Err(Failure::Stop(anyhow::anyhow!("嵌入绑定已改变")));
        }
        admission.budget_kind = ai_store::BudgetKind::Embedding;
        admission.space = Some((path.clone(), space, binding));
        self.requests += 1;
        let out = embed_batch_admitted_until(
            self.client,
            &EmbedBatchRequest {
                base_url: self.endpoint.base_url.clone(),
                api_key: self.endpoint.api_key.clone(),
                model: self.endpoint.model.clone(),
                inputs: group.iter().map(|&i| self.texts[i].clone()).collect(),
            },
            deadline,
            Some(&admission),
        )
        .await
        .map_err(|err| {
            if matches!(err, rrss_ai::Error::Admission(_) | rrss_ai::Error::BudgetExhausted | rrss_ai::Error::Capacity) {
                tracing::debug!(model = %self.endpoint.model, error = %err.log_message(), "text embedding paused");
                Failure::Stop(err.into())
            } else {
                tracing::warn!(model = %self.endpoint.model, count = group.len(), error = %err.log_message(), "text embedding failed");
                Failure::Api(err)
            }
        })?;
        let prompt = i64::from(out.prompt_tokens);
        let day = ai_store::local_day();
        spawn_db(path, move |conn| {
            ai_store::add_embed_usage(conn, &day, prompt)
        })
        .await
        .map_err(Failure::Stop)?;
        tracing::debug!(model = %self.endpoint.model, count = group.len(), dim = out.vectors[0].len(), prompt_tokens = prompt, "text embedding succeeded");
        Ok(out.vectors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::db;
    use serde_json::{Value, json};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    #[tokio::test]
    async fn cooled_text_remains_pending_but_deleted_or_exhausted_input_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pending.db");
        let capacity = crate::scheduler::model_capacity::Capacity::for_database(&path);
        let conn = db::open_at(&path).unwrap();
        conn.execute(
            "INSERT INTO interests(id,label,created_at) VALUES(1,'bad text',0)",
            [],
        )
        .unwrap();
        let hash = ai_store::hash_input("bad text");
        capacity
            .retries
            .lock()
            .unwrap()
            .failed(Kind::Interest, 1, hash.clone(), true);
        assert!(pending_inputs(&conn, &capacity).unwrap());
        for _ in 0..2 {
            capacity
                .retries
                .lock()
                .unwrap()
                .failed(Kind::Interest, 1, hash.clone(), true);
        }
        assert!(!pending_inputs(&conn, &capacity).unwrap());
        conn.execute("UPDATE interests SET label='fixed text' WHERE id=1", [])
            .unwrap();
        assert!(pending_inputs(&conn, &capacity).unwrap());
        conn.execute("DELETE FROM interests WHERE id=1", [])
            .unwrap();
        assert!(!pending_inputs(&conn, &capacity).unwrap());
    }

    /// 模拟兼容服务：向量是输入的字符数；数组超过 `max_items` 条、或含 `bad` 时返回 400。
    /// `max_items` 为 0 表示只认字符串输入。
    async fn echo_server(max_items: usize) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::method("POST"))
            .respond_with(move |req: &Request| {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                let inputs: Vec<String> = match &body["input"] {
                    Value::String(s) => vec![s.clone()],
                    Value::Array(items) if items.len() <= max_items => items
                        .iter()
                        .map(|v| v.as_str().unwrap().to_string())
                        .collect(),
                    _ => {
                        return ResponseTemplate::new(400)
                            .set_body_json(json!({"error": {"message": "batch too large"}}));
                    }
                };
                if inputs.iter().any(|s| s == "bad") {
                    return ResponseTemplate::new(400)
                        .set_body_json(json!({"error": {"message": "bad input"}}));
                }
                let data: Vec<Value> = inputs
                    .iter()
                    .enumerate()
                    .map(|(i, s)| json!({"index": i, "embedding": [s.chars().count() as f32]}))
                    .collect();
                ResponseTemplate::new(200)
                    .set_body_json(json!({"data": data, "usage": {"prompt_tokens": inputs.len()}}))
            })
            .mount(&server)
            .await;
        server
    }

    fn endpoint(server: &MockServer, model: &str) -> ai_store::EmbedEndpoint {
        ai_store::EmbedEndpoint {
            base_url: server.uri(),
            api_key: None,
            model: model.into(),
        }
    }

    /// 每次请求的输入条数（字符串输入记 1）。
    async fn request_sizes(server: &MockServer) -> Vec<usize> {
        server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| {
                let body: Value = serde_json::from_slice(&r.body).unwrap();
                body["input"].as_array().map_or(1, Vec::len)
            })
            .collect()
    }

    async fn drain(queue: &mut EmbedQueue<'_>, n: usize) -> Vec<Option<EmbedResult>> {
        let mut out = vec![None; n];
        while let Some(step) = queue.next().await {
            for (i, result) in step.unwrap() {
                assert!(out[i].is_none(), "input {i} answered twice");
                out[i] = Some(result);
            }
        }
        out
    }

    fn texts(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn batch_summary_counts_final_results_after_splitting_and_budget_stop() {
        use tracing::instrument::WithSubscriber;
        let server = echo_server(2).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let ep = endpoint(&server, "summary");
        let client = reqwest::Client::new();
        let (subscriber, handle) = crate::log::subscriber(
            crate::log::Mode::Desktop {
                log_dir: dir.path().join("logs"),
            },
            "",
        );
        async {
            let mut queue = EmbedQueue::new(
                &path,
                &client,
                &ep,
                texts(&["canary-source", "bad", "", "second"]),
            );
            let result = drain(&mut queue, 4).await;
            assert_eq!(
                result
                    .iter()
                    .filter(|result| matches!(result, Some(Ok(_))))
                    .count(),
                2
            );
            drop(queue);
            let conn = db::open_at(&path).unwrap();
            db::set_setting(&conn, ai_store::EMBED_BUDGET.id, "1").unwrap();
            ai_store::add_embed_usage(&conn, &ai_store::local_day(), 1).unwrap();
            let mut queue = EmbedQueue::new(&path, &client, &ep, texts(&["canary-pending"]));
            assert!(queue.next().await.unwrap().is_err());
        }
        .with_subscriber(subscriber)
        .await;
        handle.end();
        let log = std::fs::read_to_string(dir.path().join("logs/rrss.log")).unwrap();
        let summaries = log
            .lines()
            .filter(|line| line.contains("embedding batch finished"))
            .collect::<Vec<_>>();
        assert_eq!(summaries.len(), 2, "{log}");
        assert!(
            summaries[0].contains("succeeded=2 failed=2 pending=0")
                && summaries[0].contains("outcome=completed"),
            "{log}"
        );
        assert!(
            summaries[1].contains("succeeded=0 failed=0 pending=1")
                && summaries[1].contains("outcome=stopped"),
            "{log}"
        );
        assert!(!log.contains("canary-"), "{log}");
    }

    #[tokio::test]
    async fn split_retries_share_the_original_batch_deadline_and_keep_successes() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::method("POST")).respond_with(|request: &Request| {
            let payload: Value = serde_json::from_slice(&request.body).unwrap();
            let inputs = payload["input"].as_array().unwrap();
            if inputs.len() > 2 {
                ResponseTemplate::new(413)
            } else {
                ResponseTemplate::new(200)
                    .set_body_json(json!({"data": [{"index":0,"embedding":[1.0]}, {"index":1,"embedding":[2.0]}], "usage":{"prompt_tokens":2}}))
            }
        }).mount(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let client = reqwest::Client::new();
        let endpoint = endpoint(&server, "deadline");
        let mut queue = EmbedQueue::new(
            &path,
            &client,
            &endpoint,
            vec!["a".into(), "b".into(), "c".into(), "d".into()],
        );
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        queue.todo.front_mut().unwrap().1 = Some(deadline);
        let first = queue.next().await.unwrap().unwrap();
        assert!(first.iter().all(|(_, result)| result.is_ok()));
        assert_eq!(queue.todo.front().unwrap().1, Some(deadline));
        // Simulate the original batch expiring while its first successful part is saved.
        queue.todo.front_mut().unwrap().1 = Some(tokio::time::Instant::now());
        let error = queue.next().await.unwrap().unwrap_err();
        assert!(matches!(
            error.downcast_ref::<rrss_ai::Error>(),
            Some(rrss_ai::Error::Request { timeout: true, .. })
        ));
        assert!(queue.next().await.is_none());
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
        let usage =
            ai_store::usage_on(&db::open_at(&path).unwrap(), &ai_store::local_day()).unwrap();
        assert_eq!(usage.embed_tokens, 2);
    }

    #[tokio::test]
    async fn queue_sends_up_to_the_cap_per_request() {
        let server = echo_server(64).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let conn = db::open_at(&path).unwrap();
        ai_store::set_daily_budget_tokens(&conn, 1).unwrap();
        ai_store::add_usage(&conn, &ai_store::local_day(), 1, 0).unwrap();
        let ep = endpoint(&server, "cap-10");
        let client = reqwest::Client::new();
        let inputs: Vec<String> = (1..=12).map(|n| "x".repeat(n)).collect();
        let mut queue = EmbedQueue::new(&path, &client, &ep, inputs);
        let out = drain(&mut queue, 12).await;
        for (i, result) in out.into_iter().enumerate() {
            assert_eq!(result, Some(Ok(vec![(i + 1) as f32])));
        }
        assert_eq!(request_sizes(&server).await, [10, 2]);
        let usage = ai_store::usage_on(&db::open_at(&path).unwrap(), &ai_store::local_day())
            .unwrap()
            .embed_tokens;
        assert_eq!(usage, 12, "usage is recorded once per request");
        let generation = ai_store::usage_on(&conn, &ai_store::local_day()).unwrap();
        assert_eq!(
            (
                generation.tokens,
                generation.prompt_tokens,
                generation.completion_tokens
            ),
            (1, 1, 0)
        );
    }

    #[tokio::test]
    async fn rejected_batch_is_halved_and_remaining_requests_stay_small() {
        let server = echo_server(3).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let ep = endpoint(&server, "halve");
        let client = reqwest::Client::new();
        let inputs = texts(&["a", "bb", "ccc", "dddd", "e", "ff", "ggg", "hhhh"]);
        let mut queue = EmbedQueue::new(&path, &client, &ep, inputs.clone());
        let out = drain(&mut queue, inputs.len()).await;
        for (text, result) in inputs.iter().zip(out) {
            assert_eq!(result, Some(Ok(vec![text.len() as f32])));
        }
        assert_eq!(request_sizes(&server).await, [8, 4, 2, 2, 2, 2]);
    }

    #[tokio::test]
    async fn string_only_service_falls_back_to_single_inputs() {
        let server = echo_server(0).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let ep = endpoint(&server, "single");
        let client = reqwest::Client::new();
        let inputs = texts(&["a", "bb", "ccc"]);
        let mut queue = EmbedQueue::new(&path, &client, &ep, inputs);
        let out = drain(&mut queue, 3).await;
        assert_eq!(
            out,
            [
                Some(Ok(vec![1.0])),
                Some(Ok(vec![2.0])),
                Some(Ok(vec![3.0]))
            ]
        );
        assert_eq!(request_sizes(&server).await, [3, 1, 1, 1]);
    }

    #[tokio::test]
    async fn only_the_rejected_and_blank_inputs_fail() {
        let server = echo_server(64).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let ep = endpoint(&server, "blank");
        let client = reqwest::Client::new();
        let inputs = texts(&["a", "bb", "bad", " ", "ccc"]);
        let mut queue = EmbedQueue::new(&path, &client, &ep, inputs);
        let out = drain(&mut queue, 5).await;
        assert_eq!(out[0], Some(Ok(vec![1.0])));
        assert_eq!(out[1], Some(Ok(vec![2.0])));
        assert!(matches!(&out[2], Some(Err(msg)) if msg.contains("bad input")));
        assert!(matches!(&out[3], Some(Err(msg)) if msg.contains("empty")));
        assert_eq!(out[4], Some(Ok(vec![3.0])));
        assert!(
            request_sizes(&server).await.iter().all(|&n| n <= 4),
            "the blank input is never sent"
        );
    }

    #[tokio::test]
    async fn exhausted_budget_stops_before_any_request() {
        let server = echo_server(64).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        {
            let conn = db::open_at(&path).unwrap();
            db::set_setting(&conn, ai_store::EMBED_BUDGET.id, "5").unwrap();
            ai_store::add_embed_usage(&conn, &ai_store::local_day(), 5).unwrap();
        }
        let ep = endpoint(&server, "budget");
        let client = reqwest::Client::new();
        let mut queue = EmbedQueue::new(&path, &client, &ep, texts(&["a", "b"]));
        let err = queue.next().await.unwrap().unwrap_err();
        assert!(is_skip(&err), "{err}");
        assert!(request_sizes(&server).await.is_empty());
    }

    #[tokio::test]
    async fn unread_queue_batches_only_title_summary_chunks_across_articles() {
        let server = echo_server(64).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        {
            let conn = db::open_at(&path).unwrap();
            let pid = crate::test_net::fake_provider(&conn, "mock", server.uri());
            ai_store::set_task_binding(&conn, ai_store::TASK_EMBED, Some(pid), Some("m")).unwrap();
            conn.execute(
                "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://f.test/rss', 1)",
                [],
            )
            .unwrap();
            for id in 1..=3 {
                conn.execute(
                    "INSERT INTO articles (id, feed_id, title, hash, created_at, content_html, score)
                     VALUES (?1, 1, ?2, ?3, 1, '<p>正文一段。</p>', 1)",
                    rusqlite::params![id, format!("标题{id}"), format!("h{id}")],
                )
                .unwrap();
            }
        }
        run_embed_queue(&path, &reqwest::Client::new())
            .await
            .unwrap();
        let conn = db::open_at(&path).unwrap();
        let (total, embedded): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COUNT(embedding) FROM chunks WHERE model = 'm' OR model IS NULL",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(total, 3);
        assert_eq!(embedded, total);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM chunks WHERE ordinal = 0 AND kind = 'title_summary'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            3
        );
        let space = rrss_core::vector_space::active(&conn).unwrap().unwrap();
        assert_eq!((space.model.as_str(), space.dims), ("m", 1));
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM chunks WHERE space_id=?1",
                [space.id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            total
        );
        assert_eq!(
            request_sizes(&server).await,
            [8, total as usize],
            "all chunks of the due articles go in one request"
        );
        // 同一路径继续验证换绑定只停写，不切空间，也不请求兴趣 / 记忆向量。
        let pid = ai_store::get_task_binding(&conn, ai_store::TASK_EMBED)
            .unwrap()
            .provider_id
            .unwrap();
        assert!(matches!(
            ai_store::set_task_binding(&conn, ai_store::TASK_EMBED, Some(pid), Some("different")),
            Err(rrss_core::Error::VectorSpaceRebuildRequired { .. })
        ));
        rrss_core::interests::insert_interest(&conn, "new interest", "manual").unwrap();
        rrss_core::memories::insert_memory(
            &conn,
            rrss_core::memories::MemoryKind::Semantic,
            "new fact",
            "test",
            1.0,
        )
        .unwrap();
        assert_eq!(request_sizes(&server).await, [8, total as usize]);
        assert_eq!(
            rrss_core::vector_space::active(&conn).unwrap().unwrap().id,
            space.id
        );
        for table in ["interests", "memories"] {
            assert_eq!(
                conn.query_row(&format!("SELECT COUNT(embedding) FROM {table}"), [], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap(),
                0
            );
        }
    }

    #[tokio::test]
    async fn successful_batches_double_until_the_cap_and_the_next_queue_starts_there() {
        let server = echo_server(usize::MAX).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let ep = endpoint(&server, "grow");
        let client = reqwest::Client::new();
        let inputs: Vec<String> = (0..100).map(|i| format!("t{i}")).collect();
        let mut queue = EmbedQueue::new(&path, &client, &ep, inputs.clone());
        let out = drain(&mut queue, inputs.len()).await;
        assert!(out.iter().all(|r| matches!(r, Some(Ok(_)))));
        assert_eq!(request_sizes(&server).await, [10, 20, 40, 30]);

        let more: Vec<String> = (0..64).map(|i| format!("u{i}")).collect();
        let mut again = EmbedQueue::new(&path, &client, &ep, more.clone());
        let out = drain(&mut again, more.len()).await;
        assert!(out.iter().all(|r| matches!(r, Some(Ok(_)))));
        assert_eq!(request_sizes(&server).await, [10, 20, 40, 30, 64]);
    }

    #[tokio::test]
    async fn rejection_above_ten_freezes_the_cap_for_the_next_queue() {
        let server = echo_server(10).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let ep = endpoint(&server, "freeze");
        let client = reqwest::Client::new();
        let inputs: Vec<String> = (0..30).map(|i| format!("t{i}")).collect();
        let mut queue = EmbedQueue::new(&path, &client, &ep, inputs.clone());
        let out = drain(&mut queue, inputs.len()).await;
        assert!(out.iter().all(|r| matches!(r, Some(Ok(_)))));
        assert_eq!(request_sizes(&server).await, [10, 20, 10, 10]);

        let mut again = EmbedQueue::new(&path, &client, &ep, inputs);
        let out = drain(&mut again, 30).await;
        assert!(out.iter().all(|r| matches!(r, Some(Ok(_)))));
        assert_eq!(request_sizes(&server).await, [10, 20, 10, 10, 10, 10, 10]);
    }

    #[tokio::test]
    async fn bad_input_in_a_learned_batch_keeps_half_the_cap() {
        let server = echo_server(usize::MAX).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let ep = endpoint(&server, "bad-input");
        let client = reqwest::Client::new();
        let inputs: Vec<String> = (0..100).map(|i| format!("t{i}")).collect();
        let mut queue = EmbedQueue::new(&path, &client, &ep, inputs.clone());
        let out = drain(&mut queue, inputs.len()).await;
        assert!(out.iter().all(|r| matches!(r, Some(Ok(_)))));
        assert_eq!(request_sizes(&server).await, [10, 20, 40, 30]);

        let mut mixed: Vec<String> = (0..64).map(|i| format!("u{i}")).collect();
        mixed[0] = "bad".into();
        let mut again = EmbedQueue::new(&path, &client, &ep, mixed);
        let out = drain(&mut again, 64).await;
        assert!(matches!(&out[0], Some(Err(msg)) if msg.contains("bad input")));
        assert!(out[1..].iter().all(|r| matches!(r, Some(Ok(_)))));
        let learned = request_sizes(&server).await;
        assert_eq!(learned[4], 64);

        let third: Vec<String> = (0..64).map(|i| format!("v{i}")).collect();
        let mut third_q = EmbedQueue::new(&path, &client, &ep, third);
        let first = third_q.next().await.unwrap().unwrap();
        assert_eq!(first.len(), 32);
        assert!(first.iter().all(|(_, result)| result.is_ok()));
        let sizes = request_sizes(&server).await;
        assert_eq!(sizes[learned.len()], 32);
    }

    fn seed_articles(server: &MockServer, path: &std::path::Path, n: i64, model: &str) {
        let conn = db::open_at(path).unwrap();
        let pid = crate::test_net::fake_provider(&conn, "mock", server.uri());
        ai_store::set_task_binding(&conn, ai_store::TASK_EMBED, Some(pid), Some(model)).unwrap();
        conn.execute(
            "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://f.test/rss', 1)",
            [],
        )
        .unwrap();
        for id in 1..=n {
            conn.execute(
                "INSERT INTO articles (id, feed_id, title, hash, created_at, content_html, score)
                 VALUES (?1, 1, ?2, ?3, 1, '<p>正文一段。</p>', 1)",
                rusqlite::params![id, format!("标题{id}"), format!("h{id}")],
            )
            .unwrap();
        }
    }

    #[tokio::test]
    async fn changed_configuration_waiter_gets_a_fresh_attempt_and_budget_check() {
        let server = echo_server(64).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("configuration.db");
        seed_articles(&server, &path, 0, "configuration");
        let client = reqwest::Client::new();
        // Establish the space before the concurrent attempts so only the
        // provider configuration changes their identity.
        run_embed_queue(&path, &client).await.unwrap();
        let capacity = crate::scheduler::model_capacity::Capacity::for_database(&path);
        capacity.summaries.send_replace(None);
        let old = tokio::spawn({
            let path = path.clone();
            let client = client.clone();
            async move { run_embed_queue(&path, &client).await }
        });
        tokio::time::timeout(Duration::from_secs(10), async {
            while capacity.summaries.receiver_count() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let conn = db::open_at(&path).unwrap();
        conn.execute("UPDATE ai_providers SET api_key='fixture-new'", [])
            .unwrap();
        db::set_setting(&conn, ai_store::EMBED_BUDGET.id, "1").unwrap();
        ai_store::add_embed_usage(&conn, &ai_store::local_day(), 1).unwrap();
        let changed = tokio::spawn({
            let path = path.clone();
            async move { run_embed_queue(&path, &client).await }
        });
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if capacity
                    .embedding
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|run| run.result.receiver_count() == 1)
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!changed.is_finished());
        capacity.summaries.send_replace(Some(HashSet::new()));
        old.await.unwrap().unwrap();
        let error = changed.await.unwrap().unwrap_err();
        assert_eq!(crate::scheduler::background::pause_reason(&error), "budget");
    }

    #[tokio::test]
    async fn marking_read_embeds_body_on_the_next_queue_run() {
        let server = echo_server(64).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tier.db");
        seed_articles(&server, &path, 1, "tier");
        let client = reqwest::Client::new();
        run_embed_queue(&path, &client).await.unwrap();
        let conn = db::open_at(&path).unwrap();
        let title: Vec<u8> = conn
            .query_row(
                "SELECT embedding FROM chunks WHERE article_id=1 AND ordinal=0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        rrss_core::articles::set_read(&conn, 1, true).unwrap();
        assert_eq!(rag::due_embed_articles(&conn, 1).unwrap()[0].tier, 2);
        run_embed_queue(&path, &client).await.unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM chunks WHERE article_id=1 AND ordinal>0 AND embedding IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0)
            ).unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT embedding FROM chunks WHERE article_id=1 AND ordinal=0",
                [],
                |r| r.get::<_, Vec<u8>>(0)
            )
            .unwrap(),
            title
        );
        assert!(rag::due_embed_articles(&conn, 1).unwrap().is_empty());
        assert_eq!(request_sizes(&server).await, [8, 1, 1]);
    }

    #[tokio::test]
    async fn one_run_embeds_forty_due_articles() {
        let server = echo_server(usize::MAX).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        seed_articles(&server, &path, 40, "forty");
        run_embed_queue(&path, &reqwest::Client::new())
            .await
            .unwrap();
        let conn = db::open_at(&path).unwrap();
        let (total, embedded): (i64, i64) = conn
            .query_row("SELECT COUNT(*), COUNT(embedding) FROM chunks", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM articles", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            40
        );
        assert!(total >= 40, "each article has at least one chunk");
        assert_eq!(embedded, total);
        assert!(rag::due_embed_articles(&conn, 100).unwrap().is_empty());
    }

    #[tokio::test]
    async fn http_500_cools_the_job_without_consuming_chunk_attempts() {
        let server = MockServer::start().await;
        Mock::given(wiremock::matchers::method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        {
            let conn = db::open_at(&path).unwrap();
            let url = server.uri();
            let pid = crate::test_net::fake_provider(&conn, "mock", url.clone());
            ai_store::set_task_binding(&conn, ai_store::TASK_EMBED, Some(pid), Some("err500"))
                .unwrap();
            // 核对窗口内不再发探针，500 全部落在文章嵌入上。
            let now = rrss_core::feeds::now_ms();
            conn.execute(
                "INSERT INTO vector_spaces
                    (provider_id, base_url, model, dims, prefix_version, normalized, state, created_at, checked_at)
                 VALUES (?1, ?2, 'err500', 1, 1, 0, 'active', ?3, ?3)",
                rusqlite::params![pid, url, now],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://f.test/rss', 1)",
                [],
            )
            .unwrap();
            for id in 1..=3 {
                conn.execute(
                    "INSERT INTO articles (id, feed_id, title, hash, created_at, content_html, score)
                     VALUES (?1, 1, ?2, ?3, 1, '<p>正文一段。</p>', 1)",
                    rusqlite::params![id, format!("标题{id}"), format!("h{id}")],
                )
                .unwrap();
            }
        }
        run_embed_queue(&path, &reqwest::Client::new())
            .await
            .unwrap_err();
        let conn = db::open_at(&path).unwrap();
        let attempts: Vec<i64> = conn
            .prepare("SELECT attempts FROM chunks")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert!(!attempts.is_empty());
        assert!(attempts.iter().all(|&n| n == 0), "{attempts:?}");
    }
}
