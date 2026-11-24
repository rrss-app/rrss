//! Database-scoped job resources. Discovery holds weak references. The running
//! scheduler and outstanding work keep the state for their database alive.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use rrss_core::db::{metrics::Timing, pool::BACKGROUND_CONNECTIONS};
use tokio::sync::{Mutex as AsyncMutex, Semaphore, watch};

use super::model_capacity::Capacity;

// Database permits, write serialization and endpoint limits span a same-file restart.
// Outstanding callbacks and shutdown accounting belong to the run that admitted them.
#[derive(Default)]
struct Entry {
    admission: Weak<DatabaseAdmission>,
    work: Weak<BackgroundWork>,
    models: Weak<Capacity>,
    rank_turn: Weak<AsyncMutex<()>>,
    claimed: bool,
}

// Jobs may acquire resources before SQLite creates a new file. Resolve its
// parent in that case so its key still matches after creation.
pub(super) fn database_key(path: &Path) -> PathBuf {
    if let Ok(key) = path.canonicalize() {
        return key;
    }
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_owned());
    match (
        absolute.parent().and_then(|p| p.canonicalize().ok()),
        absolute.file_name(),
    ) {
        (Some(parent), Some(name)) => parent.join(name),
        _ => absolute,
    }
}

fn with_entry<T>(path: &Path, f: impl FnOnce(&Path, &mut Entry) -> T) -> T {
    static DATABASES: OnceLock<Mutex<HashMap<PathBuf, Entry>>> = OnceLock::new();
    let databases = DATABASES.get_or_init(Mutex::default);
    if let Some(entry) = databases.lock().unwrap().get_mut(path) {
        return f(path, entry);
    }
    let key = database_key(path);
    let mut databases = databases.lock().unwrap();
    databases.retain(|_, entry| {
        entry.admission.strong_count() > 0
            || entry.models.strong_count() > 0
            || entry.rank_turn.strong_count() > 0
    });
    f(&key, databases.entry(key.clone()).or_default())
}

fn get_or_create<T>(slot: &mut Weak<T>, create: impl FnOnce() -> T) -> Arc<T> {
    if let Some(value) = slot.upgrade() {
        return value;
    }
    let value = Arc::new(create());
    *slot = Arc::downgrade(&value);
    value
}

pub(super) fn model_capacity(path: &Path) -> Arc<Capacity> {
    with_entry(path, |_, entry| {
        get_or_create(&mut entry.models, Capacity::default)
    })
}

pub(crate) struct BackgroundWork {
    path: PathBuf,
    key: PathBuf,
    pub(crate) db: Arc<DatabaseAdmission>,
    pub(crate) rank: crate::jobs::rank::Batches,
    active: watch::Sender<usize>,
    pub(crate) subscribers: Arc<super::events::Subscribers>,
}

impl Entry {
    fn work(&mut self, key: &Path, path: &Path) -> Arc<BackgroundWork> {
        get_or_create(&mut self.work, || {
            self.claimed = false;
            BackgroundWork {
                key: key.to_owned(),
                path: path.to_owned(),
                db: get_or_create(&mut self.admission, DatabaseAdmission::default),
                // Pending IDs/results belong to this run. A preceding run's
                // submitted rank worker still excludes a new one until it exits.
                rank: crate::jobs::rank::Batches::new(get_or_create(
                    &mut self.rank_turn,
                    AsyncMutex::default,
                )),
                active: watch::channel(0).0,
                subscribers: Arc::default(),
            }
        })
    }
}

tokio::task_local! {
    static RUN_WORK: Arc<BackgroundWork>;
}

impl Default for DatabaseAdmission {
    fn default() -> Self {
        Self {
            permits: Arc::new(Semaphore::new(BACKGROUND_CONNECTIONS)),
            feed_writes: Arc::new(AsyncMutex::new(())),
            queue_time: Timing::default(),
            execution_time: Timing::default(),
        }
    }
}

