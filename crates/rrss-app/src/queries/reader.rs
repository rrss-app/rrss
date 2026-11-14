use std::path::Path;

use rusqlite::Connection;

use rrss_core::articles;
use rrss_core::feedback::{self, FeedbackKind};
use rrss_core::interests;
use rrss_core::parse_reasons;
use rrss_render::{Block, document_to_json};

use super::media::{cached_media_ref, failed_media_ref};
use crate::document::article_truncated;
use crate::document::load_document;
use crate::embed_fetch::{self, present_document};
use crate::views::*;

pub fn reading_speeds(conn: &Connection) -> rrss_core::Result<ReadingSpeedsView> {
    use rrss_core::rank::engagement::{self, Language};
    let until = chrono::Utc::now().timestamp_millis();
    let sessions = rrss_core::signals::load_session_summaries(
        conn,
        until.saturating_sub(rrss_core::signals::RETENTION_MS),
        until,
    )?;
    let history = engagement::merge_daily_sessions(&sessions, 0);
    let speed = |lang, translated| {
        let value = engagement::estimate_reading_speed(&history, lang, translated);
        (!value.uses_default).then(|| 1.0 / (lang.reading_units(1) * value.ms_per_unit))
    };
    Ok(ReadingSpeedsView {
        chinese_original: speed(Language::Chinese, false),
        chinese_translated: speed(Language::Chinese, true),
        english_original: speed(Language::English, false),
        english_translated: speed(Language::English, true),
    })
}

pub fn article_reasons(article: &rrss_core::ArticleRow) -> Vec<ReasonView> {
    let mut chips: Vec<ReasonView> = parse_reasons(article.reasons_json.as_deref())
        .into_iter()
        .map(ReasonView::from)
        .collect();
    chips.extend(
        interests::parse_interest_reasons(article.interest_reasons_json.as_deref())
            .into_iter()
            .map(|reason| ReasonView {
                rule_id: 0,
                dimension: "interest".into(),
                snippet: reason.label.clone(),
                weight: reason.weight,
                message: reason.message.into(),
            }),
    );
    chips
}

pub fn reader_view(conn: &Connection, id: i64) -> rrss_core::Result<ReaderView> {
    let article = articles::get_article(conn, id)?;
    let mut doc = load_document(&article);
    let source_hash = crate::document::source_hash(&article.title, &doc);
    let translation = crate::translate::cache::for_reader(conn, &article, &doc)?;
    present_document(&mut doc);
    let media = article_media_from_doc(conn, article.id, &doc);
    let embed_covers = embed_fetch::cached_covers(conn, article.id, &doc);
    let feedback = match feedback::feedback_for_article(conn, id).ok().flatten() {
        Some(FeedbackKind::Up) => 1,
        Some(FeedbackKind::Down) => -1,
        None => 0,
    };
    let meta = format!(
        "{} · {}",
        article.feed_title,
        format_time(article.published_at)
    );
    let view = ReaderView {
        id: article.id,
        web_extracted_at: conn.query_row(
            "SELECT web_extracted_at FROM articles WHERE id=?1",
            [id],
            |row| row.get(0),
        )?,
        source_hash,
        title: article.title.clone(),
        meta,
        feed_icon_href: super::feeds::feed_icon_href(conn, article.feed_id)?,
        url: article.canonical_url.clone().unwrap_or_default(),
        summary: article.summary.clone().unwrap_or_default(),
        score: article.score,
        feedback,
        document: serde_json::from_str(&document_to_json(&doc).unwrap_or_else(|_| "{}".into()))
            .unwrap_or(serde_json::json!({"v":1,"blocks":[]})),
        reasons: article_reasons(&article),
        media,
        embed_covers,
        translated_title: String::new(),
        translated_document: None,
        translation_ready: false,
        translation_note: String::new(),
        truncated: article_truncated(&article),
        pruned: article.pruned_at.is_some(),
        summary_stale: article
            .summary
            .as_deref()
            .is_some_and(|t| !t.trim().is_empty())
            && rrss_core::summary_extract::summary_is_stale(
                article.summary_json.as_deref(),
                article.blocks_json.as_deref(),
                article.content_html.as_deref(),
            ),
        starred: article.starred,
    };
    Ok(attach_translation(translation, view))
}

