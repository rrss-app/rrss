use super::*;
use rrss_core::translations;
use rrss_render::TranslateUnit;
use tokio::sync::{mpsc, oneshot};

struct Call {
    path: PathBuf,
    ids: Vec<u32>,
    bypass: bool,
    reply: oneshot::Sender<Result<Vec<TranslateUnit>, Failure>>,
}
impl Call {
    fn good(self) {
        let units = self
            .ids
            .iter()
            .map(|id| TranslateUnit {
                i: *id,
                t: format!("译文 {id}"),
            })
            .collect();
        let _ = self.reply.send(Ok(units));
    }
    fn fail(self, failure: Failure) {
        let _ = self.reply.send(Err(failure));
    }
}
struct Controlled(mpsc::UnboundedSender<Call>);
impl Backend for Controlled {
    fn execute<'a>(
        &'a self,
        request: window::Request<'a>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<window::Output, Failure>> + Send + 'a>,
    > {
        Box::pin(async move {
            let (reply, receive) = oneshot::channel();
            self.0
                .send(Call {
                    path: request.path.into(),
                    ids: request.ids.into(),
                    bypass: request.bypass_cache,
                    reply,
                })
                .map_err(|_| Failure::Network)?;
            let raw = receive.await.map_err(|_| Failure::Network)??;
            Ok(window::validate(
                request.plan,
                request.source,
                request.ids,
                raw,
                request.article_id,
            ))
        })
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    runtime: Arc<Runtime>,
    calls: mpsc::UnboundedReceiver<Call>,
    events: mpsc::UnboundedReceiver<Event>,
}
impl Harness {
    fn new() -> Self {
        Self::with_limits(Limits {
            chunk_chars: 1,
            ..Limits::default()
        })
    }
    fn with_limits(limits: Limits) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let conn = db::open_at(&path).unwrap();
        conn.execute(
            "INSERT INTO feeds (id,url,created_at) VALUES (1,'https://example.test/rss',1)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO articles (id,feed_id,title,content_html,hash,created_at) VALUES (7,1,'English title','<p>The first paragraph.</p><p>The last paragraph.</p>','feed-hash',1)", []).unwrap();
        translations::set_enabled(&conn, true).unwrap();
        translations::set_backend(&conn, "http").unwrap();
        translations::set_http_config(&conn, Some("http://model.example"), None).unwrap();
        let (send, calls) = mpsc::unbounded_channel();
        let (events_send, events) = mpsc::unbounded_channel();
        let runtime = Runtime::build(
            path,
            Arc::new(Controlled(send)),
            Arc::new(move |event| {
                let _ = events_send.send(event);
            }),
            limits,
        );
        RUNTIMES
            .get_or_init(Mutex::default)
            .lock()
            .unwrap()
            .push(Arc::downgrade(&runtime));
        Self {
            _dir: dir,
            runtime,
            calls,
            events,
        }
    }
    fn conn(&self) -> rrss_core::db::PooledConnection {
        db::open_at(&self.runtime.path).unwrap()
    }
    async fn start(&self, mode: StartMode) -> Snapshot {
        started(self.runtime.start(7, mode, None).await.unwrap())
    }
    async fn call(&mut self) -> Call {
        tokio::time::timeout(Duration::from_secs(20), self.calls.recv())
            .await
            .unwrap()
            .unwrap()
    }
    async fn event(&mut self) -> Event {
        tokio::time::timeout(Duration::from_secs(20), self.events.recv())
            .await
            .unwrap()
            .unwrap()
    }
    async fn end(&mut self) -> Event {
        loop {
            let event = self.event().await;
            if matches!(event.kind, EventKind::Done | EventKind::Error(_)) {
                return event;
            }
        }
    }
    async fn complete(&mut self) -> Snapshot {
        let start = self.start(StartMode::Default).await;
        for _ in 0..3 {
            self.call().await.good();
        }
        assert_eq!(self.end().await.save, SaveState::Saved);
        start
    }
}
fn started(result: StartResult) -> Snapshot {
    match result {
        StartResult::Started(snapshot) => snapshot,
        other => panic!("expected started: {other:?}"),
    }
}

fn hint(
    snapshot: &Snapshot,
    ticket: u64,
    intent: u64,
    visible: Vec<usize>,
    nearby: Vec<usize>,
) -> crate::views::TranslationHint {
    crate::views::TranslationHint {
        identity: crate::views::TranslationIdentity {
            article_id: snapshot.identity.article_id,
            generation: Some(snapshot.identity.generation),
            source_hash: snapshot.identity.source_hash.clone(),
            config_hash: snapshot.identity.config_hash.clone(),
        },
        ticket,
        intent,
        viewport: crate::views::TranslationViewport { visible, nearby },
    }
}

