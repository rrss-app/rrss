use super::{
    runtime::{Entry, Queue},
    *,
};
use std::collections::{BTreeMap, VecDeque};
use tokio::sync::{OwnedSemaphorePermit, watch};

const RECENT_LIMIT: usize = 32;
/// 刷新结果里点名的失败源个数；其余只计数。
const FAILED_FEEDS_SHOWN: usize = 3;

#[cfg(test)]
mod log_tests {
    use super::*;

    #[test]
    fn manual_summary_is_once_for_completion_skip_and_empty_operations() {
        let log = crate::scheduler::tests::capture_logs(|| {
            let make = |id| Operation {
                view: views::RefreshOperation {
                    id,
                    target: 1,
                    completed: 0,
                    inserted: 0,
                    failed: 0,
                    failed_feeds: Vec::new(),
                    deleted: 0,
                    skipped: 0,
                    done: false,
                    error: None,
                },
                remaining: [(1, Some(1))].into(),
                pending: VecDeque::new(),
                admission: None,
                started: tokio::time::Instant::now(),
            };
            let mut ops = Operations::default();
            ops.items.insert(1, make(1));
            let result = RefreshResult {
                feed_id: 1,
                status: RefreshStatus::NotModified,
            };
            assert!(ops.finish(&result, 1));
            assert!(!ops.finish(&result, 1));
            ops.items.insert(2, make(2));
            ops.skip_pending("canary-input");
            ops.stop();
            let mut empty = make(3);
            empty.view.target = 0;
            empty.remaining.clear();
            empty.complete();
            empty.complete();
        });
        assert_eq!(
            log.matches("manual feed refresh completed").count(),
            3,
            "{log}"
        );
        assert!(!log.contains("canary-input"));
    }
}

struct Operation {
    view: views::RefreshOperation,
    remaining: BTreeMap<i64, Option<u64>>,
    pending: VecDeque<i64>,
    admission: Option<OwnedSemaphorePermit>,
    started: tokio::time::Instant,
}

impl Operation {
    fn complete(&mut self) {
        if self.view.done {
            return;
        }
        self.view.done = true;
        self.admission.take();
        self.pending.clear();
        let view = &self.view;
        tracing::info!(
            operation_id = view.id,
            count = view.target,
            completed = view.completed,
            inserted = view.inserted,
            failed = view.failed,
            skipped = view.skipped,
            deleted = view.deleted,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "manual feed refresh completed"
        );
    }
}

#[derive(Default)]
pub(super) struct Operations {
    next: u64,
    pub version: u64,
    items: BTreeMap<u64, Operation>,
}

impl Operations {
    pub(super) fn pending_count(&self) -> usize {
        self.items
            .values()
            .map(|operation| {
                operation
                    .remaining
                    .values()
                    .filter(|generation| generation.is_none())
                    .count()
            })
            .sum()
    }
    pub fn attach(&mut self, feed: i64, generation: u64) {
        for operation in self.items.values_mut() {
            if let Some(target @ None) = operation.remaining.get_mut(&feed) {
                *target = Some(generation);
            }
        }
    }

    pub fn finish(&mut self, result: &RefreshResult, generation: u64) -> bool {
        let mut completed_operation = false;
        for operation in self.items.values_mut() {
            if operation.remaining.get(&result.feed_id) != Some(&Some(generation)) {
                continue;
            }
            operation.remaining.remove(&result.feed_id);
            operation.view.completed += 1;
            match &result.status {
                RefreshStatus::Updated { inserted, .. } => {
                    operation.view.inserted += *inserted as i64
                }
                RefreshStatus::Failed { .. } => {
                    operation.view.failed += 1;
                    if operation.view.failed_feeds.len() < FAILED_FEEDS_SHOWN {
                        operation.view.failed_feeds.push(result.feed_id);
                    }
                }
                RefreshStatus::NotModified => {}
                RefreshStatus::Deleted => operation.view.deleted += 1,
                RefreshStatus::Skipped { message, .. } => {
                    operation.view.skipped += 1;
                    operation.view.error.get_or_insert_with(|| message.clone());
                }
            }
            if operation.remaining.is_empty() {
                operation.complete();
                completed_operation = true;
            }
        }
        self.version += 1;
        self.trim();
        completed_operation
    }

    fn trim(&mut self) {
        let finished: Vec<_> = self
            .items
            .iter()
            .filter(|(_, op)| op.view.done)
            .map(|(id, _)| *id)
            .collect();
        for id in finished
            .iter()
            .take(finished.len().saturating_sub(RECENT_LIMIT))
        {
            self.items.remove(id);
        }
    }

    pub fn stop(&mut self) {
        self.skip_pending("后台运行时已停止，部分订阅未完成");
    }

    pub fn skip_pending(&mut self, message: &str) -> bool {
        let mut completed = false;
        for operation in self.items.values_mut().filter(|op| !op.view.done) {
            completed = true;
            operation.view.skipped += operation.remaining.len();
            operation.view.completed = operation.view.target;
            operation.view.error = Some(message.into());
            operation.remaining.clear();
            operation.complete();
        }
        self.version += 1;
        self.trim();
        completed
    }
}

