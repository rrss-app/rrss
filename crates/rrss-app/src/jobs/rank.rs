//! 焦点排序后台作业：入库增量、行为防抖与每小时刷新，不调用模型。

use super::spawn_db;
use rrss_core::db;
use rrss_focus::store::{self, REFRESHED_KEY};
use rusqlite::Connection;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

const DIRTY_KEY: &str = "rank_dirty_at";
const DEBOUNCE_MS: i64 = 30_000;
const HOURLY_MS: i64 = 3_600_000;

/// 成功写入反馈 / 行为后记录请求；持久化使重启期间的请求不会丢失。
pub(crate) fn request(conn: &Connection) -> rrss_core::Result<()> {
    db::set_setting(conn, DIRTY_KEY, &rrss_core::feeds::now_ms().to_string())
}

/// 增量排序的合并批次。排序要加载全库模型：逐源各算一次会让大量模型加载
/// 同时抢 CPU，拖慢持写锁的事务，因此同一时刻只算一批。
pub(crate) struct Batches {
    pending: Mutex<Batch>,
    turn: Arc<tokio::sync::Mutex<()>>,
}

impl Batches {
    pub(crate) fn new(turn: Arc<tokio::sync::Mutex<()>>) -> Self {
        Self {
            pending: Mutex::default(),
            turn,
        }
    }

    /// 测试用：占住轮次，模拟一次很慢的增量排序。
    #[cfg(test)]
    pub(crate) async fn hold(&self) -> tokio::sync::OwnedMutexGuard<()> {
        self.turn.clone().lock_owned().await
    }

    /// Return only after these articles have been written, or their batch failed.
    pub(crate) async fn incremental(&self, path: &Path, ids: Vec<i64>) -> anyhow::Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let done = {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            pending.ids.extend(ids);
            pending.done[0].clone()
        };
        let turn = Arc::new(self.turn.clone().lock_owned().await);
        // Another caller may already have completed this caller's batch.
        if let Some(result) = done.get() {
            return result.clone().map_err(anyhow::Error::new);
        }
        let batch = std::mem::take(&mut *self.pending.lock().unwrap_or_else(|e| e.into_inner()));
        let ids: Vec<_> = batch.ids.iter().copied().collect();
        let unfinished = Unfinished {
            batches: self,
            batch: Some(batch),
        };
        // Cancellation cannot stop a submitted blocking worker. Both the worker
        // and the result publisher retain the turn until their work is complete.
        let worker_turn = turn.clone();
        let result = spawn_db(path.to_owned(), move |conn| {
            let _turn = worker_turn;
            store::update(conn, &ids, rrss_core::feeds::now_ms())
        })
        .await;
        unfinished.finish(
            result
                .as_ref()
                .map(|_| ())
                .map_err(crate::UiError::from_anyhow_ref),
        );
        result.map(|_| ())
    }
}

/// 返回时这些文章已写入排序缓存（或所在批次失败）。
pub async fn incremental(path: &Path, ids: Vec<i64>) -> anyhow::Result<()> {
    let work = crate::scheduler::BackgroundWork::for_database(path);
    work.scope(work.rank.incremental(path, ids)).await
}

struct Batch {
    ids: BTreeSet<i64>,
    // Retried batches carry their original result slots along with their IDs.
    done: Vec<Arc<OnceLock<Result<(), crate::UiError>>>>,
}

impl Default for Batch {
    fn default() -> Self {
        Self {
            ids: BTreeSet::new(),
            done: vec![Arc::default()],
        }
    }
}

/// 计算中途被取消（如退出）时把这批文章放回待算批次，由下一个轮到的调用者补算。
struct Unfinished<'a> {
    batches: &'a Batches,
    batch: Option<Batch>,
}

impl Unfinished<'_> {
    fn finish(mut self, result: Result<(), crate::UiError>) {
        if let Some(batch) = self.batch.take() {
            for done in batch.done {
                let _ = done.set(result.clone());
            }
        }
    }
}

impl Drop for Unfinished<'_> {
    fn drop(&mut self) {
        if let Some(batch) = self.batch.take() {
            let mut pending = self
                .batches
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            pending.ids.extend(batch.ids);
            pending.done.extend(batch.done);
        }
    }
}

/// Repair only committed articles missed by incremental ranking. This is local
/// recovery and must never schedule a model batch.
pub(crate) async fn repair_missing(path: &Path) -> anyhow::Result<()> {
    spawn_db(path.to_owned(), |conn| {
        let ids = conn.prepare("SELECT a.id FROM articles a LEFT JOIN article_rank r ON r.article_id=a.id WHERE r.article_id IS NULL LIMIT 256")?
            .query_map([], |r| r.get(0))?.collect::<rusqlite::Result<Vec<i64>>>()?;
        if !ids.is_empty() { store::update(conn, &ids, rrss_core::feeds::now_ms())?; }
        Ok(())
    }).await
}

