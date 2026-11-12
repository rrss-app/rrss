//! Database-scoped, bounded feed runtime and background coordination.
use crate::{network::NetworkClients, views};
use anyhow::Result;
use rrss_core::feeds::now_ms;
use rrss_core::ingest::{
    FetchRequest, FetchResponse, FetchSchedule, RefreshResult, RefreshStatus,
    apply_fetch_scheduled, record_fetch_failure_scheduled,
};
use rrss_core::{FeedRow, db};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub(crate) mod background;
mod cooldown;
mod diagnostics;
pub(crate) mod digest_jobs;
pub(crate) mod model_capacity;
mod model_cooldown;
mod offline;
mod operations;
mod runtime;
pub use runtime::{FetchTicket, Scheduler};
const DB_WRITE_ATTEMPTS: u32 = 4;

fn is_sqlite_busy(err: &rrss_core::Error) -> bool {
    let lower = err.to_string().to_ascii_lowercase();
    lower.contains("database is locked")
        || lower.contains("database lock")
        || lower.contains("sqlite_busy")
        || lower.contains("code: 5")
}

/// 刷新的写操作。写事务在库内等待；仍遇到锁定时再试几次，避免把忙等记成源错误。
async fn write_db<T, F>(path: PathBuf, f: F) -> Result<T>
where
    T: Send + 'static,
    F: Fn(&rusqlite::Connection) -> rrss_core::Result<T> + Send + Sync + 'static,
{
    let f = Arc::new(f);
    let capacity = model_capacity::Capacity::for_database(&path);
    let mut last = None;
    for attempt in 0..DB_WRITE_ATTEMPTS {
        let path = path.clone();
        let f = Arc::clone(&f);
        let work = capacity.work();
        let metrics = capacity.clone();
        let queued = std::time::Instant::now();
        // 阻塞线程不继承调用方的 span；显式带过去，写库里的日志仍属于这个源。
        let span = tracing::Span::current();
        match tokio::task::spawn_blocking(move || {
            let _work = work;
            metrics.metrics.db_queue.record(queued.elapsed());
            span.in_scope(|| {
                let conn = db::open_at(&path)?;
                let _timing = metrics.metrics.db_work.start();
                f(&conn)
            })
        })
        .await
        {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(err)) if is_sqlite_busy(&err) && attempt + 1 < DB_WRITE_ATTEMPTS => {
                tracing::warn!(attempt, error = %crate::log::error(&err), "refresh write busy; retrying");
                last = Some(err);
                tokio::time::sleep(Duration::from_millis(200 * u64::from(attempt + 1))).await;
            }
            Ok(Err(err)) => return Err(err.into()),
            Err(err) => return Err(err.into()),
        }
    }
    Err(last
        .map(Into::into)
        .unwrap_or_else(|| anyhow::anyhow!("database is locked")))
}

impl Scheduler {
    pub async fn add_feed(&self, url: &str) -> Result<views::AddFeedResult> {
        let url = url.trim().to_string();
        let intended_url = url.clone();
        let id = crate::jobs::spawn_db(self.shared.path.clone(), move |conn| {
            crate::actions::add_feed(conn, &url)
        })
        .await?;
        let deferred = self.shared.deferred(id, &intended_url);
        let message = match self.submit_added(id) {
            Ok(_) if self.shared.connectivity.lock().unwrap().unavailable() => {
                "已添加，网络不可用，联网后继续抓取".into()
            }
            Ok(_) if deferred.is_some() => format!("已添加，{}", deferred.unwrap().message),
            Ok(_) => "已添加，正在后台抓取".into(),
            Err(error) => format!("已添加，但未安排抓取：{error}"),
        };
        Ok(views::AddFeedResult { id, message })
    }

    /// Page due IDs into bounded admission. The producer retains no per-source
    /// result or waiter after admission; completion belongs to the runtime.
    async fn enqueue_due(&self) -> Result<usize> {
        let max_id: i64 = crate::jobs::spawn_db(self.shared.path.clone(), |conn| {
            Ok(conn.query_row("SELECT COALESCE(MAX(id),0) FROM feeds", [], |r| r.get(0))?)
        })
        .await?;
        let now = self.shared.clock.now().utc;
        let mut cursor = (i64::MIN, 0);
        let mut admitted = 0;
        loop {
            let ids = crate::jobs::spawn_db(self.shared.path.clone(), move |conn| {
                rrss_core::ingest::due_feed_page(conn, now, max_id, cursor, 64)
            })
            .await?;
            if ids.is_empty() {
                break;
            }
            cursor = *ids.last().unwrap();
            for (_, id) in ids {
                drop(self.submit_wait(id, true).await?);
                admitted += 1;
            }
        }
        Ok(admitted)
    }

