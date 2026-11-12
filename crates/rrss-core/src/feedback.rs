//! 赞踩只记 `feedback`，建议规则需确认。日志 target：`rrss_core::feedback`。

use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Error, Result};
use crate::feeds::now_ms;
use crate::rules::{
    Dimension, NewRule, Polarity, RuleSource, ScopeKind, has_author_rule, insert_rule,
};

pub const SUGGEST_DOWN_THRESHOLD: i64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackKind {
    Up,
    Down,
}

impl FeedbackKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateResult {
    pub kind: FeedbackKind,
    pub suggested_rule_id: Option<i64>,
}

/// 写反馈；不写规则。同作者累计踩到阈值时插入 disabled suggested。
pub fn rate_article(conn: &Connection, article_id: i64, kind: FeedbackKind) -> Result<RateResult> {
    if crate::articles::is_temporary(conn, article_id)? {
        return Ok(RateResult {
            kind,
            suggested_rule_id: None,
        });
    }
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM articles WHERE id = ?1",
        [article_id],
        |row| row.get(0),
    )?;
    if exists == 0 {
        return Err(Error::ArticleNotFound);
    }
    conn.execute(
        "INSERT INTO feedback (article_id, kind, created_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(article_id) DO UPDATE SET kind = excluded.kind, created_at = excluded.created_at",
        params![article_id, kind.as_str(), now_ms()],
    )?;
    tracing::info!(article_id, kind = kind.as_str(), "feedback recorded");
    let suggested = if kind == FeedbackKind::Down {
        maybe_suggest_author(conn, article_id)?
    } else {
        None
    };
    Ok(RateResult {
        kind,
        suggested_rule_id: suggested,
    })
}

pub fn feedback_for_article(conn: &Connection, article_id: i64) -> Result<Option<FeedbackKind>> {
    let kind: Option<String> = conn
        .query_row(
            "SELECT kind FROM feedback WHERE article_id = ?1",
            [article_id],
            |row| row.get(0),
        )
        .optional()?;
    match kind.as_deref() {
        Some("up") => Ok(Some(FeedbackKind::Up)),
        Some("down") => Ok(Some(FeedbackKind::Down)),
        _ => Ok(None),
    }
}

fn maybe_suggest_author(conn: &Connection, article_id: i64) -> Result<Option<i64>> {
    let author: Option<String> = conn.query_row(
        "SELECT author FROM articles WHERE id = ?1",
        [article_id],
        |row| row.get(0),
    )?;
    let Some(author) = author.filter(|s| !s.trim().is_empty()) else {
        return Ok(None);
    };
    let downs: i64 = conn.query_row(
        "SELECT COUNT(*) FROM feedback f
         JOIN articles a ON a.id = f.article_id
         WHERE a.expires_at IS NULL AND f.kind = 'down' AND a.author = ?1",
        [&author],
        |row| row.get(0),
    )?;
    if downs < SUGGEST_DOWN_THRESHOLD {
        return Ok(None);
    }
    if has_author_rule(conn, &author)? {
        return Ok(None);
    }
    let id = insert_rule(
        conn,
        NewRule {
            polarity: Polarity::Dislike,
            dimension: Dimension::Author,
            pattern: author.clone(),
            scope_kind: ScopeKind::Global,
            scope_id: None,
            weight: crate::rules::DEFAULT_WEIGHT,
            source: RuleSource::Suggested,
            enabled: false,
            pack_id: None,
        },
    )?;
    tracing::info!(rule_id = id, downs, "suggested author rule");
    Ok(Some(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{PooledConnection, open_at};
    use crate::feeds::{SubscribeRequest, subscribe};
    use crate::parse::ParsedEntry;
    use crate::rules::{RuleFilter, RuleSource, get_rule, list_rules};
    use rusqlite::Connection;
    use tempfile::tempdir;

    fn open_temp() -> (tempfile::TempDir, PooledConnection) {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        (dir, conn)
    }

    fn insert_authored(conn: &Connection, n: usize) -> Vec<i64> {
        let feed = subscribe(
            conn,
            SubscribeRequest {
                url: "https://news.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        let mut ids = Vec::new();
        for i in 0..n {
            let title = format!("Story {i}");
            let url = format!("https://news.example.com/s{i}");
            let html = "<p>x</p>";
            let entry = ParsedEntry {
                guid: format!("g{i}"),
                canonical_url: Some(url.clone()),
                title: title.clone(),
                author: Some("Noise Bot".into()),
                published_at: Some(1),
                content_html: Some(html.into()),
                hash: crate::parse::entry_hash(&title, Some(&url), Some(html)),
                tags: vec![],
            };
            crate::ingest::upsert_entries(conn, feed, &[entry], 1).unwrap();
            ids.push(i as i64 + 1);
        }
        ids
    }

    #[test]
    fn thumbs_do_not_write_rules() {
        let (_dir, conn) = open_temp();
        let ids = insert_authored(&conn, 1);
        let result = rate_article(&conn, ids[0], FeedbackKind::Down).unwrap();
        assert!(result.suggested_rule_id.is_none());
        assert!(
            list_rules(&conn, &RuleFilter::default())
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            feedback_for_article(&conn, ids[0]).unwrap(),
            Some(FeedbackKind::Down)
        );
    }

    #[test]
    fn five_downs_same_author_suggests_disabled_rule() {
        let (_dir, conn) = open_temp();
        let ids = insert_authored(&conn, 5);
        let mut last = None;
        for id in &ids {
            last = rate_article(&conn, *id, FeedbackKind::Down)
                .unwrap()
                .suggested_rule_id;
        }
        let rule_id = last.expect("suggestion");
        let rule = get_rule(&conn, rule_id).unwrap();
        assert_eq!(rule.source, RuleSource::Suggested);
        assert!(!rule.enabled);
        assert_eq!(rule.pattern, "Noise Bot");
        assert_eq!(rule.dimension, Dimension::Author);
        assert_eq!(rule.polarity, Polarity::Dislike);
        let again = rate_article(&conn, ids[0], FeedbackKind::Down).unwrap();
        assert!(again.suggested_rule_id.is_none());
    }

    #[test]
    fn upsert_feedback_replaces_kind() {
        let (_dir, conn) = open_temp();
        let ids = insert_authored(&conn, 1);
        rate_article(&conn, ids[0], FeedbackKind::Down).unwrap();
        rate_article(&conn, ids[0], FeedbackKind::Up).unwrap();
        assert_eq!(
            feedback_for_article(&conn, ids[0]).unwrap(),
            Some(FeedbackKind::Up)
        );
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM feedback", [], |row| row.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }
}