#[tokio::test]
async fn viewport_latest_intent_prioritizes_pending_without_repeating_or_retrying() {
    let mut h = Harness::new();
    h.conn().execute("UPDATE articles SET content_html='<p>Zero paragraph.</p><pre>code</pre><p>One paragraph.</p><img src=\"https://example.test/a.png\" alt=\"Image description\"><p>Two paragraph.</p><p>Three paragraph.</p><p>Four paragraph.</p>' WHERE id=7", []).unwrap();
    let start = h.start(StartMode::Default).await;
    assert_eq!(
        start.unit_map,
        [None, Some(0), Some(2), Some(3), Some(4), Some(5), Some(6)]
    );
    let first = h.call().await;
    assert_eq!(first.ids, [0]);
    h.runtime.attach(start.identity.generation, 10);
    h.runtime.hint(hint(&start, 10, 1, vec![6], vec![5]));
    h.runtime
        .hint(hint(&start, 10, 3, vec![3, 4, 4, 999], vec![2, 4]));
    h.runtime.hint(hint(&start, 10, 2, vec![6], vec![5])); // 迟到意图
    first.good();
    let visible = h.call().await;
    assert_eq!(visible.ids, [4]);
    visible.fail(Failure::Network);
    let near = h.call().await;
    assert_eq!(near.ids, [2]);
    near.good();
    // 其余内容先接阅读位置（含可见图片说明）往下，前文最后补。
    let ordinary = h.call().await;
    assert_eq!(ordinary.ids, [3]);
    // 滚动回失败处不会重试；已完成和在途单元不会再次发出。
    h.runtime
        .hint(hint(&start, 10, 4, vec![4, 2, 0, 3], vec![]));
    ordinary.good();
    for expected in [1, 5, 6] {
        let call = h.call().await;
        assert_eq!(call.ids, [expected]);
        call.good();
    }
    let end = h.end().await;
    assert_eq!(end.phase, Phase::Partial);
    assert_eq!((end.done, end.failed), (6, 1));
    assert!(h.calls.try_recv().is_err());
}

#[tokio::test]
async fn viewport_identity_and_reader_release_isolate_late_hints_and_attachments() {
    let mut h = Harness::new();
    h.conn().execute("UPDATE articles SET content_html='<p>Zero paragraph.</p><p>One paragraph.</p><p>Two paragraph.</p><p>Three paragraph.</p><p>Four paragraph.</p>' WHERE id=7", []).unwrap();
    let start = h.start(StartMode::Default).await;
    let held = h.call().await;
    let generation = start.identity.generation;
    h.runtime.attach(generation, 10);
    h.runtime.hint(hint(&start, 10, 1, vec![4], vec![]));
    h.runtime.attach(generation, 11);
    h.runtime.hint(hint(&start, 11, 1, vec![3], vec![])); // 新票据可以重新从 1 编号
    for field in ["article", "generation", "source", "config"] {
        let mut bad = hint(&start, 11, 100, vec![4], vec![]);
        match field {
            "article" => bad.identity.article_id += 1,
            "generation" => bad.identity.generation = Some(generation + 1),
            "source" => bad.identity.source_hash.push('x'),
            "config" => bad.identity.config_hash.push('x'),
            _ => unreachable!(),
        }
        h.runtime.hint(bad);
    }
    h.runtime.hint(hint(&start, 10, 100, vec![4], vec![]));
    h.runtime.detach(generation, 10);
    held.good();
    let visible = h.call().await;
    assert_eq!(visible.ids, [4]);
    h.runtime.detach(generation, 11);
    h.runtime.attach(generation, 11); // 关闭后迟到的附着不能恢复前台
    h.runtime.hint(hint(&start, 11, 101, vec![4], vec![]));
    visible.good();
    let background = h.call().await;
    assert_eq!(background.ids, [1]);
    h.runtime.attach(generation, 12);
    h.runtime.hint(hint(&start, 12, 1, vec![4], vec![]));
    background.good();
    for expected in [5, 2, 3] {
        let call = h.call().await;
        assert_eq!(call.ids, [expected]);
        call.good();
    }
    assert_eq!(h.end().await.phase, Phase::Completed);
    // 同一阅读票据重译会产生新代际，旧代际的释放不影响它。
    let next = h.start(StartMode::Regenerate).await;
    h.runtime.attach(next.identity.generation, 12);
    h.runtime.detach(generation, 12);
    assert_eq!(
        h.runtime.state.lock().unwrap().foreground,
        Some((12, next.identity.generation))
    );
    h.call().await.fail(Failure::Quota);
    h.end().await;
}

