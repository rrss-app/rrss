//! Digest 查询：供 CLI / Pi 与页内追问复用。

use anyhow::{Context, Result, bail};
use rrss_core::ai_store::{self, DigestRow, DigestSource};
use rrss_render::{Block, Document, inlines_to_plain, markdown_to_document};
use rusqlite::Connection;
use serde::Serialize;

use crate::views::{CitationView, DigestSelection};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DigestContextSource {
    pub article_id: i64,
    pub title: String,
    pub feed_title: String,
    pub number: Option<usize>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DigestContextItem {
    pub number: usize,
    pub block_id: usize,
    pub text: String,
    pub article_ids: Vec<i64>,
    pub sources: Vec<DigestContextSource>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DigestContext {
    pub id: i64,
    pub period: String,
    pub period_start: i64,
    pub body: String,
    pub article_ids: Vec<i64>,
    pub items: Vec<DigestContextItem>,
    pub revision: String,
    pub source_state: String,
    pub source_index: Vec<DigestContextSource>,
    pub unlocated_sources: Vec<DigestContextSource>,
}

pub fn get_digest(
    conn: &Connection,
    id: Option<i64>,
    period: &str,
) -> Result<Option<DigestContext>> {
    let row = match id {
        Some(id) if id <= 0 => bail!("invalid digest id {id}, want a positive integer"),
        Some(id) => ai_store::digest_by_id(conn, id)?
            .with_context(|| format!("digest id {id} not found"))?,
        None => {
            validate_period(period)?;
            match ai_store::latest_digest(conn, period)? {
                Some(d) => d,
                None => return Ok(None),
            }
        }
    };
    Ok(Some(parse_digest(conn, row).context))
}

/// 选区按当前稿件的文档块定位；不把简报块号冒充原文块号。
pub fn cite_selection(
    conn: &Connection,
    digest_id: i64,
    revision: &str,
    selections: &[DigestSelection],
) -> Result<Vec<CitationView>> {
    let row = ai_store::digest_by_id(conn, digest_id)?.context(rrss_core::Error::Jobs(rrss_core::error::JobFailure::DigestMissing))?;
    let parsed = parse_digest(conn, row);
    if parsed.context.revision != revision {
        bail!(rrss_core::Error::Jobs(rrss_core::error::JobFailure::DigestUpdated));
    }
    let digest = &parsed.context;
    let document = &parsed.document;
    if selections.is_empty() || selections.len() > document.blocks.len() {
        bail!(rrss_core::Error::Jobs(rrss_core::error::JobFailure::DigestSelectSources));
    }
    let mut selected: Vec<_> = selections.iter().collect();
    selected.sort_by_key(|selection| selection.block_id);
    let mut seen_blocks = std::collections::HashSet::new();
    let mut lines = Vec::new();
    for selection in selected {
        if !seen_blocks.insert(selection.block_id) {
            bail!(rrss_core::Error::Jobs(rrss_core::error::JobFailure::DigestDuplicate));
        }
        let index = usize::try_from(selection.block_id).context(rrss_core::Error::Jobs(rrss_core::error::JobFailure::DigestBlockInvalid))?;
        let Some(Block::ListItem { inlines, .. }) = document.blocks.get(index) else {
            bail!(rrss_core::Error::Jobs(rrss_core::error::JobFailure::DigestBlockNoSources));
        };
        let ordinal = parsed
            .block_ids
            .iter()
            .position(|block_id| *block_id == index)
            .context(rrss_core::Error::Jobs(rrss_core::error::JobFailure::DigestItemChanged))?;
        let item = digest
            .items
            .get(ordinal)
            .context(rrss_core::Error::Jobs(rrss_core::error::JobFailure::DigestItemChanged))?;
        if item.sources.is_empty() {
            bail!(rrss_core::Error::Jobs(rrss_core::error::JobFailure::DigestSourcesMissing));
        }
        let excerpt = selection.excerpt.trim();
        let normalize = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
        if excerpt.is_empty()
            || !normalize(&inlines_to_plain(inlines)).contains(&normalize(excerpt))
        {
            bail!(rrss_core::Error::Jobs(rrss_core::error::JobFailure::DigestContentChanged));
        }
        lines.push((item, excerpt));
    }

    // 一次选区总计最多 500 字；跨条目分别保留摘录，不能把整段挂到首条来源上。
    let mut remaining = 500;
    let mut citations = Vec::new();
    // 在产出任何引用之前校验整次选区的所有来源，防止删除一篇后出现部分成功。
    for (item, _) in &lines {
        for source in &item.sources {
            let status = parsed
                .source_index
                .iter()
                .find(|entry| entry.article_id == source.article_id)
                .map(|entry| entry.status.as_str());
            match status {
                Some("available") => {}
                Some("removed") => bail!(rrss_core::Error::Jobs(rrss_core::error::JobFailure::DigestSourceRemoved)),
                _ => bail!(rrss_core::Error::Jobs(rrss_core::error::JobFailure::DigestSourceUnavailable)),
            }
        }
    }
    for (item, excerpt) in lines {
        if remaining == 0 {
            break;
        }
        let excerpt: String = excerpt.chars().take(remaining).collect();
        remaining -= excerpt.chars().count();
        let mut seen_sources = std::collections::HashSet::new();
        for source in &item.sources {
            if !seen_sources.insert(source.article_id) {
                continue;
            }
            let mut citation = crate::actions::cite_article(conn, source.article_id)
                .context(rrss_core::Error::Jobs(rrss_core::error::JobFailure::DigestSourceRemoved))?;
            citation.kind = "digest".into();
            citation.excerpt = excerpt.clone();
            // 当前引用只定位来源文章，不宣称这是原文中的某个块或逐字引文。
            citation.block_id = -1;
            citations.push(citation);
        }
    }
    Ok(citations)
}

fn validate_period(period: &str) -> Result<()> {
    if period == "daily" || period == "weekly" {
        Ok(())
    } else {
        bail!("invalid period {period:?}, want daily or weekly")
    }
}

/// 每个读取请求的共同解析结果。未定位快照只保留供后续来源表使用，
/// 不进入条目来源或划词引用；S2 暂不增加 IPC 字段。
pub(crate) struct ParsedDigest {
    pub context: DigestContext,
    pub document: Document,
    pub block_ids: Vec<usize>,
    pub source_index: Vec<crate::views::DigestSourceView>,
    pub unlocated_sources: Vec<crate::views::DigestSourceView>,
}

fn source_metadata(
    conn: &Connection,
    sources: &[DigestSource],
) -> std::result::Result<
    std::collections::HashMap<i64, (Option<String>, Option<i64>)>,
    rusqlite::Error,
> {
    if sources.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let slots = std::iter::repeat_n("?", sources.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!("SELECT id, summary, published_at FROM articles WHERE id IN ({slots})");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        rusqlite::params_from_iter(sources.iter().map(|source| source.article_id)),
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        },
    )?;
    rows.map(|row| {
        row.map(|(id, summary, published_at)| {
            let summary = summary
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.chars().take(240).collect());
            (id, (summary, published_at))
        })
    })
    .collect()
}

