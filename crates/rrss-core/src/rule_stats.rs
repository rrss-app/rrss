//! Rule effects and spot-check feedback, calculated from existing rows without caching.

use rusqlite::{Connection, params};

use crate::feed_scope::FeedScope;
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
}