#[tokio::test]
async fn viewport_initial_groups_noncontiguous_units_and_keeps_deeplx_single_unit() {
    for engine in ["http", "deeplx"] {
        let mut h = Harness::with_limits(Limits {
            chunk_chars: 40,
            ..Limits::default()
        });
        translations::set_backend(&h.conn(), engine).unwrap();
        let initial = TranslationInitialViewport {
            source_hash: crate::queries::reader_view(&h.conn(), 7)
                .unwrap()
                .source_hash,
            ticket: 10,
            intent: 1,
            viewport: TranslationViewport {
                visible: vec![1],
                nearby: vec![0],
            },
        };
        let start = started(
            h.runtime
                .start(7, StartMode::Default, Some(initial))
                .await
                .unwrap(),
        );
        let expected = if engine == "deeplx" {
            vec![vec![0], vec![2], vec![1]]
        } else {
            vec![vec![0, 2], vec![1]]
        };
        for ids in expected {
            let call = h.call().await;
            assert_eq!(call.ids, ids, "{engine}");
            call.good();
        }
        assert_eq!(h.end().await.phase, Phase::Completed);
        let final_state = h
            .runtime
            .snapshot(7, Some(start.identity.generation))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            final_state
                .blocks
                .iter()
                .map(|b| b.index)
                .collect::<Vec<_>>(),
            [0, 1]
        );
        assert!(h.calls.try_recv().is_err());
    }
}

#[tokio::test]
async fn deltas_arrive_before_later_windows_and_retry_only_failed_units() {
    let mut h = Harness::new();
    let start = h.start(StartMode::Default).await;
    let first = h.call().await;
    assert_eq!(first.ids, [0]);
    assert!(!first.bypass);
    let same = h.runtime.start(7, StartMode::Default, None).await.unwrap();
    assert!(matches!(same, StartResult::Resumed(ref s) if s.identity == start.identity));
    first.good();
    let middle = h.call().await;
    assert_eq!(middle.ids, [1]);
    let progress = h.runtime.snapshot(7, None).await.unwrap().unwrap();
    assert_eq!(progress.units[0], UnitState::Done);
    assert!(progress.title.is_some());
    assert!(progress.blocks.is_empty());
    middle.fail(Failure::Network);
    let last = h.call().await;
    assert_eq!(last.ids, [2]);
    last.good();
    let mut deltas = Vec::new();
    let terminal = loop {
        let e = h.event().await;
        if let EventKind::Delta {
            ref title,
            ref blocks,
            ref units,
        } = e.kind
        {
            assert_eq!(units.len(), 1);
            assert!(title.is_none() || blocks.is_empty());
            assert!(blocks.len() <= 1);
            deltas.push(e.seq);
        }
        if matches!(e.kind, EventKind::Done) {
            break e;
        }
    };
    assert_eq!(deltas, [2, 3, 4, 5, 6, 7]);
    assert_eq!(terminal.phase, Phase::Partial);
    assert_eq!((terminal.done, terminal.failed), (2, 1));
    assert!(
        h.runtime.state.lock().unwrap().jobs[&start.identity.generation]
            .work
            .is_none()
    );
    let recovered = h.runtime.snapshot(7, None).await.unwrap().unwrap();
    assert_eq!(recovered.blocks.len(), 1);
    assert_eq!(recovered.blocks[0].index, 1);
    assert!(
        !crate::queries::reader_view(&h.conn(), 7)
            .unwrap()
            .translation_ready,
        "old UI cannot present partial as complete"
    );
    let retry = h.start(StartMode::RetryFailed).await;
    assert_ne!(retry.identity.generation, start.identity.generation);
    assert_eq!(
        retry.units,
        [UnitState::Done, UnitState::Pending, UnitState::Done]
    );
    let call = h.call().await;
    assert_eq!(call.ids, [1]);
    assert!(call.bypass);
    call.good();
    assert_eq!(h.end().await.phase, Phase::Completed);
    assert!(matches!(
        h.runtime.start(7, StartMode::Default, None).await.unwrap(),
        StartResult::Cached { .. }
    ));
    assert!(h.calls.try_recv().is_err());
}

