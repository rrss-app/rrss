//! 关注短摘要、Digest、嵌入队列。失败交给调度器与界面；日志按 `rrss_app::jobs` 前缀过滤各子模块。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rrss_core::{Error as CoreError, db};
use tracing::Instrument;

pub mod cluster;
pub mod cluster_summary;
pub mod complete;
pub mod digests;
pub mod embeddings;
pub mod feed_icons;
pub mod media;
pub mod memories;
pub mod rank;
pub mod retrieval;
pub(crate) mod retry;
pub mod summaries;
pub mod vector_space;

pub use cluster::*;
pub use cluster_summary::*;
pub use digests::*;
pub use embeddings::*;
pub use memories::*;
pub use retrieval::*;
pub use summaries::*;

pub(crate) use rrss_core::ai_store::FOCUS_BATCH;
pub(crate) const DIGEST_ITEMS_DAILY: i64 = 40;
pub(crate) const DIGEST_ITEMS_WEEKLY: i64 = 60;
pub(crate) const DIGEST_PICK_MAX_DAILY: usize = 12;
pub(crate) const DIGEST_PICK_MAX_WEEKLY: usize = 12;
pub(crate) const DIGEST_PICK_MIN: usize = 6;
pub(crate) const EMBED_BATCH: i64 = 16;
pub(crate) const RAG_HITS: i64 = 6;

/// AI 缓存只留最近 30 天；本地整理，不调模型。
const AI_CACHE_KEEP_MS: i64 = 30 * 86_400_000;

pub(crate) async fn run_housekeeping(db_path: &Path) -> Result<()> {
    let now = rrss_core::feeds::now_ms();
    let cutoff = now - AI_CACHE_KEEP_MS;
    let n = spawn_db(db_path.to_path_buf(), move |conn| {
        rrss_core::storage::prune_ai_cache(conn, cutoff)
    })
    .await?;
    if n > 0 {
        tracing::info!(n, "pruned ai cache");
    }
    // 定时备份（默认关）：到期才生成，只留最近几份自动备份。
    let data_dir = db_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let path = spawn_db(db_path.to_path_buf(), move |conn| {
        crate::actions::run_scheduled_backup(conn, &data_dir, now)
    })
    .await?;
    if let Some(path) = path {
        tracing::info!(file = %rrss_core::logsafe::path(&path), "scheduled backup written");
    }
    // 保留清理（默认关）：一天一次，第一次前自动备份，每批一个写事务。
    let data_dir = db_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let (pruned, backed_up) = spawn_db(db_path.to_path_buf(), move |conn| {
        crate::actions::run_retention(conn, &data_dir, now, false)
    })
    .await?;
    if pruned > 0 {
        tracing::info!(pruned, backed_up, "pruned old article bodies");
    }
    Ok(())
}

pub(crate) fn is_skip(err: &anyhow::Error) -> bool {
    if err.chain().any(|error| {
        error
            .downcast_ref::<rrss_ai::Error>()
            .is_some_and(|error| matches!(error, rrss_ai::Error::BudgetExhausted))
    }) {
        return true;
    }
    err.downcast_ref::<CoreError>().is_some_and(|e| {
        matches!(
            e,
            CoreError::DailyBudgetExhausted
                | CoreError::ProviderNotFound
                | CoreError::TaskModelUnset(_)
        )
    })
}

/// 独立作业不继承触发它的命令；只附加类别，不产生日志行。
pub(crate) fn span(job: &str) -> tracing::Span {
    tracing::span!(parent: None, tracing::Level::WARN, "job", job)
}

pub(crate) async fn spawn_db<T, F>(path: PathBuf, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&rusqlite::Connection) -> rrss_core::Result<T> + Send + 'static,
{
    let capacity = crate::scheduler::model_capacity::Capacity::for_database(&path);
    let settling = capacity.work();
    let queued = std::time::Instant::now();
    // 已有 job / feed / pi_turn / command 链路继续沿用，独立数据库作业才建根 span。
    let current = tracing::Span::current();
    let span = if current.is_none() {
        span("db")
    } else {
        current
    };
    crate::execution::blocking(move || {
        let _settling = settling;
        capacity.metrics.db_queue.record(queued.elapsed());
        let conn = db::open_at(&path)?;
        let _timing = capacity.metrics.db_work.start();
        f(&conn)
    })
    .instrument(span)
    .await
    .context("db worker join")?
    .map_err(Into::into)
}

#[cfg(test)]
mod skip_tests {
    use super::*;

    async fn assert_skip_from_db(error: CoreError) {
        let dir = tempfile::tempdir().unwrap();
        let err = spawn_db::<(), _>(dir.path().join("test.db"), move |_| Err(error))
            .await
            .unwrap_err();
        assert!(is_skip(&err));
        assert!(is_skip(&err.context("outer job context")));
    }

    #[tokio::test]
    async fn budget_exhausted_survives_db_worker_and_context() {
        assert_skip_from_db(CoreError::DailyBudgetExhausted).await;
    }

    #[tokio::test]
    async fn missing_provider_survives_db_worker_and_context() {
        assert_skip_from_db(CoreError::ProviderNotFound).await;
    }

    #[tokio::test]
    async fn unset_model_survives_db_worker_and_context() {
        assert_skip_from_db(CoreError::TaskModelUnset("用途名已改变".into())).await;
    }

    #[tokio::test]
    async fn cancelled_waiter_keeps_work_accounted_until_database_work_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let capacity = crate::scheduler::model_capacity::Capacity::for_database(&path);
        let mut active = capacity.active_work.subscribe();
        let (started, waiting) = tokio::sync::oneshot::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let job = tokio::spawn(spawn_db(path.clone(), move |conn| {
            let _ = started.send(());
            resume
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            db::set_setting(conn, "worker_test", "settled")
        }));
        waiting.await.unwrap();
        job.abort();
        assert!(job.await.unwrap_err().is_cancelled());
        assert_eq!(*active.borrow(), 1);
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            active.wait_for(|count| *count == 0).await.unwrap();
        })
        .await
        .unwrap();
        let conn = db::open_at(&path).unwrap();
        assert_eq!(
            db::get_setting(&conn, "worker_test").unwrap().as_deref(),
            Some("settled")
        );
        assert_eq!(capacity.metrics.db_queue.snapshot().count, 1);
        assert_eq!(capacity.metrics.db_work.snapshot().count, 1);
    }

    #[test]
    fn ordinary_errors_are_not_skip_even_when_the_message_mentions_skip_reasons() {
        for message in [
            "ordinary failure",
            "daily AI budget",
            "AI provider not found",
            "还没选模型",
        ] {
            assert!(!is_skip(&anyhow::anyhow!(message).context("outer context")));
        }
        assert!(!is_skip(&anyhow::Error::new(CoreError::InvalidProvider(
            "AI provider not found".into(),
        ))));
    }
}
