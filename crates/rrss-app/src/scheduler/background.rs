//! Bounded background dispatch; requests arriving during a job retain their generation.
use super::runtime::Shared;
use super::*;
use rrss_core::network::NetworkScope;
use std::collections::HashMap;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tracing::{Instrument, instrument::WithSubscriber};

pub(super) type ManualResult =
    watch::Sender<Option<std::result::Result<String, super::digest_jobs::Failure>>>;

#[derive(Clone, Copy)]
enum ManualJob {
    Summary(i64),
    /// 相近报道综述：簇 id 与焦点范围（还没核对，执行时再解析）。
    ClusterSummary(i64, rrss_core::feed_scope::FeedScope),
    Embed,
}

impl ManualJob {
    fn key(self, conn: &rusqlite::Connection) -> rrss_core::Result<String> {
        use rrss_core::ai_store;
        let (identity, tasks) = match self {
            Self::Summary(id) => {
                let article = rrss_core::articles::get_article(conn, id)?;
                (
                    serde_json::json!([
                        "summary",
                        id,
                        article.title,
                        article.blocks_json,
                        article.content_html,
                        ai_store::input_tier(conn)?.as_str()
                    ]),
                    vec![
                        ai_store::TASK_ARTICLE_SUMMARY,
                        ai_store::TASK_ARTICLE_SEGMENT,
                    ],
                )
            }
            Self::ClusterSummary(cluster_id, scope) => {
                use rrss_core::cluster_summary;
                let resolved = rrss_core::feed_scope::resolve(conn, scope)?;
                let members = cluster_summary::visible_members(conn, cluster_id, resolved)?;
                // 成员与各篇摘要的签名：同一组、同样的输入只发一次请求，输入变了另起一份。
                (
                    serde_json::json!([
                        "cluster_summary",
                        cluster_id,
                        resolved.as_string(),
                        cluster_summary::member_marks(conn, &members)?,
                        ai_store::input_tier(conn)?.as_str()
                    ]),
                    vec![ai_store::TASK_CLUSTER_SUMMARY],
                )
            }
            Self::Embed => (
                serde_json::json!([
                    "embed",
                    rrss_core::vector_space::building(conn)?
                        .or(rrss_core::vector_space::active(conn)?)
                        .map(|space| (space.id, space.state)),
                    rrss_core::vector_space::dims_setting(conn)?
                ]),
                vec![ai_store::TASK_EMBED],
            ),
        };
        // An opaque fingerprint; never expose configuration or input in the key.
        let mut config = Vec::new();
        for task in tasks {
            match ai_store::resolve_task(conn, task) {
                Ok(resolved) => config.push(serde_json::json!([
                    task,
                    resolved.model,
                    resolved.provider.id,
                    resolved.provider.base_url,
                    resolved.provider.api_key,
                    resolved.provider.protocol,
                    resolved.template.id,
                    resolved.template.body,
                    resolved.template.version
                ])),
                // Optional segment/embed bindings can be unset. The actual job retains
                // its existing validation and skip behavior when execution starts.
                Err(
                    rrss_core::Error::TaskModelUnset(_)
                    | rrss_core::Error::TemplateNotFound(_)
                    | rrss_core::Error::ProviderNotFound,
                ) => config.push(serde_json::json!([task, null])),
                Err(error) => return Err(error),
            }
        }
        Ok(ai_store::hash_input(
            &serde_json::json!([identity, config]).to_string(),
        ))
    }
}

struct ManualOwner {
    shared: Arc<Shared>,
    key: String,
    finished: bool,
}

impl ManualOwner {
    fn finish(&mut self, result: std::result::Result<String, super::digest_jobs::Failure>) {
        if self.finished {
            return;
        }
        self.finished = true;
        if let Some(slot) = self.shared.manual_jobs.lock().unwrap().remove(&self.key) {
            slot.send_replace(Some(result));
        }
    }
}

impl Drop for ManualOwner {
    fn drop(&mut self) {
        self.finish(Err(anyhow::Error::new(rrss_core::Error::Jobs(
            rrss_core::error::JobFailure::ModelStoppedRetry,
        ))
        .into()));
    }
}