#[tokio::test]
async fn save_failure_keeps_memory_and_save_retry_sends_no_network() {
    let mut h = Harness::new();
    h.conn().execute_batch("CREATE TRIGGER reject_translation BEFORE INSERT ON article_translations BEGIN SELECT RAISE(FAIL, 'disk failure'); END;").unwrap();
    let first = h.start(StartMode::Default).await;
    for _ in 0..3 {
        h.call().await.good();
    }
    let failed = h.end().await;
    assert_eq!(failed.save, SaveState::SaveFailed);
    assert!(matches!(failed.kind, EventKind::Error(Failure::Storage)));
    assert!(
        h.runtime.state.lock().unwrap().jobs[&first.identity.generation]
            .work
            .is_some()
    );
    assert!(matches!(
        h.runtime.start(7, StartMode::Default, None).await.unwrap(),
        StartResult::Resumed(_)
    ));
    h.conn()
        .execute_batch("DROP TRIGGER reject_translation;")
        .unwrap();
    let saved = h
        .runtime
        .retry_save(7, first.identity.generation)
        .await
        .unwrap();
    assert_eq!(saved.save, SaveState::Saved);
    assert_eq!(saved.error, None);
    assert_eq!(saved.blocks.len(), 2);
    assert!(h.calls.try_recv().is_err());
}

#[tokio::test]
async fn failed_only_retry_can_recover_unsaved_partial_memory() {
    let mut h = Harness::new();
    h.conn().execute_batch("CREATE TRIGGER reject_translation BEFORE INSERT ON article_translations BEGIN SELECT RAISE(FAIL, 'disk failure'); END;").unwrap();
    h.start(StartMode::Default).await;
    h.call().await.good();
    h.call().await.fail(Failure::Timeout);
    h.call().await.good();
    assert_eq!(h.end().await.save, SaveState::SaveFailed);
    h.conn()
        .execute_batch("DROP TRIGGER reject_translation;")
        .unwrap();
    let retry = h.start(StartMode::RetryFailed).await;
    assert_eq!(
        retry.units,
        [UnitState::Done, UnitState::Pending, UnitState::Done]
    );
    let call = h.call().await;
    assert_eq!(call.ids, [1]);
    call.good();
    // Superseding the unsaved generation emits its invalidation first.
    while h.end().await.identity.generation != retry.identity.generation {}
    assert_eq!(
        h.runtime.snapshot(7, None).await.unwrap().unwrap().save,
        SaveState::Saved
    );
}

#[tokio::test]
async fn partial_regeneration_keeps_existing_complete_cache_until_all_units_finish() {
    let mut h = Harness::new();
    h.complete().await;
    fn complete(h: &Harness) -> bool {
        crate::queries::reader_view(&h.conn(), 7)
            .unwrap()
            .translation_ready
    }
    assert!(complete(&h));
    let regenerate = h.start(StartMode::Regenerate).await;
    h.call().await.good();
    h.call().await.fail(Failure::RateLimited);
    let kept = h.end().await;
    assert_eq!(kept.phase, Phase::Partial);
    assert_eq!(kept.save, SaveState::Kept);
    assert!(matches!(kept.kind, EventKind::Done));
    assert!(
        complete(&h),
        "partial regeneration must not replace a complete cache"
    );
    assert!(
        h.runtime.state.lock().unwrap().jobs[&regenerate.identity.generation]
            .work
            .is_some(),
        "unsaved result stays available for retrying failed units"
    );
    let retry = h.start(StartMode::RetryFailed).await;
    assert_eq!(
        retry.units,
        [UnitState::Done, UnitState::Pending, UnitState::Pending]
    );
    h.call().await.good();
    h.call().await.good();
    while h.end().await.identity.generation != retry.identity.generation {}
    let saved = h.runtime.snapshot(7, None).await.unwrap().unwrap();
    assert_eq!(
        (saved.phase, saved.save),
        (Phase::Completed, SaveState::Saved)
    );
    assert!(complete(&h));
}

#[tokio::test]
async fn configuration_aba_invalidates_immediately_and_discards_late_response() {
    let mut h = Harness::new();
    let start = h.start(StartMode::Default).await;
    let call = h.call().await;
    let conn = h.conn();
    let original = translations::target_lang(&conn).unwrap();
    crate::settings_schema::set(
        &conn,
        rrss_core::translations::TRANSLATE_TARGET_LANG_KEY,
        "fr",
    )
    .unwrap();
    crate::settings_schema::set(
        &conn,
        rrss_core::translations::TRANSLATE_TARGET_LANG_KEY,
        &original,
    )
    .unwrap();
    assert_eq!(
        crate::configuration::translation_revision(&conn).unwrap(),
        2
    );
    assert_eq!(h.end().await.phase, Phase::Invalidated);
    call.good();
    let replacement = h.start(StartMode::Default).await;
    assert_ne!(replacement.identity.generation, start.identity.generation);
    for _ in 0..3 {
        h.call().await.good();
    }
    assert_eq!(
        h.end().await.identity.generation,
        replacement.identity.generation
    );
    assert!(h.calls.try_recv().is_err());
}

