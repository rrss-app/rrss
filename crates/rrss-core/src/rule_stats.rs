//! Rule effects and spot-check feedback, calculated from existing rows without caching.

use rusqlite::{Connection, params};

use crate::features::FeatureChip;
use crate::feed_scope::FeedScope;
use crate::rules::Dimension;
use crate::score::{MatchInput, condition_hits, strip_html};
use crate::{Result, articles, hidden};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

pub(crate) const SPOT_EVENT_SQL: &str =
    "EXISTS(SELECT 1 FROM article_events e WHERE e.article_id = x.article_id
            AND e.at >= x.shown_at AND e.kind IN ($kinds))";

#[derive(Debug)]
pub struct RuleStats {
    pub rule_id: i64,
    pub hits_30d: i64,
    pub last_hit_at: Option<i64>,
    pub hidden_count: i64,
    pub advice: String,
}

#[derive(Debug)]
pub struct SpotCheckStats {
    pub exposed: i64,
    pub liked: i64,
    pub disliked: i64,
    pub insufficient: bool,
}

pub fn list_rule_stats(conn: &Connection, now: i64) -> Result<Vec<RuleStats>> {
    let counts = hidden::rule_counts(conn, FeedScope::All, articles::list_scope(conn)?)?;
    let reasons = hidden::REASONS_SQL;
    let mut stmt = conn.prepare(&format!(
        "WITH hits AS (
            SELECT json_extract(reason.value, '$.rule_id') rule_id,
                   COUNT(DISTINCT CASE WHEN a.created_at >= ?1 THEN a.id END) hits_30d,
                   MAX(CASE WHEN a.created_at >= ?1 THEN a.created_at END) last_hit_at,
                   COUNT(DISTINCT a.id) hits_90d
            FROM articles a, {reasons}
            WHERE a.created_at >= ?2 AND reason.type = 'object'
            GROUP BY json_extract(reason.value, '$.rule_id')
         ), total AS (SELECT COUNT(*) n FROM articles WHERE created_at >= ?1)
         SELECT r.id, COALESCE(h.hits_30d, 0), h.last_hit_at,
                CASE WHEN r.created_at < ?2 AND COALESCE(h.hits_90d, 0) = 0 THEN 'stale'
                     WHEN (CASE r.polarity WHEN 'dislike' THEN -r.weight ELSE r.weight END) < 0
                          AND total.n > 0 AND COALESCE(h.hits_30d, 0) * 100 >= total.n * 15
                     THEN 'wide' ELSE 'none' END
         FROM rules r LEFT JOIN hits h ON h.rule_id = r.id CROSS JOIN total
         ORDER BY r.id"
    ))?;
    let rows = stmt.query_map(params![now - 30 * DAY_MS, now - 90 * DAY_MS], |row| {
        let rule_id = row.get(0)?;
        Ok(RuleStats {
            rule_id,
            hits_30d: row.get(1)?,
            last_hit_at: row.get(2)?,
            hidden_count: counts.get(&rule_id).copied().unwrap_or(0),
            advice: row.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// 训练条候选在近 30 天的影响面，不含这篇本身。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CandidateImpact {
    /// 全部订阅里命中的文章数。
    pub matched: i64,
    /// 其中打开、赞、收藏、引用或追问过的。
    pub read: i64,
    /// 其中与这篇同源的。
    pub matched_in_feed: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateImpacts {
    /// 与 `candidates` 一一对应。
    pub impacts: Vec<CandidateImpact>,
    /// 这篇的来源近 30 天的其他文章数。
    pub feed_articles: i64,
}

/// 按近 30 天到达的长期文章统计每条候选的命中与读过数；只在用户展开候选时调用，不缓存。
pub fn candidate_impacts(
    conn: &Connection,
    article_id: i64,
    candidates: &[FeatureChip],
    now: i64,
) -> Result<CandidateImpacts> {
    struct Row {
        id: i64,
        feed_id: i64,
        author: Option<String>,
        title: String,
        url: Option<String>,
        html: Option<String>,
        read: bool,
    }
    let feed_id: i64 = conn.query_row(
        "SELECT feed_id FROM articles WHERE id = ?1",
        [article_id],
        |row| row.get(0),
    )?;
    let body = candidates
        .iter()
        .any(|c| matches!(c.dimension, Dimension::BodyPhrase));
    let since = now - 30 * DAY_MS;
    let mut stmt = conn.prepare(&format!(
        "SELECT a.id, a.feed_id, a.author, a.title, a.canonical_url, {html},
                a.starred = 1
                OR EXISTS(SELECT 1 FROM feedback f WHERE f.article_id = a.id AND f.kind = 'up')
                OR EXISTS(SELECT 1 FROM read_sessions s WHERE s.article_id = a.id)
                OR EXISTS(SELECT 1 FROM article_events e WHERE e.article_id = a.id
                          AND e.kind IN ('open', 'up', 'star', 'cite', 'ask', 'open_original'))
         FROM articles a
         WHERE a.created_at >= ?1 AND a.expires_at IS NULL AND a.id <> ?2",
        html = if body { "a.content_html" } else { "NULL" },
    ))?;
    let rows = stmt
        .query_map(params![since, article_id], |row| {
            Ok(Row {
                id: row.get(0)?,
                feed_id: row.get(1)?,
                author: row.get(2)?,
                title: row.get(3)?,
                url: row.get(4)?,
                html: row.get(5)?,
                read: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut tags: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
    if candidates
        .iter()
        .any(|c| matches!(c.dimension, Dimension::Tag))
    {
        let mut stmt = conn.prepare(
            "SELECT t.article_id, t.tag FROM article_tags t JOIN articles a ON a.id = t.article_id
             WHERE a.created_at >= ?1 AND a.expires_at IS NULL AND a.id <> ?2",
        )?;
        for row in stmt.query_map(params![since, article_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })? {
            let (id, tag) = row?;
            tags.entry(id).or_default().push(tag);
        }
    }
    let mut impacts = vec![CandidateImpact::default(); candidates.len()];
    let mut feed_articles = 0;
    let no_tags = Vec::new();
    for row in &rows {
        let same_feed = row.feed_id == feed_id;
        feed_articles += i64::from(same_feed);
        let body = row.html.as_deref().map(strip_html).unwrap_or_default();
        let input = MatchInput {
            feed_id: row.feed_id,
            folder_ids: &[],
            author: row.author.as_deref(),
            tags: tags.get(&row.id).unwrap_or(&no_tags),
            title: &row.title,
            url: row.url.as_deref(),
            body: &body,
        };
        for (impact, candidate) in impacts.iter_mut().zip(candidates) {
            if condition_hits(candidate.dimension, &candidate.pattern, &input) {
                impact.matched += 1;
                impact.read += i64::from(row.read);
                impact.matched_in_feed += i64::from(same_feed);
            }
        }
    }
    Ok(CandidateImpacts {
        impacts,
        feed_articles,
    })
}

pub fn spot_check_stats(conn: &Connection, now: i64) -> Result<SpotCheckStats> {
    let liked = SPOT_EVENT_SQL.replace("$kinds", "'open', 'up'");
    let disliked = SPOT_EVENT_SQL.replace("$kinds", "'down'");
    let (exposed, liked, disliked) = conn.query_row(
        &format!(
            "SELECT COUNT(DISTINCT x.article_id),
                    COUNT(DISTINCT CASE WHEN {liked} THEN x.article_id END),
                    COUNT(DISTINCT CASE WHEN {disliked} THEN x.article_id END)
             FROM exposures x WHERE x.variant = 'spot_check' AND x.shown_at >= ?1"
        ),
        [now - 30 * DAY_MS],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    Ok(SpotCheckStats {
        exposed,
        liked,
        disliked,
        insufficient: exposed < 50,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, rules};

    const NOW: i64 = 1_800_000_000_000;

    fn setup() -> (tempfile::TempDir, db::PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("stats.db")).unwrap();
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://stats.invalid/feed',0)",
            [],
        )
        .unwrap();
        (dir, conn)
    }

    fn rule(conn: &Connection, polarity: rules::Polarity, created: i64) -> i64 {
        let id = rules::insert_rule(
            conn,
            rules::NewRule::manual_global(polarity, rules::Dimension::TitleKw, "Rust"),
        )
        .unwrap();
        conn.execute(
            "UPDATE rules SET created_at=?1 WHERE id=?2",
            params![created, id],
        )
        .unwrap();
        id
    }

    fn article(conn: &Connection, id: i64, at: i64, reasons: Option<&str>) {
        conn.execute(
            "INSERT INTO articles(id,feed_id,created_at,reasons_json) VALUES(?1,1,?2,?3)",
            params![id, at, reasons],
        )
        .unwrap();
    }

    fn reason(ids: &[i64]) -> String {
        serde_json::to_string(
            &ids.iter()
                .map(|id| {
                    serde_json::json!({
                        "rule_id":id,"dimension":"title_kw","snippet":"Rust","weight":-10
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }

    #[test]
    fn hits_count_distinct_articles_and_latest_created_time_in_30_days() {
        let (_dir, conn) = setup();
        let first = rule(&conn, rules::Polarity::Dislike, NOW);
        let second = rule(&conn, rules::Polarity::Like, NOW);
        let missing = rule(&conn, rules::Polarity::Dislike, NOW);
        article(
            &conn,
            1,
            NOW - 30 * DAY_MS,
            Some(&reason(&[first, first, second])),
        );
        article(&conn, 2, NOW - 1, Some(&reason(&[first])));
        article(&conn, 3, NOW - 30 * DAY_MS - 1, Some(&reason(&[second])));
        article(&conn, 4, NOW - 91 * DAY_MS, Some(&reason(&[first])));
        article(&conn, 5, NOW, Some("invalid"));
        article(&conn, 6, NOW, None);
        article(&conn, 7, NOW, Some(r#"["invalid reason"]"#));
        let rows = list_rule_stats(&conn, NOW).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            (rows[0].rule_id, rows[0].hits_30d, rows[0].last_hit_at),
            (first, 2, Some(NOW - 1))
        );
        assert_eq!(
            (rows[1].rule_id, rows[1].hits_30d, rows[1].last_hit_at),
            (second, 1, Some(NOW - 30 * DAY_MS))
        );
        assert_eq!(
            (rows[2].rule_id, rows[2].hits_30d, rows[2].last_hit_at),
            (missing, 0, None)
        );
    }

    #[test]
    fn advice_uses_strict_90_day_age_and_inclusive_15_percent_boundary() {
        let (_dir, conn) = setup();
        let stale = rule(&conn, rules::Polarity::Dislike, NOW - 90 * DAY_MS - 1);
        let age_boundary = rule(&conn, rules::Polarity::Dislike, NOW - 90 * DAY_MS);
        let recent_hit = rule(&conn, rules::Polarity::Dislike, NOW - 100 * DAY_MS);
        let wide = rule(&conn, rules::Polarity::Dislike, NOW);
        let below = rule(&conn, rules::Polarity::Dislike, NOW);
        let positive = rule(&conn, rules::Polarity::Like, NOW);
        let zero = rule(&conn, rules::Polarity::Dislike, NOW);
        conn.execute("UPDATE rules SET weight=0 WHERE id=?1", [zero])
            .unwrap();
        assert!(
            list_rule_stats(&conn, NOW)
                .unwrap()
                .iter()
                .all(|r| r.advice != "wide")
        );
        for id in 1..=20 {
            let mut ids = Vec::new();
            if id <= 3 {
                ids.extend([wide, positive, zero]);
            }
            if id <= 2 {
                ids.push(below);
            }
            article(&conn, id, NOW, Some(&reason(&ids)));
        }
        article(&conn, 21, NOW - 91 * DAY_MS, Some(&reason(&[stale])));
        article(&conn, 22, NOW - 90 * DAY_MS, Some(&reason(&[recent_hit])));
        let rows = list_rule_stats(&conn, NOW).unwrap();
        let advice = |id| {
            rows.iter()
                .find(|row| row.rule_id == id)
                .unwrap()
                .advice
                .as_str()
        };
        assert_eq!(advice(stale), "stale");
        for id in [age_boundary, recent_hit, below, positive, zero] {
            assert_eq!(advice(id), "none");
        }
        assert_eq!(advice(wide), "wide");
    }

    fn exposure(conn: &Connection, article_id: i64, at: i64, variant: &str, session: &str) {
        conn.execute("INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
            VALUES(?1,'focus',1,-10,0,-10,?2,?3,?4)",params![article_id,variant,session,at]).unwrap();
    }

    fn event(conn: &Connection, article_id: i64, at: i64, kind: &str) {
        conn.execute(
            "INSERT INTO article_events(article_id,kind,at) VALUES(?1,?2,?3)",
            params![article_id, kind, at],
        )
        .unwrap();
    }

    #[test]
    fn spot_feedback_is_after_exposure_deduplicated_and_needs_50_articles() {
        let (_dir, conn) = setup();
        let empty = spot_check_stats(&conn, NOW).unwrap();
        assert_eq!(
            (
                empty.exposed,
                empty.liked,
                empty.disliked,
                empty.insufficient
            ),
            (0, 0, 0, true)
        );
        for id in 1..=52 {
            article(&conn, id, NOW, None);
        }
        exposure(&conn, 1, NOW - 30 * DAY_MS, "spot_check", "first");
        exposure(&conn, 1, NOW, "spot_check", "repeat");
        event(&conn, 1, NOW - 30 * DAY_MS + 1, "open");
        event(&conn, 1, NOW - 30 * DAY_MS + 2, "up");
        exposure(&conn, 2, NOW - 1, "spot_check", "first");
        event(&conn, 2, NOW - 2, "open");
        event(&conn, 2, NOW - 2, "down");
        event(&conn, 2, NOW - 1, "down");
        exposure(&conn, 3, NOW - 5, "spot_check", "first");
        event(&conn, 3, NOW - 4, "up");
        event(&conn, 3, NOW, "down");
        exposure(&conn, 4, NOW - 30 * DAY_MS - 1, "spot_check", "old");
        event(&conn, 4, NOW, "up");
        exposure(&conn, 5, NOW - 1, "default", "ordinary");
        event(&conn, 5, NOW, "up");
        exposure(&conn, 6, NOW, "spot_check", "first");
        event(&conn, 6, NOW, "star");
        let stats = spot_check_stats(&conn, NOW).unwrap();
        assert_eq!(
            (
                stats.exposed,
                stats.liked,
                stats.disliked,
                stats.insufficient
            ),
            (4, 2, 2, true)
        );
        for id in 7..=51 {
            exposure(&conn, id, NOW, "spot_check", "first");
        }
        let stats = spot_check_stats(&conn, NOW).unwrap();
        assert_eq!((stats.exposed, stats.insufficient), (49, true));
        exposure(&conn, 52, NOW, "spot_check", "first");
        let stats = spot_check_stats(&conn, NOW).unwrap();
        assert_eq!(
            (
                stats.exposed,
                stats.liked,
                stats.disliked,
                stats.insufficient
            ),
            (50, 2, 2, false)
        );
    }

    #[test]
    fn candidate_impacts_count_other_recent_articles_and_what_was_read() {
        let (_dir, conn) = setup();
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(2,'https://other.invalid/feed',0)",
            [],
        )
        .unwrap();
        let insert = |id: i64, feed: i64, at: i64, author: &str, title: &str, tag: Option<&str>| {
            conn.execute(
                "INSERT INTO articles(id,feed_id,created_at,author,title,canonical_url)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    id,
                    feed,
                    at,
                    author,
                    title,
                    format!("https://deals.invalid/{id}")
                ],
            )
            .unwrap();
            if let Some(tag) = tag {
                conn.execute(
                    "INSERT INTO article_tags(article_id,tag) VALUES(?1,?2)",
                    params![id, tag],
                )
                .unwrap();
            }
        };
        insert(1, 1, NOW, "Mira", "Frame sale", Some("Deals"));
        insert(2, 1, NOW - DAY_MS, "Mira", "Earbuds review", Some("deals"));
        insert(3, 1, NOW - DAY_MS, "Mira", "Another frame sale", None);
        insert(4, 2, NOW - DAY_MS, "Ken", "Frame history", Some("Deals"));
        insert(
            5,
            1,
            NOW - 31 * DAY_MS,
            "Mira",
            "Old frame sale",
            Some("Deals"),
        );
        conn.execute("UPDATE articles SET expires_at = 1 WHERE id = 4", [])
            .unwrap();
        insert(6, 2, NOW - DAY_MS, "Ken", "Sale roundup", Some("Deals"));
        conn.execute(
            "INSERT INTO article_events(article_id,kind,at) VALUES(2,'open',?1)",
            [NOW],
        )
        .unwrap();
        // 只是标成已读不算读过；这篇本身读过也不计入。
        conn.execute("UPDATE articles SET read = 1 WHERE id IN (3, 6)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO article_events(article_id,kind,at) VALUES(1,'open',?1)",
            [NOW],
        )
        .unwrap();
        let chip = |dimension, pattern: &str| FeatureChip {
            dimension,
            pattern: pattern.into(),
        };
        let out = candidate_impacts(
            &conn,
            1,
            &[
                chip(Dimension::Tag, "Deals"),
                chip(Dimension::Author, "mira"),
                chip(Dimension::TitleKw, "sale"),
                chip(Dimension::UrlPhrase, "deals.invalid"),
            ],
            NOW,
        )
        .unwrap();
        assert_eq!(out.feed_articles, 2);
        let counts: Vec<_> = out
            .impacts
            .iter()
            .map(|i| (i.matched, i.read, i.matched_in_feed))
            .collect();
        assert_eq!(counts, [(2, 1, 1), (2, 1, 2), (2, 0, 1), (3, 1, 2)]);
    }
}