impl Scheduler {
    pub fn request_background(&self) -> Result<u64> {
        let mut version = 0;
        self.shared.dirty.send_if_modified(|dirty| {
            if *self.shared.stop.borrow() {
                return false;
            }
            *dirty += 1;
            version = *dirty;
            true
        });
        anyhow::ensure!(
            version != 0,
            rrss_core::Error::Jobs(rrss_core::error::JobFailure::RuntimeStopped)
        );
        Ok(version)
    }
    /// Wait for a finite round, including local maintenance but not media downloads.
    /// Completion means attempted (including skips/errors), not that the backlog is empty.
    pub async fn wait_background(&self, version: u64) -> Result<()> {
        let mut completed = self.shared.completed.subscribe();
        let mut stop = self.shared.stop.subscribe();
        tokio::select! {
            biased;
            _ = stop.wait_for(|stopped| *stopped) => anyhow::bail!(rrss_core::Error::Jobs(rrss_core::error::JobFailure::RuntimeStopped)),
            result = completed.wait_for(|done| *done >= version) => {
                result.map_err(|_| anyhow::anyhow!(rrss_core::Error::Jobs(rrss_core::error::JobFailure::RuntimeStopped)))?;
                Ok(())
            },
        }
    }
    pub async fn summarize_article(&self, id: i64) -> Result<String> {
        self.manual_job(ManualJob::Summary(id)).await
    }
    /// 相近报道综述：`scope` 与焦点页相同（`all` / `folder:<id>`）。
    pub async fn summarize_cluster(&self, cluster_id: i64, scope: &str) -> Result<String> {
        let scope = rrss_core::feed_scope::FeedScope::parse_str(scope);
        self.manual_job(ManualJob::ClusterSummary(cluster_id, scope))
            .await
    }
    pub async fn generate_digest(&self, period: &'static str) -> Result<()> {
        anyhow::ensure!(
            !*self.shared.stop.borrow(),
            rrss_core::Error::Jobs(rrss_core::error::JobFailure::RuntimeStopped)
        );
        let shared = self.shared.clone();
        let admission = shared.manual.clone().try_acquire_owned().map_err(|_| {
            anyhow::anyhow!(rrss_core::Error::Jobs(
                rrss_core::error::JobFailure::ModelBusy
            ))
        })?;
        let work = shared.capacity.work();
        // The request owns its result even if its IPC waiter goes away.
        let span = crate::jobs::span(period);
        tokio::spawn(
            async move {
                let (_work, _admission) = (work, admission);
                let mut stop = shared.stop.subscribe();
                tokio::select! {
                    biased;
                    _ = stop.wait_for(|stopped| *stopped) => anyhow::bail!(rrss_core::Error::Jobs(rrss_core::error::JobFailure::RuntimeStopped)),
                    result = async {
                        let client = shared.network.client(NetworkScope::Api).await?;
                        crate::jobs::run_digest(&shared.path, &client, period, true).await
                    } => result,
                }
            }
            .instrument(span)
            .with_current_subscriber(),
        )
        .await?
    }
    pub async fn embed_now(&self) -> Result<()> {
        anyhow::ensure!(
            !*self.shared.stop.borrow(),
            rrss_core::Error::Jobs(rrss_core::error::JobFailure::RuntimeStopped)
        );
        self.shared
            .capacity
            .retries
            .lock()
            .unwrap()
            .reset_embeddings();
        self.manual_job(ManualJob::Embed).await.map(|_| ())
    }

