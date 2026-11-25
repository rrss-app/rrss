//! 模型翻译的上下文超限拆窗与超长单元按句拆片，走真实 HttpBackend 与模拟端点。
use super::*;
use std::sync::atomic::AtomicUsize;

fn request_units(request: &Request) -> Vec<TranslateUnit> {
    let body: Value = request.body_json().unwrap();
    let user = body["messages"][1]["content"].as_str().unwrap();
    let (_, raw) = user.split_once('\n').unwrap();
    serde_json::from_str(raw).unwrap()
}

/// 保留占位，只给文字加前缀，模拟听话的模型。
fn translated(units: &[TranslateUnit]) -> ResponseTemplate {
    let out: Vec<_> = units
        .iter()
        .map(|u| json!({"i": u.i, "t": format!("译{}", u.t)}))
        .collect();
    ResponseTemplate::new(200).set_body_json(json!({
        "choices":[{"message":{"content":json!({"units":out}).to_string()}}],
        "usage":{"prompt_tokens":10,"completion_tokens":5}
    }))
}

fn context_overflow() -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(json!({"error":{
        "code":400,
        "message":"the request exceeds the available context size, try increasing it",
        "type":"exceed_context_size_error",
        "n_prompt_tokens":2300,
        "n_ctx":2048
    }}))
}

/// 每句约 90 字，含一个链接与一段粗体。
fn long_paragraph(sentences: usize, poison: Option<usize>) -> String {
    (0..sentences)
        .map(|i| {
            let word = if poison == Some(i) { "POISON" } else { "keeps" };
            format!(
                "Sentence {i} mentions <a href=\"https://a.example/{i}\">link {i}</a> with <strong>bold words</strong> and {word} going for a while."
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn set_article(h: &Harness, title: &str, html: &str) {
    h.conn()
        .execute(
            "UPDATE articles SET title = ?1, content_html = ?2 WHERE id = 7",
            rusqlite::params![title, html],
        )
        .unwrap();
}

/// 占位编号集合是否从 0 起连续。
fn local_ids(text: &str) -> Vec<usize> {
    let mut ids: Vec<usize> = text
        .match_indices("<x")
        .filter_map(|(at, _)| {
            let digits: String = text[at + 2..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse().ok()
        })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

async fn cached_document(h: &Harness) -> rrss_render::Document {
    match h.runtime.start(7, StartMode::Default, None).await.unwrap() {
        StartResult::Cached { document, .. } => document,
        other => panic!("expected cached: {other:?}"),
    }
}

async fn wait_idle(h: &Harness) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let idle = {
                let state = h.runtime.state.lock().unwrap();
                !state.worker && state.inflight == 0
            };
            if idle {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn context_overflow_splits_multi_unit_window_but_plain_bad_request_stops() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|r: &Request| {
            let units = request_units(r);
            if units.len() > 1 {
                context_overflow()
            } else {
                translated(&units)
            }
        })
        .mount(&server)
        .await;
    let mut h = Harness::http(&server.uri(), "complete", Limits::default());
    h.start(StartMode::Default).await;
    let end = h.end().await;
    assert_eq!(end.phase, Phase::Completed);
    assert_eq!(end.done, 3);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 4, "whole window, then each unit alone");
    assert!(requests[1..].iter().all(|r| request_units(r).len() == 1));

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error":{"message":"invalid model name","type":"invalid_request_error"}
        })))
        .mount(&server)
        .await;
    let mut h = Harness::http(&server.uri(), "complete", Limits::default());
    h.start(StartMode::Default).await;
    let end = h.end().await;
    assert_eq!(end.phase, Phase::Failed);
    assert_eq!(end.error, Some(Failure::Configuration));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn long_single_unit_is_sent_in_sentence_segments_and_patched_back_whole() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|r: &Request| translated(&request_units(r)))
        .mount(&server)
        .await;
    let mut h = Harness::http(
        &server.uri(),
        "complete",
        Limits {
            chunk_chars: 1,
            ..Limits::default()
        },
    );
    set_article(
        &h,
        "English title",
        &format!("<p>{}</p>", long_paragraph(8, None)),
    );
    h.start(StartMode::Default).await;
    let end = h.end().await;
    assert_eq!(end.phase, Phase::Completed);
    assert_eq!((end.total, end.done), (2, 2), "still one unit per block");
    let requests = server.received_requests().await.unwrap();
    let segments: Vec<String> = requests
        .iter()
        .map(|r| {
            let units = request_units(r);
            assert_eq!(units.len(), 1);
            assert_eq!(units[0].i, 0);
            units[0].t.clone()
        })
        .filter(|t| t.starts_with("Sentence"))
        .collect();
    assert!(segments.len() >= 3, "{}", segments.len());
    for segment in &segments {
        // 每片不超过窗字数下限 200，占位从 0 起连续编号。
        assert!(segment.chars().count() <= 200, "{segment}");
        let ids = local_ids(segment);
        assert_eq!(ids, (0..ids.len()).collect::<Vec<_>>(), "{segment}");
    }
    let document = cached_document(&h).await;
    let rrss_render::Block::Paragraph { inlines } = &document.blocks[0] else {
        panic!("paragraph");
    };
    let hrefs: Vec<&str> = inlines
        .iter()
        .filter_map(|inline| match inline {
            rrss_render::Inline::Link { href, .. } => Some(href.as_str()),
            _ => None,
        })
        .collect();
    let expected: Vec<String> = (0..8).map(|i| format!("https://a.example/{i}")).collect();
    assert_eq!(hrefs, expected);
    let strong = inlines
        .iter()
        .filter(|inline| matches!(inline, rrss_render::Inline::Strong { .. }))
        .count();
    assert_eq!(strong, 8);
    let text = rrss_render::inlines_to_plain(inlines);
    assert!(
        text.starts_with("译Sentence 0") && text.contains("Sentence 7"),
        "{text}"
    );
    assert_eq!(text.matches("译Sentence").count(), segments.len(), "{text}");
    assert_eq!(
        h.conn()
            .query_row::<i64, _, _>("SELECT count(*) FROM ai_cache", [], |r| r.get(0))
            .unwrap() as usize,
        1 + segments.len(),
        "title and every segment cached on their own"
    );
}

