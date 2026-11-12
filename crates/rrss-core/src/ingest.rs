//! 抓取结果入库与到期源。HTTP 由调用方通过 [`HttpFetcher`] 提供。
//! 日志 target：`rrss_core::ingest`。

pub mod feed_http;
#[cfg(test)]
mod log_tests;
mod schedule;
pub use schedule::{FailureClass, FetchSchedule, retry_after_deadline, slow_retry_interval};

use rusqlite::{Connection, params};

use crate::error::{Error, Result};
use crate::feeds::{FeedRow, now_ms};
use crate::kind::FeedKind;
use crate::network::ErrorKind;
use crate::parse::{ParsedEntry, parse_feed_bytes};
use crate::score::{MatchInput, load_compiled_rules};

#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub url: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FetchResponse {
    pub status: u16,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub retry_after: Option<String>,
    pub body: Vec<u8>,
}

pub trait HttpFetcher {
    fn get(&self, req: FetchRequest) -> Result<FetchResponse>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshStatus {
    Deleted,
    NotModified,
    Skipped {
        message: String,
        retry_at: Option<i64>,
    },
    Updated {
        inserted: u32,
        skipped: u32,
        derived_degraded: bool,
    },
    Failed {
        message: String,
    },
}

#[derive(Debug, Clone)]
pub struct RefreshResult {
    pub feed_id: i64,
    pub status: RefreshStatus,
}

pub fn due_feeds(conn: &Connection, now: i64) -> Result<Vec<FeedRow>> {
    let ids = conn
        .prepare("SELECT id FROM feeds WHERE kind!='manual' AND next_fetch_at<=?1 ORDER BY next_fetch_at, id")?
        .query_map([now], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.into_iter()
        .map(|id| crate::feeds::get_feed(conn, id))
        .collect()
}

/// Keyset pagination over the due index. `now` and `max_id` are fixed for the
/// scan, so additions and completed fetches cannot indefinitely extend it.
pub fn due_feed_page(
    conn: &Connection,
    now: i64,
    max_id: i64,
    after: (i64, i64),
    limit: usize,
) -> Result<Vec<(i64, i64)>> {
    Ok(conn
        .prepare(
            "SELECT next_fetch_at, id FROM feeds WHERE kind!='manual'
                  AND (next_fetch_at, id)>(?1, ?2) AND next_fetch_at<=?3 AND id<=?4
                  ORDER BY next_fetch_at, id LIMIT ?5",
        )?
        .query_map(
            params![after.0, after.1, now, max_id, limit as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?
        .collect::<rusqlite::Result<_>>()?)
}

/// 抓取期间订阅被改址时，旧响应的处理结果。
const STALE_URL_MESSAGE: &str = "订阅地址已改变，已丢弃旧响应";

/// 核对抓取时的源快照：源已删除或地址已改变时返回该结果，调用方不得再写这个源。
/// 必须在写事务内调用，核对与写入之间才不会插进改址或删除。
fn stale_snapshot(conn: &Connection, feed: &FeedRow) -> Result<Option<RefreshStatus>> {
    match crate::feeds::get_feed(conn, feed.id) {
        Ok(current) if current.url == feed.url => Ok(None),
        Ok(_) => Ok(Some(RefreshStatus::Failed {
            message: STALE_URL_MESSAGE.into(),
        })),
        Err(Error::FeedNotFound) => Ok(Some(RefreshStatus::Deleted)),
        Err(error) => Err(error),
    }
}

/// 在一个写事务里核对快照并记下这次抓取（成功清零健康计数，失败累加）。
fn record_checked(
    conn: &Connection,
    feed: &FeedRow,
    now: i64,
    validators: (Option<&str>, Option<&str>),
    error: Option<&str>,
    status: RefreshStatus,
    schedule: FetchSchedule,
) -> Result<RefreshResult> {
    let (result, event) =
        record_checked_pending(conn, feed, now, validators, error, status, schedule)?;
    // An outer transaction owns its commit. Its caller must use the pending
    // variant and publish the event explicitly after committing.
    if conn.is_autocommit()
        && let Some(event) = event
    {
        event.log_committed();
    }
    Ok(result)
}

fn record_checked_pending(
    conn: &Connection,
    feed: &FeedRow,
    now: i64,
    validators: (Option<&str>, Option<&str>),
    error: Option<&str>,
    status: RefreshStatus,
    schedule: FetchSchedule,
) -> Result<(RefreshResult, Option<FeedHealthEvent>)> {
    let (status, event) = crate::db::atomic_write(conn, |conn| {
        if let Some(stale) = stale_snapshot(conn, feed)? {
            return Ok((stale, None));
        }
        let event = mark_fetched(conn, feed, now, validators.0, validators.1, error, schedule)?;
        Ok((status, event))
    })?;
    Ok((
        RefreshResult {
            feed_id: feed.id,
            status,
        },
        event,
    ))
}

/// Contains only safe metadata from the health write, never the error text.
/// Outer transaction owners must discard this on rollback.
pub struct FeedHealthEvent {
    feed_id: i64,
    host: String,
    was_failing: bool,
    failed: bool,
    schedule: FetchSchedule,
}

impl FeedHealthEvent {
    /// Call once, only after the outermost health transaction has committed.
    pub fn log_committed(self) {
        let feed_id = crate::logsafe::unless_in_span("feed_id", self.feed_id);
        let host = self.host;
        let class = match self.schedule.failure {
            FailureClass::Fast => "fast",
            FailureClass::Slow => "slow",
        };
        let status = self.schedule.status;
        let error_kind = self.schedule.error_kind.as_str();
        if !self.failed {
            tracing::info!(feed_id, host, status, "feed fetch recovered");
        } else if self.was_failing {
            tracing::debug!(
                feed_id,
                host,
                class,
                error_kind,
                status,
                "feed fetch still failing"
            );
        } else {
            tracing::warn!(
                feed_id,
                host,
                class,
                error_kind,
                status,
                "feed fetch failing"
            );
        }
    }
}

/// 把一次 HTTP 响应应用到库：304 跳过正文，200 解析入库。`feed` 是发请求时的快照，
/// 写入前在同一写事务里核对；源已删除或改址时丢弃响应，不动源健康。
pub fn apply_fetch(
    conn: &Connection,
    feed: &FeedRow,
    resp: &FetchResponse,
    now: i64,
) -> Result<RefreshResult> {
    apply_fetch_scheduled(conn, feed, resp, now, FetchSchedule::default())
}

pub fn apply_fetch_scheduled(
    conn: &Connection,
    feed: &FeedRow,
    resp: &FetchResponse,
    now: i64,
    schedule: FetchSchedule,
) -> Result<RefreshResult> {
    // A received HTTP error or a parse failure is fast, regardless of the
    // caller's transport classification. Success also clears the slow streak.
    let schedule = schedule
        .with_failure(FailureClass::Fast)
        .with_error(ErrorKind::http(resp.status), Some(resp.status));
    if resp.status == 304 {
        tracing::debug!(
            feed_id = crate::logsafe::unless_in_span("feed_id", feed.id),
            "not modified"
        );
        return record_checked(
            conn,
            feed,
            now,
            (resp.etag.as_deref(), resp.last_modified.as_deref()),
            None,
            RefreshStatus::NotModified,
            schedule,
        );
    }
    if !(200..300).contains(&resp.status) {
        let message = format!("HTTP {}", resp.status);
        return record_checked(
            conn,
            feed,
            now,
            (None, None),
            Some(&message),
            RefreshStatus::Failed {
                message: message.clone(),
            },
            schedule,
        );
    }
    let metrics = crate::db::metrics::for_connection(conn);
    let parsing = metrics.as_ref().map(|metrics| metrics.feed_parse.start());
    let parsed = parse_feed_bytes(&resp.body, Some(&feed.url));
    drop(parsing);
    let parsed = match parsed {
        Ok(p) => p,
        Err(err) => {
            let message = err.to_string();
            return record_checked(
                conn,
                feed,
                now,
                (None, None),
                Some(&message),
                RefreshStatus::Failed {
                    message: message.clone(),
                },
                schedule.with_error(ErrorKind::Parse, Some(resp.status)),
            );
        }
    };
    // 解析与正文渲染在锁外完成；源信息、文章与抓取状态在同一写事务里写入。
    let mut health_event = None;
    let ingested = ingest_entries(conn, feed.id, &parsed.entries, now, |conn| {
        if let Some(stale) = stale_snapshot(conn, feed)? {
            return Ok(Some(stale));
        }
        // 用户或 OPML 指定的名称（title_custom）不用源标题覆盖；与源标题一致时恢复跟随。
        if feed.kind != FeedKind::Rsshub {
            let _ = conn.execute(
                "UPDATE feeds SET
                     title = CASE WHEN title_custom = 1 THEN title ELSE COALESCE(?1, title) END,
                     title_custom = CASE WHEN title = ?1 THEN 0 ELSE title_custom END,
                     site_url = COALESCE(?2, site_url), kind = ?3
                 WHERE id = ?4",
                params![
                    parsed.title,
                    parsed.site_url,
                    parsed.wire_kind.as_str(),
                    feed.id
                ],
            );
        } else {
            let _ = conn.execute(
                "UPDATE feeds SET
                     title = CASE WHEN title_custom = 1 THEN title ELSE COALESCE(?1, title) END,
                     title_custom = CASE WHEN title = ?1 THEN 0 ELSE title_custom END,
                     site_url = COALESCE(?2, site_url)
                 WHERE id = ?3",
                params![parsed.title, parsed.site_url, feed.id],
            );
        }
        health_event = mark_fetched(
            conn,
            feed,
            now,
            resp.etag.as_deref(),
            resp.last_modified.as_deref(),
            None,
            schedule,
        )?;
        Ok(None)
    })?;
    let report = match ingested {
        Ok(report) => report,
        Err(stale) => {
            return Ok(RefreshResult {
                feed_id: feed.id,
                status: stale,
            });
        }
    };
    if let Some(event) = health_event {
        event.log_committed();
    }
    let (inserted, skipped) = (report.inserted, report.skipped);
    tracing::debug!(
        feed_id = crate::logsafe::unless_in_span("feed_id", feed.id),
        inserted,
        skipped,
        "feed updated"
    );
    Ok(RefreshResult {
        feed_id: feed.id,
        status: RefreshStatus::Updated {
            inserted,
            skipped,
            derived_degraded: report.derived_degraded,
        },
    })
}

/// 同一条生产入库路径的分段计时；夹具构造和 HTTP 不计入。
#[derive(Debug, Default)]
pub struct IngestReport {
    pub derived_degraded: bool,
    pub inserted: u32,
    pub skipped: u32,
    pub dedup: std::time::Duration,
    pub prepare: std::time::Duration,
    pub write: std::time::Duration,
    pub commit: std::time::Duration,
    pub cluster: std::time::Duration,
    pub total: std::time::Duration,
}

pub fn upsert_entries(
    conn: &Connection,
    feed_id: i64,
    entries: &[ParsedEntry],
    now: i64,
) -> Result<(u32, u32)> {
    let report = upsert_entries_measured(conn, feed_id, entries, now)?;
    Ok((report.inserted, report.skipped))
}

pub fn upsert_entries_measured(
    conn: &Connection,
    feed_id: i64,
    entries: &[ParsedEntry],
    now: i64,
) -> Result<IngestReport> {
    match ingest_entries(conn, feed_id, entries, now, |_| Ok(None))? {
        Ok(report) => Ok(report),
        Err(_) => unreachable!("no snapshot guard"),
    }
}

/// `in_write` 在写事务开头执行；返回 `Some` 时整个事务回滚，原样交回该结果。
fn ingest_entries(
    conn: &Connection,
    feed_id: i64,
    entries: &[ParsedEntry],
    now: i64,
    in_write: impl FnOnce(&Connection) -> Result<Option<RefreshStatus>>,
) -> Result<std::result::Result<IngestReport, RefreshStatus>> {
    use rusqlite::OptionalExtension;
    use std::time::Instant;
    let started = Instant::now();
    let metrics = crate::db::metrics::for_connection(conn);
    let _timing = metrics.as_ref().map(|metrics| metrics.feed_ingest.start());
    let mut report = IngestReport::default();
    let compiled = load_compiled_rules(conn)?;
    // 源可能已被删除；锁外只读不报错，交给写事务里的快照核对。
    let folder_id: Option<i64> = conn
        .query_row(
            "SELECT folder_id FROM feeds WHERE id = ?1",
            [feed_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let folder_ids = crate::score::folder_chain(conn, folder_id)?;
    // 正文解析和规则匹配不占写锁。写锁只包住存在性检查和插入。
    let phase = Instant::now();
    let staged: Vec<StagedEntry<'_>> = entries
        .iter()
        .map(|entry| stage_entry(entry, feed_id, &folder_ids, &compiled))
        .collect();
    report.prepare = phase.elapsed();
    let mut inserted = 0u32;
    let mut skipped = 0u32;
    let mut inserted_ids = Vec::new();
    let tx = crate::db::write_transaction(conn)?;
    if let Some(stale) = in_write(&tx)? {
        return Ok(Err(stale));
    }
    for staged in &staged {
        let phase = Instant::now();
        let exists = entry_exists(&tx, feed_id, staged.entry)?;
        report.dedup += phase.elapsed();
        if exists {
            skipped += 1;
            continue;
        }
        let phase = Instant::now();
        let entry = staged.entry;
        tx.execute(
            "INSERT INTO articles (
                feed_id, guid, canonical_url, title, author, published_at,
                content_html, blocks_json, hash, simhash, score, reasons_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                feed_id,
                entry.guid,
                entry.canonical_url,
                entry.title,
                entry.author,
                entry.published_at,
                entry.content_html,
                staged.blocks_json,
                entry.hash,
                staged.simhash,
                staged.score,
                staged.reasons_json,
                now
            ],
        )?;
        let article_id = tx.last_insert_rowid();
        inserted_ids.push(article_id);
        for tag in &entry.tags {
            tx.execute(
                "INSERT OR IGNORE INTO article_tags (article_id, tag) VALUES (?1, ?2)",
                params![article_id, tag],
            )?;
        }
        inserted += 1;
        report.write += phase.elapsed();
    }
    let phase = Instant::now();
    tx.commit()?;
    report.commit = phase.elapsed();
    let phase = Instant::now();
    // Articles have committed. Derived failure must not turn successful ingest
    // into a feed/network failure or repeat its health accounting.
    if let Err(error) = crate::cluster::assign_articles(conn, &inserted_ids) {
        report.derived_degraded = true;
        tracing::warn!(
            %error,
            feed_id = crate::logsafe::unless_in_span("feed_id", feed_id),
            "incremental clustering deferred"
        );
    }
    report.cluster = phase.elapsed();
    report.total = started.elapsed();
    report.inserted = inserted;
    report.skipped = skipped;
    tracing::debug!(
        ?report,
        feed_id = crate::logsafe::unless_in_span("feed_id", feed_id),
        "ingest completed"
    );
    Ok(Ok(report))
}

struct StagedEntry<'a> {
    entry: &'a ParsedEntry,
    blocks_json: Option<String>,
    score: i64,
    reasons_json: Option<String>,
    simhash: i64,
}

fn stage_entry<'a>(
    entry: &'a ParsedEntry,
    feed_id: i64,
    folder_ids: &[i64],
    compiled: &[crate::score::CompiledRule],
) -> StagedEntry<'a> {
    let blocks_json = entry.content_html.as_deref().and_then(|html| {
        let opts = rrss_render::RenderOpts {
            base_url: entry.canonical_url.as_deref().and_then(|u| u.parse().ok()),
        };
        let doc = rrss_render::html_to_document(html, &opts);
        rrss_render::document_to_json(&doc).ok()
    });
    let result = if compiled.is_empty() {
        None
    } else {
        let body = crate::score::strip_html(entry.content_html.as_deref().unwrap_or(""));
        Some(crate::score::match_rules(
            compiled,
            &MatchInput {
                feed_id,
                folder_ids,
                author: entry.author.as_deref(),
                tags: &entry.tags,
                title: &entry.title,
                url: entry.canonical_url.as_deref(),
                body: &body,
            },
        ))
    };
    StagedEntry {
        entry,
        blocks_json,
        score: result.as_ref().map(|r| r.score).unwrap_or(0),
        reasons_json: result
            .as_ref()
            .map(|r| serde_json::to_string(&r.reasons).unwrap_or_else(|_| "[]".into())),
        simhash: crate::simhash::fingerprint_article(&entry.title, entry.content_html.as_deref()),
    }
}

fn entry_exists(conn: &Connection, feed_id: i64, entry: &ParsedEntry) -> Result<bool> {
    let by_guid: bool = conn
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM articles WHERE feed_id = ?1 AND guid = ?2)")?
        .query_row(params![feed_id, entry.guid], |row| row.get(0))?;
    if by_guid {
        return Ok(true);
    }
    if let Some(url) = &entry.canonical_url {
        let by_url: bool = conn
            .prepare_cached(
                "SELECT EXISTS(SELECT 1 FROM articles WHERE feed_id = ?1 AND canonical_url = ?2)",
            )?
            .query_row(params![feed_id, url], |row| row.get(0))?;
        if by_url {
            return Ok(true);
        }
    }
    conn.prepare_cached("SELECT EXISTS(SELECT 1 FROM articles WHERE feed_id = ?1 AND hash = ?2)")?
        .query_row(params![feed_id, entry.hash], |row| row.get(0))
        .map_err(Into::into)
}

fn mark_fetched(
    conn: &Connection,
    feed: &FeedRow,
    now: i64,
    etag: Option<&str>,
    last_modified: Option<&str>,
    error: Option<&str>,
    schedule: FetchSchedule,
) -> Result<Option<FeedHealthEvent>> {
    let (interval, previous_slow, was_failing): (i64, i64, bool) = conn.query_row(
        "SELECT interval_secs,slow_fail_count,error IS NOT NULL FROM feeds WHERE id=?1",
        [feed.id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let slow = if error.is_some() && schedule.failure == FailureClass::Slow {
        previous_slow.saturating_add(1)
    } else {
        0
    };
    let next = schedule.next_at(now, interval, slow);
    // 源健康：失败累加连续次数并记下这轮失败的开始；成功（200 / 304）清零并记最近成功。
    conn.execute(
        "UPDATE feeds SET last_fetched_at = ?1, etag = COALESCE(?2, etag),
                last_modified = COALESCE(?3, last_modified), error = ?4,
                fail_count = CASE WHEN ?4 IS NULL THEN 0 ELSE fail_count + 1 END,
                failing_since = CASE WHEN ?4 IS NULL THEN NULL ELSE COALESCE(failing_since, ?1) END,
                last_success_at = CASE WHEN ?4 IS NULL THEN ?1 ELSE last_success_at END,
                next_fetch_at = ?6, slow_fail_count = ?7
         WHERE id = ?5",
        params![now, etag, last_modified, error, feed.id, next, slow],
    )?;
    Ok((was_failing || error.is_some()).then(|| FeedHealthEvent {
        feed_id: feed.id,
        host: crate::logsafe::host_str(&feed.url),
        was_failing,
        failed: error.is_some(),
        schedule,
    }))
}

/// 同步刷新：调用方已拿到 [`HttpFetcher`]（测试用 mock）。
pub fn refresh_feed(
    conn: &Connection,
    client: &dyn HttpFetcher,
    feed_id: i64,
) -> Result<RefreshResult> {
    let feed = crate::feeds::get_feed(conn, feed_id)?;
    if feed.kind == FeedKind::Manual {
        return Ok(RefreshResult {
            feed_id,
            status: RefreshStatus::NotModified,
        });
    }
    let resp = match client.get(FetchRequest {
        url: feed.url.clone(),
        etag: feed.etag.clone(),
        last_modified: feed.last_modified.clone(),
    }) {
        Ok(r) => r,
        Err(err) => return record_fetch_failure(conn, &feed, &err.to_string()),
    };
    apply_fetch(conn, &feed, &resp, now_ms())
}

/// 记下一次没有拿到可用响应的抓取（网络失败、正文超限等）。核对快照：
/// 源已删除或改址时旧请求的失败不计入新地址的健康。
pub fn record_fetch_failure(
    conn: &Connection,
    feed: &FeedRow,
    message: &str,
) -> Result<RefreshResult> {
    record_fetch_failure_scheduled(conn, feed, message, now_ms(), FetchSchedule::default())
}

pub fn record_fetch_failure_scheduled(
    conn: &Connection,
    feed: &FeedRow,
    message: &str,
    now: i64,
    schedule: FetchSchedule,
) -> Result<RefreshResult> {
    record_checked(
        conn,
        feed,
        now,
        (None, None),
        Some(message),
        RefreshStatus::Failed {
            message: message.into(),
        },
        schedule,
    )
}

/// Same health write, with logging deferred to the outer transaction owner.
/// This is needed when the scheduler holds its connectivity guard across the
/// health update. A rolled-back transaction must discard the returned event.
pub fn record_fetch_failure_pending(
    conn: &Connection,
    feed: &FeedRow,
    message: &str,
    now: i64,
    schedule: FetchSchedule,
) -> Result<(RefreshResult, Option<FeedHealthEvent>)> {
    record_checked_pending(
        conn,
        feed,
        now,
        (None, None),
        Some(message),
        RefreshStatus::Failed {
            message: message.into(),
        },
        schedule,
    )
}

/// A shared host cooldown only advances the derived due date, never source
/// health. As with results, deletion/address changes are checked under the writer.
pub fn record_fetch_deferred(
    conn: &Connection,
    feed: &FeedRow,
    until: Option<i64>,
    message: &str,
) -> Result<RefreshResult> {
    let record = |conn: &Connection| record_deferred_checked(conn, feed, until, message);
    if until.is_some() {
        crate::db::atomic_write(conn, record)
    } else {
        record(conn)
    }
}

/// One bounded writer transaction for a metadata batch. Source identity is
/// checked inside it; a failed update rolls back all derived deadlines.
pub fn record_fetch_deferred_batch(
    conn: &Connection,
    entries: &[(&FeedRow, Option<i64>, &str)],
) -> Result<Vec<RefreshResult>> {
    let record = |conn: &Connection| {
        entries
            .iter()
            .map(|(feed, until, message)| record_deferred_checked(conn, feed, *until, message))
            .collect()
    };
    if entries.iter().any(|(_, until, _)| until.is_some()) {
        crate::db::atomic_write(conn, record)
    } else {
        // A local admission/persistence wait has no derived deadline to write.
        // Do not wait for the very snapshot writer that caused the deferral.
        record(conn)
    }
}

fn record_deferred_checked(
    conn: &Connection,
    feed: &FeedRow,
    until: Option<i64>,
    message: &str,
) -> Result<RefreshResult> {
    let status = if let Some(stale) = stale_snapshot(conn, feed)? {
        stale
    } else {
        if let Some(until) = until {
            conn.prepare_cached(
                "UPDATE feeds SET next_fetch_at=max(next_fetch_at,?1) WHERE id=?2",
            )?
            .execute(params![until, feed.id])?;
        }
        RefreshStatus::Skipped {
            message: message.into(),
            retry_at: until,
        }
    };
    Ok(RefreshResult {
        feed_id: feed.id,
        status,
    })
}

/// 示例与独立调用使用的环境代理客户端，不读取用户数据库。
/// 应用生产入口必须使用 [`http_client_with_proxy`] 或应用层配置工厂。
pub fn http_client() -> Result<reqwest::Client> {
    http_client_with_proxy(&crate::network::ProxyConfig {
        global: crate::network::ProxyRule {
            mode: crate::network::ProxyMode::System,
            ..Default::default()
        },
        ..Default::default()
    })
}

/// 订阅客户端保留浏览器 UA，代理由内容用途的配置决定。
pub fn http_client_with_proxy(config: &crate::network::ProxyConfig) -> Result<reqwest::Client> {
    config.build_client(crate::network::NetworkScope::Content, http_client_builder())
}

/// 抓取的默认 headers 与代理选择分离，供按用途缓存客户端的应用层复用。
pub fn http_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
}

/// 带 ETag / Last-Modified 的 GET。在 tokio 上跑，不要在 UI 线程调用。
pub async fn fetch_http(client: &reqwest::Client, req: FetchRequest) -> Result<FetchResponse> {
    use reqwest::header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED, RETRY_AFTER};

    tracing::debug!(host = %crate::logsafe::host_str(&req.url), "fetching feed");
    let mut builder = client.get(&req.url);
    if let Some(etag) = &req.etag {
        builder = builder.header(IF_NONE_MATCH, etag.as_str());
    }
    if let Some(lm) = &req.last_modified {
        builder = builder.header(IF_MODIFIED_SINCE, lm.as_str());
    }
    let resp = builder.send().await.map_err(|err| {
        let safe = crate::network::request_error(&err);
        tracing::warn!(host = %crate::logsafe::host_str(&req.url), error = %safe, "http request failed");
        safe
    })?;
    let status = resp.status().as_u16();
    // 代理认证失败的响应体可能回显凭据，不能交给订阅解析或 CLI 网页输出。
    if status == 407 {
        return Err(Error::Http {
            status,
            message: "代理认证失败，请检查网络代理设置".into(),
        });
    }
    let etag = resp
        .headers()
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let last_modified = resp
        .headers()
        .get(LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let retry_after = resp
        .headers()
        .get(RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = resp.bytes().await.map_err(|err| {
        let mut safe = crate::network::request_error(&err);
        if let Error::Http { status: code, .. } = &mut safe {
            *code = status;
        }
        tracing::warn!(host = %crate::logsafe::host_str(&req.url), status, error = %safe, "reading response body failed");
        safe
    })?;
    tracing::debug!(
        host = %crate::logsafe::host_str(&req.url),
        status,
        bytes = body.len(),
        "http fetch completed"
    );
    Ok(FetchResponse {
        status,
        etag,
        last_modified,
        retry_after,
        body: body.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{PooledConnection, open_at};
    use crate::feeds::{SubscribeRequest, subscribe};
    use crate::parse::parse_feed_bytes;
    use tempfile::tempdir;

    const RSS2: &str = include_str!("../../../testdata/feeds/rss2-sample.xml");

    struct Mock {
        resp: FetchResponse,
    }

    impl HttpFetcher for Mock {
        fn get(&self, _req: FetchRequest) -> Result<FetchResponse> {
            Ok(self.resp.clone())
        }
    }

    fn open_temp() -> (tempfile::TempDir, PooledConnection) {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        (dir, conn)
    }

    #[test]
    fn due_pages_use_persisted_deadlines_and_never_resample_offset() {
        let (_dir, conn) = open_temp();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,created_at,next_fetch_at,last_fetched_at)
             VALUES(1,'https://a.test/feed',0,200,0),
                   (2,'https://b.test/feed',0,100,99999999),
                   (3,'https://c.test/feed',0,100,99999999);",
        )
        .unwrap();
        let first = due_feed_page(&conn, 200, 3, (i64::MIN, 0), 2).unwrap();
        assert_eq!(first, [(100, 2), (100, 3)]);
        assert_eq!(
            due_feed_page(&conn, 200, 3, first[1], 2).unwrap(),
            [(200, 1)]
        );
        let response = FetchResponse {
            status: 304,
            etag: None,
            last_modified: None,
            retry_after: None,
            body: Vec::new(),
        };
        let source = crate::feeds::get_feed(&conn, 1).unwrap();
        apply_fetch_scheduled(
            &conn,
            &source,
            &response,
            1000,
            FetchSchedule::with_random(|| 360_000),
        )
        .unwrap();
        let saved = crate::feeds::get_feed(&conn, 1).unwrap().next_fetch_at;
        assert_eq!(saved, 3_961_000);
        for _ in 0..3 {
            assert_eq!(due_feeds(&conn, saved - 1).unwrap().len(), 2);
            assert_eq!(
                crate::feeds::get_feed(&conn, 1).unwrap().next_fetch_at,
                saved
            );
        }
        assert_eq!(due_feeds(&conn, saved).unwrap().len(), 3);
        let plan: Vec<String> = conn
            .prepare(
                "EXPLAIN QUERY PLAN SELECT next_fetch_at,id FROM feeds
            WHERE kind!='manual' AND (next_fetch_at,id)>(?1,?2) AND next_fetch_at<=?3 AND id<=?4
            ORDER BY next_fetch_at,id LIMIT 64",
            )
            .unwrap()
            .query_map(params![0, 0, saved, 3], |r| r.get(3))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(
            plan.iter().any(|line| line.contains("idx_feeds_due")),
            "{plan:?}"
        );
    }

    #[test]
    fn slow_staircase_is_separate_from_health_and_resets_on_fast_failure_or_success() {
        let (_dir, conn) = open_temp();
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://slow.test/feed',0)",
            [],
        )
        .unwrap();
        // Reuse the original snapshot: the slow counter must be read in the
        // write transaction, not restored from this old snapshot.
        let feed = crate::feeds::get_feed(&conn, 1).unwrap();
        let slow = FetchSchedule::with_random(|| 0).with_failure(FailureClass::Slow);
        for (count, hours) in [(1, 1), (2, 2), (3, 4), (4, 4)] {
            let now = count * 1000;
            record_fetch_failure_scheduled(&conn, &feed, "timeout", now, slow).unwrap();
            let current = crate::feeds::get_feed(&conn, 1).unwrap();
            assert_eq!(current.slow_fail_count, count);
            assert_eq!(current.next_fetch_at, now + hours * 3_600_000);
            let health: (i64, i64) = conn
                .query_row(
                    "SELECT fail_count,failing_since FROM feeds WHERE id=1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(health, (count, 1000));
        }
        let fast = FetchSchedule::with_random(|| 0);
        record_fetch_failure_scheduled(&conn, &feed, "connection refused", 5000, fast).unwrap();
        let current = crate::feeds::get_feed(&conn, 1).unwrap();
        assert_eq!(
            (current.slow_fail_count, current.next_fetch_at),
            (0, 3_605_000)
        );
        assert_eq!(
            conn.query_row("SELECT fail_count FROM feeds WHERE id=1", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            5
        );
        assert!(
            due_feeds(&conn, current.next_fetch_at - 1)
                .unwrap()
                .is_empty()
        );
        assert_eq!(due_feeds(&conn, current.next_fetch_at).unwrap().len(), 1);
        for status in [200, 304] {
            record_fetch_failure_scheduled(&conn, &feed, "timeout", 6000, slow).unwrap();
            let response = FetchResponse {
                status,
                etag: None,
                last_modified: None,
                retry_after: None,
                body: RSS2.as_bytes().to_vec(),
            };
            apply_fetch_scheduled(&conn, &feed, &response, 7000, fast).unwrap();
            let current = crate::feeds::get_feed(&conn, 1).unwrap();
            assert_eq!(
                (current.slow_fail_count, current.next_fetch_at),
                (0, 3_607_000)
            );
            let health: (i64, Option<i64>) = conn
                .query_row(
                    "SELECT fail_count,failing_since FROM feeds WHERE id=1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(health, (0, None));
        }
        record_fetch_failure_scheduled(&conn, &feed, "timeout", 8000, slow).unwrap();
        let malformed = FetchResponse {
            status: 200,
            etag: None,
            last_modified: None,
            retry_after: None,
            body: b"not a feed".to_vec(),
        };
        apply_fetch_scheduled(&conn, &feed, &malformed, 9000, slow).unwrap();
        assert_eq!(crate::feeds::get_feed(&conn, 1).unwrap().slow_fail_count, 0);
    }

    #[test]
    fn deferred_batches_are_atomic_and_recheck_source_snapshots() {
        let (_dir, conn) = open_temp();
        for id in 1..=3 {
            conn.execute(
                "INSERT INTO feeds(id,url,created_at,fail_count,slow_fail_count)
                VALUES(?1,?2,0,3,2)",
                params![id, format!("https://cooled.invalid/{id}")],
            )
            .unwrap();
        }
        let feeds = (1..=3)
            .map(|id| crate::feeds::get_feed(&conn, id).unwrap())
            .collect::<Vec<_>>();
        conn.execute_batch(
            "CREATE TRIGGER fail_deferred BEFORE UPDATE OF next_fetch_at ON feeds
            WHEN NEW.id=3 BEGIN SELECT RAISE(FAIL,'injected'); END;",
        )
        .unwrap();
        let entries = feeds
            .iter()
            .map(|feed| (feed, Some(10_000), "cooldown"))
            .collect::<Vec<_>>();
        assert!(record_fetch_deferred_batch(&conn, &entries).is_err());
        assert_eq!(crate::feeds::get_feed(&conn, 1).unwrap().next_fetch_at, 0);
        conn.execute_batch(
            "DROP TRIGGER fail_deferred;
            UPDATE feeds SET url='https://replacement.invalid/feed' WHERE id=2;
            DELETE FROM feeds WHERE id=3;",
        )
        .unwrap();
        let results = record_fetch_deferred_batch(&conn, &entries).unwrap();
        assert!(matches!(
            results[0].status,
            RefreshStatus::Skipped {
                retry_at: Some(10_000),
                ..
            }
        ));
        assert!(
            matches!(&results[1].status, RefreshStatus::Failed { message } if message == STALE_URL_MESSAGE)
        );
        assert!(matches!(results[2].status, RefreshStatus::Deleted));
        let current = crate::feeds::get_feed(&conn, 1).unwrap();
        assert_eq!(
            (current.next_fetch_at, current.slow_fail_count),
            (10_000, 2)
        );
        assert_eq!(crate::feeds::get_feed(&conn, 2).unwrap().next_fetch_at, 0);
        assert_eq!(
            conn.query_row("SELECT fail_count FROM feeds WHERE id=1", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            3
        );
        record_fetch_deferred_batch(&conn, &[(&feeds[0], Some(5000), "shorter")]).unwrap();
        assert_eq!(
            crate::feeds::get_feed(&conn, 1).unwrap().next_fetch_at,
            10_000
        );
    }

    #[test]
    fn health_slow_counter_and_due_date_roll_back_together_and_stale_failure_is_ignored() {
        let (_dir, conn) = open_temp();
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://old.test/feed',0)",
            [],
        )
        .unwrap();
        let feed = crate::feeds::get_feed(&conn, 1).unwrap();
        let slow = FetchSchedule::with_random(|| 0).with_failure(FailureClass::Slow);
        record_fetch_failure_scheduled(&conn, &feed, "timeout", 1000, slow).unwrap();
        let state = || {
            conn.query_row("SELECT fail_count,failing_since,slow_fail_count,next_fetch_at,last_fetched_at FROM feeds WHERE id=1", [], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?))).unwrap()
        };
        let previous = state();
        conn.execute_batch("CREATE TEMP TRIGGER block_due BEFORE UPDATE OF next_fetch_at ON feeds BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        assert!(record_fetch_failure_scheduled(&conn, &feed, "timeout", 2000, slow).is_err());
        assert_eq!(state(), previous);
        conn.execute_batch(
            "DROP TRIGGER block_due; UPDATE feeds SET url='https://new.test/feed' WHERE id=1;",
        )
        .unwrap();
        record_fetch_failure_scheduled(&conn, &feed, "timeout", 3000, slow).unwrap();
        assert_eq!(state(), previous);
        conn.execute("DELETE FROM feeds WHERE id=1", []).unwrap();
        assert_eq!(
            record_fetch_failure_scheduled(&conn, &feed, "timeout", 4000, slow)
                .unwrap()
                .status,
            RefreshStatus::Deleted
        );
    }

    #[test]
    fn upsert_skips_same_guid() {
        let (_dir, conn) = open_temp();
        let id = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://news.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        let parsed =
            parse_feed_bytes(RSS2.as_bytes(), Some("https://news.example.com/rss.xml")).unwrap();
        let (ins, skip) = upsert_entries(&conn, id, &parsed.entries, now_ms()).unwrap();
        assert_eq!(ins, 2);
        assert_eq!(skip, 0);
        let (ins2, skip2) = upsert_entries(&conn, id, &parsed.entries, now_ms()).unwrap();
        assert_eq!(ins2, 0);
        assert_eq!(skip2, 2);
    }

    #[test]
    fn dedup_guid_url_hash_and_mixed_refresh_preserve_article_state() {
        let (_dir, conn) = open_temp();
        let fid = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://dedup.test/feed".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        let parsed = parse_feed_bytes(RSS2.as_bytes(), Some("https://dedup.test/feed")).unwrap();
        let first = parsed.entries[0].clone();
        assert_eq!(
            upsert_entries(&conn, fid, &[first.clone(), first.clone()], 1).unwrap(),
            (1, 1)
        );
        conn.execute(
            "UPDATE articles SET read=1,starred=1,summary='saved summary'",
            [],
        )
        .unwrap();
        let mut same_url = first.clone();
        same_url.guid = "different-guid".into();
        same_url.hash = "different-hash".into();
        let mut same_hash = first.clone();
        same_hash.guid = "hash-guid".into();
        same_hash.canonical_url = None;
        let second = parsed.entries[1].clone();
        assert_eq!(
            upsert_entries(&conn, fid, &[first, same_url, same_hash, second], 2).unwrap(),
            (1, 3)
        );
        let saved: (i64, i64, String) = conn
            .query_row(
                "SELECT read,starred,summary FROM articles WHERE id=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(saved, (1, 1, "saved summary".into()));
        let missing: i64 = conn
            .query_row(
                "SELECT count(*) FROM articles WHERE cluster_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(missing, 0);
    }

    #[test]
    fn failed_insert_rolls_back_articles_tags_and_fts() {
        let (_dir, conn) = open_temp();
        let fid = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://fail.test/feed".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        let parsed = parse_feed_bytes(RSS2.as_bytes(), Some("https://fail.test/feed")).unwrap();
        conn.execute_batch(
            "CREATE TEMP TRIGGER fail_second BEFORE INSERT ON articles
            WHEN (SELECT count(*) FROM articles) = 1 BEGIN SELECT RAISE(ABORT,'injected'); END;",
        )
        .unwrap();
        assert!(upsert_entries(&conn, fid, &parsed.entries, 1).is_err());
        for table in ["articles", "articles_fts", "article_tags", "clusters"] {
            let n: i64 = conn
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "partial writes in {table}");
        }
        conn.execute_batch("DROP TRIGGER fail_second").unwrap();
        assert_eq!(
            upsert_entries(&conn, fid, &parsed.entries, 2).unwrap(),
            (2, 0)
        );
    }

    #[test]
    fn wal_reader_snapshot_survives_concurrent_ingest() {
        let (dir, reader) = open_temp();
        let fid = subscribe(
            &reader,
            SubscribeRequest {
                url: "https://wal.test/feed".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        reader.execute_batch("BEGIN").unwrap();
        let count = || {
            reader
                .query_row("SELECT count(*) FROM articles", [], |r| r.get::<_, i64>(0))
                .unwrap()
        };
        assert_eq!(count(), 0);
        let path = dir.path().join("rrss.db");
        std::thread::spawn(move || {
            let conn = open_at(&path).unwrap();
            let parsed = parse_feed_bytes(RSS2.as_bytes(), Some("https://wal.test/feed")).unwrap();
            assert_eq!(
                upsert_entries(&conn, fid, &parsed.entries, 1).unwrap(),
                (2, 0)
            );
        })
        .join()
        .unwrap();
        assert_eq!(count(), 0, "reader keeps its snapshot while writer commits");
        reader.execute_batch("COMMIT").unwrap();
        assert_eq!(count(), 2);
    }

    #[test]
    fn upsert_waits_for_other_writer_instead_of_failing_busy() {
        use std::sync::{Arc, Barrier};
        use std::thread;
        use std::time::Duration;

        let (dir, conn) = open_temp();
        let id = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://busy.test/feed".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        let path = dir.path().join("rrss.db");
        let gate = Arc::new(Barrier::new(2));
        let worker_gate = Arc::clone(&gate);
        let worker = thread::spawn(move || {
            let worker = open_at(&path).unwrap();
            worker_gate.wait();
            let parsed = parse_feed_bytes(RSS2.as_bytes(), Some("https://busy.test/feed")).unwrap();
            upsert_entries(&worker, id, &parsed.entries, 1)
        });
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        conn.execute("UPDATE feeds SET error = 'held' WHERE id = ?1", [id])
            .unwrap();
        gate.wait();
        thread::sleep(Duration::from_millis(400));
        conn.execute_batch("COMMIT").unwrap();
        assert_eq!(worker.join().unwrap().unwrap(), (2, 0));
    }

    #[test]
    fn stale_snapshot_discards_response_without_touching_articles_or_health() {
        let (_dir, conn) = open_temp();
        let id = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://old.test/feed".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        let snapshot = crate::feeds::get_feed(&conn, id).unwrap();
        conn.execute(
            "UPDATE feeds SET url = 'https://new.test/feed' WHERE id = ?1",
            [id],
        )
        .unwrap();
        let resp = |status: u16| FetchResponse {
            status,
            etag: Some("old-etag".into()),
            last_modified: None,
            retry_after: None,
            body: RSS2.as_bytes().to_vec(),
        };
        for status in [200, 304, 500] {
            let result = apply_fetch(&conn, &snapshot, &resp(status), 10).unwrap();
            assert_eq!(
                result.status,
                RefreshStatus::Failed {
                    message: STALE_URL_MESSAGE.into()
                }
            );
        }
        let failed = record_fetch_failure(&conn, &snapshot, "timeout").unwrap();
        assert!(matches!(failed.status, RefreshStatus::Failed { .. }));
        let current = crate::feeds::get_feed(&conn, id).unwrap();
        assert_eq!(
            current.title, snapshot.title,
            "old feed metadata is not applied"
        );
        assert!(current.last_fetched_at.is_none());
        assert!(current.etag.is_none());
        let (articles, fail_count): (i64, i64) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM articles), fail_count FROM feeds WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((articles, fail_count), (0, 0));

        conn.execute("DELETE FROM feeds WHERE id = ?1", [id])
            .unwrap();
        assert_eq!(
            apply_fetch(&conn, &snapshot, &resp(200), 20)
                .unwrap()
                .status,
            RefreshStatus::Deleted
        );
        assert_eq!(
            record_fetch_failure(&conn, &snapshot, "timeout")
                .unwrap()
                .status,
            RefreshStatus::Deleted
        );
    }

    #[test]
    fn cluster_failure_leaves_committed_articles_recoverable_by_backfill() {
        let (_dir, conn) = open_temp();
        let fid = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://cluster-fail.test/feed".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        let parsed =
            parse_feed_bytes(RSS2.as_bytes(), Some("https://cluster-fail.test/feed")).unwrap();
        conn.execute_batch(
            "CREATE TEMP TRIGGER fail_cluster BEFORE UPDATE OF cluster_id ON articles
            BEGIN SELECT RAISE(ABORT,'injected'); END;",
        )
        .unwrap();
        let result = apply_fetch(
            &conn,
            &crate::feeds::get_feed(&conn, fid).unwrap(),
            &FetchResponse {
                status: 200,
                etag: None,
                last_modified: None,
                retry_after: None,
                body: RSS2.as_bytes().to_vec(),
            },
            1,
        )
        .unwrap();
        assert!(matches!(
            result.status,
            RefreshStatus::Updated {
                inserted: 2,
                derived_degraded: true,
                ..
            }
        ));
        let feed = crate::feeds::get_feed(&conn, fid).unwrap();
        assert!(feed.error.is_none());
        assert_eq!(
            conn.query_row("SELECT fail_count FROM feeds WHERE id=?1", [fid], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        let count: i64 = conn
            .query_row("SELECT count(*) FROM articles", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            count, 2,
            "article transaction commits before the separate cluster transaction"
        );
        conn.execute_batch("DROP TRIGGER fail_cluster").unwrap();
        assert_eq!(
            upsert_entries(&conn, fid, &parsed.entries, 2).unwrap(),
            (0, 2)
        );
        assert_eq!(crate::cluster::assign_unclustered(&conn, 100).unwrap(), 2);
        let missing: i64 = conn
            .query_row(
                "SELECT count(*) FROM articles WHERE cluster_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(missing, 0);
    }

    #[test]
    fn refresh_304_clears_error() {
        let (_dir, conn) = open_temp();
        let id = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://news.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        let mock = Mock {
            resp: FetchResponse {
                status: 304,
                etag: Some("\"abc\"".into()),
                last_modified: None,
                retry_after: None,
                body: vec![],
            },
        };
        let result = refresh_feed(&conn, &mock, id).unwrap();
        assert_eq!(result.status, RefreshStatus::NotModified);
        let row = crate::feeds::get_feed(&conn, id).unwrap();
        assert!(row.error.is_none());
        assert!(row.last_fetched_at.is_some());
    }

    #[test]
    fn refresh_200_inserts() {
        let (_dir, conn) = open_temp();
        let id = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://news.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        let mock = Mock {
            resp: FetchResponse {
                status: 200,
                etag: Some("\"v1\"".into()),
                last_modified: None,
                retry_after: None,
                body: RSS2.as_bytes().to_vec(),
            },
        };
        let result = refresh_feed(&conn, &mock, id).unwrap();
        assert_eq!(
            result.status,
            RefreshStatus::Updated {
                inserted: 2,
                skipped: 0,
                derived_degraded: false,
            }
        );
        let cards = crate::articles::list_focus(&conn, 10).unwrap();
        assert_eq!(cards.len(), 2);
        assert!(!cards[0].read);
    }

    #[test]
    fn refresh_keeps_custom_titles_and_follows_the_source_otherwise() {
        let (_dir, conn) = open_temp();
        let id = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://news.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        let refresh = |body: &str| {
            let mock = Mock {
                resp: FetchResponse {
                    status: 200,
                    etag: None,
                    last_modified: None,
                    retry_after: None,
                    body: body.as_bytes().to_vec(),
                },
            };
            refresh_feed(&conn, &mock, id).unwrap();
            crate::feeds::get_feed(&conn, id).unwrap().title
        };
        assert_eq!(refresh(RSS2).as_deref(), Some("Example News"));
        crate::feeds::rename_feed(&conn, id, "我的新闻").unwrap();
        assert_eq!(refresh(RSS2).as_deref(), Some("我的新闻"));
        // 清空名称即恢复跟随源标题。
        crate::feeds::rename_feed(&conn, id, "  ").unwrap();
        assert_eq!(refresh(RSS2).as_deref(), Some("Example News"));
        // 与源标题相同的命名不算自定义，源改名后照常跟随。
        crate::feeds::rename_feed(&conn, id, "Example News").unwrap();
        assert_eq!(refresh(RSS2).as_deref(), Some("Example News"));
        let renamed = RSS2.replace(
            "<title>Example News</title>",
            "<title>Example Daily</title>",
        );
        assert_eq!(refresh(&renamed).as_deref(), Some("Example Daily"));
    }
}