pub(crate) struct DatabaseAdmission {
    pub(crate) permits: Arc<Semaphore>,
    pub(super) feed_writes: Arc<AsyncMutex<()>>,
    pub(crate) queue_time: Timing,
    pub(crate) execution_time: Timing,
}

impl BackgroundWork {
    pub(crate) fn for_database(path: &Path) -> Arc<Self> {
        if let Ok(Some(work)) = RUN_WORK.try_with(|work| {
            (work.path == path || work.key == path || work.key == database_key(path))
                .then(|| work.clone())
        }) {
            return work;
        }
        with_entry(path, |key, entry| entry.work(key, path))
    }

    pub(super) fn for_run(key: &Path, path: &Path) -> Arc<Self> {
        with_entry(key, |key, entry| {
            // Adopt any work already admitted before the first scheduler started.
            // On restart the preceding run keeps its own counter and subscribers.
            if entry.claimed {
                entry.work = Weak::new();
            }
            let work = entry.work(key, path);
            entry.claimed = true;
            work
        })
    }

    /// Carry the admitting run into a spawned task and its path-based job calls.
    /// Register before spawning so shutdown also sees tasks awaiting their first poll.
    pub(crate) fn scope<F: Future>(
        self: &Arc<Self>,
        task: F,
    ) -> impl Future<Output = F::Output> + use<F> {
        let guard = self.track();
        self.bind(async move {
            let _guard = guard;
            task.await
        })
    }

    /// Carry the run through an inline wait. Its database work and detached
    /// tasks register separately for shutdown.
    pub(crate) fn bind<F: Future>(
        self: &Arc<Self>,
        task: F,
    ) -> impl Future<Output = F::Output> + use<F> {
        RUN_WORK.scope(self.clone(), task)
    }

    pub(crate) fn track(self: &Arc<Self>) -> WorkGuard {
        self.active.send_modify(|count| *count += 1);
        WorkGuard(self.clone())
    }

    pub(crate) fn active(&self) -> watch::Receiver<usize> {
        self.active.subscribe()
    }
}

#[must_use]
pub(crate) struct WorkGuard(Arc<BackgroundWork>);

impl Drop for WorkGuard {
    fn drop(&mut self) {
        self.0.active.send_modify(|count| *count -= 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn restarted_runs_keep_separate_rank_batches_but_share_the_execution_turn() {
        let (_dir, path) = super::super::tests::fixture();
        let conn = rrss_core::db::open_at(&path).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://rank.example/feed',0);
             INSERT INTO articles(id,feed_id,title,created_at)
             VALUES(1,1,'Previous run',0),(2,1,'Current run',0);",
        )
        .unwrap();
        let key = database_key(&path);
        let old = BackgroundWork::for_run(&key, &path);
        let held = old.rank.hold().await;
        let mut cancelled = Box::pin(old.scope(old.rank.incremental(&path, vec![1])));
        tokio::select! {
            biased;
            _ = &mut cancelled => panic!("previous rank turn is held"),
            _ = std::future::ready(()) => {},
        }
        drop(cancelled);

        let new = BackgroundWork::for_run(&key, &path);
        let mut current = Box::pin(new.scope(new.rank.incremental(&path, vec![2])));
        tokio::select! {
            biased;
            _ = &mut current => panic!("previous run still owns the rank turn"),
            _ = std::future::ready(()) => {},
        }
        assert_eq!(*old.active().borrow(), 0);
        assert_eq!(*new.active().borrow(), 1);
        drop(held);
        current.await.unwrap();
        let ids = conn
            .prepare("SELECT article_id FROM article_rank ORDER BY article_id")
            .unwrap()
            .query_map([], |row| row.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(ids, vec![2]);
        assert_eq!(*new.active().borrow(), 0);
        // Existing local repair finds the previous run's committed article from SQLite.
        crate::jobs::rank::repair_missing(&path).await.unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM article_rank", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
}