#[tokio::test]
async fn source_refresh_preserves_identical_ir_but_changed_or_pruned_source_invalidates() {
    for change in ["same", "changed", "pruned"] {
        let mut h = Harness::new();
        h.start(StartMode::Default).await;
        let call = h.call().await;
        let conn = h.conn();
        if change == "pruned" {
            conn.execute("UPDATE articles SET pruned_at=2 WHERE id=7", [])
                .unwrap();
        } else {
            let text = if change == "same" {
                "The first paragraph.\n\nThe last paragraph."
            } else {
                "A new paragraph."
            };
            let doc = rrss_render::markdown_to_document(text);
            articles::set_fulltext(
                &conn,
                7,
                text,
                &rrss_render::document_to_json(&doc).unwrap(),
            )
            .unwrap();
        }
        call.good();
        if change == "same" {
            h.call().await.good();
            h.call().await.good();
            assert_eq!(h.end().await.phase, Phase::Completed);
        } else {
            assert_eq!(h.end().await.phase, Phase::Invalidated);
            assert!(h.calls.try_recv().is_err());
        }
    }
}

#[tokio::test]
async fn regenerate_rejects_old_generation_and_all_failed_keeps_previous_cache() {
    let mut h = Harness::new();
    let original = h.complete().await;
    let old_cache: String = h
        .conn()
        .query_row("SELECT blocks_json FROM article_translations", [], |row| {
            row.get(0)
        })
        .unwrap();
    let stale = h.start(StartMode::Regenerate).await;
    let inflight = h.call().await;
    let newest = h.start(StartMode::Regenerate).await;
    assert!(
        h.runtime
            .snapshot(7, Some(original.identity.generation))
            .await
            .unwrap()
            .is_none()
    );
    assert_ne!(newest.identity.generation, stale.identity.generation);
    assert_ne!(newest.identity.generation, original.identity.generation);
    inflight.good();
    for _ in 0..3 {
        let call = h.call().await;
        assert!(call.bypass);
        call.fail(Failure::Timeout);
    }
    let terminal = loop {
        let e = h.end().await;
        if e.identity.generation == newest.identity.generation {
            break e;
        }
    };
    assert_eq!(terminal.phase, Phase::Failed);
    assert!(matches!(terminal.kind, EventKind::Error(Failure::Timeout)));
    assert_eq!(
        h.conn()
            .query_row::<String, _, _>("SELECT blocks_json FROM article_translations", [], |row| {
                row.get(0)
            })
            .unwrap(),
        old_cache
    );
}

#[tokio::test]
async fn fatal_error_stops_pending_but_window_timeout_continues() {
    let mut h = Harness::with_limits(Limits {
        chunk_chars: 1,
        timeout: Duration::from_millis(80),
        ..Limits::default()
    });
    h.start(StartMode::Default).await;
    let timed_out = h.call().await;
    let next = h.call().await; // First network future expires, next window is dispatched.
    assert_eq!(next.ids, [1]);
    assert!(timed_out.reply.is_closed());
    next.good();
    h.call().await.fail(Failure::Authentication);
    let terminal = h.end().await;
    assert_eq!(terminal.phase, Phase::Partial);
    assert_eq!(terminal.failed, 2);
    assert_eq!(
        super::super::ipc::event(terminal).error,
        Some(Failure::Authentication)
    );

    let mut h = Harness::new();
    h.start(StartMode::Default).await;
    h.call().await.fail(Failure::Quota);
    assert_eq!(h.end().await.failed, 3);
    assert!(h.calls.try_recv().is_err());
}

