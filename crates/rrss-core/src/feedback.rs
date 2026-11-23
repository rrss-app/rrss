//! 赞踩只记 `feedback`，建议规则需确认；评分可撤销。日志 target：`rrss_core::feedback`。

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

    fn parse(value: &str) -> Option<Self> {
        match value {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            _ => None,
        }
    }
}

/// 这次评分之前的那条反馈；撤销时原样放回。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviousFeedback {
    pub kind: FeedbackKind,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateResult {
    pub kind: FeedbackKind,
    pub suggested_rule_id: Option<i64>,
    pub previous: Option<PreviousFeedback>,
}

/// 撤销所需的全部信息：`rate_article` 的结果原样带回。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RatingUndo {
    pub kind: FeedbackKind,
    pub previous: Option<PreviousFeedback>,
    pub suggested_rule_id: Option<i64>,
}

/// 写反馈；不写规则。同作者累计踩到阈值时插入 disabled suggested。
pub fn rate_article(conn: &Connection, article_id: i64, kind: FeedbackKind) -> Result<RateResult> {
    if crate::articles::is_temporary(conn, article_id)? {
        return Ok(RateResult {
            kind,
            suggested_rule_id: None,
            previous: None,
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
    let previous = previous_feedback(conn, article_id)?;
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
        previous,
    })
}

/// 撤销一次评分。这篇的反馈仍是那次写入的种类时，换回之前的反馈（之前没有就删除）；
/// 之后又改过评分就不动反馈。那次生成、仍未确认的作者建议一并删除。返回反馈是否改回。
pub fn undo_rating(conn: &Connection, article_id: i64, undo: RatingUndo) -> Result<bool> {
    let tx = conn
        .is_autocommit()
        .then(|| crate::db::write_transaction(conn))
        .transpose()?;
    let current = previous_feedback(conn, article_id)?.map(|f| f.kind);
    let restored = current == Some(undo.kind);
    if restored {
        match undo.previous {
            Some(previous) => conn.execute(
                "UPDATE feedback SET kind = ?2, created_at = ?3 WHERE article_id = ?1",
                params![article_id, previous.kind.as_str(), previous.created_at],
            )?,
            None => conn.execute("DELETE FROM feedback WHERE article_id = ?1", [article_id])?,
        };
    }
    if let Some(rule_id) = undo.suggested_rule_id {
        conn.execute(
            "DELETE FROM rules WHERE id = ?1 AND source = 'suggested' AND enabled = 0",
            [rule_id],
        )?;
    }
    if let Some(tx) = tx {
        tx.commit()?;
    }
    tracing::info!(article_id, restored, "feedback undone");
    Ok(restored)
}

pub fn feedback_for_article(conn: &Connection, article_id: i64) -> Result<Option<FeedbackKind>> {
    Ok(previous_feedback(conn, article_id)?.map(|f| f.kind))
}

/// 一批文章各自的评分；没有评分的不在结果里。列表卡片据此标出已赞 / 已踩。
pub fn feedback_for_articles(
    conn: &Connection,
    article_ids: &[i64],
) -> Result<std::collections::HashMap<i64, FeedbackKind>> {
    if article_ids.is_empty() {
        return Ok(Default::default());
    }
    let ids = serde_json::to_string(article_ids).expect("integer ids serialize");
    let mut stmt = conn.prepare_cached(
        "SELECT article_id, kind FROM feedback WHERE article_id IN (SELECT value FROM json_each(?1))",
    )?;
    let rows = stmt.query_map([ids], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut out = std::collections::HashMap::new();
    for row in rows {
        let (id, kind) = row?;
        if let Some(kind) = FeedbackKind::parse(&kind) {
            out.insert(id, kind);
        }
    }
    Ok(out)
}

fn previous_feedback(conn: &Connection, article_id: i64) -> Result<Option<PreviousFeedback>> {
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT kind, created_at FROM feedback WHERE article_id = ?1",
            [article_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    Ok(row.and_then(|(kind, created_at)| {
        FeedbackKind::parse(&kind).map(|kind| PreviousFeedback { kind, created_at })
    }))
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

    fn undo_of(result: &RateResult) -> RatingUndo {
        RatingUndo {
            kind: result.kind,
            previous: result.previous,
            suggested_rule_id: result.suggested_rule_id,
        }
    }

    #[test]
    fn undo_restores_the_rating_before_the_dislike() {
        let (_dir, conn) = open_temp();
        let ids = insert_authored(&conn, 2);
        let fresh = rate_article(&conn, ids[0], FeedbackKind::Down).unwrap();
        assert!(undo_rating(&conn, ids[0], undo_of(&fresh)).unwrap());
        assert_eq!(feedback_for_article(&conn, ids[0]).unwrap(), None);

        rate_article(&conn, ids[1], FeedbackKind::Up).unwrap();
        conn.execute(
            "UPDATE feedback SET created_at = 42 WHERE article_id = ?1",
            [ids[1]],
        )
        .unwrap();
        let changed = rate_article(&conn, ids[1], FeedbackKind::Down).unwrap();
        assert_eq!(
            changed.previous,
            Some(PreviousFeedback {
                kind: FeedbackKind::Up,
                created_at: 42
            })
        );
        assert!(undo_rating(&conn, ids[1], undo_of(&changed)).unwrap());
        let restored: (String, i64) = conn
            .query_row(
                "SELECT kind, created_at FROM feedback WHERE article_id = ?1",
                [ids[1]],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(restored, ("up".into(), 42));
        let kinds = feedback_for_articles(&conn, &ids).unwrap();
        assert_eq!(kinds.get(&ids[1]), Some(&FeedbackKind::Up));
        assert!(!kinds.contains_key(&ids[0]));
    }

    #[test]
    fn undo_keeps_a_later_rating() {
        let (_dir, conn) = open_temp();
        let ids = insert_authored(&conn, 1);
        let down = rate_article(&conn, ids[0], FeedbackKind::Down).unwrap();
        rate_article(&conn, ids[0], FeedbackKind::Up).unwrap();
        assert!(!undo_rating(&conn, ids[0], undo_of(&down)).unwrap());
        assert_eq!(
            feedback_for_article(&conn, ids[0]).unwrap(),
            Some(FeedbackKind::Up)
        );
    }

    #[test]
    fn undo_drops_only_the_unconfirmed_suggestion_it_created() {
        let (_dir, conn) = open_temp();
        let ids = insert_authored(&conn, 6);
        for id in &ids[..4] {
            rate_article(&conn, *id, FeedbackKind::Down).unwrap();
        }
        let fifth = rate_article(&conn, ids[4], FeedbackKind::Down).unwrap();
        let rule_id = fifth.suggested_rule_id.expect("suggestion");
        undo_rating(&conn, ids[4], undo_of(&fifth)).unwrap();
        assert!(get_rule(&conn, rule_id).is_err());

        let again = rate_article(&conn, ids[4], FeedbackKind::Down).unwrap();
        let rule_id = again.suggested_rule_id.expect("suggestion again");
        crate::rules::confirm_rule(&conn, rule_id).unwrap();
        undo_rating(&conn, ids[4], undo_of(&again)).unwrap();
        assert!(get_rule(&conn, rule_id).unwrap().enabled);
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