pub fn related_articles(
    conn: &Connection,
    article_id: i64,
    limit: i64,
) -> rrss_core::Result<Vec<RelatedArticleView>> {
    articles::get_article(conn, article_id)?;
    let articles = rrss_core::rag::related_articles(conn, article_id, limit)?;
    if articles.is_empty() {
        return Ok(Vec::new());
    }
    let icons = super::feeds::feed_icon_hrefs(conn)?;
    Ok(articles
        .into_iter()
        .map(|article| RelatedArticleView {
            article_id: article.article_id,
            feed_id: article.feed_id,
            feed_title: rrss_core::feeds::display_title(&article.feed_title, ""),
            icon_href: icons.get(&article.feed_id).cloned().unwrap_or_default(),
            title: article.title,
            published_at: article.published_at,
            one_liner: rrss_core::digest::parse_summary_details_for_digest(
                &article.summary,
                article.summary_json.as_deref(),
            )
            .0,
            read: article.read,
            similarity: article.similarity,
        })
        .collect())
}

fn attach_translation(
    translation: Option<(String, rrss_render::Document)>,
    mut view: ReaderView,
) -> ReaderView {
    let Some((title, mut translated)) = translation else {
        return view;
    };
    present_document(&mut translated);
    view.translated_title = title;
    view.translated_document = serde_json::to_value(translated).ok();
    view.translation_ready = view.translated_document.is_some();
    view
}

pub fn article_media(conn: &Connection, id: i64) -> rrss_core::Result<Vec<MediaRef>> {
    let article = articles::get_article(conn, id)?;
    let doc = load_document(&article);
    Ok(article_media_from_doc(conn, article.id, &doc))
}

pub fn article_media_from_doc(
    conn: &Connection,
    article_id: i64,
    doc: &rrss_render::Document,
) -> Vec<MediaRef> {
    let mut media = Vec::new();
    for block in &doc.blocks {
        if let Block::Image { src, .. } = block {
            let cached = rrss_core::media::lookup(conn, article_id, src)
                .ok()
                .flatten();
            media.push(match cached {
                Some(ref m)
                    if m.local_path
                        .as_ref()
                        .map_or(false, |p| Path::new(p).exists()) =>
                {
                    cached_media_ref(m, src)
                }
                // 失败原因只在下载作业的进程内存里，由 `jobs::media` 补上。
                _ => failed_media_ref(src, ""),
            });
        }
    }
    media
}