#[tokio::test]
async fn global_execution_is_serial_capacity_is_bounded_and_latest_reader_ticket_wins() {
    let mut a = Harness::new();
    let mut b = Harness::new();
    a.start(StartMode::Default).await;
    let held = a.call().await;
    b.start(StartMode::Default).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(30), b.calls.recv())
            .await
            .is_err()
    );
    let conn = a.conn();
    let mut starts = Vec::new();
    for id in 8..=10 {
        conn.execute("INSERT INTO articles (id,feed_id,title,content_html,created_at) VALUES (?1,1,'Another English title','',1)", [id]).unwrap();
        starts.push(started(
            a.runtime.start(id, StartMode::Default, None).await.unwrap(),
        ));
    }
    conn.execute(
        "INSERT INTO articles (id,feed_id,title,created_at) VALUES (11,1,'Capacity test',1)",
        [],
    )
    .unwrap();
    assert_eq!(
        a.runtime
            .start(11, StartMode::Default, None)
            .await
            .unwrap_err(),
        Failure::Capacity
    );
    a.runtime.attach(starts[0].identity.generation, 5);
    a.runtime.attach(starts[2].identity.generation, 6);
    a.runtime.detach(starts[0].identity.generation, 5);
    a.runtime.attach(starts[1].identity.generation, 4);
    held.good();
    // b is already waiting for the global execution slot.
    for turn in 0..6 {
        tokio::select! {
            Some(call) = a.calls.recv() => {
                if turn == 1 { assert_eq!(call.ids, [0]); }
                assert_eq!(call.path, a.runtime.path);
                call.good();
            }
            Some(call) = b.calls.recv() => { call.good(); }
        }
    }
    // There are 5 remaining a windows + 3 b windows; two still to finish.
    for _ in 0..2 {
        tokio::select! { Some(call) = a.calls.recv() => call.good(), Some(call) = b.calls.recv() => call.good() }
    }
    assert_eq!(
        a.end().await.identity.article_id,
        10,
        "latest foreground gets the next a window"
    );
    for _ in 0..3 {
        a.end().await;
    }
    b.end().await;
    let foreground_start = starts[2].identity.generation;
    // Terminal order is visible in seq-independent done events in the loop above;
    // ticket identity is retained even when an older reader releases.
    assert_eq!(
        a.runtime.state.lock().unwrap().foreground,
        Some((6, foreground_start))
    );
}

#[tokio::test]
async fn terminal_expiry_recovers_cache_without_generation_or_network() {
    let mut h = Harness::new();
    let start = h.complete().await;
    {
        let mut state = h.runtime.state.lock().unwrap();
        state
            .jobs
            .get_mut(&start.identity.generation)
            .unwrap()
            .ended = Some(Instant::now() - TERMINAL_TTL);
    }
    assert!(matches!(
        h.runtime.recover(7).await.unwrap(),
        Recovery::Cached { .. }
    ));
    assert!(h.runtime.state.lock().unwrap().jobs.is_empty());
    assert!(h.calls.try_recv().is_err());
    // Metadata removed and cache invalidated: retry must not silently become full generation.
    h.conn()
        .execute("UPDATE articles SET title='A changed title' WHERE id=7", [])
        .unwrap();
    assert_eq!(
        h.runtime
            .start(7, StartMode::RetryFailed, None)
            .await
            .unwrap_err(),
        Failure::RetryUnavailable
    );
    assert!(matches!(
        h.runtime.recover(7).await.unwrap(),
        Recovery::Missing
    ));
}

#[test]
fn window_validation_keeps_good_units_and_rejects_duplicate_or_empty() {
    let doc =
        rrss_render::markdown_to_document("A **strong** [link](https://example.test) and `code`.");
    let plan = TranslationPlan::new("English title", &doc);
    let unit = |i, t: &str| TranslateUnit { i, t: t.into() };
    let empty = window::validate(
        &plan,
        &doc,
        &[0, 1],
        vec![unit(0, "标题"), unit(1, "  ")],
        None,
    );
    assert!(empty[0].1.is_ok());
    assert_eq!(empty[1].1, Err(Failure::InvalidOutput));
    let duplicate = window::validate(
        &plan,
        &doc,
        &[0, 1],
        vec![unit(0, "标题"), unit(0, "重复")],
        Some(7),
    );
    assert!(duplicate.iter().all(|(_, result)| result.is_err()));
    let unknown = window::validate(
        &plan,
        &doc,
        &[0],
        vec![unit(0, "标题"), unit(9, "越界")],
        None,
    );
    assert_eq!(unknown.len(), 1, "stray ids are dropped, not reported");
    assert!(
        unknown[0].1.is_ok(),
        "a stray id does not fail requested units"
    );
    let reordered = window::validate(
        &plan,
        &doc,
        &[1],
        vec![unit(1, "<x2/> <x1>链接</x1> <x0>加粗</x0>")],
        None,
    );
    assert!(reordered[0].1.is_ok());
}

mod http;

#[tokio::test]
async fn terminal_limit_and_timer_release_results_without_further_commands() {
    let mut h = Harness::new();
    let conn = h.conn();
    for id in 8..=12 {
        conn.execute(
            "INSERT INTO articles (id,feed_id,title,created_at) VALUES (?1,1,'English title',1)",
            [id],
        )
        .unwrap();
        h.runtime.start(id, StartMode::Default, None).await.unwrap();
        h.call().await.good();
        h.end().await;
    }
    assert!(h.runtime.snapshot(8, None).await.unwrap().is_none());
    assert!(h.runtime.state.lock().unwrap().jobs.len() <= MAX_TERMINAL_JOBS);

    let mut h = Harness::with_limits(Limits {
        chunk_chars: 1,
        terminal_ttl: Duration::from_millis(60),
        ..Limits::default()
    });
    h.complete().await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        h.runtime.state.lock().unwrap().jobs.is_empty(),
        "timer must reclaim without another snapshot/start"
    );
}

