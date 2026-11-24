//! Database-scoped, bounded feed runtime and background coordination.
use crate::{network::NetworkClients, views};
use anyhow::Result;
use rrss_core::feeds::now_ms;
use rrss_core::ingest::{FetchRequest, FetchResponse, FetchSchedule, RefreshResult, RefreshStatus};
use rrss_core::{FeedRow, db};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub(crate) mod background;
mod cooldown;
mod diagnostics;
mod events;
pub(crate) use events::Subscribers;
pub use events::{SchedulerEvent, Subscription};
pub(crate) mod digest_jobs;
pub(crate) mod model_capacity;
mod model_cooldown;
mod offline;
mod operations;
mod resources;
mod response;
mod runtime;
pub(crate) use resources::BackgroundWork;
pub use runtime::{FetchTicket, Scheduler};
const DB_WRITE_ATTEMPTS: u32 = 4;

/// Retry only repeatable database work after the previous transaction has ended.
/// Requests, state publication and events belong outside this retry boundary.
async fn write_db<T, F>(path: PathBuf, f: F) -> Result<T>
where
    T: Send + 'static,
    F: Fn(&rusqlite::Connection) -> rrss_core::Result<T> + Send + Sync + 'static,
{
    let f = Arc::new(f);
    let background = BackgroundWork::for_database(&path);
    let _work = background.track();
    let mut attempt = 0;
    loop {
        let path = path.clone();
        let f = Arc::clone(&f);
        let turn = background.db.feed_writes.clone().lock_owned().await;
        match crate::jobs::spawn_db(path, move |conn| {
            let _turn = turn;
            f(conn)
        })
        .await
        {
            Ok(value) => return Ok(value),
            Err(err)
                if err
                    .downcast_ref::<rrss_core::Error>()
                    .is_some_and(db::is_retryable_lock)
                    && attempt + 1 < DB_WRITE_ATTEMPTS =>
            {
                tracing::warn!(attempt, error = %crate::log::error(err.as_ref()), "refresh write busy; retrying");
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(200 * u64::from(attempt))).await;
            }
            Err(err) => return Err(err),
        }
    }
}

impl Scheduler {
    pub async fn add_feed(&self, url: &str) -> Result<views::AddFeedResult> {
        let url = url.trim().to_string();
        let intended_url = url.clone();
        let id = self
            .shared
            .work
            .scope(crate::jobs::spawn_db(
                self.shared.path.clone(),
                move |conn| crate::actions::add_feed(conn, &url),
            ))
            .await?;
        let deferred = self.shared.deferred(id, &intended_url);
        let notice = match self.submit_added(id) {
            Ok(_) if self.shared.connectivity.lock().unwrap().unavailable() => {
                views::AddFeedNotice::Offline {}
            }
            Ok(_) if let Some(deferred) = deferred => views::AddFeedNotice::Deferred {
                error: deferred.ui(),
            },
            Ok(_) => views::AddFeedNotice::Fetching {},
            Err(error) => views::AddFeedNotice::NotScheduled {
                error: views::UiError::from(error),
            },
        };
        Ok(views::AddFeedResult { id, notice })
    }

    /// Enable automatic feed polling and ranking. Workers hold only the run's internal state.
    pub fn start_polling(&self) {
        self.start_polling_with_interval(Duration::from_secs(15 * 60));
    }

    fn start_polling_with_interval(&self, max_wait: Duration) {
        let stopped = self.shared.stop.borrow();
        if *stopped
            || !self.shared.automatic.send_if_modified(|enabled| {
                if *enabled {
                    return false;
                }
                *enabled = true;
                true
            })
        {
            return;
        }
        let shared = self.shared.clone();
        tokio::spawn(
            self.shared
                .work
                .scope(async move { shared.run_loop_with_interval(max_wait).await }),
        );
    }

    /// Fill each feed's rate-limit end from the in-memory cooldowns.
    pub fn annotate_limits(&self, tree: &mut views::FeedTree) {
        let feeds = tree.uncategorized.iter_mut().chain(
            tree.folders
                .iter_mut()
                .flat_map(|folder| folder.feeds.iter_mut()),
        );
        for feed in feeds {
            feed.limited_until = self.shared.limited_until(feed.id, &feed.url);
        }
    }

    /// Manual retry of one source: bypasses local slow-failure backoff but not
    /// server cooldowns, and returns once its result is recorded.
    pub async fn refresh_feed(&self, id: i64) -> Result<()> {
        self.submit(id)?.wait().await?;
        Ok(())
    }

    /// Refresh intervals or deadlines changed outside a fetch; rescan now
    /// instead of waiting for the previously computed wake.
    pub fn reschedule(&self) {
        self.shared.due_changed.notify_one();
    }
}

impl runtime::Shared {
    /// Page due IDs into bounded admission. The producer retains no per-source
    /// result or waiter after admission; completion belongs to the runtime.
    async fn enqueue_due(&self) -> Result<usize> {
        let max_id: i64 = crate::jobs::spawn_db(self.path.clone(), |conn| {
            Ok(conn.query_row("SELECT COALESCE(MAX(id),0) FROM feeds", [], |r| r.get(0))?)
        })
        .await?;
        let now = self.clock.now().utc;
        let mut cursor = (i64::MIN, 0);
        let mut admitted = 0;
        loop {
            let ids = crate::jobs::spawn_db(self.path.clone(), move |conn| {
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

    /// Sleep until the earliest persisted deadline, bounded by `max_wait`. While
    /// a due feed is still in flight or parked, its next deadline is unknown,
    /// so recheck after a short pause instead of the full bound.
    async fn next_wait(&self, max_wait: Duration) -> Duration {
        const IN_FLIGHT_RECHECK: Duration = Duration::from_secs(60);
        const MIN_WAIT: Duration = Duration::from_secs(1);
        let earliest = crate::jobs::spawn_db(self.path.clone(), |conn| {
            rrss_core::fetch_interval::earliest_due(conn)
        })
        .await;
        let now = self.clock.now().utc;
        match earliest {
            Ok(Some(at)) if at <= now => IN_FLIGHT_RECHECK,
            Ok(Some(at)) => Duration::from_millis((at - now) as u64).max(MIN_WAIT),
            Ok(None) => max_wait,
            Err(error) => {
                tracing::warn!(%error, "reading next feed deadline failed");
                max_wait
            }
        }
        .min(max_wait)
    }

    async fn run_loop_with_interval(&self, max_wait: Duration) {
        let mut stop = self.stop.subscribe();
        tokio::select! {
            biased;
            _ = stop.wait_for(|stopped| *stopped) => {},
            _ = crate::jobs::rank::run_loop(self.path.clone()) => {},
            _ = async {
                loop {
                    if let Err(error) = self.enqueue_due().await { tracing::warn!(%error, "scheduled refresh failed"); }
                    let wait = self.next_wait(max_wait).await;
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {},
                        _ = self.due_changed.notified() => {},
                    }
                }
            } => {},
        }
    }
}

#[cfg(test)]
mod tests;
