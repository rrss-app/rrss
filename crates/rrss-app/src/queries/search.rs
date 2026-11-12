use rrss_core::search::{self, SearchHit};
use rrss_render::{Block, inlines_to_plain};
use rusqlite::Connection;

use crate::document::{load_document, source_hash};
use crate::embed_fetch::present_document;
use crate::views::{
    SearchBlockHitView, SearchHitBlocksView, SearchHitView, SearchQuery, SearchScope,
};

pub const SEARCH_BLOCK_HIT_LIMIT: usize = 50;
pub const SEARCH_BLOCK_TEXT_GRAPHEMES: usize = 400;

pub fn search_hits(
    conn: &Connection,
    query: &SearchQuery,
) -> rrss_core::Result<Vec<SearchHitView>> {
    let options = search::SearchQuery {
        query: query.query.clone(),
        scope: match query.scope {
            SearchScope::All => search::SearchScope::All,
            SearchScope::Unread => search::SearchScope::Unread,
            SearchScope::Starred => search::SearchScope::Starred,
        },
        feed_ids: query.feed_ids.clone(),
        published_since: query.published_since,
        limit: query.limit,
    };
    let icons = super::feeds::feed_icon_hrefs(conn)?;
    Ok(search::search_articles_with_query(conn, &options)?
        .into_iter()
        .map(|hit| {
            let icon = icons.get(&hit.feed_id).cloned().unwrap_or_default();
            hit_view(hit, icon)
        })
        .collect())
}

fn hit_view(hit: SearchHit, feed_icon_href: String) -> SearchHitView {
    SearchHitView {
        id: hit.id,
        title: hit.title,
        feed_title: hit.feed_title,
        feed_icon_href,
        published_at: hit.published_at,
        score: hit.score,
        snippet: hit.snippet,
        read: hit.read,
        starred: hit.starred,
        title_match: hit.title_match,
        body_match: hit.body_match,
    }
}