#[test]
fn setting_revisions_cover_provider_pool_and_binding_changes_and_roll_back_atomically() {
    use crate::configuration;
    use crate::views::{
        ModelChoice, ProviderInput, SaveAiSettingsArgs, SaveModelBindingsArgs, SaveModelPoolArgs,
        TaskModelInput,
    };
    use rrss_core::{ai_store, model_pool};
    let h = Harness::new();
    let conn = h.conn();
    let pid = crate::test_net::fake_provider(&conn, "mock", "http://model.example".into());
    translations::set_backend(&conn, "complete").unwrap();
    model_pool::update_pool(
        &conn,
        &[
            model_pool::ModelRef::new(pid, "a"),
            model_pool::ModelRef::new(pid, "b"),
        ],
        &[],
        None,
    )
    .unwrap();
    let bind = |model: &str| {
        crate::actions::save_model_bindings(
            &conn,
            &SaveModelBindingsArgs {
                bindings: vec![TaskModelInput {
                    task: ai_store::TASK_ARTICLE_TRANSLATE.into(),
                    provider_id: pid,
                    model: model.into(),
                }],
            },
        )
    };
    bind("a").unwrap();
    assert_eq!(configuration::translation_revision(&conn).unwrap(), 1);
    bind("a").unwrap();
    assert_eq!(configuration::translation_revision(&conn).unwrap(), 1);
    bind("b").unwrap();
    bind("a").unwrap();
    assert_eq!(configuration::translation_revision(&conn).unwrap(), 3);
    assert!(bind("missing").is_err());
    assert_eq!(configuration::translation_revision(&conn).unwrap(), 3);
    let mut input = SaveAiSettingsArgs {
        provider: ProviderInput {
            id: pid,
            kind: None,
            name: "renamed".into(),
            base_url: "http://model.example".into(),
            api_key: None,
            clear_key: None,
            protocol: "chat".into(),
            compat_json: None,
        },
    };
    crate::actions::save_ai_settings(&conn, &input).unwrap();
    assert_eq!(configuration::translation_revision(&conn).unwrap(), 3);
    input.provider.api_key = Some("secret".into());
    crate::actions::save_ai_settings(&conn, &input).unwrap();
    assert_eq!(configuration::translation_revision(&conn).unwrap(), 4);
    let remove = SaveModelPoolArgs {
        add: vec![],
        remove: vec![ModelChoice {
            provider_id: pid,
            model: "a".into(),
        }],
        replace: Some(ModelChoice {
            provider_id: pid,
            model: "b".into(),
        }),
    };
    crate::model_pool::save_model_pool(&conn, &remove).unwrap();
    assert_eq!(configuration::translation_revision(&conn).unwrap(), 5);
    crate::actions::delete_provider(&conn, pid).unwrap();
    assert_eq!(configuration::translation_revision(&conn).unwrap(), 6);
    // A failed write that already changed settings must roll back both the data and revision.
    let before = translations::target_lang(&conn).unwrap();
    let result: rrss_core::Result<()> = configuration::write(&conn, |tx| {
        translations::set_target_lang(tx, "fr")?;
        Err(rrss_core::Error::InvalidSetting("rollback".into()))
    });
    assert!(result.is_err());
    assert_eq!(configuration::translation_revision(&conn).unwrap(), 6);
    assert_eq!(translations::target_lang(&conn).unwrap(), before);
}

#[tokio::test]
async fn malformed_progress_cache_does_not_claim_resumable_units() {
    let mut h = Harness::new();
    h.complete().await;
    let conn = h.conn();
    let raw: String = conn
        .query_row("SELECT blocks_json FROM article_translations", [], |r| {
            r.get(0)
        })
        .unwrap();
    let article = articles::get_article(&conn, 7).unwrap();
    let source = crate::document::load_document(&article);
    let config = Config::read(&conn).unwrap();
    for change in ["legacy", "version", "missing", "pending", "empty", "blocks"] {
        let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        match change {
            "legacy" => {
                value["version"] = serde_json::json!(1);
                value.as_object_mut().unwrap().remove("states");
            }
            "version" => value["version"] = serde_json::json!(999),
            "missing" => value["states"] = serde_json::json!(["done"]),
            "pending" => value["states"][1] = serde_json::json!("pending"),
            "empty" => value["states"] = serde_json::json!([]),
            "blocks" => value["document"]["blocks"] = serde_json::json!([]),
            _ => unreachable!(),
        }
        conn.execute(
            "UPDATE article_translations SET blocks_json=?1",
            [value.to_string()],
        )
        .unwrap();
        assert!(
            cache::read_progress(&conn, &article, &source, &config)
                .unwrap()
                .is_none(),
            "{change}"
        );
    }
}