impl Queue {
    pub(super) fn fill_operations(&mut self, limit: usize) {
        if self.stopped {
            return;
        }
        for operation in self.operations.items.values_mut() {
            while let Some(id) = operation.pending.front().copied() {
                if operation.remaining.get(&id) != Some(&None) {
                    operation.pending.pop_front();
                    continue;
                }
                if let Some(entry) = self.entries.get(&id) {
                    operation.remaining.insert(id, Some(entry.generation));
                } else {
                    if self.entries.len() >= limit {
                        break;
                    }
                    self.generation += 1;
                    let (slot, _) = watch::channel(None);
                    self.entries.insert(
                        id,
                        Entry {
                            enqueued: tokio::time::Instant::now(),
                            dispatched: false,
                            generation: self.generation,
                            slot,
                            added: false,
                            keep_due: false,
                            parked: None,
                            resuming: None,
                        },
                    );
                    operation.remaining.insert(id, Some(self.generation));
                    self.pending.push_back(id);
                }
                operation.pending.pop_front();
            }
        }
        // Attach later operations to all work now admitted, even if their cursor
        // has not reached that feed. A fast completion cannot escape their snapshot.
        for (id, entry) in &self.entries {
            self.operations.attach(*id, entry.generation);
        }
    }
}

impl Scheduler {
    pub async fn start_refresh(&self) -> Result<views::RefreshStarted> {
        let started = tokio::time::Instant::now();
        anyhow::ensure!(!*self.shared.stop.borrow(), "后台运行时已停止");
        let online = self.shared.check_network(true).await;
        self.shared.await_cooldown_ready().await?;
        let admission = self
            .shared
            .refresh_admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| anyhow::anyhow!("刷新操作过多，请等待已有刷新完成"))?;
        // One consistent SQLite read snapshot, paged within the blocking worker.
        // Only IDs survive the query; no response bodies or per-feed Tokio tasks.
        let state = self.shared.clone();
        let (targets, skipped, error) = crate::jobs::spawn_db(self.shared.path.clone(), move |conn| {
            let tx = conn.unchecked_transaction()?;
            let mut ids = Vec::new();
            let mut skipped = 0;
            let mut error = None;
            let cooling = state.cooldowns.lock().unwrap().clone();
            let now = state.clock.now();
            let mut cursor = 0;
            loop {
                let page = tx
                    .prepare(
                        "SELECT id,url FROM feeds WHERE id>?1 AND kind!='manual' ORDER BY id LIMIT 256",
                    )?
                    .query_map([cursor], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                if page.is_empty() {
                    break;
                }
                cursor = page.last().unwrap().0;
                for (id, url) in page {
                    let reason = if !online {
                        Some(super::offline::UNAVAILABLE.to_string())
                    } else {
                        cooling.blocked_source(id, &url, now)
                            .or_else(|| cooling.recovery(now))
                            .map(|deferred| deferred.message)
                    };
                    if let Some(reason) = reason {
                        skipped += 1;
                        error.get_or_insert(reason);
                    } else {
                        ids.push(id);
                    }
                }
            }
            tx.commit()?;
            Ok((ids, skipped, error))
        })
        .await?;
        let (id, background) = {
            let mut queue = self.shared.queue.lock().unwrap();
            anyhow::ensure!(
                !queue.stopped && !*self.shared.stop.borrow(),
                "后台运行时已停止"
            );
            queue.operations.next += 1;
            let id = queue.operations.next;
            let done = targets.is_empty();
            let remaining = targets
                .iter()
                .map(|feed| (*feed, queue.entries.get(feed).map(|entry| entry.generation)))
                .collect();
            let view = views::RefreshOperation {
                id,
                target: targets.len() + skipped,
                completed: skipped,
                inserted: 0,
                failed: 0,
                failed_feeds: Vec::new(),
                deleted: 0,
                skipped,
                done: false,
                error,
            };
            for feed in &targets {
                queue.wake_parked(*feed);
            }
            let mut operation = Operation {
                view,
                remaining,
                pending: targets.into(),
                admission: (!done).then_some(admission),
                started,
            };
            if done {
                operation.complete();
            }
            queue.operations.items.insert(id, operation);
            queue.operations.version += 1;
            queue.operations.trim();
            let background = if online {
                queue.fill_operations(self.shared.limits.pending);
                self.shared.metrics.admitted.set(queue.entries.len());
                done
            } else {
                queue.operations.skip_pending(super::offline::UNAVAILABLE);
                true
            };
            (id, background)
        };
        // Preserve the manual AI trigger even when every feed was skipped or
        // the snapshot was empty; model work is independent of RSS connectivity.
        if background {
            self.request_background()?;
        }
        self.shared.wake.notify_one();
        self.shared.invalidate();
        Ok(views::RefreshStarted {
            run_id: self.shared.run_id.clone(),
            operation_id: id,
        })
    }

    pub fn snapshot(&self) -> views::SchedulerSnapshot {
        self.shared.snapshot()
    }

    pub fn set_observer(
        &self,
        observer: impl Fn(views::SchedulerSnapshot) + Send + Sync + 'static,
    ) {
        let _ = self.shared.observer.set(Arc::new(observer));
    }
}

impl super::runtime::Shared {
    pub(super) fn snapshot(&self) -> views::SchedulerSnapshot {
        let queue = self.queue.lock().unwrap();
        views::SchedulerSnapshot {
            run_id: self.run_id.clone(),
            version: self.revision.load(std::sync::atomic::Ordering::Relaxed),
            operations: queue
                .operations
                .items
                .values()
                .map(|operation| operation.view.clone())
                .collect(),
            background_requested: *self.dirty.borrow(),
            background_completed: *self.completed.borrow(),
            stopped: *self.stop.borrow(),
            background: self.background_status.borrow().clone(),
            feed_pause: self
                .connectivity
                .lock()
                .unwrap()
                .reason()
                .or_else(|| self.cooldowns.lock().unwrap().reason()),
        }
    }

    pub(super) fn notify_status(&self) {
        self.revision
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.events.notify_one();
    }
}