pub fn pending_images(conn: &Connection, id: i64) -> Vec<(String, String)> {
    let Ok(article) = articles::get_article(conn, id) else {
        return Vec::new();
    };
    let referer = article.canonical_url.clone().unwrap_or_default();
    load_document(&article)
        .blocks
        .into_iter()
        .filter_map(|b| match b {
            Block::Image { src, .. } if rrss_core::media::allowed_url(&src) => {
                Some((src, referer.clone()))
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::articles;
    use rrss_core::db;
    use rrss_core::error::Error;
    use rrss_core::feeds::{SubscribeRequest, subscribe};
    use rrss_core::parse::ParsedEntry;

    fn open_temp() -> (tempfile::TempDir, rrss_core::db::PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        (dir, conn)
    }

    fn seed_article(conn: &rusqlite::Connection, guid: &str, title: &str) -> i64 {
        let feed = subscribe(
            conn,
            SubscribeRequest {
                url: format!("https://news.example.com/{guid}.xml"),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        rrss_core::ingest::upsert_entries(
            conn,
            feed,
            &[ParsedEntry {
                guid: guid.into(),
                canonical_url: Some(format!("https://news.example.com/{guid}")),
                title: title.into(),
                author: None,
                published_at: Some(1),
                content_html: Some("<p>body</p>".into()),
                hash: guid.into(),
                tags: vec![],
            }],
            1,
        )
        .unwrap();
        conn.query_row("SELECT id FROM articles WHERE guid = ?1", [guid], |row| {
            row.get(0)
        })
        .unwrap()
    }

    #[test]
    fn old_reason_rows_keep_legacy_text_and_new_rows_expose_codes_without_rewriting_old_rows() {
        use rrss_core::rules::{Dimension, NewRule, Polarity, insert_rule};
        let (_dir, conn) = open_temp();
        insert_rule(
            &conn,
            NewRule::manual_global(Polarity::Like, Dimension::TitleKw, "Rust"),
        )
        .unwrap();
        let old = seed_article(&conn, "old", "Rust archive");
        let current = seed_article(&conn, "current", "Rust current");
        let old_rules = r#"[{"rule_id":1,"dimension":"title_kw","snippet":"Rust","text":"old rule sentence","weight":10,"points":"ignored legacy field"}]"#;
        let old_interests =
            r#"[{"interest_id":2,"label":"Archive","weight":2,"points":"ignored legacy field"}]"#;
        conn.execute(
            "UPDATE articles SET reasons_json=?1,interest_reasons_json=?2 WHERE id=?3",
            rusqlite::params![old_rules, old_interests, old],
        )
        .unwrap();
        conn.execute("UPDATE articles SET interest_reasons_json=?1 WHERE id=?2", rusqlite::params![r#"[{"component":"interest","code":"interest_match","params":{"pattern":"Current"},"points":2,"weight":9,"interest_id":2,"label":"Current"}]"#,current]).unwrap();
        let legacy = serde_json::to_value(reader_view(&conn, old).unwrap()).unwrap();
        assert_eq!(legacy["reasons"][0]["code"], "legacy_text");
        assert_eq!(
            legacy["reasons"][0]["params"],
            serde_json::json!({"text":"old rule sentence","prefix":null})
        );
        assert_eq!(legacy["reasons"][1]["code"], "legacy_text");
        assert_eq!(
            legacy["reasons"][1]["params"],
            serde_json::json!({"text":"Archive","prefix":"interest"})
        );
        let coded = serde_json::to_value(reader_view(&conn, current).unwrap()).unwrap();
        assert_eq!(coded["reasons"][0]["code"], "rule_title_kw");
        assert_eq!(
            coded["reasons"][0]["params"],
            serde_json::json!({"pattern":"Rust"})
        );
        assert_eq!(coded["reasons"][1]["code"], "interest_match");
        assert_eq!(
            coded["reasons"][1]["params"],
            serde_json::json!({"pattern":"Current"})
        );
        assert_eq!(coded["reasons"][1]["weight"], 2);
        let row = articles::get_article(&conn, old).unwrap();
        assert_eq!(row.reasons_json.as_deref(), Some(old_rules));
        assert_eq!(row.interest_reasons_json.as_deref(), Some(old_interests));
    }

    #[test]
    fn reading_speeds_require_personal_samples_per_context() {
        let (_dir, conn) = open_temp();
        let now = chrono::Utc::now().timestamp_millis();
        for (guid, title, translated, active_ms, expected) in [
            ("zh", "中文", false, 120_000, 0.01),
            ("zh-translated", "かな", true, 240_000, 0.005),
            ("en", "English 中", false, 60_000, 0.02),
            ("en-translated", "English", true, 30_000, 0.04),
        ] {
            let id = seed_article(&conn, guid, title);
            let insert = |day: i64| {
                conn.execute(
                    "INSERT INTO read_sessions(article_id,opened_at,active_ms,idle_ms,read_chars,
                    skimmed_chars,total_chars,max_block,reached_end,translated,ended_by,via)
                    VALUES(?1,?2,?3,0,1200,0,1200,1,1,?4,'back','focus')",
                    rusqlite::params![id, now - day * 86_400_000, active_ms, translated],
                )
                .unwrap();
            };
            let get = || {
                let view = reading_speeds(&conn).unwrap();
                match (guid.starts_with("zh"), translated) {
                    (true, false) => view.chinese_original,
                    (true, true) => view.chinese_translated,
                    (false, false) => view.english_original,
                    (false, true) => view.english_translated,
                }
            };
            insert(181); // 窗口外的样本不计。
            for day in 1..10 {
                insert(day);
            }
            insert(1); // 同篇同日重复打开不增加样本数。
            assert_eq!(get(), None);
            insert(10);
            assert!((get().unwrap() - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn cached_images_carry_their_pixel_size() {
        let (dir, conn) = open_temp();
        let id = seed_article(&conn, "img", "图");
        let doc = rrss_render::Document {
            v: 1,
            blocks: vec![
                Block::Image {
                    src: "https://img.example/a.png".into(),
                    alt: String::new(),
                },
                Block::Image {
                    src: "https://img.example/b.png".into(),
                    alt: String::new(),
                },
            ],
        };
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&1200u32.to_be_bytes());
        png.extend_from_slice(&630u32.to_be_bytes());
        rrss_core::media::store_image(&conn, dir.path(), id, "https://img.example/a.png", &png)
            .unwrap();
        let media = article_media_from_doc(&conn, id, &doc);
        assert_eq!((media[0].width, media[0].height), (Some(1200), Some(630)));
        assert!(!media[0].href.is_empty());
        // 还没缓存的图没有地址，也不给尺寸。
        assert_eq!(
            (media[1].href.as_str(), media[1].width, media[1].height),
            ("", None, None)
        );
    }

    #[test]
    fn reader_view_does_not_mark_read() {
        let (_dir, conn) = open_temp();
        let id = seed_article(&conn, "pure", "Snapshot only");
        assert!(!articles::get_article(&conn, id).unwrap().read);

        let view = reader_view(&conn, id).unwrap();
        assert_eq!(view.id, id);
        assert_eq!(view.title, "Snapshot only");
        assert!(
            !articles::get_article(&conn, id).unwrap().read,
            "pure reader_view must not mark the article read"
        );

        articles::set_read(&conn, id, false).unwrap();
        let _ = reader_view(&conn, id).unwrap();
        assert!(
            !articles::get_article(&conn, id).unwrap().read,
            "manual unread must survive a later snapshot"
        );
    }

    #[test]
    fn open_article_marks_read_after_valid_snapshot() {
        let (_dir, conn) = open_temp();
        let id = seed_article(&conn, "open", "Open me");
        let view = crate::actions::open_article(&conn, id).unwrap();
        assert_eq!(view.id, id);
        assert!(articles::get_article(&conn, id).unwrap().read);

        articles::set_read(&conn, id, false).unwrap();
        let _ = reader_view(&conn, id).unwrap();
        assert!(
            !articles::get_article(&conn, id).unwrap().read,
            "tool/snapshot refresh must not re-mark a manually unread article"
        );
    }

    #[test]
    fn missing_article_errors_for_query_and_open() {
        let (_dir, conn) = open_temp();
        assert!(matches!(
            reader_view(&conn, 9_001).unwrap_err(),
            Error::ArticleNotFound
        ));
        assert!(matches!(
            crate::actions::open_article(&conn, 9_001).unwrap_err(),
            Error::ArticleNotFound
        ));
        assert!(matches!(
            related_articles(&conn, 9_001, 3).unwrap_err(),
            Error::ArticleNotFound
        ));
    }

    #[test]
    fn related_article_views_use_stored_summaries_and_local_feed_icons() {
        let (dir, conn) = open_temp();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,title,created_at) VALUES(1,'https://news.example/rss','News',1);
             INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at)
             VALUES(1,'','fixture',3,1,0,'active',1);
             INSERT INTO articles(id,feed_id,title,created_at,published_at,read,summary,summary_json) VALUES
               (1,1,'Source',1,NULL,0,NULL,NULL),
               (2,1,'Structured',1,42,1,'Fallback。','{\"one_liner\":\"结构化一句话\"}'),
               (3,1,'Plain',1,NULL,0,'第一句。第二句。',NULL),
               (4,1,'No summary',1,NULL,0,NULL,NULL);",
        )
        .unwrap();
        for id in 1..=4 {
            conn.execute(
                "INSERT INTO chunks(article_id,ordinal,kind,text,embedding,space_id)
                 VALUES(?1,0,'title_summary','Fixture',?2,1)",
                rusqlite::params![id, rrss_core::rag::f32s_to_le(&[1.0, 0.0, 0.0])],
            )
            .unwrap();
        }
        let icon = dir.path().join("icon.png");
        std::fs::write(&icon, []).unwrap();
        conn.execute(
            "INSERT INTO media(id,feed_id,url,local_path,created_at)
             VALUES(7,1,'https://news.example/icon.png',?1,1)",
            [icon.to_str().unwrap()],
        )
        .unwrap();
        let rows = related_articles(&conn, 1, 3).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.article_id).collect::<Vec<_>>(),
            [2, 3, 4]
        );
        assert_eq!(
            rows.iter()
                .map(|r| r.one_liner.as_str())
                .collect::<Vec<_>>(),
            ["结构化一句话", "第一句。", ""]
        );
        let row = &rows[0];
        assert_eq!(
            (row.feed_id, row.feed_title.as_str(), row.title.as_str()),
            (1, "News", "Structured")
        );
        assert_eq!(row.published_at, Some(42));
        assert!(row.read);
        assert!((row.similarity - 1.0).abs() < 1e-6);
        assert!(
            rows.iter()
                .all(|r| r.icon_href == super::super::media::media_href(7))
        );
        assert!(rows[1].published_at.is_none());
        assert!(!rows[1].read);
    }

    #[test]
    fn trigger_aborted_read_write_propagates_from_open() {
        let (_dir, conn) = open_temp();
        let id = seed_article(&conn, "abort", "Abort mark");
        conn.execute(
            "CREATE TRIGGER fail_read BEFORE UPDATE OF read ON articles
             BEGIN SELECT RAISE(ABORT, 'injected'); END;",
            [],
        )
        .unwrap();
        let err = crate::actions::open_article(&conn, id).unwrap_err();
        assert!(
            err.to_string().contains("injected"),
            "open_article must surface the aborted write, got {err}"
        );
        assert!(!articles::get_article(&conn, id).unwrap().read);
    }
}