    async fn manual_job(&self, job: ManualJob) -> Result<String> {
        anyhow::ensure!(
            !*self.shared.stop.borrow(),
            rrss_core::Error::Jobs(rrss_core::error::JobFailure::RuntimeStopped)
        );
        let key =
            crate::jobs::spawn_db(self.shared.path.clone(), move |conn| job.key(conn)).await?;
        let (mut result, owner) = {
            let mut active = self.shared.manual_jobs.lock().unwrap();
            anyhow::ensure!(
                !*self.shared.stop.borrow(),
                rrss_core::Error::Jobs(rrss_core::error::JobFailure::RuntimeStopped)
            );
            if let Some(slot) = active.get(&key) {
                anyhow::ensure!(
                    slot.receiver_count() < 32,
                    rrss_core::Error::Jobs(rrss_core::error::JobFailure::WaitLimit)
                );
                (slot.subscribe(), None)
            } else {
                anyhow::ensure!(
                    active.len() < 32,
                    rrss_core::Error::Jobs(rrss_core::error::JobFailure::ModelBusy)
                );
                let (slot, receiver) = watch::channel(None);
                active.insert(key.clone(), slot);
                (
                    receiver,
                    Some(ManualOwner {
                        shared: self.shared.clone(),
                        key,
                        finished: false,
                    }),
                )
            }
        };
        if let Some(mut owner) = owner {
            let shared = self.shared.clone();
            let work_guard = shared.capacity.work();
            let span = crate::jobs::span(match job {
                ManualJob::Summary(_) => "summaries",
                ManualJob::ClusterSummary(_, _) => "clustersummaries",
                ManualJob::Embed => "embeddings",
            });
            tokio::spawn(async move {
                let _work = work_guard;
                let mut stop = shared.stop.subscribe();
                let work = async {
                    let _admission = shared
                        .manual
                        .clone()
                        .try_acquire_owned()
                        .map_err(|_| anyhow::anyhow!(rrss_core::Error::Jobs(rrss_core::error::JobFailure::ModelBusy)))?;
                    let path = shared.path.clone();
                    let current = crate::jobs::spawn_db(path, move |conn| job.key(conn)).await?;
                    anyhow::ensure!(current == owner.key, rrss_core::Error::Jobs(rrss_core::error::JobFailure::InputChanged));
                    let client = shared.network.client(NetworkScope::Api).await?;
                    let path = &shared.path;
                    match job {
                        ManualJob::Summary(id) => {
                            crate::jobs::summarize_article(path, &client, id).await
                        }
                        ManualJob::ClusterSummary(cluster_id, scope) => {
                            crate::jobs::summarize_cluster(path, &client, cluster_id, scope).await
                        }
                        ManualJob::Embed => crate::jobs::run_embed_queue(path, &client)
                            .await
                            .map(|_| String::new()),
                    }
                };
                let outcome = tokio::select! {
                    biased;
                    _ = stop.wait_for(|stopped| *stopped) => Err(anyhow::anyhow!(rrss_core::Error::Jobs(rrss_core::error::JobFailure::RuntimeStopped))),
                    result = work => result,
                };
                owner.finish(outcome.map_err(super::digest_jobs::Failure::from));
            }.instrument(span).with_current_subscriber());
        }
        loop {
            if let Some(result) = result.borrow_and_update().clone() {
                return result.map_err(anyhow::Error::new);
            }
            result.changed().await.map_err(|_| {
                anyhow::anyhow!(rrss_core::Error::Jobs(
                    rrss_core::error::JobFailure::ModelStopped
                ))
            })?;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Job {
    Summaries,
    ClusterSummaries,
    Embeddings,
    Clusters,
    Interests,
    Daily,
    Weekly,
    Housekeeping,
    Images,
}

const JOBS: [Job; 9] = [
    Job::Summaries,
    Job::ClusterSummaries,
    Job::Embeddings,
    Job::Clusters,
    Job::Interests,
    Job::Daily,
    Job::Weekly,
    Job::Housekeeping,
    Job::Images,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lane {
    Model,
    Embedding,
    Digest,
    Local,
    Media,
}

impl Job {
    fn lane(self) -> Lane {
        match self {
            Self::Summaries | Self::ClusterSummaries => Lane::Model,
            Self::Daily | Self::Weekly => Lane::Digest,
            Self::Embeddings => Lane::Embedding,
            Self::Clusters | Self::Interests | Self::Housekeeping => Lane::Local,
            Self::Images => Lane::Media,
        }
    }

    fn dependency(self) -> Option<Self> {
        // Article readiness is tracked by the finite candidate snapshot, not a
        // whole-library stage barrier. Weekly still follows its daily input.
        match self {
            Self::ClusterSummaries => Some(Self::Summaries),
            Self::Interests => Some(Self::Clusters),
            Self::Weekly => Some(Self::Daily),
            _ => None,
        }
    }
}

pub(super) mod schedule;
use schedule::{Environment, Pending};

/// Only this loop owns the per-category state. At most one task per lane exists;
/// database work and resource waits run in workers, never inside the loop.
async fn coordinate<F, Fut>(
    mut dirty: watch::Receiver<u64>,
    mut stop: watch::Receiver<bool>,
    mut derived: watch::Receiver<u64>,
    completed: watch::Sender<u64>,
    mut environment: watch::Receiver<Environment>,
    mut run: F,
    update: impl Fn(Vec<views::BackgroundJobView>, schedule::Diagnostics, bool),
) where
    F: FnMut(Job, u64) -> Fut,
    Fut: std::future::Future<Output = Result<bool>> + Send + 'static,
{
    let mut pending = Pending::default();
    let mut workers = JoinSet::new();
    let mut owners = HashMap::new();
    let mut previous = Vec::new();
    let mut finished_work = false;
    let mut publish = |pending: &Pending, finished_work: bool| {
        let snapshot = pending.snapshot();
        // Diagnostics must stay current even on an unchanged metadata wake.
        // A completion may immediately reclaim the same generation (catch-up),
        // leaving the public state unchanged despite new database facts.
        let invalidate = snapshot != previous || finished_work;
        log_budget_changes(&previous, &snapshot);
        previous.clone_from(&snapshot);
        update(snapshot, pending.diagnostics(), invalidate);
    };
    loop {
        if *stop.borrow() {
            break;
        }
        pending.request(*dirty.borrow_and_update());
        pending.derived(*derived.borrow_and_update());
        pending.environment(
            environment.borrow_and_update().clone(),
            tokio::time::Instant::now(),
        );
        for job in JOBS {
            if *stop.borrow() {
                break;
            }
            if let Some(version) = pending.claim(job, tokio::time::Instant::now()) {
                let work = run(job, version);
                let stopped = stop.clone();
                let span = crate::jobs::span(&format!("{job:?}").to_lowercase());
                let task = workers.spawn(
                    async move {
                        anyhow::ensure!(
                            !*stopped.borrow(),
                            rrss_core::Error::Jobs(rrss_core::error::JobFailure::RuntimeStopped)
                        );
                        work.await
                    }
                    .instrument(span)
                    .with_current_subscriber(),
                );
                owners.insert(task.id(), job);
            }
        }
        publish(&pending, std::mem::take(&mut finished_work));
        let deadline = pending.deadline();
        completed.send_if_modified(|done| {
            let version = pending.completed();
            if version <= *done {
                return false;
            }
            *done = version;
            true
        });
        tokio::select! {
            biased;
            _ = stop.wait_for(|stopped| *stopped) => break,
            result = environment.changed() => if result.is_err() { break; },
            _ = async { match deadline {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending::<()>().await,
            } } => {},
            result = dirty.changed() => if result.is_err() { break; },
            result = derived.changed() => if result.is_err() { break; },
            Some(result) = workers.join_next_with_id(), if !workers.is_empty() => {
                let (id, outcome) = match result {
                    Ok(result) => result,
                    Err(error) => (error.id(), Err(error.into())),
                };
                let job = owners.remove(&id).expect("every worker has an owner");
                let reason = outcome.as_ref().err().map(pause_reason);
                if let Err(error) = &outcome && reason != Some("budget") {
                    tracing::warn!(job = rrss_core::logsafe::unless_in_span("job", format!("{job:?}").to_lowercase()), error = %crate::log::error(error.as_ref()), "background job did not complete");
                }
                pending.finish(job, reason, outcome.unwrap_or(false), tokio::time::Instant::now());
                finished_work = true;
            }
        }
    }
    // Dropping async workers cancels their network waits, not already running
    // spawn_blocking transactions or independent received-usage accounting.
    workers.shutdown().await;
    pending.stop(tokio::time::Instant::now());
    publish(&pending, finished_work);
}

#[derive(Debug)]
pub(crate) struct SpacePaused;
impl std::fmt::Display for SpacePaused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("向量空间暂不可读")
    }
}
impl std::error::Error for SpacePaused {}

fn log_budget_changes(
    previous: &[views::BackgroundJobView],
    snapshot: &[views::BackgroundJobView],
) {
    for job in snapshot {
        let was_budget = previous
            .iter()
            .any(|old| old.kind == job.kind && old.reason.as_deref() == Some("budget"));
        let budget = job.reason.as_deref() == Some("budget");
        if budget && !was_budget {
            tracing::warn!(
                job = rrss_core::logsafe::unless_in_span("job", job.kind.as_str()),
                error_kind = "budget",
                "automatic model job paused"
            );
        } else if was_budget && !budget {
            tracing::info!(
                job = rrss_core::logsafe::unless_in_span("job", job.kind.as_str()),
                "automatic model budget pause cleared"
            );
        }
    }
}

pub(crate) fn pause_reason(error: &anyhow::Error) -> &'static str {
    use rrss_core::Error;
    if let Some(shared) = error.downcast_ref::<super::digest_jobs::Failure>() {
        return shared.reason;
    }
    if error.is::<SpacePaused>() {
        return "vector_space";
    }
    for error in error
        .chain()
        .filter_map(|error| error.downcast_ref::<rrss_ai::Error>())
    {
        match error {
            rrss_ai::Error::BudgetExhausted => return "budget",
            rrss_ai::Error::ServiceCooldown => return "service_cooldown",
            rrss_ai::Error::Http {
                status: 401 | 403, ..
            }
            | rrss_ai::Error::NoProvider
            | rrss_ai::Error::Configuration(_) => return "model_configuration",
            rrss_ai::Error::Http { status: 407, .. } => return "network_configuration",
            _ => {}
        }
    }
    match error.downcast_ref::<Error>() {
        Some(Error::DailyBudgetExhausted) => "budget",
        Some(Error::TaskModelUnset(_) | Error::ProviderNotFound | Error::TemplateNotFound(_)) => {
            "model_configuration"
        }
        Some(Error::InvalidNetworkProxy(_)) => "network_configuration",
        Some(Error::InvalidSetting(_)) => "configuration",
        _ => "job_failed",
    }
}

pub(super) async fn drive(shared: Arc<Shared>) {
    let (environment, observed) = watch::channel(Environment::default());
    let observe = observe_environment(shared.clone(), environment);
    let coordinate = coordinate(
        shared.dirty.subscribe(),
        shared.stop.subscribe(),
        shared.capacity.derived.subscribe(),
        shared.completed.clone(),
        observed,
        |job, _version| {
            let guard = (job == Job::Summaries).then(|| {
                shared.capacity.summaries.send_replace(None);
                SummaryCandidates(shared.capacity.clone(), *shared.capacity.stopped.borrow())
            });
            let shared = shared.clone();
            async move {
                let _guard = guard;
                execute(shared, job).await
            }
        },
        |snapshot, diagnostics, invalidate| {
            *shared.background_diagnostics.lock().unwrap() = diagnostics;
            if invalidate {
                shared.background_status.send_replace(snapshot);
                shared.invalidate();
            }
        },
    );
    tokio::join!(observe, coordinate);
}

/// Configuration recovery is a bounded metadata read, not another model
/// attempt. In particular, unchanged budgets/configuration never wake a pause.
async fn observe_environment(shared: Arc<Shared>, send: watch::Sender<Environment>) {
    let mut stop = shared.stop.subscribe();
    let mut automatic = shared.automatic.subscribe();
    loop {
        let capacity = shared.capacity.clone();
        let read = crate::jobs::spawn_db(shared.path.clone(), move |conn| {
            use rrss_core::{ai_store as ai, vector_space as spaces};
            let tasks: [&[&str]; 5] = [
                &[ai::TASK_FOCUS_SUMMARY],
                &[ai::TASK_EMBED],
                &[ai::TASK_DIGEST_DAILY, ai::TASK_DIGEST_PICK_DAILY],
                &[ai::TASK_DIGEST_WEEKLY, ai::TASK_DIGEST_PICK_WEEKLY],
                &[ai::TASK_CLUSTER_SUMMARY],
            ];
            let mut models: [String; 5] = Default::default();
            for (index, tasks) in tasks.into_iter().enumerate() {
                let values = tasks
                    .iter()
                    .map(|task| match ai::resolve_task(conn, task) {
                        Ok(task) => serde_json::json!([
                            task.model,
                            task.provider.id,
                            task.provider.base_url,
                            task.provider.api_key,
                            task.provider.protocol,
                            task.template.body,
                            task.template.version
                        ]),
                        Err(error) => serde_json::json!(error.to_string()),
                    })
                    .collect::<Vec<_>>();
                models[index] = ai::hash_input(
                    &serde_json::json!([
                        values,
                        match index {
                            0 => Some(ai::FOCUS_AUTO_SUMMARY.enabled(conn)?),
                            4 => Some(ai::CLUSTER_AUTO_SUMMARY.enabled(conn)?),
                            _ => None,
                        },
                        if index != 1 {
                            db::get_setting(conn, ai::AI_INPUT_TIER_KEY)?
                        } else {
                            None
                        },
                        if matches!(index, 2 | 3) {
                            db::get_setting(conn, rrss_core::digest::DIGEST_GENERATE_MINUTES_KEY)?
                        } else {
                            None
                        },
                        if index == 1 {
                            db::get_setting(conn, spaces::DIMS_SETTING)?
                        } else {
                            None
                        },
                        (index == 1).then(|| capacity
                            .retry_revision
                            .load(std::sync::atomic::Ordering::Relaxed))
                    ])
                    .to_string(),
                );
            }
            let space = spaces::building(conn)?.or(spaces::active(conn)?);
            Ok(Environment {
                automatic: false,
                embedding_pending: crate::jobs::embeddings::pending_inputs(conn, &capacity)?,
                day: ai::local_day(),
                budget: ai::daily_budget_tokens(conn)?,
                embed_budget: ai::embed_daily_budget_tokens(conn)?,
                models,
                network: ai::hash_input(
                    &db::get_setting(conn, "network_proxy")?.unwrap_or_default(),
                ),
                network_environment: rrss_core::network::load(conn)
                    .ok()
                    .filter(|config| {
                        config.effective_for(NetworkScope::Api).mode
                            == rrss_core::network::ProxyMode::System
                    })
                    .map(|_| rrss_core::network::ProxyEnvironment::capture()),
                space: ai::hash_input(
                    &serde_json::json!([
                        space.map(|s| (s.id, s.state, s.checked_at)),
                        spaces::dims_setting(conn)?
                    ])
                    .to_string(),
                ),
            })
        });
        tokio::select! {
            biased;
            _ = stop.wait_for(|value| *value) => return,
            result = read => match result {
                Ok(mut env) => {
                    env.automatic = *automatic.borrow_and_update();
                    send.send_if_modified(|old| {
                        if *old == env { return false; }
                        *old = env;
                        true
                    });
                }
                Err(error) => tracing::warn!(%error, "background configuration check failed"),
            }
        }
        tokio::select! {
            _ = stop.wait_for(|value| *value) => return,
            _ = automatic.changed() => {},
            _ = tokio::time::sleep(schedule::CHECK_INTERVAL) => {},
        }
    }
}

struct SummaryCandidates(Arc<super::model_capacity::Capacity>, u64);
impl Drop for SummaryCandidates {
    fn drop(&mut self) {
        let epoch = self.0.stopped.borrow();
        if *epoch == self.1 {
            self.0.summaries.send_replace(Some(Default::default()));
        }
    }
}

async fn execute(shared: Arc<Shared>, job: Job) -> Result<bool> {
    use crate::jobs;
    let _local = if job.lane() == Lane::Local {
        Some(shared.capacity.local.lock().await)
    } else {
        None
    };
    anyhow::ensure!(
        !*shared.stop.borrow(),
        rrss_core::Error::Jobs(rrss_core::error::JobFailure::RuntimeStopped)
    );
    if job == Job::Clusters {
        jobs::rank::repair_missing(&shared.path).await?;
    }
    if matches!(job, Job::Clusters | Job::Interests) {
        let available = crate::jobs::spawn_db(shared.path.clone(), |conn| {
            Ok(rrss_core::vector_space::active(conn)?.is_none_or(|space| space.state == "active"))
        })
        .await?;
        if !available {
            return Err(SpacePaused.into());
        }
    }
    let result = match job {
        Job::Summaries | Job::ClusterSummaries | Job::Embeddings | Job::Daily | Job::Weekly => {
            let client = shared.network.client(NetworkScope::Api).await?;
            match job {
                Job::Summaries => jobs::run_focus_summaries(&shared.path, &client).await,
                Job::ClusterSummaries => jobs::run_cluster_summaries(&shared.path, &client).await,
                Job::Embeddings => {
                    jobs::run_embed_queue(&shared.path, &client).await?;
                    return jobs::embeddings::has_pending(&shared.path).await;
                }
                Job::Daily => jobs::run_digest(&shared.path, &client, "daily", false).await,
                Job::Weekly => jobs::run_digest(&shared.path, &client, "weekly", false).await,
                _ => unreachable!(),
            }
        }
        Job::Clusters => jobs::run_cluster_jobs(&shared.path).await,
        Job::Interests => jobs::run_interest_boost(&shared.path).await,
        Job::Housekeeping => jobs::run_housekeeping(&shared.path).await,
        Job::Images => {
            if let Some(data_dir) = shared.path.parent() {
                crate::image_preload::run(&shared.path, data_dir, &shared.network).await;
            }
            Ok(())
        }
    };
    result.map(|_| false)
}

#[cfg(test)]
mod tests;
