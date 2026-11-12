use super::*;
use crate::ReaderView;
use rrss_core::{db, retention};
use rrss_render::{already_target_lang, apply_units, extract_units, units_plain};
use serde_json::json;
use std::path::PathBuf;
use wiremock::{Mock, MockServer, Request, ResponseTemplate, matchers::method};

fn article_db(url: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("translate.db");
    let conn = db::open_at(&path).unwrap();
    conn.execute(
        "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://feed.test/rss', 1)",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO articles (id, feed_id, title, content_html, hash, created_at) VALUES (7, 1, 'An article', '<p>The original article body.</p>', 'feed-hash', 1)", []).unwrap();
    translations::set_backend(&conn, ENGINE_HTTP).unwrap();
    translations::set_http_config(&conn, Some(url), None).unwrap();
    (dir, path)
}

fn http_reply(request: &Request) -> ResponseTemplate {
    let body: Value = request.body_json().unwrap();
    let text = match &body["q"] {
        Value::Array(items) => json!(items.iter().map(|_| "已翻译").collect::<Vec<_>>()),
        _ => json!("已翻译"),
    };
    ResponseTemplate::new(200).set_body_json(json!({"translatedText": text}))
}

fn fulltext(conn: &rusqlite::Connection, body: &str) {
    let doc = rrss_render::markdown_to_document(body);
    articles::set_fulltext(conn, 7, body, &rrss_render::document_to_json(&doc).unwrap()).unwrap();
}

#[tokio::test]
async fn actual_body_and_title_control_cache_even_when_feed_hash_is_unchanged() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(http_reply)
        .mount(&server)
        .await;
    let (_dir, path) = article_db(&server.uri());
    let client = reqwest::Client::new();
    assert!(
        translate_article(&path, &client, 7)
            .await
            .unwrap()
            .translation_ready
    );
    let conn = db::open_at(&path).unwrap();
    assert!(
        crate::queries::reader_view(&conn, 7)
            .unwrap()
            .translation_ready
    );
    fulltext(&conn, "The original article body.");
    assert!(
        crate::queries::reader_view(&conn, 7)
            .unwrap()
            .translation_ready,
        "same actual document preserves translation"
    );
    assert!(
        translate_article(&path, &client, 7)
            .await
            .unwrap()
            .translation_ready
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);

    fulltext(&conn, "A completely different full article body.");
    assert_eq!(
        articles::get_article(&conn, 7).unwrap().hash.as_deref(),
        Some("feed-hash")
    );
    assert!(
        !crate::queries::reader_view(&conn, 7)
            .unwrap()
            .translation_ready,
        "same block count is not the same content"
    );
    assert!(
        translate_article(&path, &client, 7)
            .await
            .unwrap()
            .translation_ready
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    conn.execute("UPDATE articles SET title='A new title' WHERE id=7", [])
        .unwrap();
    assert!(
        !crate::queries::reader_view(&conn, 7)
            .unwrap()
            .translation_ready
    );
}

#[tokio::test]
async fn changed_or_pruned_source_and_changed_configuration_reject_inflight_commit() {
    for change in ["body", "prune", "endpoint", "disabled", "credential"] {
        let server = MockServer::start().await;
        let (_dir, path) = article_db(&server.uri());
        let mutate_path = path.clone();
        Mock::given(method("POST"))
            .respond_with(move |request: &Request| {
                let conn = db::open_at(&mutate_path).unwrap();
                match change {
                    "body" => fulltext(&conn, "Updated while the request was running."),
                    "prune" => {
                        let tx = db::write_transaction(&conn).unwrap();
                        retention::prune_articles(&tx, &[7], 2).unwrap();
                        tx.commit().unwrap();
                    }
                    "endpoint" => {
                        translations::set_http_config(&conn, Some("https://different.test"), None)
                            .unwrap()
                    }
                    "disabled" => translations::set_enabled(&conn, false).unwrap(),
                    "credential" => {
                        translations::set_http_config(&conn, None, Some("replacement-secret"))
                            .unwrap()
                    }
                    _ => unreachable!(),
                }
                http_reply(request)
            })
            .mount(&server)
            .await;
        let result = translate_article(&path, &reqwest::Client::new(), 7).await;
        assert!(
            result.is_err(),
            "{change}: stale translation must not commit"
        );
        let conn = db::open_at(&path).unwrap();
        assert!(
            translations::get(&conn, 7, "zh", ENGINE_HTTP)
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn damaged_new_cache_is_rejected_and_pruned_articles_do_not_request_translation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(http_reply)
        .mount(&server)
        .await;
    let (_dir, path) = article_db(&server.uri());
    translate_article(&path, &reqwest::Client::new(), 7)
        .await
        .unwrap();
    let conn = db::open_at(&path).unwrap();
    let original = translations::get(&conn, 7, "zh", ENGINE_HTTP)
        .unwrap()
        .unwrap();
    for change in ["version", "config", "length", "block"] {
        let mut row = original.clone();
        let mut json: Value = serde_json::from_str(&row.blocks_json).unwrap();
        match change {
            "version" => json["version"] = json!(999),
            "config" => json["config_hash"] = json!("different"),
            "length" => json["document"]["blocks"] = json!([]),
            "block" => {
                json["document"]["blocks"][0] =
                    json!({"t":"img", "src":"https://wrong.test", "alt":""})
            }
            _ => unreachable!(),
        }
        row.blocks_json = json.to_string();
        translations::upsert(&conn, &row).unwrap();
        assert!(
            !crate::queries::reader_view(&conn, 7)
                .unwrap()
                .translation_ready,
            "{change}"
        );
    }
    let tx = db::write_transaction(&conn).unwrap();
    retention::prune_articles(&tx, &[7], 2).unwrap();
    tx.commit().unwrap();
    let view = translate_article(&path, &reqwest::Client::new(), 7)
        .await
        .unwrap();
    assert!(view.pruned && !view.translation_ready);
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn complete_windows_keep_the_original_model_and_refuse_changed_binding_at_commit() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let server = MockServer::start().await;
    let (_dir, path) = article_db(&server.uri());
    let conn = db::open_at(&path).unwrap();
    let pid = ai_store::insert_provider(
        &conn,
        &ai_store::NewProvider {
            name: "fixed".into(),
            base_url: server.uri(),
            api_key: None,
            protocol: "chat".into(),
        },
    )
    .unwrap();
    ai_store::set_task_binding(
        &conn,
        TASK_ARTICLE_TRANSLATE,
        Some(pid),
        Some("original-model"),
    )
    .unwrap();
    translations::set_backend(&conn, ENGINE_COMPLETE).unwrap();
    fulltext(
        &conn,
        &format!(
            "{}\n\n{}",
            "long paragraph ".repeat(140),
            "second paragraph ".repeat(140)
        ),
    );
    let mutate_path = path.clone();
    let changed = Arc::new(AtomicBool::new(false));
    Mock::given(method("POST"))
        .respond_with(move |request: &Request| {
            if !changed.swap(true, Ordering::SeqCst) {
                let conn = db::open_at(&mutate_path).unwrap();
                ai_store::set_task_binding(
                    &conn,
                    TASK_ARTICLE_TRANSLATE,
                    Some(pid),
                    Some("new-model"),
                )
                .unwrap();
            }
            complete_reply("译文")(request)
        })
        .mount(&server)
        .await;
    assert!(
        translate_article(&path, &reqwest::Client::new(), 7)
            .await
            .is_err()
    );
    let requests = server.received_requests().await.unwrap();
    assert!(requests.len() >= 2);
    assert!(
        requests
            .iter()
            .all(|r| r.body_json::<Value>().unwrap()["model"] == "original-model")
    );
    assert!(
        translations::get(&conn, 7, "zh", ENGINE_COMPLETE)
            .unwrap()
            .is_none()
    );
    let usage: i64 = conn
        .query_row(
            "SELECT SUM(prompt_tokens + completion_tokens) FROM ai_usage",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(usage, requests.len() as i64 * 15);
}

#[test]
fn incomplete_or_invalid_unit_output_cannot_be_accepted_as_a_complete_cache() {
    let source = vec![
        TranslateUnit {
            i: 3,
            t: "one".into(),
        },
        TranslateUnit {
            i: 9,
            t: "two".into(),
        },
    ];
    for raw in [
        json!({"units":[{"i":3,"t":"一"}]}),
        json!({"units":[{"i":3,"t":"一"},{"i":3,"t":"二"}]}),
        json!({"units":[{"i":3,"t":"一"},{"i":99,"t":"二"}]}),
        json!({"units":[{"i":3,"t":"一"},{"i":9,"t":"  "}]}),
        json!({"units":[{"i":3,"t":"一"},{"i":4294967305u64,"t":"二"}]}),
        json!({"units":[{"i":3,"t":"一"},{"i":9,"t":"二"},{"i":"bad","t":"三"}]}),
    ] {
        assert!(
            parse_units(&raw)
                .and_then(|units| validate_units(&source, &units))
                .is_err()
        );
    }
    let valid = parse_units(&json!({"units":[{"i":9,"t":"二"},{"i":3,"t":"一"}]})).unwrap();
    validate_units(&source, &valid).unwrap();
}

#[tokio::test]
async fn legacy_and_malformed_caches_are_not_attached_or_silently_retranslated() {
    let (_dir, path) = article_db("http://127.0.0.1:1");
    let conn = db::open_at(&path).unwrap();
    for blocks_json in [
        r#"{"v":1,"blocks":[{"t":"p","inlines":[{"t":"t","text":"旧译文"}]}]}"#,
        r#"{"unknown":true}"#,
    ] {
        translations::upsert(
            &conn,
            &ArticleTranslation {
                article_id: 7,
                target_lang: "zh".into(),
                source_hash: "feed-hash".into(),
                engine: ENGINE_HTTP.into(),
                model: None,
                prompt_version: 1,
                title: "旧标题".into(),
                blocks_json: blocks_json.into(),
                created_at: 1,
            },
        )
        .unwrap();
        assert!(
            !crate::queries::reader_view(&conn, 7)
                .unwrap()
                .translation_ready
        );
        assert!(
            translations::get(&conn, 7, "zh", ENGINE_HTTP)
                .unwrap()
                .is_some(),
            "reading must not delete old cache"
        );
    }
}

fn complete_reply(word: &'static str) -> impl Fn(&Request) -> ResponseTemplate {
    move |request: &Request| {
        let body: Value = request.body_json().unwrap();
        let user = body["messages"].as_array().unwrap().last().unwrap()["content"]
            .as_str()
            .unwrap();
        let (_, input) = user.split_once('\n').unwrap();
        let units: Vec<TranslateUnit> = serde_json::from_str(input).unwrap();
        let units: Vec<_> = units
            .into_iter()
            .map(|u| json!({"i":u.i,"t":word}))
            .collect();
        ResponseTemplate::new(200).set_body_json(json!({
            "choices":[{"message":{"content":json!({"units":units}).to_string()}}],
            "usage":{"prompt_tokens":10,"completion_tokens":5}
        }))
    }
}

#[tokio::test]
async fn complete_cache_separates_endpoints_and_prompts_but_ignores_display_name() {
    let first = MockServer::start().await;
    let second = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(complete_reply("第一份译文"))
        .mount(&first)
        .await;
    Mock::given(method("POST"))
        .respond_with(complete_reply("第二份译文"))
        .mount(&second)
        .await;
    let (_dir, path) = article_db(&first.uri());
    let conn = db::open_at(&path).unwrap();
    let pid = ai_store::insert_provider(
        &conn,
        &ai_store::NewProvider {
            name: "test".into(),
            base_url: first.uri(),
            api_key: Some("private-key".into()),
            protocol: "auto".into(),
        },
    )
    .unwrap();
    ai_store::set_task_binding(&conn, TASK_ARTICLE_TRANSLATE, Some(pid), Some("same-model"))
        .unwrap();
    translations::set_backend(&conn, ENGINE_COMPLETE).unwrap();
    let client = reqwest::Client::new();
    assert_eq!(
        translate_article(&path, &client, 7)
            .await
            .unwrap()
            .translated_title,
        "第一份译文"
    );
    assert_eq!(
        ai_store::get_provider(&conn, pid).unwrap().protocol,
        "auto",
        "fixed translation config must not mutate itself after detection"
    );
    // 摘要 / 简报在同一连接上 Auto 探测后写回协议：不是用户改配置，译文与在途作业都不失效。
    let revision = crate::configuration::translation_revision(&conn).unwrap();
    crate::configuration::write(&conn, |tx| ai_store::set_provider_protocol(tx, pid, "chat"))
        .unwrap();
    assert_eq!(
        crate::configuration::translation_revision(&conn).unwrap(),
        revision
    );
    assert!(
        crate::queries::reader_view(&conn, 7)
            .unwrap()
            .translation_ready,
        "detected protocol write-back keeps the completed cache"
    );
    let mut provider = ai_store::get_provider(&conn, pid).unwrap();
    provider.name = "renamed".into();
    provider.api_key = None;
    ai_store::update_provider(&conn, &provider).unwrap();
    assert!(
        crate::queries::reader_view(&conn, 7)
            .unwrap()
            .translation_ready,
        "completed cache survives display name / credential changes"
    );
    provider.base_url = second.uri();
    ai_store::update_provider(&conn, &provider).unwrap();
    assert!(
        !crate::queries::reader_view(&conn, 7)
            .unwrap()
            .translation_ready
    );
    assert_eq!(
        translate_article(&path, &client, 7)
            .await
            .unwrap()
            .translated_title,
        "第二份译文"
    );
    assert_eq!(
        second.received_requests().await.unwrap().len(),
        1,
        "same model name must not reuse another endpoint's AI cache"
    );
    let task = ai_store::resolve_task(&conn, TASK_ARTICLE_TRANSLATE).unwrap();
    conn.execute(
        "UPDATE prompt_templates SET body=body || ' changed' WHERE id=?1",
        [task.template.id],
    )
    .unwrap();
    assert!(
        !crate::queries::reader_view(&conn, 7)
            .unwrap()
            .translation_ready,
        "prompt content matters even without a version bump"
    );
    translate_article(&path, &client, 7).await.unwrap();
    assert_eq!(second.received_requests().await.unwrap().len(), 2);
    let row = translations::get(&conn, 7, "zh", ENGINE_COMPLETE)
        .unwrap()
        .unwrap();
    assert!(!row.blocks_json.contains("private-key"));
    assert!(!row.blocks_json.contains(&second.uri()));
}

// 旧整篇路径只作为缓存兼容 / 事务回归夹具，生产使用 runtime + ipc。
async fn translate_article(
    db_path: &Path,
    client: &reqwest::Client,
    id: i64,
) -> Result<ReaderView> {
    use std::ops::ControlFlow;

    let path = db_path.to_path_buf();
    let prepared = spawn_db(path.clone(), move |conn| {
        // 所有准备读取固定在同一个 SQLite 快照，网络等待前结束事务。
        let tx = conn.unchecked_transaction()?;
        let article = articles::get_article(&tx, id)?;
        if !translations::is_enabled(&tx)? {
            return Ok(ControlFlow::Break(
                "翻译功能未开启，请先在「设置 → 翻译」中启用",
            ));
        }
        if article.pruned_at.is_some() {
            return Ok(ControlFlow::Break("正文已清理，请先重新抓取正文"));
        }
        let doc = crate::document::load_document(&article);
        let units = extract_units(&article.title, &doc);
        if units.is_empty() {
            return Ok(ControlFlow::Break("没有可翻译的正文"));
        }
        let lang = translations::target_lang(&tx)?;
        if already_target_lang(&units_plain(&units), &lang) {
            return Ok(ControlFlow::Break("已是目标语言，无需翻译"));
        }
        let config = Config::read(&tx)?;
        let ready = cache::read(&tx, &article, &doc, &config)?.is_some();
        tx.commit()?;
        Ok(ControlFlow::Continue((article, doc, units, config, ready)))
    })
    .await?;
    let (article, doc, units, config, ready) = match prepared {
        ControlFlow::Break(note) => return with_note(db_path, id, note).await,
        ControlFlow::Continue(input) => input,
    };
    let expected_hash = crate::document::source_hash(&article.title, &doc);
    if ready {
        return spawn_db(path, move |conn| crate::queries::reader_view(&conn, id)).await;
    }
    let merged = translate_units(&config, db_path, client, &units, false).await?;
    validate_units(&units, &merged)?;
    let (title, translated) = apply_units(&article.title, &doc, &merged);
    let article_id = article.id;
    spawn_db(path, move |conn| {
        cache::commit(conn, article_id, &expected_hash, &config, title, translated)?;
        crate::queries::reader_view(&conn, article_id)
    })
    .await
}

async fn with_note(db_path: &Path, id: i64, note: &str) -> Result<ReaderView> {
    let path = db_path.to_path_buf();
    let note = note.to_string();
    spawn_db(path, move |conn| {
        let mut view = crate::queries::reader_view(&conn, id)?;
        view.translation_note = note;
        Ok(view)
    })
    .await
}
