use super::*;

#[tokio::test]
async fn ranking_turn_survives_registry_cleanup_without_a_run() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let old_turn = BackgroundWork::for_database(&path).rank.hold().await;
    // A registry miss prunes entries after the old work has been released.
    let _other = BackgroundWork::for_database(&dir.path().join("other.db"));
    let work = BackgroundWork::for_database(&path);
    let next_turn = work.rank.hold();
    tokio::pin!(next_turn);
    tokio::select! {
        biased;
        _ = &mut next_turn => panic!("the old ranking turn is still held"),
        _ = std::future::ready(()) => {}
    }
    drop(old_turn);
    drop(next_turn.await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_creators_share_one_run_and_polling_stops_with_its_last_handle() {
    let (dir, path) = fixture();
    std::fs::create_dir(dir.path().join("alias")).unwrap();
    let alias = dir.path().join("alias/../test.db");
    let models = model_capacity::Capacity::for_database(&alias);
    let gate = Arc::new(tokio::sync::Barrier::new(2));
    let create = |path: PathBuf| {
        let gate = gate.clone();
        tokio::spawn(async move {
            gate.wait().await;
            Scheduler::create(NetworkClients::new(path))
        })
    };
    let (first, second) = tokio::join!(create(path.clone()), create(alias.clone()));
    let run = match (first.unwrap(), second.unwrap()) {
        (Ok(run), Err(_)) | (Err(_), Ok(run)) => run,
        _ => panic!("expected one creator"),
    };
    assert!(Arc::ptr_eq(&models, &run.shared.capacity));
    let handle = Scheduler::get(&alias).unwrap();
    assert!(Arc::ptr_eq(&run.shared, &handle.shared));
    let operation = run.start_refresh().await.unwrap();
    assert_eq!(handle.snapshot().operations[0].id, operation.operation_id);
    let mut active = run.shared.work.active();
    let mut stopped = run.shared.stop.subscribe();
    run.start_polling();
    handle.start_polling();
    drop(run);
    assert!(!*stopped.borrow());
    drop(handle);
    tokio::time::timeout(Duration::from_secs(5), async {
        stopped.wait_for(|value| *value).await.unwrap();
        active.wait_for(|count| *count == 0).await.unwrap();
    })
    .await
    .unwrap();
    assert!(Scheduler::get(&path).is_none());
}

#[tokio::test]
async fn restarted_run_accepts_new_work_while_old_settlement_drains_independently() {
    let (_dir, path) = fixture();
    let old = Scheduler::create(NetworkClients::new(&path)).unwrap();
    // A response admitted by this run may persist its result after its caller exits.
    let (resume, resumed) = tokio::sync::oneshot::channel();
    let (entered, inside) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let old_work = old.shared.work.clone();
    let db = path.clone();
    let settlement = tokio::spawn(old.shared.work.scope(async move {
        resumed.await.unwrap();
        assert!(Arc::ptr_eq(&BackgroundWork::for_database(&db), &old_work));
        crate::jobs::spawn_db(db, move |_| {
            entered.send(()).unwrap();
            released.recv().unwrap();
            Ok(())
        })
        .await
        .unwrap();
    }));
    old.stop();
    let new = Scheduler::create(NetworkClients::new(&path)).unwrap();
    assert!(!Arc::ptr_eq(&old.shared.work, &new.shared.work));
    assert!(Arc::ptr_eq(&old.shared.work.db, &new.shared.work.db));
    assert!(Arc::ptr_eq(
        &BackgroundWork::for_database(&path),
        &new.shared.work
    ));
    resume.send(()).unwrap();
    inside.await.unwrap();
    let shutdown = old.shutdown();
    tokio::pin!(shutdown);
    tokio::select! {
        biased;
        _ = &mut shutdown => panic!("old work is still in flight"),
        _ = std::future::ready(()) => {},
    }
    new.start_refresh().await.unwrap();
    assert!(new.shutdown().await);
    assert!(*old.shared.work.active().borrow() > 0);
    release.send(()).unwrap();
    settlement.await.unwrap();
    assert!(shutdown.await);
}

#[tokio::test]
async fn unsubscribe_and_restart_keep_snapshot_events_with_their_originating_run() {
    let (_dir, path) = fixture();
    let old = Scheduler::create(NetworkClients::new(&path)).unwrap();
    let subscribe = |run: &Scheduler| {
        let (tx, rx) = mpsc::unbounded_channel();
        let token = run.subscribe(move |event| {
            if let SchedulerEvent::Status(snapshot) = event {
                tx.send(snapshot.clone()).unwrap();
            }
        });
        (token, rx)
    };
    let (_old_token, mut old_events) = subscribe(&old);
    let (removed, mut removed_events) = subscribe(&old);
    let before = old.snapshot();
    let old_op = old.start_refresh().await.unwrap();
    let received = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let view = removed_events.recv().await.unwrap();
            if !view.operations.is_empty() {
                break view;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(received.run_id, old_op.run_id);
    assert!(received.version > before.version);
    drop(removed);
    assert!(old.shutdown().await);
    while let Some(view) = removed_events.recv().await {
        assert!(!view.stopped);
    }

    let new = Scheduler::create(NetworkClients::new(&path)).unwrap();
    let (_new_token, mut new_events) = subscribe(&new);
    let new_op = new.start_refresh().await.unwrap();
    assert_ne!(old_op.run_id, new_op.run_id);
    assert!(new.shutdown().await);
    let mut old_terminal = false;
    while let Ok(view) = old_events.try_recv() {
        assert_eq!(view.run_id, old_op.run_id);
        old_terminal |= view.stopped;
    }
    assert!(old_terminal);
    let mut new_terminal = false;
    while let Ok(view) = new_events.try_recv() {
        assert_eq!(view.run_id, new_op.run_id);
        new_terminal |= view.stopped
            && view
                .operations
                .iter()
                .any(|op| op.id == new_op.operation_id);
    }
    assert!(new_terminal);
}
