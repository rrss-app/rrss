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
    conn.execute(
        "UPDATE articles SET title=?1 WHERE id=1",
        ["東京の自転車通勤、この10年でどう変わったか"],
    )
    .unwrap();
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

#[test]
fn stored_languages_and_legacy_fallback_agree_without_writing() {
    let (_dir, path) = setup("http://127.0.0.1:9");
    set(&path, translations::AUTO_TITLES_KEY, "1");
    let conn = db::open_at(&path).unwrap();
    conn.execute_batch("UPDATE feeds SET language=CASE id WHEN 1 THEN 'en' ELSE 'zh' END;
        UPDATE articles SET title='Docker 28 beta',lang='en',content_html='<p>中文正文</p>' WHERE id=1;
        UPDATE articles SET title='PostgreSQL 18' WHERE id=2;
        UPDATE articles SET title='iPhone 17 Pro' WHERE id=3;
        INSERT INTO articles(id,feed_id,title,lang,created_at) VALUES(4,1,'Docker 28 beta','zh',0),(5,1,'Fixture',NULL,0),(6,1,'Fixture',NULL,0);").unwrap();
    let chinese = "这是关于城市公共图书馆的报道，介绍了新的借阅规则。".repeat(15);
    conn.execute(
        "UPDATE articles SET content_html=?1 WHERE id=5",
        [format!("<p>{chinese}</p>")],
    )
    .unwrap();
    conn.execute("UPDATE articles SET fulltext_md=?1 WHERE id=6", [&chinese])
        .unwrap();
    conn.pragma_update(None, "query_only", true).unwrap();
    let tx = conn.unchecked_transaction().unwrap();
    let ids = [1, 2, 3, 4, 5, 6];
    let mut cards: Vec<_> = ids
        .iter()
        .map(|id| card(*id, if *id == 3 { 2 } else { 1 }, "unused card text"))
        .collect();
    annotate(&tx, &mut cards).unwrap();
    let annotated: Vec<_> = cards
        .iter()
        .filter_map(|a| a.title_lang.as_ref().map(|lang| (a.id, lang.as_str())))
        .collect();
    assert_eq!(annotated, vec![(1, "en"), (2, "en")]);
    let Prepared::Ready { items, .. } = prepare(&tx, &ids).unwrap() else {
        panic!("ready");
    };
    assert_eq!(
        items.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(
        tx.query_row(
            "SELECT count(*) FROM articles WHERE lang IS NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        4
    );
    for id in [2, 3, 5, 6] {
        let article = rrss_core::articles::get_article(&tx, id).unwrap();
        let doc = crate::document::load_document(&article);
        let reader = reader_auto(&tx, &article, &doc, false).unwrap();
        assert_eq!(reader.map(|r| r.lang), cards[(id - 1) as usize].title_lang);
    }
    tx.commit().unwrap();
}

#[test]
fn reader_preserves_entry_language_and_pruned_article_language() {
    let (_dir, path) = setup("http://127.0.0.1:9");
    set(&path, translations::AUTO_TITLES_KEY, "1");
    let conn = db::open_at(&path).unwrap();
    conn.execute_batch("UPDATE feeds SET language='en';
        UPDATE articles SET title='東京都議会選挙',lang='ja',content_html='<p>東京都</p>' WHERE id=1;
        UPDATE articles SET title='iPhone 17 Pro',lang='zh',pruned_at=1 WHERE id=2;").unwrap();
    for id in [1, 2] {
        let article = rrss_core::articles::get_article(&conn, id).unwrap();
        let doc = crate::document::load_document(&article);
        let mut cards = vec![card(id, 1, &article.title)];
        annotate(&conn, &mut cards).unwrap();
        assert_eq!(
            reader_auto(&conn, &article, &doc, false)
                .unwrap()
                .map(|r| r.lang),
            cards[0].title_lang
        );
    }
    // Synthetic manual sources never supply priors to old title-only articles.
    let feed = rrss_core::feeds::manual_feed(&conn).unwrap();
    conn.execute("UPDATE feeds SET language='zh' WHERE id=?1", [feed])
        .unwrap();
    conn.execute("UPDATE articles SET feed_id=?1,title='Why async traits took five years',lang=NULL,content_html=NULL WHERE id=1",[feed]).unwrap();
    let article = rrss_core::articles::get_article(&conn, 1).unwrap();
    assert_eq!(
        reader_auto(&conn, &article, &rrss_render::Document::new(vec![]), false)
            .unwrap()
            .unwrap()
            .lang,
        "en"
    );
}

#[test]
fn legacy_html_fallback_ignores_code_and_style_like_reader() {
    let (_dir, path) = setup("http://127.0.0.1:9");
    set(&path, translations::AUTO_TITLES_KEY, "1");
    let conn = db::open_at(&path).unwrap();
    let code =
        "pub fn handle_request(context: &Context) -> Result<(), Error> { Ok(()) }\n".repeat(10);
    let html = format!(
        "<style>.container {{ font-family: sans-serif; margin: 0 auto; }}</style><pre><code>{code}</code></pre><p>这篇中文文章记录了图书馆系统的升级步骤和注意事项。</p>"
    );
    conn.execute(
        "UPDATE articles SET title='Fixture',lang=NULL,content_html=?1 WHERE id=1",
        [&html],
    )
    .unwrap();
    let mut cards = vec![card(1, 1, "Fixture")];
    annotate(&conn, &mut cards).unwrap();
    assert_eq!(cards[0].title_lang, None);
    let Prepared::Ready { items, .. } = prepare(&conn, &[1]).unwrap() else {
        panic!("ready");
    };
    assert!(items.is_empty());
    let article = rrss_core::articles::get_article(&conn, 1).unwrap();
    let doc = crate::document::load_document(&article);
    assert!(reader_auto(&conn, &article, &doc, false).unwrap().is_none());
}