    /// Feed polling, ranking and background job deadlines advance independently.
    pub async fn run_loop(&self) {
        self.run_loop_with_interval(Duration::from_secs(15 * 60))
            .await;
    }

    async fn run_loop_with_interval(&self, interval: Duration) {
        use std::sync::atomic::Ordering;
        if self.shared.loop_running.swap(true, Ordering::AcqRel) {
            return;
        }
        struct Running<'a>(&'a std::sync::atomic::AtomicBool);
        impl Drop for Running<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _running = Running(&self.shared.loop_running);
        let mut stop = self.shared.stop.subscribe();
        self.shared.automatic.send_replace(true);
        tokio::select! {
            _ = stop.wait_for(|stopped| *stopped) => {},
            _ = crate::jobs::rank::run_loop(self.shared.path.clone()) => {},
            _ = async {
                loop {
                    if let Err(error) = self.enqueue_due().await { tracing::warn!(%error, "scheduled refresh failed"); }
                    tokio::time::sleep(interval).await;
                }
            } => {},
        }
    }
}

/// 应用一次响应并返回本次新入库的文章 ID。增量排序不在这里等：由调用方交给
/// 后台串行合并，解析工作者与源事件不等排序（见阶段 25「抓取、入库与刷新完成」）。
async fn apply_response(
    path: PathBuf,
    feed: FeedRow,
    resp: FetchResponse,
    schedule: FetchSchedule,
    now: i64,
) -> Result<(RefreshResult, Vec<i64>)> {
    let feed_id = feed.id;
    let err_feed = feed.clone();
    let err_path = path.clone();
    match write_db(path, move |conn| {
        // apply_fetch re-checks the source snapshot inside its write transaction:
        // a deleted or re-addressed source discards this response untouched.
        let previous: i64 =
            conn.query_row("SELECT COALESCE(MAX(id),0) FROM articles", [], |r| r.get(0))?;
        let result = apply_fetch_scheduled(conn, &feed, &resp, now, schedule)?;
        let ids = conn
            .prepare("SELECT id FROM articles WHERE feed_id=?1 AND id>?2")?
            .query_map(rusqlite::params![feed_id, previous], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        Ok((result, ids))
    })
    .await
    {
        Ok(applied) => Ok(applied),
        Err(err) => {
            tracing::warn!(
                feed_id = rrss_core::logsafe::unless_in_span("feed_id", feed_id),
                error = %crate::log::error(err.as_ref()),
                "feed response write failed"
            );
            Ok((
                refresh_failed(err_path, err_feed, err.to_string(), schedule).await?,
                Vec::new(),
            ))
        }
    }
}
async fn refresh_failed(
    path: PathBuf,
    feed: FeedRow,
    raw: String,
    schedule: FetchSchedule,
) -> Result<RefreshResult> {
    let message = rrss_core::feeds::humanize_refresh_error(&raw);
    let fallback = RefreshResult {
        feed_id: feed.id,
        status: RefreshStatus::Failed {
            message: message.clone(),
        },
    };
    // The snapshot check shares the write transaction with the health update, so an
    // old request cannot count against a source that was changed or deleted meanwhile.
    let feed_id = feed.id;
    let recorded = write_db(path, move |conn| {
        record_fetch_failure_scheduled(conn, &feed, &message, now_ms(), schedule)
    })
    .await;
    Ok(recorded.unwrap_or_else(|error| {
        tracing::warn!(
            feed_id = rrss_core::logsafe::unless_in_span("feed_id", feed_id),
            error = %crate::log::error(error.as_ref()),
            "feed health write failed"
        );
        fallback
    }))
}

async fn refresh_network_failure(
    state: Arc<runtime::Shared>,
    feed: FeedRow,
    raw: String,
    schedule: FetchSchedule,
) -> Result<Option<RefreshResult>> {
    let message = rrss_core::feeds::humanize_refresh_error(&raw);
    write_db(state.path.clone(), move |conn| {
        let pending = db::atomic_write(conn, |conn| {
            // Acquire the SQLite writer before this short state check. A probe
            // cannot mark offline between the check and the health/deadline
            // update; no network lock is held while waiting for SQLite.
            let network = state.connectivity.lock().unwrap();
            if network.paused() {
                return Ok(None);
            }
            let result = rrss_core::ingest::record_fetch_failure_pending(
                conn,
                &feed,
                &message,
                now_ms(),
                schedule,
            )?;
            drop(network);
            Ok(Some(result))
        })?;
        Ok(pending.map(|(result, event)| {
            if let Some(event) = event {
                event.log_committed();
            }
            result
        }))
    })
    .await
}

#[cfg(test)]
mod tests;
