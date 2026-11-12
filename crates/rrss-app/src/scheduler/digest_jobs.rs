//! One owner per period; explicit regeneration coalesces into one trailing generation.
use super::model_capacity::Capacity;
use anyhow::Result;
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub(crate) struct Failure {
    pub reason: &'static str,
    pub message: String,
}
impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Self {
        Self {
            reason: super::background::pause_reason(&error),
            message: error.to_string(),
        }
    }
}
type Outcome = Option<std::result::Result<(), Failure>>;
#[derive(Default)]
pub(crate) struct Digests(Mutex<HashMap<(&'static str, i64, u64), Entry>>);
struct Entry {
    current: watch::Sender<Outcome>,
    next: Option<watch::Sender<Outcome>>,
}
struct Owner {
    capacity: Arc<Capacity>,
    key: (&'static str, i64, u64),
    finished: bool,
}
impl Drop for Owner {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if let Some(entry) = self.capacity.digests.0.lock().unwrap().remove(&self.key) {
            entry.current.send_replace(Some(Err(Failure {
                reason: "stopped",
                message: "简报任务已停止".into(),
            })));
            if let Some(next) = entry.next {
                next.send_replace(Some(Err(Failure {
                    reason: "stopped",
                    message: "简报任务已停止".into(),
                })));
            }
        }
    }
}

pub(crate) async fn run(
    path: &Path,
    client: &reqwest::Client,
    period: &'static str,
    force: bool,
) -> Result<()> {
    let db = path.to_owned();
    let client = client.clone();
    run_task(path, period, force, move |start, force| {
        let path = db.clone();
        let client = client.clone();
        async move { crate::jobs::digests::generate(&path, &client, period, start, force).await }
    })
    .await
}

async fn run_task<F, Fut>(
    path: &Path,
    period: &'static str,
    force: bool,
    mut generate: F,
) -> Result<()>
where
    F: FnMut(i64, bool) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<()>> + Send + 'static,
{
    anyhow::ensure!(matches!(period, "daily" | "weekly"), "无效的简报周期");
    let capacity = Capacity::for_database(path);
    let key = (
        period,
        rrss_core::ai_store::period_start_ms(period),
        *capacity.stopped.borrow(),
    );
    let (mut receiver, owns) = {
        let mut jobs = capacity.digests.0.lock().unwrap();
        if let Some(entry) = jobs.get_mut(&key) {
            let slot = if force {
                entry.next.get_or_insert_with(|| watch::channel(None).0)
            } else {
                &mut entry.current
            };
            anyhow::ensure!(slot.receiver_count() < 32, "简报等待请求过多，请稍后重试");
            (slot.subscribe(), false)
        } else {
            anyhow::ensure!(jobs.len() < 4, "简报任务繁忙，请稍后重试");
            let (current, receiver) = watch::channel(None);
            jobs.insert(
                key,
                Entry {
                    current,
                    next: None,
                },
            );
            (receiver, true)
        }
    };
    if owns {
        let work = capacity.work();
        let mut owner = Owner {
            capacity: capacity.clone(),
            key,
            finished: false,
        };
        tokio::spawn(async move {
            let _work = work;
            let mut stop = capacity.stopped.subscribe();
            let mut force = force;
            loop {
                let result = tokio::select! {
                    biased;
                    _ = stop.wait_for(|epoch| *epoch != key.2) => break,
                    result = generate(key.1, force) => result.map_err(Failure::from),
                };
                let mut jobs = capacity.digests.0.lock().unwrap();
                let entry = jobs.get_mut(&key).expect("digest owner");
                entry.current.send_replace(Some(result));
                if let Some(next) = entry.next.take() {
                    entry.current = next;
                    force = true;
                } else {
                    jobs.remove(&key);
                    // Retire ownership under the same mutex so a new generation
                    // cannot be removed by the old owner's Drop.
                    owner.finished = true;
                    break;
                }
            }
            drop(owner);
        });
    }
    loop {
        if let Some(result) = receiver.borrow_and_update().clone() {
            return result.map_err(anyhow::Error::new);
        }
        receiver
            .changed()
            .await
            .map_err(|_| anyhow::anyhow!("简报任务已停止"))?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::{mpsc, oneshot};

    fn request(
        path: &Path,
        force: bool,
        started: mpsc::UnboundedSender<(bool, oneshot::Sender<Result<()>>)>,
    ) -> tokio::task::JoinHandle<Result<()>> {
        let path = path.to_owned();
        tokio::spawn(async move {
            run_task(&path, "daily", force, move |_, force| {
                let started = started.clone();
                async move {
                    let (release, result) = oneshot::channel();
                    started.send((force, release)).unwrap();
                    result.await.unwrap()
                }
            })
            .await
        })
    }

    #[tokio::test]
    async fn explicit_requests_coalesce_into_one_followup_and_waiter_leaving_does_not_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let capacity = Capacity::for_database(&path);
        let (started, mut starts) = mpsc::unbounded_channel();
        let first = request(&path, false, started.clone());
        let (force, release) = starts.recv().await.unwrap();
        assert!(!force);
        let second = request(&path, true, started.clone());
        let third = request(&path, true, started);
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if capacity.digests.0.lock().unwrap().values().any(|entry| {
                    entry
                        .next
                        .as_ref()
                        .is_some_and(|next| next.receiver_count() == 2)
                }) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            starts.try_recv().is_err(),
            "next generation cannot run concurrently"
        );
        first.abort();
        release.send(Ok(())).unwrap();
        let (force, release) = starts.recv().await.unwrap();
        assert!(force);
        assert!(!second.is_finished() && !third.is_finished());
        release.send(Ok(())).unwrap();
        second.await.unwrap().unwrap();
        third.await.unwrap().unwrap();
        assert!(capacity.digests.0.lock().unwrap().is_empty());
        assert!(starts.try_recv().is_err());
    }

    #[tokio::test]
    async fn stopped_generation_cannot_remove_or_cancel_restarted_work() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let capacity = Capacity::for_database(&path);
        let (started, mut starts) = mpsc::unbounded_channel();
        let old = request(&path, true, started.clone());
        let (_, old_release) = starts.recv().await.unwrap();
        capacity.stopped.send_modify(|epoch| *epoch += 1);
        let new = request(&path, true, started);
        let (_, new_release) = starts.recv().await.unwrap();
        assert!(old.await.unwrap().is_err());
        assert!(old_release.is_closed());
        assert_eq!(capacity.digests.0.lock().unwrap().len(), 1);
        new_release.send(Ok(())).unwrap();
        new.await.unwrap().unwrap();
        assert!(capacity.digests.0.lock().unwrap().is_empty());
    }
}
