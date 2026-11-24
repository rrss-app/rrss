//! Classify a downloaded response and persist its source outcome. Queue
//! transitions and notifications belong to the runtime after this returns.

use super::{
    cooldown::Deferred,
    runtime::{ResponseLease, Shared},
    write_db,
};
use anyhow::Result;
use rrss_core::{
    FeedRow, db,
    feeds::now_ms,
    ingest::{
        FetchResponse, FetchSchedule, RefreshResult, RefreshStatus, apply_fetch_scheduled,
        feed_http::FetchFailure, record_fetch_failure_scheduled,
    },
};
use std::{path::PathBuf, sync::Arc};

pub(super) enum Outcome {
    Retry,
    Deferred(Deferred),
    Complete {
        result: RefreshResult,
        inserted_ids: Vec<i64>,
    },
}

pub(super) async fn record(
    state: &Arc<Shared>,
    feed: FeedRow,
    response: std::result::Result<FetchResponse, FetchFailure>,
    lease: Arc<ResponseLease>,
) -> Result<Outcome> {
    let schedule = state.fetch_schedule(&feed);
    let result = match response {
        Ok(response) => {
            let deferred = matches!(response.status, 429 | 503)
                .then(|| state.deferred(feed.id, &feed.url))
                .flatten();
            let (result, inserted_ids) = apply_response(
                state.path.clone(),
                feed,
                response,
                schedule,
                state.clock.now().utc,
                deferred,
                lease,
            )
            .await?;
            return Ok(Outcome::Complete {
                result,
                inserted_ids,
            });
        }
        Err(FetchFailure {
            error: rrss_core::Error::FetchPaused,
            ..
        }) => return Ok(Outcome::Retry),
        Err(FetchFailure {
            error: rrss_core::Error::FetchDeferred { until, reason },
            ..
        }) => {
            let deferred = state
                .deferred(feed.id, &feed.url)
                .unwrap_or_else(|| Deferred::from_reason(until, reason));
            return Ok(Outcome::Deferred(deferred));
        }
        Err(error) if error.network => {
            match refresh_network_failure(state.clone(), feed, error, schedule).await? {
                Some(result) => result,
                None => return Ok(Outcome::Retry),
            }
        }
        Err(error) => {
            let schedule = schedule
                .with_failure(error.class)
                .with_error(error.kind, error.status());
            refresh_failed(
                state.path.clone(),
                feed,
                rrss_core::feeds::refresh_error_json(&error.error),
                schedule,
            )
            .await?
        }
    };
    Ok(Outcome::Complete {
        result,
        inserted_ids: Vec::new(),
    })
}

/// 应用一次响应并返回本次新入库的文章 ID。增量排序不在这里等：由调用方交给
/// 后台串行合并，解析工作者与源事件不等待排序完成。
async fn apply_response(
    path: PathBuf,
    feed: FeedRow,
    resp: FetchResponse,
    schedule: FetchSchedule,
    now: i64,
    deferred: Option<Deferred>,
    lease: Arc<ResponseLease>,
) -> Result<(RefreshResult, Vec<i64>)> {
    let feed_id = feed.id;
    let err_feed = feed.clone();
    let err_path = path.clone();
    match write_db(path, move |conn| {
        // A cancelled async caller cannot release bytes still held by this worker.
        let _lease = &lease;
        // apply_fetch re-checks the source snapshot inside its write transaction:
        // a deleted or re-addressed source discards this response untouched.
        let previous: i64 =
            conn.query_row("SELECT COALESCE(MAX(id),0) FROM articles", [], |r| r.get(0))?;
        let result = if matches!(resp.status, 429 | 503) {
            rrss_core::ingest::record_http_failure(
                conn,
                &feed,
                resp.status,
                now,
                schedule,
                deferred
                    .as_ref()
                    .map(|deferred| (deferred.until, deferred.reason)),
            )?
        } else {
            apply_fetch_scheduled(conn, &feed, &resp, now, schedule)?
        };
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
                refresh_failed(
                    err_path,
                    err_feed,
                    crate::ui_error::feeds::error_record(crate::views::UiError::from(err)),
                    schedule,
                )
                .await?,
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
    let message = raw;
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
    state: Arc<Shared>,
    feed: FeedRow,
    failure: FetchFailure,
    schedule: FetchSchedule,
) -> Result<Option<RefreshResult>> {
    let schedule = schedule
        .with_failure(failure.class)
        .with_error(failure.kind, failure.status());
    let message = rrss_core::feeds::refresh_error_json(&failure.error);
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
