use super::*;
use rrss_core::{ai_store, db};
use serde_json::json;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, method},
};

/// 两个源：1 号英文、2 号中文；文章 1–2 在 1 号源，3 在 2 号源。目标语言固定为中文。
fn setup(url: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("titles.db");
    let conn = db::open_at(&path).unwrap();
    translations::set_target_lang(&conn, "zh").unwrap();
    translations::set_backend(&conn, "http").unwrap();
    translations::set_http_config(&conn, Some(url), None).unwrap();
    conn.execute_batch(
        "INSERT INTO feeds (id, url, title, kind, created_at) VALUES
            (1, 'https://en.example/feed', 'En', 'rss', 1),
            (2, 'https://zh.example/feed', 'Zh', 'rss', 1);
         INSERT INTO articles (id, feed_id, title, hash, created_at) VALUES
            (1, 1, 'Quillstore rewrites its storage engine', 'h1', 1),
            (2, 1, 'Why async traits took five years', 'h2', 1),
            (3, 2, '老旧小区加装电梯的三种出资方式', 'h3', 1);",
    )
    .unwrap();
    (dir, path)
}

fn set(path: &Path, id: &str, value: &str) {
    crate::settings_schema::set(&db::open_at(path).unwrap(), id, value).unwrap();
}

fn card(id: i64, feed_id: i64, title: &str) -> ArticleView {
    serde_json::from_value(json!({
        "id": id, "title": title, "feed_id": feed_id, "feed_title": "", "feed_icon_href": "",
        "published_at": null, "url": "", "read": false, "score": 0, "summary": "",
        "row_kind": ROW_CARD, "cluster_id": 0, "cluster_size": 0, "cluster_label": "",
        "cluster_span": "", "noisy": false,
    }))
    .unwrap()
}

fn libre(texts: &[&str]) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "translatedText": texts }))
}

#[tokio::test]
async fn titles_stay_untouched_until_the_switch_is_on() {
    let server = MockServer::start().await;
    let (_dir, path) = setup(&server.uri());
    let conn = db::open_at(&path).unwrap();
    let mut rows = vec![card(1, 1, "Quillstore rewrites its storage engine")];
    annotate(&conn, &mut rows).unwrap();
    assert_eq!(rows[0].title_lang, None, "switch off: no prefix");
    let out = translate_titles(&path, &reqwest::Client::new(), vec![1])
        .await
        .unwrap();
    assert!(out.titles.is_empty() && out.paused.is_none());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn translates_only_foreign_titles_once_and_annotates_cards() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("Quillstore"))
        .respond_with(libre(&[
            "Quillstore 重写存储引擎",
            "为什么 async trait 花了五年",
        ]))
        .expect(1)
        .mount(&server)
        .await;
    let (_dir, path) = setup(&server.uri());
    set(&path, translations::AUTO_TITLES_KEY, "1");
    let client = reqwest::Client::new();
    let out = translate_titles(&path, &client, vec![3, 2, 1, 1, 999])
        .await
        .unwrap();
    let got: Vec<(i64, &str)> = out
        .titles
        .iter()
        .map(|t| (t.id, t.title.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            (1, "Quillstore 重写存储引擎"),
            (2, "为什么 async trait 花了五年")
        ]
    );
    // 已有译文的不再请求（mock 只允许一次），直接带回存下的译文。
    let again = translate_titles(&path, &client, vec![1, 2]).await.unwrap();
    assert_eq!(again.titles.len(), 2);

    let conn = db::open_at(&path).unwrap();
    let mut rows = vec![
        card(1, 1, "Quillstore rewrites its storage engine"),
        card(3, 2, "老旧小区加装电梯的三种出资方式"),
    ];
    annotate(&conn, &mut rows).unwrap();
    assert_eq!(rows[0].title_lang.as_deref(), Some("en"));
    assert_eq!(
        rows[0].title_translation.as_deref(),
        Some("Quillstore 重写存储引擎")
    );
    assert_eq!(rows[0].body_translated, Some(false));
    assert_eq!(rows[1].title_lang, None, "Chinese title is not foreign");
}