fn due(now: i64, refreshed: Option<i64>, dirty: Option<i64>) -> bool {
    refreshed.is_none_or(|at| now.saturating_sub(at) >= HOURLY_MS)
        || dirty.is_some_and(|at| now.saturating_sub(at) >= DEBOUNCE_MS)
}

fn run_due(conn: &Connection, now: i64) -> rrss_core::Result<bool> {
    let read = |key| {
        Ok::<_, rrss_core::Error>(db::get_setting(conn, key)?.and_then(|v| v.parse::<i64>().ok()))
    };
    let outdated: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM article_rank WHERE algo_version<>?1)",
        [rrss_focus::combine::ALGO_VERSION],
        |r| r.get(0),
    )?;
    if !outdated && !due(now, read(REFRESHED_KEY)?, read(DIRTY_KEY)?) {
        return Ok(false);
    }
    // 模型在读快照里算，写锁只包住写回与标记；两者同一事务提交。
    store::refresh(conn, None, now, |tx| {
        // 排队期间可能收到更晚的行为；本次 cutoff 之外的请求留给下一轮。
        tx.execute(
            "DELETE FROM settings WHERE key=?1 AND CAST(value AS INTEGER)<=?2",
            rusqlite::params![DIRTY_KEY, now],
        )?;
        db::set_setting(tx, REFRESHED_KEY, &now.to_string())
    })?;
    Ok(true)
}