#[tokio::test]
async fn whole_unit_overflow_or_timeout_is_split_into_smaller_segments() {
    for mode in ["overflow", "timeout"] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(move |r: &Request| {
                let units = request_units(r);
                if units.iter().all(|u| u.t.chars().count() <= 250) {
                    translated(&units)
                } else if mode == "overflow" {
                    context_overflow()
                } else {
                    translated(&units).set_delay(Duration::from_secs(5))
                }
            })
            .mount(&server)
            .await;
        // 每次请求各自计时：首试超时后，后面几片不会被同一个期限连带超时。
        let mut h = Harness::http(
            &server.uri(),
            "complete",
            Limits {
                timeout: Duration::from_secs(1),
                ..Limits::default()
            },
        );
        set_article(&h, "", &format!("<p>{}</p>", long_paragraph(5, None)));
        h.start(StartMode::Default).await;
        let end = h.end().await;
        assert_eq!(end.phase, Phase::Completed, "{mode}");
        assert_eq!((end.total, end.done), (1, 1), "{mode}");
        let requests = server.received_requests().await.unwrap();
        assert!(request_units(&requests[0])[0].t.chars().count() > 250);
        assert!(requests.len() >= 3, "{mode}: {}", requests.len());
    }
}

#[tokio::test]
async fn segment_that_still_overflows_fails_only_its_unit() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|r: &Request| {
            let units = request_units(r);
            if units.iter().any(|u| u.t.contains("POISON")) {
                context_overflow()
            } else {
                translated(&units)
            }
        })
        .mount(&server)
        .await;
    let mut h = Harness::http(&server.uri(), "complete", Limits::default());
    set_article(
        &h,
        "English title",
        &format!(
            "<p>Short first paragraph.</p><p>{}</p>",
            long_paragraph(6, Some(3))
        ),
    );
    h.start(StartMode::Default).await;
    let end = h.end().await;
    // 整窗超限拆窗，长段单独成窗后按句拆片；含 POISON 的片降到下限仍超限，只有这一段失败。
    assert_eq!(end.phase, Phase::Partial);
    assert_eq!((end.done, end.failed), (2, 1));
    let requests = server.received_requests().await.unwrap();
    assert!(requests.len() <= 12, "{}", requests.len());
    // 失败片之前已译好的片照常缓存，之后的片不再发出。
    let sent: Vec<String> = requests
        .iter()
        .map(|r| request_units(r))
        .filter(|units| units.len() == 1)
        .map(|units| units[0].t.clone())
        .collect();
    assert!(sent.iter().any(|t| t.starts_with("Sentence 0")));
    assert!(!sent.iter().any(|t| t.starts_with("Sentence 5")));
    let cached = h
        .conn()
        .query_row::<i64, _, _>("SELECT count(*) FROM ai_cache", [], |r| r.get(0))
        .unwrap();
    assert!(cached >= 2, "{cached}");
}

