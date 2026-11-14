use super::*;
use tokio::sync::{mpsc, oneshot};

#[tokio::test]
async fn job_events_keep_their_category_across_dispatch_and_database_work() {
    use tracing::instrument::WithSubscriber;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.db");
    let (subscriber, log) = crate::log::subscriber(
        crate::log::Mode::Desktop {
            log_dir: dir.path().join("logs"),
        },
        "rrss=warn",
    );
    async {
        crate::jobs::spawn_db(path.clone(), |_| {
            tracing::warn!(error_kind = "fixture", "standalone job fixture");
            Ok(())
        })
        .await
        .unwrap();
        let (_dirty, requests) = watch::channel(1);
        let (_derived, changes) = watch::channel(0);
        let (stop, stopped) = watch::channel(false);
        let (done, mut completed) = watch::channel(0);
        let (_environment, observed) = watch::channel(Environment::default());
        let task = tokio::spawn(
            coordinate(
                requests,
                stopped,
                changes,
                done,
                observed,
                move |job, _| {
                    let path = path.clone();
                    async move {
                        crate::jobs::spawn_db(path, move |_| {
                            tracing::warn!(
                                job = rrss_core::logsafe::unless_in_span(
                                    "job",
                                    format!("{job:?}").to_lowercase()
                                ),
                                error_kind = "fixture",
                                "scheduled job fixture"
                            );
                            Ok(false)
                        })
                        .await
                    }
                },
                |_, _, _| {},
            )
            .with_current_subscriber(),
        );
        tokio::time::timeout(
            Duration::from_secs(10),
            completed.wait_for(|done| *done >= 1),
        )
        .await
        .unwrap()
        .unwrap();
        stop.send_replace(true);
        task.await.unwrap();
    }
    .with_subscriber(subscriber)
    .await;
    log.end();
    let text = std::fs::read_to_string(dir.path().join("logs/rrss.log")).unwrap();
    let standalone = text
        .lines()
        .find(|line| line.contains("standalone job fixture"))
        .unwrap();
    assert_eq!(standalone.matches("job=db").count(), 1, "{standalone}");
    let scheduled = text
        .lines()
        .find(|line| line.contains("scheduled job fixture") && line.contains("housekeeping"))
        .unwrap();
    assert!(scheduled.contains(" job: "), "{scheduled}");
    assert_eq!(
        scheduled.matches("job=housekeeping").count(),
        1,
        "{scheduled}"
    );
}