/// 独立于网络 / 摘要链；5 秒轮询，防抖重算在最后写入后 30–35 秒执行。
pub async fn run_loop(path: PathBuf) {
    let capacity = crate::scheduler::model_capacity::Capacity::for_database(&path);
    loop {
        let permit = capacity.local.lock().await;
        if let Err(error) = spawn_db(path.clone(), |conn| {
            run_due(conn, rrss_core::feeds::now_ms())
        })
        .await
        {
            tracing::warn!(%error,"rank refresh failed");
        }
        drop(permit);
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{future::Future, pin::Pin, task::Poll};

    fn fixture() -> (tempfile::TempDir, PathBuf, db::PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rank.db");
        let conn = db::open_at(&path).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://rank.example/feed',0);
             INSERT INTO articles(id,feed_id,title,created_at)
             VALUES(1,1,'First story',0),(2,1,'Second story',0),(3,1,'Third story',0);",
        )
        .unwrap();
        (dir, path, conn)
    }

    async fn pending<F: Future>(mut future: Pin<&mut F>) {
        std::future::poll_fn(|cx| {
            assert!(future.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }

    fn ranked(conn: &Connection) -> Vec<i64> {
        conn.prepare("SELECT article_id FROM article_rank ORDER BY article_id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[tokio::test]
    async fn cancelling_a_queued_waiter_keeps_its_articles_in_the_shared_batch() {
        let (_dir, path, conn) = fixture();
        let work = crate::scheduler::BackgroundWork::for_database(&path);
        let held = work.rank.hold().await;
        let mut first = Box::pin(incremental(&path, vec![1]));
        let mut second = Box::pin(incremental(&path, vec![2]));
        pending(first.as_mut()).await;
        pending(second.as_mut()).await;
        drop(first);
        assert!(ranked(&conn).is_empty());
        drop(held);
        second.await.unwrap();
        assert_eq!(ranked(&conn), vec![1, 2]);
        assert_eq!(*work.active().borrow(), 0);
    }

    #[tokio::test]
    async fn cancelled_executor_preserves_all_waiters_through_failure_and_repair() {
        let (_dir, path, conn) = fixture();
        conn.execute_batch("CREATE TRIGGER fail_rank BEFORE INSERT ON article_rank BEGIN SELECT RAISE(ABORT,'injected rank'); END;").unwrap();
        let work = crate::scheduler::BackgroundWork::for_database(&path);
        let held = work.rank.hold().await;
        let permits = work
            .db
            .permits
            .clone()
            .acquire_many_owned(db::pool::BACKGROUND_CONNECTIONS as u32)
            .await
            .unwrap();
        let mut first = Box::pin(incremental(&path, vec![1]));
        let mut second = Box::pin(incremental(&path, vec![2]));
        let mut third = Box::pin(incremental(&path, vec![3]));
        pending(first.as_mut()).await;
        pending(second.as_mut()).await;
        pending(third.as_mut()).await;
        drop(held);
        pending(first.as_mut()).await;
        // The executor has taken the batch but cannot submit database work yet.
        assert!(work.rank.pending.lock().unwrap().ids.is_empty());
        drop(first);
        drop(permits);
        let error = second.await.unwrap_err();
        assert!(ranked(&conn).is_empty());
        conn.execute_batch("DROP TRIGGER fail_rank;").unwrap();
        // This waiter must see its own batch's failure even after the DB is fixed.
        let other_error = third.await.unwrap_err();
        assert_eq!(
            serde_json::to_value(crate::UiError::from_anyhow_ref(&error)).unwrap(),
            serde_json::to_value(crate::UiError::from_anyhow_ref(&other_error)).unwrap()
        );
        repair_missing(&path).await.unwrap();
        assert_eq!(ranked(&conn), vec![1, 2, 3]);
        assert_eq!(*work.active().borrow(), 0);
    }

    #[tokio::test]
    async fn articles_arriving_during_execution_wait_for_the_next_batch() {
        let (_dir, path, conn) = fixture();
        let work = crate::scheduler::BackgroundWork::for_database(&path);
        let permits = work
            .db
            .permits
            .clone()
            .acquire_many_owned(db::pool::BACKGROUND_CONNECTIONS as u32)
            .await
            .unwrap();
        let mut first = Box::pin(incremental(&path, vec![1]));
        pending(first.as_mut()).await;
        let mut second = Box::pin(incremental(&path, vec![2]));
        pending(second.as_mut()).await;
        drop(permits);
        first.await.unwrap();
        assert_eq!(ranked(&conn), vec![1]);
        second.await.unwrap();
        assert_eq!(ranked(&conn), vec![1, 2]);
    }

    #[tokio::test]
    async fn cancelled_submitted_worker_keeps_the_turn_until_its_transaction_ends() {
        let (_dir, path, conn) = fixture();
        let work = crate::scheduler::BackgroundWork::for_database(&path);
        let writer = db::write_transaction(&conn).unwrap();
        let mut first = Box::pin(incremental(&path, vec![1]));
        pending(first.as_mut()).await;
        let metrics = db::pool::metrics(&path).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !metrics
                .activity
                .snapshot()
                .active
                .iter()
                .any(|sample| sample.stage == "writer_admission")
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(first);
        assert!(work.rank.turn.try_lock().is_err());
        assert_eq!(*work.active().borrow(), 1);
        let mut second = Box::pin(incremental(&path, vec![2]));
        pending(second.as_mut()).await;
        assert!(ranked(&conn).is_empty());
        writer.commit().unwrap();
        second.await.unwrap();
        assert_eq!(ranked(&conn), vec![1, 2]);
        assert_eq!(*work.active().borrow(), 0);
    }

    #[tokio::test]
    async fn startup_incremental_debounce_and_hourly_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rank.db");
        let conn = db::open_at(&path).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://rank.invalid/feed',0);
            INSERT INTO articles(id,feed_id,title,created_at) VALUES(1,1,'one',0);",
        )
        .unwrap();
        assert!(run_due(&conn, 100).unwrap());
        conn.execute(
            "INSERT INTO articles(id,feed_id,title,created_at) VALUES(2,1,'two',0)",
            [],
        )
        .unwrap();
        incremental(&path, vec![2]).await.unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM article_rank", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
        db::set_setting(&conn, DIRTY_KEY, "101").unwrap();
        assert!(!run_due(&conn, 30_100).unwrap());
        db::set_setting(&conn, DIRTY_KEY, "1000").unwrap(); // 新动作延后期限。
        assert!(!run_due(&conn, 30_101).unwrap());
        assert!(run_due(&conn, 31_000).unwrap());
        assert!(!run_due(&conn, 31_001).unwrap());
        assert!(run_due(&conn, 31_000 + HOURLY_MS).unwrap());
        conn.execute("UPDATE article_rank SET algo_version=0", [])
            .unwrap();
        assert!(run_due(&conn, 31_001 + HOURLY_MS).unwrap());
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_incremental_calls_return_only_after_their_articles_are_ranked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rank.db");
        let conn = db::open_at(&path).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://rank.invalid/feed',0);
            WITH RECURSIVE n(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM n WHERE id<40)
            INSERT INTO articles(id,feed_id,title,created_at) SELECT id,1,'t'||id,0 FROM n;",
        )
        .unwrap();
        let calls = (1..=40).map(|id| {
            let path = path.clone();
            tokio::spawn(async move {
                incremental(&path, vec![id]).await.unwrap();
                let conn = db::open_at(&path).unwrap();
                conn.query_row(
                    "SELECT COUNT(*) FROM article_rank WHERE article_id=?1",
                    [id],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap()
            })
        });
        for call in calls.collect::<Vec<_>>() {
            assert_eq!(call.await.unwrap(), 1);
        }
    }
}