#[tokio::test]
async fn stop_errors_and_invalidation_cancel_remaining_segments() {
    for mode in ["auth", "invalidated"] {
        let server = MockServer::start().await;
        let mut h = Harness::http(
            &server.uri(),
            "complete",
            Limits {
                chunk_chars: 1,
                ..Limits::default()
            },
        );
        let db_path = h.runtime.path.clone();
        let segments = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&segments);
        Mock::given(method("POST"))
            .respond_with(move |r: &Request| {
                let units = request_units(r);
                if !units[0].t.starts_with("Sentence") {
                    return translated(&units);
                }
                let n = seen.fetch_add(1, Ordering::SeqCst);
                match (mode, n) {
                    ("auth", 1) => ResponseTemplate::new(401)
                        .set_body_json(json!({"error":{"message":"invalid api key"}})),
                    ("invalidated", 0) => {
                        let conn = db::open_at(&db_path).unwrap();
                        for lang in ["fr", "zh"] {
                            crate::settings_schema::set(
                                &conn,
                                rrss_core::translations::TRANSLATE_TARGET_LANG_KEY,
                                lang,
                            )
                            .unwrap();
                        }
                        translated(&units)
                    }
                    _ => translated(&units),
                }
            })
            .mount(&server)
            .await;
        set_article(
            &h,
            "English title",
            &format!("<p>{}</p>", long_paragraph(8, None)),
        );
        h.start(StartMode::Default).await;
        let end = h.end().await;
        wait_idle(&h).await;
        if mode == "auth" {
            // 标题已译好，长段在第二片遇到认证失败：整段按原文保留，不写半段译文。
            assert_eq!(end.error, Some(Failure::Authentication));
            assert_eq!((end.done, end.failed), (1, 1));
            assert_eq!(segments.load(Ordering::SeqCst), 2, "{mode}");
            continue;
        }
        assert_eq!(end.phase, Phase::Invalidated);
        assert_eq!(segments.load(Ordering::SeqCst), 1, "{mode}");
        assert_eq!(
            h.conn()
                .query_row::<i64, _, _>("SELECT count(*) FROM article_translations", [], |r| r
                    .get(0))
                .unwrap(),
            0,
            "{mode}: invalidated results are never saved"
        );
    }
}