#[tokio::test]
async fn credential_revision_invalidates_work_but_preserves_completed_cache() {
    let mut h = Harness::new();
    h.complete().await;
    crate::configuration::write(&h.conn(), |tx| {
        translations::set_http_config(tx, Some("http://model.example"), Some("new-key"))
    })
    .unwrap();
    let snapshot = h.runtime.snapshot(7, None).await.unwrap().unwrap();
    assert_eq!(snapshot.phase, Phase::Completed);
    assert_eq!(snapshot.blocks.len(), 2);
    assert!(matches!(
        h.runtime.start(7, StartMode::Default, None).await.unwrap(),
        StartResult::Cached { .. }
    ));
    assert!(h.calls.try_recv().is_err());
}

#[tokio::test]
async fn reader_protocol_recovers_partial_cache_without_starting_a_job() {
    use crate::translate::ipc;
    use crate::views::TranslationReplyKind;
    let mut h = Harness::new();
    let initial = ipc::recover(&h.runtime, &h.runtime.path, 7, None)
        .await
        .unwrap();
    assert_eq!(initial.kind, TranslationReplyKind::Missing);
    assert!(initial.snapshot.is_none());
    assert_ne!(initial.reader.source_hash, "feed-hash");
    assert!(h.calls.try_recv().is_err());
    let started = ipc::start(&h.runtime, &h.runtime.path, 7, StartMode::Default, None)
        .await
        .unwrap();
    assert_eq!(started.kind, TranslationReplyKind::Started);
    assert_eq!(
        started.snapshot.as_ref().unwrap().identity.source_hash,
        started.reader.source_hash
    );
    h.call().await.good();
    h.call().await.fail(Failure::Network);
    h.call().await.good();
    h.end().await;
    // 模拟内存会话已回收，仅从持久化的部分缓存恢复。
    {
        let mut state = h.runtime.state.lock().unwrap();
        state.jobs.clear();
        state.current.clear();
    }
    let cached = ipc::recover(&h.runtime, &h.runtime.path, 7, None)
        .await
        .unwrap();
    assert_eq!(cached.kind, TranslationReplyKind::Cached);
    let snapshot = cached.snapshot.unwrap();
    assert!(snapshot.identity.generation.is_none());
    assert_eq!(snapshot.phase, Phase::Partial);
    assert_eq!(snapshot.unit_map, [None, Some(0), Some(1)]);
    assert_eq!(snapshot.blocks.len(), 1);
    assert_eq!(snapshot.blocks[0].index, 1);
    assert!(h.calls.try_recv().is_err());
    // 重译全部失败时，恢复协议仍提供原来部分成功的正文，且不伪造当前单元成功。
    h.start(StartMode::Regenerate).await;
    h.call().await.fail(Failure::Quota);
    h.end().await;
    let failed = ipc::recover(&h.runtime, &h.runtime.path, 7, None)
        .await
        .unwrap();
    assert_eq!(failed.snapshot.unwrap().phase, Phase::Failed);
    assert!(failed.reader.translation_ready);
    let document = failed.reader.translated_document.unwrap();
    assert_eq!(document["blocks"][1], snapshot.blocks[0].ir);
}

#[tokio::test]
async fn reader_protocol_does_not_attach_old_translation_to_new_source() {
    use crate::translate::ipc;
    let mut h = Harness::new();
    let started = ipc::start(&h.runtime, &h.runtime.path, 7, StartMode::Default, None)
        .await
        .unwrap();
    let call = h.call().await;
    let conn = db::open_at(&h.runtime.path).unwrap();
    conn.execute(
        "UPDATE articles SET content_html='<p>Replacement body</p>' WHERE id=7",
        [],
    )
    .unwrap();
    let recovered = ipc::recover(&h.runtime, &h.runtime.path, 7, None)
        .await
        .unwrap();
    assert_ne!(recovered.reader.source_hash, started.reader.source_hash);
    assert!(recovered.snapshot.is_none());
    assert!(!recovered.note.is_empty());
    call.good();
}