/// 文档加载 / hash 时机 / 展示处理与 reader_view 一致，不进入打开、媒体或翻译作业。
pub fn search_hit_blocks(
    conn: &Connection,
    article_id: i64,
    query: &str,
) -> rrss_core::Result<SearchHitBlocksView> {
    let article = rrss_core::articles::get_article(conn, article_id)?;
    let mut doc = load_document(&article);
    let hash = source_hash(&article.title, &doc);
    present_document(&mut doc);
    let q = search::normalize_search_query(query);
    let mut view = SearchHitBlocksView {
        source_hash: hash,
        total: 0,
        title_match: search::contains_query(&article.title, &q),
        hits: vec![],
    };
    if article.pruned_at.is_some() {
        return Ok(view);
    }
    let mut section = String::new();
    for (block_id, block) in doc.blocks.iter().enumerate() {
        let text = match block {
            Block::Heading { inlines, .. }
            | Block::Paragraph { inlines }
            | Block::ListItem { inlines, .. }
            | Block::Quote { inlines } => inlines_to_plain(inlines),
            Block::CodeBlock { text, .. } => text.clone(),
            Block::Table { .. } => rrss_render::block_plain(block),
            Block::Image { .. } | Block::Embed { .. } | Block::Rule => continue,
        };
        // 空文本不计数；标题计数 / 可命中，但其 section 为前置标题，不是自身。
        if !text.trim().is_empty() {
            view.total += 1;
            if view.hits.len() < SEARCH_BLOCK_HIT_LIMIT && search::contains_query(&text, &q) {
                view.hits.push(SearchBlockHitView {
                    block_id,
                    ordinal: view.total,
                    section: section.clone(),
                    text: search::hit_excerpt(&text, &q, SEARCH_BLOCK_TEXT_GRAPHEMES),
                });
            }
        }
        if matches!(block, Block::Heading { .. }) {
            section = text;
        }
    }
    Ok(view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::{
        articles, db,
        feeds::{SubscribeRequest, subscribe},
        ingest,
        parse::ParsedEntry,
    };
    use rrss_render::{Document, EmbedKind, Inline};

    fn seed(conn: &Connection, doc: &Document) -> i64 {
        let feed = subscribe(
            conn,
            SubscribeRequest {
                url: "https://search.test/rss".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        ingest::upsert_entries(
            conn,
            feed,
            &[ParsedEntry {
                guid: "blocks".into(),
                canonical_url: Some("https://search.test/article".into()),
                title: "Needle title".into(),
                author: None,
                published_at: Some(1),
                content_html: None,
                hash: "blocks".into(),
                tags: vec![],
            }],
            1,
        )
        .unwrap();
        let id = conn
            .query_row("SELECT id FROM articles WHERE guid = 'blocks'", [], |r| {
                r.get(0)
            })
            .unwrap();
        conn.execute(
            "UPDATE articles SET fulltext_md = 'needle body', blocks_json = ?1 WHERE id = ?2",
            rusqlite::params![serde_json::to_string(doc).unwrap(), id],
        )
        .unwrap();
        id
    }

    #[test]
    fn hit_blocks_use_reader_indices_hash_sections_ordinals_and_are_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("search.db")).unwrap();
        let doc = Document::new(vec![
            Block::Image {
                src: "https://search.test/a.png".into(),
                alt: "needle ignored".into(),
            },
            Block::Paragraph {
                inlines: vec![Inline::text("Needle before headings")],
            },
            Block::Heading {
                level: 2,
                inlines: vec![Inline::text("Section A")],
            },
            Block::Paragraph {
                inlines: vec![
                    Inline::Strong {
                        children: vec![Inline::text("nee")],
                    },
                    Inline::Code { text: "dle".into() },
                ],
            },
            Block::Rule,
            Block::ListItem {
                indent: 1,
                marker: "•".into(),
                inlines: vec![Inline::text("NEEDLE list needle")],
            },
            Block::Quote {
                inlines: vec![Inline::text("needle quote")],
            },
            Block::Heading {
                level: 3,
                inlines: vec![Inline::text("Needle section B")],
            },
            Block::CodeBlock {
                lang: None,
                text: "needle code".into(),
            },
            Block::Embed {
                kind: EmbedKind::Youtube,
                url: "https://youtu.be/abcdefghijk".into(),
                title: "needle ignored".into(),
            },
            Block::Paragraph {
                inlines: vec![Inline::text(" ")],
            },
            Block::Paragraph {
                inlines: vec![Inline::text("plain final paragraph")],
            },
            rrss_render::markdown_to_document(
                "| H | |\n| --- | --- |\n| needle | value |\n| second needle | |",
            )
            .blocks
            .remove(0),
        ]);
        let id = seed(&conn, &doc);
        let result = search_hit_blocks(&conn, id, "  \"needle\"  ").unwrap();
        let reader = crate::queries::reader_view(&conn, id).unwrap();
        assert_eq!(result.source_hash, reader.source_hash);
        assert_eq!(result.total, 9);
        assert!(result.title_match);
        assert_eq!(
            result
                .hits
                .iter()
                .map(|h| (h.block_id, h.ordinal, h.section.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (1, 1, ""),
                (3, 3, "Section A"),
                (5, 4, "Section A"),
                (6, 5, "Section A"),
                (7, 6, "Section A"),
                (8, 7, "Needle section B"),
                (12, 9, "Needle section B"),
            ]
        );
        let shown: Document = serde_json::from_value(reader.document).unwrap();
        for hit in &result.hits {
            let block = &shown.blocks[hit.block_id];
            let text = match block {
                Block::CodeBlock { text, .. } => text.clone(),
                Block::Table { .. } => rrss_render::block_plain(block),
                Block::Heading { inlines, .. }
                | Block::Paragraph { inlines }
                | Block::ListItem { inlines, .. }
                | Block::Quote { inlines } => inlines_to_plain(inlines),
                _ => panic!("non-text hit"),
            };
            assert_eq!(hit.text, text);
            assert!(search::contains_query(&text, "needle"));
        }
        assert!(!articles::get_article(&conn, id).unwrap().read);
        for table in ["feedback", "media", "chunks", "ai_cache"] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "pure query must not create work in {table}");
        }
        assert!(search_hit_blocks(&conn, id, "").unwrap().hits.is_empty());
        assert!(!search_hit_blocks(&conn, id, "NEÉDLE").unwrap().title_match);
    }

    #[test]
    fn hit_blocks_cap_hits_and_text_but_count_the_entire_document() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("search.db")).unwrap();
        let doc = Document::new(
            (0..55)
                .map(|_| Block::Paragraph {
                    inlines: vec![Inline::text(format!("needle {}", "👩🏽‍💻".repeat(450)))],
                })
                .collect(),
        );
        let id = seed(&conn, &doc);
        let view = search_hit_blocks(&conn, id, "needle").unwrap();
        assert_eq!(view.total, 55);
        assert_eq!(view.hits.len(), SEARCH_BLOCK_HIT_LIMIT);
        assert_eq!(view.hits[49].ordinal, 50);
        assert_eq!(view.hits[0].text, format!("needle {}…", "👩🏽‍💻".repeat(199)));
    }

    #[test]
    fn pruned_or_missing_body_has_empty_hits_and_retains_title_match() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("search.db")).unwrap();
        let id = seed(
            &conn,
            &Document::new(vec![Block::Paragraph {
                inlines: vec![Inline::text("needle")],
            }]),
        );
        conn.execute("UPDATE articles SET pruned_at = 1 WHERE id = ?1", [id])
            .unwrap();
        let pruned = search_hit_blocks(&conn, id, "needle").unwrap();
        assert!(pruned.hits.is_empty() && pruned.title_match);
        assert_eq!(pruned.total, 0);
        assert_eq!(
            pruned.source_hash,
            crate::queries::reader_view(&conn, id).unwrap().source_hash
        );
        conn.execute("UPDATE articles SET pruned_at = NULL, fulltext_md = NULL, content_html = NULL, content_md = NULL, blocks_json = NULL WHERE id = ?1", [id]).unwrap();
        let empty = search_hit_blocks(&conn, id, "needle").unwrap();
        assert!(empty.hits.is_empty() && empty.title_match);
        assert_eq!(empty.total, 0);
        assert_eq!(
            empty.source_hash,
            crate::queries::reader_view(&conn, id).unwrap().source_hash
        );
        assert!(!articles::get_article(&conn, id).unwrap().read);
    }

    #[test]
    fn html_and_markdown_fallback_hits_use_the_same_reader_document() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("search.db")).unwrap();
        let id = seed(&conn, &Document::new(vec![]));
        for (md, html) in [
            (Some("## Section\n\nNeedle *text*\n\n- needle item"), None),
            (
                None,
                Some(
                    "<h2>Section</h2><p>Needle <strong>text</strong></p><ul><li>needle item</li></ul>",
                ),
            ),
        ] {
            conn.execute("UPDATE articles SET fulltext_md = ?1, content_html = ?2, blocks_json = NULL WHERE id = ?3", rusqlite::params![md, html, id]).unwrap();
            let reader = crate::queries::reader_view(&conn, id).unwrap();
            let view = search_hit_blocks(&conn, id, "needle").unwrap();
            assert_eq!(view.source_hash, reader.source_hash);
            assert_eq!(view.total, 3);
            assert_eq!(
                view.hits
                    .iter()
                    .map(|h| (h.block_id, h.ordinal, h.section.as_str()))
                    .collect::<Vec<_>>(),
                vec![(1, 2, "Section"), (2, 3, "Section")]
            );
            let doc: Document = serde_json::from_value(reader.document).unwrap();
            for h in view.hits {
                let text = match &doc.blocks[h.block_id] {
                    Block::Paragraph { inlines } | Block::ListItem { inlines, .. } => {
                        inlines_to_plain(inlines)
                    }
                    _ => panic!("unexpected block"),
                };
                assert_eq!(h.text, text);
            }
            assert!(!articles::get_article(&conn, id).unwrap().read);
        }
    }

    #[test]
    fn search_query_defaults_and_view_fields() {
        let query: SearchQuery = serde_json::from_str(r#"{"query":"needle"}"#).unwrap();
        assert!(matches!(query.scope, SearchScope::All));
        assert_eq!(query.limit, 20);
        assert!(query.feed_ids.is_empty() && query.published_since.is_none());
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("search.db")).unwrap();
        let id = seed(&conn, &Document::new(vec![]));
        conn.execute("UPDATE articles SET read = 1, starred = 1, summary = 'ignored AI summary' WHERE id = ?1", [id]).unwrap();
        let views = search_hits(&conn, &query).unwrap();
        assert_eq!(views.len(), 1);
        assert!(views[0].read && views[0].starred && views[0].title_match && views[0].body_match);
        assert_eq!(views[0].snippet, "needle body");
    }
}