#[tokio::test]
async fn broken_segment_placeholders_split_and_unclosed_tags_at_floor_fail() {
    // 场景 A：较长片丢了占位符时，不直接拼进去走宽松降级，而是对半细分重试恢复全部占位。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|r: &Request| {
            let units = request_units(r);
            if units[0].t.chars().count() > 260 {
                // 丢掉所有占位符，模拟模型在长片里弄丢 <xN> 标签。
                let stripped = rrss_render::units_plain(&units);
                return ResponseTemplate::new(200).set_body_json(json!({
                    "choices":[{"message":{"content":json!({"units":[{"i":0,"t":format!("译{stripped}")}]}).to_string()}}],
                    "usage":{"prompt_tokens":10,"completion_tokens":5}
                }));
            }
            translated(&units)
        })
        .mount(&server)
        .await;
    let mut h = Harness::http(
        &server.uri(),
        "complete",
        Limits {
            chunk_chars: 420,
            ..Limits::default()
        },
    );
    set_article(&h, "", &format!("<p>{}</p>", long_paragraph(6, None)));
    h.start(StartMode::Default).await;
    let end = h.end().await;
    assert_eq!(end.phase, Phase::Completed);
    let document = cached_document(&h).await;
    let rrss_render::Block::Paragraph { inlines } = &document.blocks[0] else {
        panic!("paragraph");
    };
    let links = inlines
        .iter()
        .filter(|inline| matches!(inline, rrss_render::Inline::Link { .. }))
        .count();
    assert_eq!(links, 6, "all 6 links must be restored after segment split");

    // 场景 B：已降到下限仍返回未闭合标签（会跨片吞并后续段落）或无可见文字时，必须判失败。
    for bad in ["<x0>未闭合链接", "<x0></x0> <x1></x1>"] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(move |r: &Request| {
                let units = request_units(r);
                if units[0].t.starts_with("Sentence 0") {
                    ResponseTemplate::new(200).set_body_json(json!({
                        "choices":[{"message":{"content":json!({"units":[{"i":0,"t":bad}]}).to_string()}}],
                        "usage":{"prompt_tokens":10,"completion_tokens":5}
                    }))
                } else {
                    translated(&units)
                }
            })
            .mount(&server)
            .await;
        let mut h = Harness::http(
            &server.uri(),
            "complete",
            Limits {
                chunk_chars: 1,
                ..Limits::default()
            },
        );
        set_article(
            &h,
            "English title",
            &format!("<p>{}</p>", long_paragraph(4, None)),
        );
        h.start(StartMode::Default).await;
        let end = h.end().await;
        assert_eq!(end.phase, Phase::Partial, "{bad}");
        assert_eq!((end.done, end.failed), (1, 1), "{bad}");
    }
}

#[tokio::test]
async fn partial_missing_units_and_json_boundary_leak_retry_solo_once() {
    // 复现本地小模型在多单元 JSON 里漏写闭合引号：
    // 单元 1 译文尾部粘进 `}, {`，且单元 2 被截断缺失；
    // 两者均应被自动重排为单单元各补翻一次，最终 3/3 干净完成且不含 `}, {` 残片。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(|r: &Request| {
            let units = request_units(r);
            if units.len() > 1 {
                return ResponseTemplate::new(200).set_body_json(json!({
                    "choices":[{"message":{"content":r#"{"units":[{"i":0,"t":"标题"},{"i":1,"t":"第一段。}, {"}]}"#}}],
                    "usage":{"prompt_tokens":10,"completion_tokens":5}
                }));
            }
            translated(&units)
        })
        .mount(&server)
        .await;
    let mut h = Harness::http(&server.uri(), "complete", Limits::default());
    h.start(StartMode::Default).await;
    let end = h.end().await;
    assert_eq!(end.phase, Phase::Completed);
    assert_eq!((end.done, end.failed), (3, 0));
    let requests = server.received_requests().await.unwrap();
    // 1 次整窗（3 单元）+ 2 次单单元补翻（单元 1 与单元 2）。
    assert_eq!(requests.len(), 3);
    assert_eq!(request_units(&requests[1]).len(), 1);
    assert_eq!(request_units(&requests[2]).len(), 1);
    let doc = cached_document(&h).await;
    let rrss_render::Block::Paragraph { inlines } = &doc.blocks[0] else {
        panic!("paragraph");
    };
    let text = rrss_render::inlines_to_plain(inlines);
    assert!(!text.contains("}, {"), "leaked JSON boundary must be rejected and retried: {text}");
}
