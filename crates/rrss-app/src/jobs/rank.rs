//! 阶段 23「接入方式 · 排序作业」：入库增量、行为防抖与每小时刷新，不调用模型。

use super::spawn_db;
use rrss_core::{
    db,
    rank::store::{self, REFRESHED_KEY},
};
use rusqlite::Connection;
use std::{
    path::{Path, PathBuf},
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
#[derive(Default)]
pub(crate) struct Batches {
    pending: std::sync::Mutex<Batch>,
    turn: std::sync::Arc<tokio::sync::Mutex<()>>,
}

#[cfg(test)]
impl Batches {
    /// 测试用：占住轮次，模拟一次很慢的增量排序。
    pub(crate) async fn hold(&self) -> tokio::sync::OwnedMutexGuard<()> {
        self.turn.clone().lock_owned().await
    }
}

#[derive(Default)]
struct Batch {
    ids: std::collections::BTreeSet<i64>,
    done: std::sync::Arc<std::sync::OnceLock<Result<(), String>>>,
}

/// 返回时这些文章已写入排序缓存（或所在批次失败）。
pub async fn incremental(path: &Path, ids: Vec<i64>) -> anyhow::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let capacity = crate::scheduler::model_capacity::Capacity::for_database(path);
    let batches = &capacity.rank;
    let done = {
        let mut pending = batches.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.ids.extend(ids);
        pending.done.clone()
    };
    let _turn = batches.turn.lock().await;
    // 前一个轮到的调用者已把本批一起算完。
    if let Some(result) = done.get() {
        return result.clone().map_err(anyhow::Error::msg);
    }
    // 没算过的批次只能是当前待算批次：取批与计算都在持有 turn 时完成。
    let batch = std::mem::take(&mut *batches.pending.lock().unwrap_or_else(|e| e.into_inner()));
    let ids: Vec<_> = batch.ids.iter().copied().collect();
    let unfinished = Unfinished {
        batches,
        batch: Some(batch),
    };
    let result = spawn_db(path.to_owned(), move |conn| {
        store::update(conn, &ids, rrss_core::feeds::now_ms())
    })
    .await;
    unfinished.finish(result.as_ref().map(|_| ()).map_err(|e| format!("{e:#}")));
    result.map(|_| ())
}

/// 计算中途被取消（如退出）时把这批文章放回待算批次，由下一个轮到的调用者补算。
struct Unfinished<'a> {
    batches: &'a Batches,
    batch: Option<Batch>,
}

impl Unfinished<'_> {
    fn finish(mut self, result: Result<(), String>) {
        if let Some(batch) = self.batch.take() {
            let _ = batch.done.set(result);
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
        [rrss_core::rank::combine::ALGO_VERSION],
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