fn indexed_views(
    conn: &Connection,
    index: &[DigestSource],
    unlocated: &[DigestSource],
) -> (
    Vec<crate::views::DigestSourceView>,
    Vec<crate::views::DigestSourceView>,
) {
    let mut seen = std::collections::HashSet::new();
    let all: Vec<_> = index
        .iter()
        .chain(unlocated)
        .filter(|source| seen.insert(source.article_id))
        .cloned()
        .collect();
    let metadata = source_metadata(conn, &all);
    let view = |source: &DigestSource, number| {
        let (summary, published_at, status) = match &metadata {
            Err(_) => (None, None, "unavailable"),
            Ok(found) => match found.get(&source.article_id) {
                Some((summary, published_at)) => (summary.clone(), *published_at, "available"),
                None => (None, None, "removed"),
            },
        };
        crate::views::DigestSourceView {
            article_id: source.article_id,
            title: source.title.clone(),
            feed_title: source.feed_title.clone(),
            number,
            summary,
            published_at,
            status: status.into(),
        }
    };
    (
        index
            .iter()
            .enumerate()
            .map(|(i, s)| view(s, Some(i + 1)))
            .collect(),
        unlocated.iter().map(|s| view(s, None)).collect(),
    )
}

/// 早期成稿把输入〈序号〉写进了正文。旧稿不回写，只在读取的文字节点里去掉；
/// 块结构不变，来源槽位与划词块号不受影响。
fn strip_input_markers(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('〈') {
        out.push_str(&rest[..start]);
        let after = &rest[start + '〈'.len_utf8()..];
        let digits = after.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 && after[digits..].starts_with('〉') {
            rest = &after[digits + '〉'.len_utf8()..];
        } else {
            out.push('〈');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn strip_inline_markers(inlines: &mut [rrss_render::Inline]) {
    for inline in inlines {
        match inline {
            rrss_render::Inline::Text { text } => *text = strip_input_markers(text),
            rrss_render::Inline::Strong { children }
            | rrss_render::Inline::Emphasis { children }
            | rrss_render::Inline::Link { children, .. } => strip_inline_markers(children),
            _ => {}
        }
    }
}

fn strip_document_markers(document: &mut Document) {
    for block in &mut document.blocks {
        match block {
            Block::Heading { inlines, .. }
            | Block::Paragraph { inlines }
            | Block::ListItem { inlines, .. }
            | Block::Quote { inlines } => strip_inline_markers(inlines),
            _ => {}
        }
    }
}

pub(crate) fn parse_digest(conn: &Connection, row: DigestRow) -> ParsedDigest {
    let mut document = markdown_to_document(&row.body);
    strip_document_markers(&mut document);
    let block_ids: Vec<usize> = document
        .blocks
        .iter()
        .enumerate()
        .filter_map(|(index, block)| matches!(block, Block::ListItem { .. }).then_some(index))
        .collect();
    let decoded = ai_store::decode_digest_sources(&row.body, &row.items_json, &document);
    let revision = ai_store::digest_revision(&row.body, &row.items_json);
    let mut flat_ids = Vec::new();
    let mut source_index: Vec<DigestSource> = Vec::new();
    let mut numbers = std::collections::HashMap::<i64, usize>::new();

    let items: Vec<DigestContextItem> = block_ids
        .iter()
        .enumerate()
        .map(|(idx, block_id)| {
            let Block::ListItem { inlines, .. } = &document.blocks[*block_id] else {
                unreachable!("block_ids contains only list items")
            };
            let text = inlines_to_plain(inlines);
            let line = decoded.lines.get(idx).cloned().unwrap_or_default();
            let mut per_item = std::collections::HashSet::new();
            let sources: Vec<DigestContextSource> = line
                .into_iter()
                .filter(|source| source.article_id > 0 && per_item.insert(source.article_id))
                .take(3)
                .map(|source| {
                    let number = *numbers.entry(source.article_id).or_insert_with(|| {
                        source_index.push(source.clone());
                        flat_ids.push(source.article_id);
                        source_index.len()
                    });
                    source_to_context(source, Some(number))
                })
                .collect();
            let article_ids = sources.iter().map(|source| source.article_id).collect();
            DigestContextItem {
                number: idx + 1,
                block_id: *block_id,
                text,
                article_ids,
                sources,
            }
        })
        .collect();
    let unlocated = decoded.unlocated;
    let (source_views, unlocated_views) = indexed_views(conn, &source_index, &unlocated);
    let source_state = decoded.state.as_str().to_string();
    let context = DigestContext {
        id: row.id,
        period: row.period,
        period_start: row.period_start,
        body: row.body,
        article_ids: flat_ids,
        items,
        revision,
        source_state,
        source_index: source_index
            .into_iter()
            .enumerate()
            .map(|(i, source)| source_to_context(source, Some(i + 1)))
            .collect(),
        unlocated_sources: unlocated
            .into_iter()
            .map(|source| source_to_context(source, None))
            .collect(),
    };
    ParsedDigest {
        context,
        document,
        block_ids,
        source_index: source_views,
        unlocated_sources: unlocated_views,
    }
}

fn source_to_context(s: DigestSource, number: Option<usize>) -> DigestContextSource {
    DigestContextSource {
        article_id: s.article_id,
        title: s.title,
        feed_title: s.feed_title,
        number,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::db::open_at;
    use tempfile::tempdir;

    fn cite_selection(
        conn: &Connection,
        id: i64,
        selections: &[DigestSelection],
    ) -> Result<Vec<CitationView>> {
        let row = ai_store::digest_by_id(conn, id)?.context("简报不存在")?;
        super::cite_selection(
            conn,
            id,
            &ai_store::digest_revision(&row.body, &row.items_json),
            selections,
        )
    }

    fn citation_fixture() -> (tempfile::TempDir, rrss_core::db::PooledConnection, i64) {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        conn.execute("INSERT INTO feeds (id, url, title, created_at) VALUES (1, 'https://example.test/feed', '测试源', 1)", []).unwrap();
        for (id, title) in [
            (11, "Mars report"),
            (12, "Moon report"),
            (13, "Ocean report"),
        ] {
            conn.execute("INSERT INTO articles (id, feed_id, title, canonical_url, created_at) VALUES (?1, 1, ?2, ?3, 1)", rusqlite::params![id, title, format!("https://example.test/{id}")]).unwrap();
        }
        let sources = r#"[{"sources":[{"article_id":11,"title":"Mars report","feed_title":"测试源"},{"article_id":12,"title":"Moon report","feed_title":"测试源"}]},{"sources":[{"article_id":13,"title":"Ocean report","feed_title":"测试源"}]},{"sources":[]}]"#;
        let id = ai_store::insert_digest(&conn, "daily", 1, "# 简报\n\n- Mars report 与 Moon report：共享的新进展\n- Ocean report：独立的海洋事件\n- Unmapped issue", sources).unwrap();
        (dir, conn, id)
    }

    #[test]
    fn selection_maps_each_block_to_all_its_sources_and_persists_snapshots() {
        let (_dir, conn, id) = citation_fixture();
        ai_store::insert_digest(&conn, "daily", 2, "- 最新期不能替换引用期", "[]").unwrap();
        let selections = vec![
            DigestSelection {
                block_id: 2,
                excerpt: "独立的海洋事件".into(),
            },
            DigestSelection {
                block_id: 1,
                excerpt: "共享的新进展".into(),
            },
        ];
        let citations = cite_selection(&conn, id, &selections).unwrap();
        assert_eq!(
            citations.iter().map(|c| c.article_id).collect::<Vec<_>>(),
            vec![11, 12, 13]
        );
        assert_eq!(citations[0].excerpt, "共享的新进展");
        assert_eq!(citations[1].excerpt, "共享的新进展");
        assert_eq!(citations[2].excerpt, "独立的海洋事件");
        assert!(
            citations
                .iter()
                .all(|c| c.kind == "digest" && c.block_id == -1)
        );
        let args = crate::views::SendMessageArgs {
            thread_id: 0,
            digest_id: Some(id),
            text: "核对这几条".into(),
            citations,
            model: None,
            thinking: None,
        };
        let prepared = crate::chat::prepare_send(&conn, &args).unwrap();
        assert!(
            prepared
                .prompt
                .contains("划选的简报摘录（简报的概括，不是原文逐字）")
        );
        crate::actions::insert_user_message(&conn, prepared.thread.id, &args.text, &args.citations)
            .unwrap();
        let messages = crate::queries::messages(&conn, prepared.thread.id, None, None)
            .unwrap()
            .messages;
        assert_eq!(messages[0].citations.len(), 3);
        assert_eq!(messages[0].citations[2].excerpt, "独立的海洋事件");
        assert_eq!(messages[0].text, "核对这几条");
    }

    #[test]
    fn selection_rejects_headings_unmapped_stale_or_deleted_sources() {
        let (_dir, conn, id) = citation_fixture();
        for (block_id, excerpt) in [
            (0, "简报"),
            (3, "Unmapped issue"),
            (1, "已被改写的旧文本"),
            (-1, "共享的新进展"),
        ] {
            assert!(
                cite_selection(
                    &conn,
                    id,
                    &[DigestSelection {
                        block_id,
                        excerpt: excerpt.into()
                    }]
                )
                .is_err()
            );
        }
        assert!(cite_selection(&conn, id, &[]).is_err());
        let valid = DigestSelection {
            block_id: 1,
            excerpt: "共享的新进展".into(),
        };
        assert!(cite_selection(&conn, id, &[valid.clone(), valid.clone()]).is_err());
        assert!(cite_selection(&conn, 9999, &[valid.clone()]).is_err());
        conn.execute("DELETE FROM articles WHERE id=12", [])
            .unwrap();
        assert!(cite_selection(&conn, id, &[valid]).is_err());
        assert!(
            rrss_core::threads::list_threads(&conn, 80)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn selection_caps_total_excerpt_at_500_unicode_characters() {
        let (_dir, conn, id) = citation_fixture();
        let first = format!("Mars report {}", "🪐".repeat(300));
        let second = format!("Ocean report {}", "海".repeat(400));
        let sources = r#"[{"sources":[{"article_id":11,"title":"Mars report","feed_title":"测试源"}]},{"sources":[{"article_id":13,"title":"Ocean report","feed_title":"测试源"}]}]"#;
        ai_store::insert_digest(
            &conn,
            "daily",
            1,
            &format!("- {first}\n- {second}"),
            sources,
        )
        .unwrap();
        let citations = cite_selection(
            &conn,
            id,
            &[
                DigestSelection {
                    block_id: 0,
                    excerpt: first.clone(),
                },
                DigestSelection {
                    block_id: 1,
                    excerpt: second,
                },
            ],
        )
        .unwrap();
        assert_eq!(citations.len(), 2);
        assert_eq!(citations[0].excerpt, first);
        assert_eq!(
            citations
                .iter()
                .map(|c| c.excerpt.chars().count())
                .sum::<usize>(),
            500
        );
    }

    #[test]
    fn legacy_input_markers_are_hidden_without_moving_blocks() {
        assert_eq!(
            strip_input_markers("A〈1〉、B〈12〉〈x〉〈〉"),
            "A、B〈x〉〈〉"
        );
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let body = "### 旧稿\n\n- **Alpha**〈1〉 发布\n- Beta〈2〉〈3〉 更新\n";
        let id = ai_store::insert_digest(&conn, "daily", 1_000, body, "[]").unwrap();
        let ctx = get_digest(&conn, Some(id), "daily").unwrap().expect("ctx");
        assert_eq!(ctx.body, body);
        let texts: Vec<_> = ctx.items.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(texts, ["Alpha 发布", "Beta 更新"]);
        assert_eq!(ctx.items[1].block_id, 2);
    }

    #[test]
    fn get_digest_hits_historical_id_after_newer_issue() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let old_id =
            ai_store::insert_digest(&conn, "daily", 1_000, "- first\n- second\n", "[]").unwrap();
        ai_store::insert_digest(&conn, "daily", 2_000, "- newer only\n", "[]").unwrap();

        let ctx = get_digest(&conn, Some(old_id), "daily")
            .unwrap()
            .expect("ctx");
        assert_eq!(ctx.id, old_id);
        assert_eq!(ctx.body, "- first\n- second\n");
        assert_eq!(ctx.items.len(), 2);
    }

    #[test]
    fn historical_reads_share_snapshot_slots_and_never_write_or_fill_from_new_articles() {
        let (_dir, conn, id) = citation_fixture();
        let body = "# 历史期\n\n- 全新中文正文\n- 第二条\n- 第三条";
        let raw = r#"[
            {"sources":[{"article_id":11,"title":"English snapshot title","feed_title":"旧源"}]},
            {"article_id":"invalid"},
            {"sources":[{"article_id":13,"title":"Snapshot C","feed_title":"旧源"}]}
        ]"#;
        ai_store::insert_digest(&conn, "daily", 1, body, raw).unwrap();
        conn.execute("INSERT INTO articles (id, feed_id, title, created_at) VALUES (99, 1, '全新中文正文', 999)", []).unwrap();
        let before = conn.total_changes();
        for _ in 0..3 {
            let ctx = get_digest(&conn, Some(id), "daily").unwrap().unwrap();
            let view = crate::queries::digest(&conn, id).unwrap();
            assert_eq!(ctx.article_ids, vec![11, 13]);
            assert_eq!(ctx.items.len(), view.items.len());
            for (item, line) in ctx.items.iter().zip(&view.items) {
                assert_eq!(
                    item.article_ids,
                    line.sources
                        .iter()
                        .map(|source| source.article_id)
                        .collect::<Vec<_>>()
                );
            }
            assert!(ctx.items[1].sources.is_empty());
            assert_eq!(ctx.items[2].article_ids, vec![13]);
            assert_eq!(ctx.items[0].sources[0].title, "English snapshot title");
            assert_eq!(
                cite_selection(
                    &conn,
                    id,
                    &[DigestSelection {
                        block_id: 1,
                        excerpt: "全新中文正文".into()
                    }]
                )
                .unwrap()[0]
                    .article_id,
                11
            );
            assert!(
                cite_selection(
                    &conn,
                    id,
                    &[DigestSelection {
                        block_id: 2,
                        excerpt: "第二条".into()
                    }]
                )
                .is_err()
            );
            let row = ai_store::digest_by_id(&conn, id).unwrap().unwrap();
            assert_eq!(row.body, body);
            assert_eq!(row.items_json, raw);
        }
        assert_eq!(conn.total_changes(), before);
    }

    #[test]
    fn unlocated_snapshots_stay_separate_from_display_tool_and_selection_sources() {
        let (_dir, conn, id) = citation_fixture();
        let raw = r#"[{"article_id":11,"title":"Mars report","feed_title":"旧源"}]"#;
        for body in ["- Mars report\n- Ocean report", "没有列表的说明段落"] {
            ai_store::insert_digest(&conn, "daily", 1, body, raw).unwrap();
            let row = ai_store::digest_by_id(&conn, id).unwrap().unwrap();
            let parsed = parse_digest(&conn, row);
            assert_eq!(parsed.unlocated_sources.len(), 1);
            assert_eq!(parsed.unlocated_sources[0].article_id, 11);
            assert!(parsed.context.article_ids.is_empty());
            assert!(
                parsed
                    .context
                    .items
                    .iter()
                    .all(|item| item.sources.is_empty())
            );
            let view = crate::queries::digest(&conn, id).unwrap();
            assert_eq!(view.body, body);
            assert!(view.items.iter().all(|line| line.sources.is_empty()));
            assert!(
                cite_selection(
                    &conn,
                    id,
                    &[DigestSelection {
                        block_id: 0,
                        excerpt: "Mars report".into()
                    }]
                )
                .is_err()
            );
        }
    }

    #[test]
    fn empty_or_invalid_snapshots_preserve_body_without_library_sources() {
        let (_dir, conn, id) = citation_fixture();
        for raw in ["[]", "", "not json", r#"{"version":999}"#] {
            let body = "- Mars report\n- Ocean report";
            ai_store::insert_digest(&conn, "daily", 1, body, raw).unwrap();
            let ctx = get_digest(&conn, Some(id), "daily").unwrap().unwrap();
            let view = crate::queries::digest(&conn, id).unwrap();
            assert_eq!(ctx.body, body);
            assert_eq!(view.body, body);
            assert!(ctx.article_ids.is_empty());
            assert_eq!(ctx.items.len(), 2);
            assert_eq!(view.items.len(), 2);
            assert!(view.items.iter().all(|item| item.sources.is_empty()));
        }
    }

    #[test]
    fn missing_article_and_metadata_failure_have_distinct_status_and_citation_errors() {
        let (_dir, conn, id) = citation_fixture();
        let selection = [DigestSelection {
            block_id: 1,
            excerpt: "共享的新进展".into(),
        }];
        conn.execute("DELETE FROM articles WHERE id=12", [])
            .unwrap();
        let view = crate::queries::digest(&conn, id).unwrap();
        assert_eq!(view.items[0].sources[1].status, "removed");
        assert!(
            cite_selection(&conn, id, &selection)
                .map_err(|error| crate::ui_error::jobs::display_anyhow(&error)).unwrap_err()
                .contains("已移除")
        );
        conn.execute("DROP TABLE articles", []).unwrap();
        let view = crate::queries::digest(&conn, id).unwrap();
        assert!(
            view.items[0]
                .sources
                .iter()
                .all(|source| source.status == "unavailable")
        );
        assert!(
            cite_selection(&conn, id, &selection)
                .map_err(|error| crate::ui_error::jobs::display_anyhow(&error)).unwrap_err()
                .contains("暂不可用")
        );
    }

    #[test]
    fn invalid_structured_draft_never_overwrites_existing_issue() {
        let (_dir, conn, id) = citation_fixture();
        let before = ai_store::digest_by_id(&conn, id).unwrap().unwrap();
        let result = ai_store::prepare_digest_draft(
            &serde_json::json!({
                "sections": [{"title": "失效", "items": [{"text": "<b>不支持</b>", "refs": [1]}]}]
            }),
            &[],
        );
        assert!(result.is_err());
        let after = ai_store::digest_by_id(&conn, id).unwrap().unwrap();
        assert_eq!(after.id, before.id);
        assert_eq!(after.body, before.body);
        assert_eq!(after.items_json, before.items_json);
    }

    #[test]
    fn table_is_rejected_by_digest_shape_validation() {
        let table = "| H |\n| --- |\n| value |";
        let doc = markdown_to_document(&format!("### Section\n\n- item\n\n{table}"));
        assert!(matches!(doc.blocks.last(), Some(Block::Table { .. })));
        assert!(ai_store::canonical_digest_items(&doc).is_none());
    }

    #[test]
    fn revision_rejects_old_selection_even_if_body_and_id_remain_same() {
        let (_dir, conn, id) = citation_fixture();
        let original = get_digest(&conn, Some(id), "daily").unwrap().unwrap();
        let selection = [DigestSelection {
            block_id: 1,
            excerpt: "共享的新进展".into(),
        }];
        ai_store::insert_digest(&conn, "daily", 1, &original.body,
            r#"[{"sources":[{"article_id":13,"title":"Ocean report","feed_title":"测试源"}]},{"sources":[]},{"sources":[]}]"#).unwrap();
        let updated = get_digest(&conn, Some(id), "daily").unwrap().unwrap();
        assert_eq!(updated.id, original.id);
        assert_eq!(updated.body, original.body);
        assert_ne!(updated.revision, original.revision);
        assert!(
            super::cite_selection(&conn, id, &original.revision, &selection)
                .map_err(|error| crate::ui_error::jobs::display_anyhow(&error)).unwrap_err()
                .contains("简报已更新")
        );
        assert_eq!(
            super::cite_selection(&conn, id, &updated.revision, &selection).unwrap()[0].article_id,
            13
        );
    }

    #[test]
    fn display_tool_and_selection_share_v2_block_and_source_numbers() {
        let (_dir, conn, id) = citation_fixture();
        let numbered = vec![
            rrss_core::ai_store::DigestItem {
                article_id: 11,
                title: "Mars report".into(),
                feed_title: "测试源".into(),
                summary: String::new(),
                summary_json: None,
                cluster_id: None,
            },
            rrss_core::ai_store::DigestItem {
                article_id: 13,
                title: "Ocean report".into(),
                feed_title: "测试源".into(),
                summary: String::new(),
                summary_json: None,
                cluster_id: None,
            },
        ];
        let draft = ai_store::prepare_digest_draft(
            &serde_json::json!({"sections":[{"title":"行星", "items":[
                {"text":"Mars report 的新进展", "refs":[1,2]},
                {"text":"Ocean report 的新进展", "refs":[2]}
            ]}]}),
            &numbered,
        )
        .unwrap();
        ai_store::insert_digest(&conn, "daily", 1, &draft.body, &draft.items_json).unwrap();
        let context = get_digest(&conn, Some(id), "daily").unwrap().unwrap();
        let view = crate::queries::digest(&conn, id).unwrap();
        assert_eq!(view.source_state, "verified");
        assert_eq!(view.revision, context.revision);
        assert_eq!(context.items[0].block_id, 1);
        assert_eq!(view.items[0].block_id, 1);
        assert_eq!(
            view.items[0]
                .sources
                .iter()
                .map(|s| s.number)
                .collect::<Vec<_>>(),
            [Some(1), Some(2)]
        );
        assert_eq!(context.items[1].sources[0].number, Some(2));
        assert_eq!(view.source_index.len(), 2);
        assert_eq!(
            super::cite_selection(
                &conn,
                id,
                &view.revision,
                &[DigestSelection {
                    block_id: 1,
                    excerpt: "Mars report".into()
                }]
            )
            .unwrap()
            .len(),
            2
        );
    }

    #[test]
    fn get_digest_invalid_id_and_period() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();

        assert!(get_digest(&conn, Some(0), "daily").is_err());
        assert!(get_digest(&conn, Some(-3), "daily").is_err());
        assert!(get_digest(&conn, Some(99), "daily").is_err());
        assert!(get_digest(&conn, None, "monthly").is_err());

        assert!(get_digest(&conn, None, "daily").unwrap().is_none());
    }

    #[test]
    fn get_digest_items_second_multi_first_empty_no_shift() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let body = "- alpha line\n- beta line\n";
        let items_json = r#"[
          {"sources": []},
          {"sources": [
            {"article_id": 10, "title": "T10", "feed_title": "F10"},
            {"article_id": 11, "title": "T11", "feed_title": "F11"}
          ]}
        ]"#;
        let id = ai_store::insert_digest(&conn, "daily", 1, body, items_json).unwrap();

        let ctx = get_digest(&conn, Some(id), "daily").unwrap().expect("ctx");
        assert_eq!(ctx.items.len(), 2);
        assert_eq!(ctx.items[0].number, 1);
        assert!(ctx.items[0].sources.is_empty());
        assert_eq!(ctx.items[0].text, "alpha line");
        assert_eq!(ctx.items[1].number, 2);
        assert_eq!(ctx.items[1].article_ids, vec![10, 11]);
        assert_eq!(ctx.article_ids, vec![10, 11]);
    }

    #[test]
    fn get_digest_flat_article_ids_deduped() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let body = "- one\n- two\n";
        let items_json = r#"[
          {"article_id": 5, "title": "A", "feed_title": "F"},
          {"sources": [
            {"article_id": 5, "title": "A dup", "feed_title": "F"},
            {"article_id": 7, "title": "B", "feed_title": "F"}
          ]}
        ]"#;
        let id = ai_store::insert_digest(&conn, "weekly", 1, body, items_json).unwrap();

        let ctx = get_digest(&conn, Some(id), "weekly").unwrap().expect("ctx");
        assert_eq!(ctx.period, "weekly");
        assert_eq!(ctx.article_ids, vec![5, 7]);
        assert_eq!(ctx.items[0].sources.len(), 1);
        assert_eq!(ctx.items[0].sources[0].article_id, 5);
    }

    #[test]
    fn get_digest_id_ignores_period_mismatch() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let id = ai_store::insert_digest(&conn, "weekly", 9, "- w\n", "[]").unwrap();
        let ctx = get_digest(&conn, Some(id), "daily").unwrap().expect("ctx");
        assert_eq!(ctx.period, "weekly");
    }
}
