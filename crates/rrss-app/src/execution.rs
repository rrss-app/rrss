//! 阻塞执行底座：保留调用方的 tracing 上下文与错误类型，不创建调度器或认领作业额度。

use std::path::PathBuf;

use anyhow::{Context, Result};
use rrss_core::db::{self, PooledConnection};

/// 普通文件 / 数据库工作与作业适配共用；关闭收尾、队列计时等资源由闭包持有。
pub async fn blocking<T, F>(work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let span = tracing::Span::current();
    let dispatch = tracing::dispatcher::get_default(Clone::clone);
    tokio::task::spawn_blocking(move || {
        tracing::dispatcher::with_default(&dispatch, || span.in_scope(work))
    })
    .await
    .context("blocking worker join")
}

/// 在阻塞线程里开库、执行并归还连接；业务错误不转成字符串。
pub async fn with_db<T, F>(path: PathBuf, work: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&PooledConnection) -> Result<T> + Send + 'static,
{
    blocking(move || {
        let conn = db::open_at(&path)?;
        work(&conn)
    })
    .await?
}

/// 保持 core 查询 / 动作的领域错误；只有工作线程失败映射成 I/O 错误。
pub async fn core_db<T, F>(path: PathBuf, work: F) -> rrss_core::Result<T>
where
    T: Send + 'static,
    F: FnOnce(&PooledConnection) -> rrss_core::Result<T> + Send + 'static,
{
    blocking(move || {
        let conn = db::open_at(&path)?;
        work(&conn)
    })
    .await
    .map_err(std::io::Error::other)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread")]
    async fn waiting_for_a_write_lock_leaves_the_async_executor_available() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let conn = db::open_at(&path).unwrap();
        let transaction = db::write_transaction(&conn).unwrap();
        let (started, waiting) = tokio::sync::oneshot::channel();
        let worker = tokio::spawn(with_db(path, move |conn| {
            let _ = started.send(());
            db::set_setting(conn, "worker_test", "saved")?;
            Ok(())
        }));
        waiting.await.unwrap();
        tokio::task::yield_now().await;
        assert!(!worker.is_finished());
        transaction.commit().unwrap();
        worker.await.unwrap().unwrap();
        assert_eq!(
            db::get_setting(&conn, "worker_test").unwrap().as_deref(),
            Some("saved")
        );
    }

    #[tokio::test]
    async fn errors_keep_their_type_and_worker_panics_are_join_errors() {
        let dir = tempfile::tempdir().unwrap();
        let error = with_db(dir.path().join("test.db"), |_| {
            Err::<(), _>(rrss_ai::Error::BudgetExhausted.into())
        })
        .await
        .unwrap_err();
        assert!(matches!(
            error.downcast_ref::<rrss_ai::Error>(),
            Some(rrss_ai::Error::BudgetExhausted)
        ));
        let error = blocking(|| panic!("fixture worker panic"))
            .await
            .unwrap_err();
        assert!(
            error
                .downcast_ref::<tokio::task::JoinError>()
                .unwrap()
                .is_panic()
        );
    }

    #[tokio::test]
    async fn database_work_inherits_the_callers_span() {
        use tracing::instrument::WithSubscriber;
        let dir = tempfile::tempdir().unwrap();
        let (subscriber, log) = crate::log::subscriber(
            crate::log::Mode::Desktop {
                log_dir: dir.path().join("logs"),
            },
            "",
        );
        async {
            let span = tracing::info_span!("fixture_command", command = "worker_test");
            let work = with_db(dir.path().join("test.db"), |_| {
                tracing::info!("worker fixture completed");
                Ok(())
            });
            use tracing::Instrument;
            work.instrument(span).await.unwrap();
        }
        .with_subscriber(subscriber)
        .await;
        log.end();
        let text = std::fs::read_to_string(dir.path().join("logs/rrss.log")).unwrap();
        let line = text
            .lines()
            .find(|line| line.contains("worker fixture completed"))
            .unwrap();
        assert!(line.contains("worker_test"));
    }
}