#[tokio::test]
async fn feed_overrides_and_scope_decide_which_titles_count() {
    let server = MockServer::start().await;
    let (_dir, path) = setup(&server.uri());
    let conn = db::open_at(&path).unwrap();
    translations::set_feed_auto(&conn, 1, FeedAuto::Always).unwrap();
    let mut rows = vec![card(1, 1, "Quillstore rewrites its storage engine")];
    annotate(&conn, &mut rows).unwrap();
    assert_eq!(
        rows[0].title_lang.as_deref(),
        Some("en"),
        "always wins over global off"
    );

    set(&path, translations::AUTO_TITLES_KEY, "1");
    translations::set_feed_auto(&conn, 1, FeedAuto::Never).unwrap();
    let mut rows = vec![card(1, 1, "Quillstore rewrites its storage engine")];
    annotate(&conn, &mut rows).unwrap();
    assert_eq!(rows[0].title_lang, None, "never wins over global on");

    translations::set_feed_auto(&conn, 1, FeedAuto::Follow).unwrap();
    set(&path, translations::AUTO_SCOPE_KEY, "en");
    let mut rows = vec![card(1, 1, "東京の自転車通勤、この10年でどう変わったか")];
    annotate(&conn, &mut rows).unwrap();
    assert_eq!(
        rows[0].title_lang, None,
        "English-only scope skips Japanese"
    );

    translations::set_enabled(&conn, false).unwrap();
    let out = translate_titles(&path, &reqwest::Client::new(), vec![1])
        .await
        .unwrap();
    assert_eq!(out.paused.as_deref(), Some(PAUSED_DISABLED));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn model_backend_pauses_when_the_generation_budget_is_spent() {
    let server = MockServer::start().await;
    let (_dir, path) = setup(&server.uri());
    let conn = db::open_at(&path).unwrap();
    let provider = ai_store::insert_provider(
        &conn,
        &ai_store::NewProvider {
            name: "Mock model".into(),
            base_url: server.uri(),
            api_key: Some("private-key".into()),
            protocol: "chat".into(),
        },
    )
    .unwrap();
    ai_store::set_task_binding(
        &conn,
        ai_store::TASK_ARTICLE_TRANSLATE,
        Some(provider),
        Some("m"),
    )
    .unwrap();
    translations::set_backend(&conn, "complete").unwrap();
    set(&path, translations::AUTO_TITLES_KEY, "1");
    ai_store::set_daily_budget_tokens(&conn, 10).unwrap();
    ai_store::add_usage(&conn, &ai_store::local_day(), 10, 5).unwrap();
    let out = translate_titles(&path, &reqwest::Client::new(), vec![1])
        .await
        .unwrap();
    assert_eq!(out.paused.as_deref(), Some(PAUSED_BUDGET));
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn failed_batches_store_nothing_and_report_the_failure_kind() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(libre(&["只有一条"]))
        .mount(&server)
        .await;
    let (_dir, path) = setup(&server.uri());
    set(&path, translations::AUTO_TITLES_KEY, "1");
    let err = translate_titles(&path, &reqwest::Client::new(), vec![1, 2])
        .await
        .unwrap_err();
    assert_eq!(err.kind, TranslationFailure::InvalidOutput);
    let conn = db::open_at(&path).unwrap();
    assert!(
        translations::title_translations(&conn, &[1, 2], "zh")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn reader_states_follow_switches_length_and_cache() {
    let (_dir, path) = setup("http://127.0.0.1:9");
    let conn = db::open_at(&path).unwrap();
    let article = rrss_core::articles::get_article(&conn, 1).unwrap();
    let doc = rrss_render::Document {
        v: rrss_render::IR_VERSION,
        blocks: vec![rrss_render::Block::Paragraph {
            inlines: vec![rrss_render::Inline::Text {
                text: "The maintainers say write throughput rose by about forty percent.".into(),
            }],
        }],
    };
    assert!(
        reader_auto(&conn, &article, &doc, false).unwrap().is_none(),
        "both off"
    );

    set(&path, translations::AUTO_TITLES_KEY, "1");
    let view = reader_auto(&conn, &article, &doc, false).unwrap().unwrap();
    assert_eq!((view.lang.as_str(), view.body), ("en", AutoBodyState::Off));

    set(&path, translations::AUTO_BODY_KEY, "1");
    let view = reader_auto(&conn, &article, &doc, false).unwrap().unwrap();
    assert_eq!(view.body, AutoBodyState::Start);
    assert_eq!(view.view, "bilingual");
    assert_eq!(
        reader_auto(&conn, &article, &doc, true)
            .unwrap()
            .unwrap()
            .body,
        AutoBodyState::Cached
    );
    set(&path, translations::AUTO_MAX_WORDS_KEY, "500");
    let long = rrss_render::Document {
        v: rrss_render::IR_VERSION,
        blocks: vec![rrss_render::Block::Paragraph {
            inlines: vec![rrss_render::Inline::Text {
                text: "the storage engine ".repeat(200),
            }],
        }],
    };
    let view = reader_auto(&conn, &article, &long, false).unwrap().unwrap();
    assert_eq!((view.body, view.max_words), (AutoBodyState::TooLong, 500));
    assert!(view.words > 500);

    translations::set_feed_auto(&conn, 1, FeedAuto::Never).unwrap();
    assert!(reader_auto(&conn, &article, &doc, false).unwrap().is_none());
}