#[test]
fn budget_log_follows_pause_and_resume_without_repeating() {
    let dir = tempfile::tempdir().unwrap();
    let (subscriber, handle) = crate::log::subscriber(
        crate::log::Mode::Desktop {
            log_dir: dir.path().into(),
        },
        "",
    );
    tracing::subscriber::with_default(subscriber, || {
        let mut pending = Pending::default();
        pending.request(1);
        let before = pending.snapshot();
        let now = tokio::time::Instant::now();
        pending.claim(Job::Summaries, now).unwrap();
        pending.finish(Job::Summaries, Some("budget"), false, now);
        let paused = pending.snapshot();
        log_budget_changes(&before, &paused);
        log_budget_changes(&paused, &pending.snapshot());
        pending.environment(
            Environment {
                budget: 10,
                ..Default::default()
            },
            now,
        );
        log_budget_changes(&paused, &pending.snapshot());
    });
    handle.end();
    let text = std::fs::read_to_string(dir.path().join("rrss.log")).unwrap();
    assert_eq!(text.matches("automatic model job paused").count(), 1);
    assert_eq!(
        text.matches("automatic model budget pause cleared").count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn diagnostics_distinguish_budget_pause_from_eligible_wait_and_retry() {
    use tokio::time::Instant;
    let mut pending = Pending::default();
    pending.request(1);
    tokio::time::advance(Duration::from_secs(2)).await;
    let stats = pending.diagnostics().snapshot();
    assert_eq!(stats[0].pending_batches, 1);
    assert_eq!(stats[0].oldest_eligible_wait_ms, 2000);
    assert_eq!(pending.claim(Job::Summaries, Instant::now()), Some(1));
    tokio::time::advance(Duration::from_secs(3)).await;
    pending.finish(Job::Summaries, Some("budget"), false, Instant::now());
    tokio::time::advance(Duration::from_secs(60)).await;
    let stats = pending.diagnostics().snapshot();
    assert_eq!(stats[0].oldest_eligible_wait_ms, 0);
    assert_eq!(stats[0].pauses, 1);
    assert_eq!(stats[0].duration.total_ms, 3000.0);
    pending.environment(
        Environment {
            budget: 10,
            ..Default::default()
        },
        Instant::now(),
    );
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(
        pending.diagnostics().snapshot()[0].oldest_eligible_wait_ms,
        1000
    );
    pending.claim(Job::Summaries, Instant::now()).unwrap();
    pending.finish(Job::Summaries, Some("job_failed"), false, Instant::now());
    let stats = pending.diagnostics().snapshot();
    assert_eq!(stats[0].failures, 1);
    assert_eq!(
        stats[0].next_attempt_in_ms,
        Some(schedule::CATCH_UP.as_millis() as u64)
    );
    assert_eq!(stats[0].oldest_eligible_wait_ms, 0);
    tokio::time::advance(schedule::CATCH_UP).await;
    pending.claim(Job::Summaries, Instant::now()).unwrap();
    assert_eq!(pending.diagnostics().snapshot()[0].retries, 1);
    tokio::time::advance(Duration::from_secs(2)).await;
    pending.stop(Instant::now());
    let stats = pending.diagnostics().snapshot();
    assert!(
        stats
            .iter()
            .all(|job| job.state == "stopped" && job.running_ms == 0 && job.pending_batches == 0)
    );
    assert_eq!(stats[0].duration.total_ms, 5000.0);
}

struct Started {
    job: Job,
    version: u64,
    release: oneshot::Sender<Result<bool>>,
}
struct Harness {
    dirty: watch::Sender<u64>,
    environment: watch::Sender<Environment>,
    derived: watch::Sender<u64>,
    stop: watch::Sender<bool>,
    completed: watch::Receiver<u64>,
    starts: mpsc::UnboundedReceiver<Started>,
    updates: mpsc::UnboundedReceiver<(Vec<views::BackgroundJobView>, schedule::Diagnostics, bool)>,
    task: tokio::task::JoinHandle<()>,
}
impl Harness {
    fn new() -> Self {
        let (dirty, requests) = watch::channel(0);
        let (derived, changes) = watch::channel(0);
        let (stop, stopped) = watch::channel(false);
        let (done, completed) = watch::channel(0);
        let (started, starts) = mpsc::unbounded_channel();
        let (updated, updates) = mpsc::unbounded_channel();
        let (environment, observed) = watch::channel(Environment::default());
        let task = tokio::spawn(coordinate(
            requests,
            stopped,
            changes,
            done,
            observed,
            move |job, version| {
                let started = started.clone();
                async move {
                    let (release, finish) = oneshot::channel();
                    started
                        .send(Started {
                            job,
                            version,
                            release,
                        })
                        .unwrap();
                    finish.await.unwrap()
                }
            },
            move |snapshot, diagnostics, invalidate| {
                updated.send((snapshot, diagnostics, invalidate)).ok();
            },
        ));
        Self {
            dirty,
            environment,
            derived,
            stop,
            completed,
            starts,
            updates,
            task,
        }
    }
    async fn next(&mut self) -> Started {
        tokio::time::timeout(Duration::from_secs(10), self.starts.recv())
            .await
            .unwrap()
            .unwrap()
    }
    async fn stop(self) {
        self.stop.send_replace(true);
        tokio::time::timeout(Duration::from_secs(10), self.task)
            .await
            .unwrap()
            .unwrap();
    }
    async fn update(&mut self) -> (Vec<views::BackgroundJobView>, schedule::Diagnostics, bool) {
        tokio::time::timeout(Duration::from_secs(10), self.updates.recv())
            .await
            .unwrap()
            .unwrap()
    }
}

#[tokio::test]
async fn unchanged_wakes_refresh_diagnostics_without_invalidating_frontend() {
    let mut h = Harness::new();
    assert!(h.update().await.2);
    h.environment.send_replace(Environment::default());
    assert!(!h.update().await.2);
    h.dirty.send_replace(0);
    assert!(!h.update().await.2);

    h.dirty.send_replace(1);
    let (running, _, invalidate) = h.update().await;
    assert!(invalidate && running.iter().any(|job| job.state == "running"));
    let start = h.next().await;
    h.environment.send_replace(Environment::default());
    let (same, _, invalidate) = h.update().await;
    assert_eq!(same, running);
    assert!(!invalidate);
    start.release.send(Ok(false)).unwrap();
    assert!(
        h.update().await.2,
        "finished work still invalidates persisted data"
    );
    h.stop.send_replace(true);
    let (stopped, _, invalidate) = h.update().await;
    assert!(invalidate && stopped.iter().all(|job| job.state == "stopped"));
    h.task.await.unwrap();
}

#[tokio::test]
async fn slow_summaries_and_rebuild_do_not_gate_digest_or_local_work() {
    let mut h = Harness::new();
    h.dirty.send_replace(1);
    let mut held = Vec::new();
    let mut finished = Vec::new();
    for _ in 0..JOBS.len() - 1 {
        let start = h.next().await;
        match start.job {
            Job::Summaries | Job::Embeddings | Job::Images => held.push(start.release),
            job => {
                finished.push(job);
                start.release.send(Ok(false)).unwrap();
            }
        }
    }
    assert!(finished.contains(&Job::Weekly) && finished.contains(&Job::Interests));
    assert!(!finished.contains(&Job::ClusterSummaries));
    assert_eq!(*h.completed.borrow(), 0);
    for release in held {
        release.send(Ok(false)).unwrap();
    }
    let clusters = h.next().await;
    assert_eq!(clusters.job, Job::ClusterSummaries);
    clusters.release.send(Ok(false)).unwrap();
    tokio::time::timeout(Duration::from_secs(10), h.completed.wait_for(|v| *v == 1))
        .await
        .unwrap()
        .unwrap();
    h.stop().await;
}

#[tokio::test]
async fn failure_and_panic_consume_latest_request_without_bypassing_cooldown() {
    let mut h = Harness::new();
    h.dirty.send_replace(1);
    let mut first = Vec::new();
    for _ in 0..5 {
        first.push(h.next().await);
    }
    h.dirty.send_replace(3);
    for start in first {
        match start.job {
            Job::Summaries => start
                .release
                .send(Err(anyhow::anyhow!("injected")))
                .unwrap(),
            Job::Clusters => drop(start.release),
            _ => start.release.send(Ok(false)).unwrap(),
        }
    }
    loop {
        tokio::select! {
            result = h.completed.wait_for(|v| *v == 3) => { result.unwrap(); break; },
            Some(start) = h.starts.recv() => {
                assert!(!matches!(start.job, Job::Summaries | Job::Clusters));
                assert_ne!(start.version, 2);
                start.release.send(Ok(false)).unwrap();
            }
            _ = tokio::time::sleep(Duration::from_secs(10)) => panic!("finite request stuck behind cooldown"),
        }
    }
    h.stop().await;
}

#[tokio::test]
async fn object_changes_only_wake_derived_jobs_and_do_not_lose_changes_during_work() {
    let mut h = Harness::new();
    h.update().await;
    h.derived.send_replace(1);
    let first = h.next().await;
    assert_eq!(first.job, Job::Clusters);
    let (running, _, invalidate) = h.update().await;
    assert!(invalidate);
    h.derived.send_replace(2);
    h.derived.send_replace(3);
    let (same, diagnostics, invalidate) = h.update().await;
    assert_eq!(same, running);
    assert!(!invalidate);
    assert!(diagnostics.snapshot()[Job::Clusters as usize].pending_batches > 0);
    first.release.send(Ok(false)).unwrap();
    let next = h.next().await;
    assert_eq!(next.job, Job::Clusters);
    let (same, _, invalidate) = h.update().await;
    assert_eq!(same, running);
    assert!(
        invalidate,
        "completed data must be invalidated even when immediately reclaimed"
    );
    next.release.send(Ok(false)).unwrap();
    let interests = h.next().await;
    assert_eq!(interests.job, Job::Interests);
    interests.release.send(Ok(false)).unwrap();
    // A later finite request proves completion consumed only the claimed dirty
    // version; no summary, embedding or digest was created by the changes.
    h.dirty.send_replace(1);
    for _ in 0..JOBS.len() {
        h.next().await.release.send(Ok(false)).unwrap();
    }
    tokio::time::timeout(Duration::from_secs(10), h.completed.wait_for(|v| *v == 1))
        .await
        .unwrap()
        .unwrap();
    assert!(h.starts.try_recv().is_err());
    h.stop().await;
}

#[tokio::test]
async fn stop_cancels_owners_without_reporting_completion() {
    let mut h = Harness::new();
    h.dirty.send_replace(1);
    let mut held = Vec::new();
    for _ in 0..5 {
        held.push(h.next().await.release);
    }
    let completed = h.completed.clone();
    h.stop().await;
    assert!(held.iter().all(oneshot::Sender::is_closed));
    assert_eq!(*completed.borrow(), 0);
}

#[test]
fn pause_reasons_are_structured_and_separate() {
    assert_eq!(
        pause_reason(&rrss_core::Error::DailyBudgetExhausted.into()),
        "budget"
    );
    assert_eq!(
        pause_reason(&rrss_core::Error::TaskModelUnset("test".into()).into()),
        "model_configuration"
    );
    assert_eq!(
        pause_reason(
            &rrss_core::Error::InvalidNetworkProxy(
                rrss_core::network::ProxyError::ConfigUnavailable
            )
            .into()
        ),
        "network_configuration"
    );
    assert_eq!(pause_reason(&SpacePaused.into()), "vector_space");
    assert_eq!(pause_reason(&anyhow::anyhow!("injected")), "job_failed");
    assert_eq!(
        pause_reason(&rrss_ai::Error::Configuration("invalid endpoint").into()),
        "model_configuration"
    );
}

#[test]
fn housekeeping_deadline_does_not_wait_for_model_jobs() {
    let now = tokio::time::Instant::now();
    let mut p = Pending::default();
    p.environment(
        Environment {
            automatic: true,
            embedding_pending: true,
            day: "2026-10-30".into(),
            ..Default::default()
        },
        now,
    );
    assert_eq!(p.claim(Job::Housekeeping, now), Some(0));
    p.finish(Job::Housekeeping, None, false, now);
    assert_eq!(p.claim(Job::Summaries, now + schedule::CATCH_UP), Some(0));
    assert_eq!(p.claim(Job::Embeddings, now + schedule::CATCH_UP), Some(0));
    assert_eq!(
        p.claim(Job::Housekeeping, now + Duration::from_secs(15 * 60)),
        Some(0)
    );
}

#[test]
fn system_proxy_repair_resumes_only_the_network_pause() {
    let now = tokio::time::Instant::now();
    let mut p = Pending::default();
    let mut env = Environment {
        day: "2026-10-30".into(),
        ..Default::default()
    };
    p.environment(env.clone(), now);
    p.request(1);
    assert_eq!(p.claim(Job::Embeddings, now), Some(1));
    p.finish(Job::Embeddings, Some("network_configuration"), false, now);
    assert_eq!(p.claim(Job::Summaries, now), Some(1));
    p.finish(Job::Summaries, Some("budget"), false, now);
    env.network_environment = Some(rrss_core::network::ProxyEnvironment::from_lookup(|key| {
        (key == "HTTPS_PROXY").then(|| "http://proxy.test:8080".into())
    }));
    p.environment(env, now);
    assert_eq!(p.claim(Job::Embeddings, now), Some(1));
    assert_eq!(p.claim(Job::Summaries, now), None);
}

#[test]
fn embedding_batches_yield_then_stop_when_empty_without_repeating_summaries() {
    let now = tokio::time::Instant::now();
    let mut p = Pending::default();
    p.request(1);
    for job in JOBS {
        assert_eq!(p.claim(job, now), Some(1));
        p.finish(job, None, job == Job::Embeddings, now);
    }
    assert_eq!(
        p.completed(),
        1,
        "finite request does not wait for catch-up"
    );
    assert_eq!(
        p.claim(
            Job::Embeddings,
            now + schedule::CATCH_UP - Duration::from_secs(1)
        ),
        None
    );
    let later = now + schedule::CATCH_UP;
    assert_eq!(p.claim(Job::Embeddings, later), Some(1));
    assert_eq!(p.claim(Job::Summaries, later), None);
    p.finish(Job::Embeddings, None, false, later);
    assert_eq!(p.deadline(), None);
}

#[test]
fn new_input_between_batch_completion_and_metadata_scan_is_not_lost() {
    let now = tokio::time::Instant::now();
    let mut p = Pending::default();
    let env = Environment {
        automatic: true,
        day: "2026-10-30".into(),
        embedding_pending: true,
        ..Default::default()
    };
    p.environment(env.clone(), now);
    let due = now + schedule::CATCH_UP;
    assert_eq!(p.claim(Job::Embeddings, due), Some(0));
    p.finish(Job::Embeddings, None, false, due);
    // New input replaced the just-completed backlog before the scanner could
    // observe an empty library; the boolean value never changed.
    p.environment(env, due);
    assert_eq!(p.claim(Job::Embeddings, due + schedule::CATCH_UP), Some(0));
}

#[test]
fn budget_config_and_space_resume_only_after_their_conditions_change() {
    let now = tokio::time::Instant::now();
    let mut p = Pending::default();
    let mut env = Environment {
        day: "2026-10-30".into(),
        budget: 10,
        embed_budget: 10,
        ..Default::default()
    };
    p.environment(env.clone(), now);
    p.request(1);
    for job in JOBS {
        assert_eq!(p.claim(job, now), Some(1));
        p.finish(
            job,
            match job {
                Job::Embeddings | Job::Daily => Some("budget"),
                Job::Summaries => Some("model_configuration"),
                Job::Clusters => Some("vector_space"),
                _ => None,
            },
            false,
            now,
        );
    }
    p.request(20);
    p.environment(env.clone(), now + Duration::from_secs(86400));
    for job in [Job::Embeddings, Job::Summaries, Job::Clusters] {
        assert_eq!(p.claim(job, now + Duration::from_secs(86400)), None);
    }
    env.budget = 20;
    p.environment(env.clone(), now);
    assert_eq!(p.claim(Job::Daily, now), Some(20));
    p.finish(Job::Daily, Some("budget"), false, now);
    assert_eq!(p.claim(Job::Embeddings, now), None);
    env.embed_budget = 20;
    p.environment(env.clone(), now);
    assert_eq!(p.claim(Job::Daily, now), None);
    assert_eq!(p.claim(Job::Embeddings, now), Some(20));
    p.finish(Job::Embeddings, Some("budget"), false, now);
    env.day = "2026-10-31".into();
    p.environment(env.clone(), now);
    assert_eq!(p.claim(Job::Embeddings, now), Some(20));
    assert_eq!(p.claim(Job::Summaries, now), None);
    env.models[0] = "repaired".into();
    p.environment(env.clone(), now);
    assert_eq!(p.claim(Job::Summaries, now), Some(20));
    env.space = "active".into();
    p.environment(env, now);
    assert_eq!(p.claim(Job::Clusters, now), Some(20));
}

#[test]
fn cooldown_survives_refresh_and_retries_without_a_new_round() {
    let now = tokio::time::Instant::now();
    let mut p = Pending::default();
    p.request(1);
    assert_eq!(p.claim(Job::Embeddings, now), Some(1));
    p.finish(Job::Embeddings, Some("job_failed"), false, now);
    p.request(99);
    assert_eq!(p.claim(Job::Embeddings, now), None);
    assert_eq!(p.snapshot()[Job::Embeddings as usize].state, "cooldown");
    assert_eq!(p.claim(Job::Embeddings, now + schedule::CATCH_UP), Some(99));
}

#[test]
fn repair_during_running_attempt_is_not_lost_to_its_stale_pause() {
    let now = tokio::time::Instant::now();
    let mut p = Pending::default();
    p.environment(
        Environment {
            day: "2026-10-30".into(),
            ..Default::default()
        },
        now,
    );
    p.request(1);
    assert_eq!(p.claim(Job::Embeddings, now), Some(1));
    let mut env = Environment::default();
    env.models[1] = "fixed".into();
    p.environment(env, now);
    p.finish(Job::Embeddings, Some("model_configuration"), false, now);
    assert_eq!(p.claim(Job::Embeddings, now), Some(1));
}

#[tokio::test(start_paused = true)]
async fn independent_digest_timer_runs_while_summary_and_rebuild_remain_busy() {
    let mut h = Harness::new();
    h.environment.send_replace(Environment {
        automatic: true,
        embedding_pending: true,
        day: "2026-10-30".into(),
        ..Default::default()
    });
    let mut held = Vec::new();
    let mut daily = 0;
    // Automatic startup first runs local jobs, then conservatively starts model work.
    while daily < 2 {
        let started = tokio::time::timeout(Duration::from_secs(20 * 60), h.starts.recv())
            .await
            .unwrap()
            .unwrap();
        match started.job {
            Job::Summaries | Job::Embeddings => held.push(started.release),
            Job::Daily => {
                daily += 1;
                started.release.send(Ok(false)).unwrap();
            }
            _ => {
                started.release.send(Ok(false)).unwrap();
            }
        }
    }
    assert_eq!(held.len(), 2);
    h.stop().await;
}

#[test]
fn cluster_summaries_follow_summary_batches_with_independent_pause_recovery() {
    let now = tokio::time::Instant::now();
    let mut pending = Pending::default();
    let mut env = Environment {
        automatic: true,
        day: "2026-11-07".into(),
        ..Default::default()
    };
    pending.environment(env.clone(), now);
    pending.request(1);
    assert_eq!(pending.claim(Job::ClusterSummaries, now), None);
    assert_eq!(pending.claim(Job::Summaries, now), Some(1));
    assert_eq!(pending.claim(Job::ClusterSummaries, now), None);
    pending.finish(Job::Summaries, None, false, now);
    assert_eq!(pending.claim(Job::ClusterSummaries, now), Some(1));
    pending.finish(Job::ClusterSummaries, Some("budget"), false, now);
    let later = now + Duration::from_secs(rrss_core::ai_store::FOCUS_BATCH_MINUTES * 60);
    assert_eq!(pending.claim(Job::Summaries, later), Some(1));
    pending.finish(Job::Summaries, Some("model_configuration"), false, later);
    assert_eq!(pending.claim(Job::ClusterSummaries, later), None);
    env.day = "2026-11-08".into();
    pending.environment(env.clone(), later);
    assert_eq!(pending.claim(Job::ClusterSummaries, later), Some(1));
    pending.finish(
        Job::ClusterSummaries,
        Some("model_configuration"),
        false,
        later,
    );
    env.models[4] = "repaired".into();
    pending.environment(env, later);
    assert_eq!(pending.claim(Job::Summaries, later), None);
    assert_eq!(pending.claim(Job::ClusterSummaries, later), Some(1));
    pending.finish(Job::ClusterSummaries, None, false, later);
    assert_eq!(
        pending.claim(Job::ClusterSummaries, later + Duration::from_secs(60)),
        None
    );
    assert_eq!(
        pending.claim(Job::ClusterSummaries, later + Duration::from_secs(15 * 60)),
        Some(1)
    );
}

#[tokio::test]
async fn manual_job_result_channel_retains_domain_codes() {
    let (_dir, path) = crate::scheduler::tests::fixture();
    let scheduler = Scheduler::start(path.clone(), NetworkClients::new(&path), |_| {});
    // These are the same owner and watch boundary used by summary, cluster
    // summary and embedding jobs, including when their request is coalesced.
    for (key, error, code) in [
        (
            "summary",
            rrss_core::Error::ArticleNotFound,
            "article_not_found",
        ),
        (
            "cluster",
            rrss_core::Error::Jobs(rrss_core::error::JobFailure::ClusterEmpty),
            "job_cluster_empty",
        ),
        (
            "embed",
            rrss_core::Error::InvalidSetting(
                rrss_core::error::settings::SettingProblem::VectorSpaceStale,
            ),
            "setting_vector_space_stale",
        ),
    ] {
        let (sender, mut receiver) = watch::channel(None);
        let sender: ManualResult = sender;
        scheduler
            .shared
            .manual_jobs
            .lock()
            .unwrap()
            .insert(key.into(), sender);
        let mut owner = ManualOwner {
            shared: scheduler.shared.clone(),
            key: key.into(),
            finished: false,
        };
        owner.finish(Err(super::super::digest_jobs::Failure::from(
            anyhow::Error::new(error),
        )));
        let error = receiver.borrow_and_update().clone().unwrap().unwrap_err();
        let ui = crate::UiError::from(anyhow::Error::new(error));
        assert_eq!(ui.code(), code);
    }
    scheduler.shutdown().await;
}
