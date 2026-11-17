//! 显式兴趣 CRUD 与可见列表弱偏向。不改隐藏。日志 target：`rrss_core::interests`。

use crate::error::rules::MemoryProblem;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::db::{
    self, CLUSTER_GROUP_ENABLED_KEY, FOCUS_RERANK_ENABLED_KEY, HIDE_NOISY_KEY,
    INTEREST_BOOST_ENABLED_KEY,
};
use crate::error::{Error, Result};
use crate::feeds::now_ms;
use crate::rag::{SelectedArticle, cosine_distance, le_to_f32s, load_selected_article_vectors};
use crate::rank::reasons::{ReasonMessage, ReasonPrefix, StoredReason};

pub const INTEREST_DIST: f32 = 0.35;
pub const DEFAULT_WEIGHT: i64 = 2;
mod prepare;

const INTEREST_BOOST_SIG_KEY: &str = "interest_boost_sig";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interest {
    pub id: i64,
    pub label: String,
    pub weight: i64,
    pub source: String,
    pub enabled: bool,
    pub has_embedding: bool,
    pub created_at: i64,
    pub hit_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct InterestReason {
    pub interest_id: i64,
    pub label: String,
    pub weight: i64,
    // Only the database/IPC projection changes; rrss tool keeps these fields.
    #[serde(skip)]
    pub message: ReasonMessage,
}

impl<'de> Deserialize<'de> for InterestReason {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Stored {
            interest_id: i64,
            #[serde(default)]
            label: Option<String>,
            #[serde(default)]
            weight: Option<i64>,
            #[serde(flatten)]
            message: serde_json::Value,
        }
        let stored = Stored::deserialize(deserializer)?;
        let coded = stored.message.get("code").is_some();
        let weight = if coded {
            stored
                .message
                .get("points")
                .and_then(serde_json::Value::as_i64)
                .or(stored.weight)
        } else {
            stored.weight
        }
        .ok_or_else(|| serde::de::Error::missing_field(if coded { "points" } else { "weight" }))?;
        let message = ReasonMessage::from_stored(
            &stored.message,
            stored.label.as_deref().unwrap_or_default(),
            Some(ReasonPrefix::Interest),
        );
        Ok(Self {
            interest_id: stored.interest_id,
            label: stored
                .label
                .unwrap_or_else(|| message.content().unwrap_or_default().to_string()),
            weight,
            message,
        })
    }
}

pub fn boost_enabled(conn: &Connection) -> Result<bool> {
    db::setting_enabled(conn, INTEREST_BOOST_ENABLED_KEY, true)
}

pub fn set_boost_enabled(conn: &Connection, enabled: bool) -> Result<()> {
    db::set_setting_enabled(conn, INTEREST_BOOST_ENABLED_KEY, enabled)
}

pub fn rerank_enabled(conn: &Connection) -> Result<bool> {
    db::setting_enabled(conn, FOCUS_RERANK_ENABLED_KEY, false)
}

pub fn set_rerank_enabled(conn: &Connection, enabled: bool) -> Result<()> {
    db::set_setting_enabled(conn, FOCUS_RERANK_ENABLED_KEY, enabled)
}

pub fn cluster_group_enabled(conn: &Connection) -> Result<bool> {
    db::setting_enabled(conn, CLUSTER_GROUP_ENABLED_KEY, true)
}

pub fn set_cluster_group_enabled(conn: &Connection, enabled: bool) -> Result<()> {
    db::set_setting_enabled(conn, CLUSTER_GROUP_ENABLED_KEY, enabled)
}

pub fn hide_noisy_enabled(conn: &Connection) -> Result<bool> {
    db::setting_enabled(conn, HIDE_NOISY_KEY, false)
}

pub fn set_hide_noisy_enabled(conn: &Connection, enabled: bool) -> Result<()> {
    db::set_setting_enabled(conn, HIDE_NOISY_KEY, enabled)
}

pub fn interest_ready(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM interests
         WHERE enabled = 1 AND embedding IS NOT NULL AND length(embedding) > 0 AND space_id = ?1",
        [crate::vector_space::active(conn)?.map(|s| s.id)],
        |row| row.get(0),
    )?;
    Ok(n > 0)
}

fn has_enabled_rules(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM rules WHERE enabled = 1", [], |row| {
        row.get(0)
    })?;
    Ok(n > 0)
}

/// 兴趣加分 / 关注重排实际没在工作时给界面的说明码（`interest_boost_inactive` /
/// `interest_not_ready`），文字由前端生成。就绪则空串。
pub fn ranking_note(conn: &Connection) -> Result<String> {
    let boost = boost_enabled(conn)?;
    let rerank = rerank_enabled(conn)?;
    if !boost && !rerank {
        return Ok(String::new());
    }
    if interest_ready(conn)? {
        return Ok(String::new());
    }
    let rules = has_enabled_rules(conn).unwrap_or(false);
    if rerank && rules && !boost {
        return Ok(String::new());
    }
    if boost && rules {
        return Ok("interest_boost_inactive".into());
    }
    Ok("interest_not_ready".into())
}

pub fn list_interests(conn: &Connection) -> Result<Vec<Interest>> {
    let mut stmt = conn.prepare(
        "SELECT id, label, weight, source, enabled,
                CASE WHEN embedding IS NOT NULL AND length(embedding) > 0 THEN 1 ELSE 0 END,
                created_at,
                (SELECT COUNT(*) FROM article_interests ai WHERE ai.interest_id = interests.id)
         FROM interests
         ORDER BY id",
    )?;
    let rows = stmt.query_map([], map_interest)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn insert_interest(conn: &Connection, label: &str, source: &str) -> Result<i64> {
    let label = label.trim();
    if label.is_empty() {
        return Err(Error::InvalidMemory(MemoryProblem::InterestLabelMissing));
    }
    conn.execute(
        "INSERT INTO interests (label, weight, source, enabled, created_at)
         VALUES (?1, ?2, ?3, 1, ?4)",
        params![label, DEFAULT_WEIGHT, source, now_ms()],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Update an interest, deferring large boost rebuilds; requires autocommit mode.
pub fn set_enabled(conn: &Connection, id: i64, enabled: bool) -> Result<()> {
    prepare::run(conn, Some(prepare::Change::Enabled(id, enabled))).map(|_| ())
}

/// Delete an interest, deferring large boost rebuilds; requires autocommit mode.
pub fn delete_interest(conn: &Connection, id: i64) -> Result<()> {
    prepare::run(conn, Some(prepare::Change::Delete(id))).map(|_| ())
}

pub fn parse_interest_reasons(json: Option<&str>) -> Vec<InterestReason> {
    let Some(json) = json.filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    serde_json::from_str(json).unwrap_or_default()
}

/// Recompute in a read snapshot and publish atomically; requires autocommit mode.
pub fn recompute_interest_boost(conn: &Connection) -> Result<u32> {
    prepare::run(conn, None)
}

fn boost_up_to_date(conn: &Connection) -> Result<bool> {
    let stored = db::get_setting(conn, INTEREST_BOOST_SIG_KEY)?;
    let prototypes = if boost_enabled(conn)? {
        load_prototypes(conn)?
    } else {
        Vec::new()
    };
    if prototypes.is_empty() {
        let leftover: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM article_interests)
                OR EXISTS(SELECT 1 FROM articles
                          WHERE interest_boost != 0 OR interest_reasons_json IS NOT NULL)",
            [],
            |r| r.get(0),
        )?;
        return Ok(stored.as_deref() == Some("") && !leftover);
    }
    let threshold = crate::articles::hide_threshold(conn)?;
    let selected = load_selected_article_vectors(conn, threshold)?;
    Ok(stored.as_deref() == Some(boost_signature(&prototypes, threshold, &selected).as_str()))
}

fn clear_boosts(conn: &Connection) -> Result<u32> {
    conn.execute("DELETE FROM article_interests", [])?;
    let n = conn.execute(
        "UPDATE articles SET interest_boost = 0, interest_reasons_json = NULL
         WHERE interest_boost != 0 OR interest_reasons_json IS NOT NULL",
        [],
    )?;
    db::set_setting(conn, INTEREST_BOOST_SIG_KEY, "")?;
    Ok(n as u32)
}

fn hex_sha256(hasher: Sha256) -> String {
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
            s
        })
}

fn boost_signature(
    prototypes: &[Prototype],
    threshold: i64,
    selected: &[SelectedArticle],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"v1");
    hasher.update(threshold.to_le_bytes());
    hasher.update((prototypes.len() as u64).to_le_bytes());
    for proto in prototypes {
        hasher.update(proto.id.to_le_bytes());
        hasher.update((proto.label.len() as u64).to_le_bytes());
        hasher.update(proto.label.as_bytes());
        hasher.update(proto.weight.to_le_bytes());
        hasher.update((proto.vector.len() as u64).to_le_bytes());
        for x in &proto.vector {
            hasher.update(x.to_le_bytes());
        }
    }
    hasher.update((selected.len() as u64).to_le_bytes());
    for item in selected {
        hasher.update(item.article_id.to_le_bytes());
        hasher.update((item.blob.len() as u64).to_le_bytes());
        hasher.update(&item.blob);
    }
    hex_sha256(hasher)
}

pub(crate) struct Prototype {
    id: i64,
    label: String,
    weight: i64,
    vector: Vec<f32>,
}

pub(crate) fn load_prototypes(conn: &Connection) -> Result<Vec<Prototype>> {
    let mut stmt = conn.prepare(
        "SELECT id, label, weight, embedding FROM interests
         WHERE enabled = 1 AND embedding IS NOT NULL AND length(embedding) > 0 AND space_id = ?1
         ORDER BY id",
    )?;
    let rows = stmt.query_map([crate::vector_space::active(conn)?.map(|s| s.id)], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Vec<u8>>(3)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, label, weight, blob) = row?;
        let Some(vector) = le_to_f32s(&blob) else {
            continue;
        };
        out.push(Prototype {
            id,
            label,
            weight,
            vector,
        });
    }
    Ok(out)
}

/// 阶段 23「M2 离线回放」：写入路径与离线回放共用的纯兴趣加分。
pub(crate) fn calculate_boost(vec: &[f32], prototypes: &[Prototype]) -> (i64, Vec<InterestReason>) {
    let mut reasons = Vec::new();
    let mut boost = 0i64;
    for proto in prototypes {
        let dist = cosine_distance(vec, &proto.vector);
        if dist < INTEREST_DIST {
            let scaled = ((proto.weight as f32) * (1.0 - dist / INTEREST_DIST)).round() as i64;
            let weight = scaled.max(1);
            boost += weight;
            let pattern = crate::rank::reasons::safe_text(&proto.label);
            reasons.push(InterestReason {
                interest_id: proto.id,
                label: proto.label.clone(),
                weight,
                message: if pattern == proto.label {
                    ReasonMessage::InterestMatch { pattern }
                } else {
                    ReasonMessage::LegacyText {
                        text: proto.label.clone(),
                        prefix: Some(ReasonPrefix::Interest),
                    }
                },
            });
        }
    }
    boost = boost.max(0);
    (boost, reasons)
}

fn reasons_json(reasons: &[InterestReason]) -> Option<String> {
    #[derive(Serialize)]
    struct StoredInterest<'a> {
        #[serde(flatten)]
        reason: StoredReason<'a>,
        interest_id: i64,
        label: &'a str,
    }
    let json = if reasons.is_empty() {
        None
    } else {
        let stored: Vec<_> = reasons
            .iter()
            .map(|reason| StoredInterest {
                reason: StoredReason {
                    component: "interest",
                    message: &reason.message,
                    points: reason.weight,
                },
                interest_id: reason.interest_id,
                label: &reason.label,
            })
            .collect();
        Some(serde_json::to_string(&stored).unwrap_or_else(|_| "[]".into()))
    };
    json
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterestHit {
    pub article_id: i64,
    pub title: String,
    pub feed_title: String,
    pub weight: i64,
}

pub fn list_interest_hits(conn: &Connection, interest_id: i64) -> Result<Vec<InterestHit>> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.title, COALESCE(f.title, f.url), ai.weight
         FROM article_interests ai
         JOIN articles a ON a.id = ai.article_id
         JOIN feeds f ON f.id = a.feed_id
         WHERE a.expires_at IS NULL AND ai.interest_id = ?1
         ORDER BY ai.weight DESC, COALESCE(a.published_at, 0) DESC",
    )?;
    let rows = stmt.query_map([interest_id], |row| {
        Ok(InterestHit {
            article_id: row.get(0)?,
            title: row.get(1)?,
            feed_title: crate::feeds::display_title(&row.get::<_, String>(2)?, ""),
            weight: row.get(3)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn map_interest(row: &rusqlite::Row<'_>) -> rusqlite::Result<Interest> {
    Ok(Interest {
        id: row.get(0)?,
        label: row.get(1)?,
        weight: row.get(2)?,
        source: row.get(3)?,
        enabled: row.get::<_, i64>(4)? != 0,
        has_embedding: row.get::<_, i64>(5)? != 0,
        created_at: row.get(6)?,
        hit_count: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    fn seed_embedding_corpus(conn: &Connection, articles: i64, dims: usize) {
        crate::vector_space::seed_active(conn, "fixture", dims as i64);
        let space = crate::vector_space::active(conn).unwrap().unwrap();
        let vector = crate::rag::f32s_to_le(&vec![1.0 / (dims as f32).sqrt(); dims]);
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://interest.example/feed',0)",
            [],
        )
        .unwrap();
        let tx = db::write_transaction(conn).unwrap();
        for id in 1..=articles {
            tx.execute(
                "INSERT INTO articles(id,feed_id,title,created_at) VALUES(?1,1,'fixture',0)",
                [id],
            )
            .unwrap();
            tx.execute("INSERT INTO chunks(article_id,ordinal,kind,text,embedding,space_id) VALUES(?1,0,'title_summary','fixture',?2,?3)", params![id,vector,space.id]).unwrap();
        }
        for id in 1..=2 {
            tx.execute("INSERT INTO interests(id,label,weight,source,enabled,embedding,space_id,created_at) VALUES(?1,'fixture',2,'manual',1,?2,?3,0)", params![id,vector,space.id]).unwrap();
        }
        tx.commit().unwrap();
    }

    #[test]
    fn large_foreground_save_survives_embedding_writes() {
        for (article_count, dims) in [(2049, 3), (1025, 2048)] {
            for operation in 0..3 {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("interests.db");
                let conn = open_at(&path).unwrap();
                seed_embedding_corpus(&conn, article_count, dims);
                if operation == 0 {
                    conn.execute("UPDATE interests SET enabled=0 WHERE id=1", [])
                        .unwrap();
                }
                recompute_interest_boost(&conn).unwrap();
                let before = article_boost(&conn, 1);
                let other = open_at(&path).unwrap();
                let rounds = std::cell::Cell::new(0);
                let change = match operation {
                    0 => super::prepare::Change::Enabled(1, true),
                    1 => super::prepare::Change::Enabled(1, false),
                    _ => super::prepare::Change::Delete(1),
                };
                let started = std::time::Instant::now();
                let result = super::prepare::run_with(&conn, Some(change), || {
                    rounds.set(rounds.get() + 1);
                    let mut vector = vec![1.0 / (dims as f32).sqrt(); dims];
                    vector[0] += rounds.get() as f32 / 10000.0;
                    other.execute(
                        "UPDATE chunks SET embedding=?1 WHERE article_id=1",
                        [crate::rag::f32s_to_le(&vector)],
                    )?;
                    Ok(())
                });
                eprintln!(
                    "large foreground save: articles={article_count}, dims={dims}, operation={operation}, rounds={}, total_ms={:.2}, result={result:?}",
                    rounds.get(),
                    started.elapsed().as_secs_f64() * 1000.0
                );
                result.unwrap();
                assert_eq!(rounds.get(), 1);
                if operation == 2 {
                    assert_eq!(
                        conn.query_row("SELECT COUNT(*) FROM interests WHERE id=1", [], |r| r
                            .get::<_, i64>(0))
                            .unwrap(),
                        0
                    );
                } else {
                    assert_eq!(
                        conn.query_row("SELECT enabled FROM interests WHERE id=1", [], |r| r
                            .get::<_, bool>(0))
                            .unwrap(),
                        operation == 0
                    );
                }
                // No partially calculated rows are published when the writer budget is
                // exceeded; the existing background entry point finishes the rebuild.
                assert_eq!(article_boost(&conn, 1), before);
                assert_eq!(stored_sig(&conn).as_deref(), Some("recheck"));
                recompute_interest_boost(&conn).unwrap();
                let expected = if operation == 0 {
                    DEFAULT_WEIGHT * 2
                } else {
                    DEFAULT_WEIGHT
                };
                assert_eq!(article_boost(&conn, 1), expected);
                assert_eq!(article_boost(&conn, article_count), expected);
                assert_ne!(stored_sig(&conn).as_deref(), Some("recheck"));
            }
        }
    }

    #[test]
    #[ignore = "explicit offline foreground conflict latency probe"]
    fn foreground_edit_latency_during_embedding_batches() {
        for article_count in [300, 2049] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("latency.db");
            let conn = open_at(&path).unwrap();
            seed_embedding_corpus(&conn, article_count, 1024);
            let other = open_at(&path).unwrap();
            recompute_interest_boost(&conn).unwrap();
            let mut totals = Vec::new();
            let mut holds = Vec::new();
            let mut failures = 0;
            for sample in 0..20 {
                let started = std::time::Instant::now();
                let round = std::cell::Cell::new(0);
                let prior_hold = db::pool::metrics(&path)
                    .unwrap()
                    .write_hold
                    .snapshot()
                    .total_ms;
                let result = super::prepare::run_with(
                    &conn,
                    Some(super::prepare::Change::Enabled(1, sample % 2 == 0)),
                    || {
                        round.set(round.get() + 1);
                        let mut changed = vec![1.0 / 32.0; 1024];
                        changed[0] += (sample * 3 + round.get()) as f32 / 10000.0;
                        other.execute(
                            "UPDATE chunks SET embedding=?1 WHERE article_id<=30",
                            [crate::rag::f32s_to_le(&changed)],
                        )?;
                        Ok(())
                    },
                );
                if result.is_err() {
                    failures += 1;
                }
                totals.push(started.elapsed().as_secs_f64() * 1000.0);
                holds.push(
                    db::pool::metrics(&path)
                        .unwrap()
                        .write_hold
                        .snapshot()
                        .total_ms
                        - prior_hold,
                );
                recompute_interest_boost(&conn).unwrap();
            }
            eprintln!(
                "foreground conflict probe: articles={article_count}, dims=1024, samples={}, failures={failures}, max_total_ms={:.2}, over_100ms={}, max_sum_write_transactions_ms={:.2}",
                totals.len(),
                totals.iter().copied().fold(0.0, f64::max),
                totals.iter().filter(|ms| **ms > 100.0).count(),
                holds.iter().copied().fold(0.0, f64::max)
            );
            assert_eq!(failures, 0);
        }
    }

    #[test]
    fn foreground_edits_converge_during_continuous_vector_writes() {
        for operation in 0..3 {
            let (dir, conn) = open_temp();
            let (article, interest) =
                seed_matching(&conn, "subject", "https://interest.example/item");
            if operation == 0 {
                set_enabled(&conn, interest, false).unwrap();
            }
            recompute_interest_boost(&conn).unwrap();
            let other = crate::db::open_at(&dir.path().join("rrss.db")).unwrap();
            let rounds = std::cell::Cell::new(0);
            let change = match operation {
                0 => super::prepare::Change::Enabled(interest, true),
                1 => super::prepare::Change::Enabled(interest, false),
                _ => super::prepare::Change::Delete(interest),
            };
            super::prepare::run_with(&conn, Some(change), || {
                let round = rounds.get() + 1;
                rounds.set(round);
                set_chunk_embedding(
                    &other,
                    title_chunk_id(&other, article),
                    "test-embed",
                    &[1.0, round as f32 / 100.0, 0.0],
                )?;
                Ok(())
            })
            .unwrap();
            assert_eq!(rounds.get(), 1);
            let before = article_boost(&conn, article);
            assert_eq!(before, if operation == 0 { DEFAULT_WEIGHT } else { 0 });
            recompute_interest_boost(&conn).unwrap();
            assert_eq!(article_boost(&conn, article), before);
        }
    }

    #[test]
    fn foreground_rebase_does_not_publish_after_configuration_conflicts() {
        let (dir, conn) = open_temp();
        let (article, interest) =
            seed_matching(&conn, "subject", "https://interest.example/conflict");
        recompute_interest_boost(&conn).unwrap();
        let before = article_boost(&conn, article);
        let other = crate::db::open_at(&dir.path().join("rrss.db")).unwrap();
        let result = super::prepare::run_with(
            &conn,
            Some(super::prepare::Change::Enabled(interest, false)),
            || {
                other.execute(
                    "UPDATE interests SET label=label||'x' WHERE id=?1",
                    [interest],
                )?;
                Ok(())
            },
        );
        assert!(result.is_err());
        assert_eq!(article_boost(&conn, article), before);
        assert_eq!(
            conn.query_row(
                "SELECT enabled FROM interests WHERE id=?1",
                [interest],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn foreground_rebase_includes_new_vectors_and_clears_removed_ones() {
        let (dir, conn) = open_temp();
        let (removed, interest) =
            seed_matching(&conn, "subject", "https://interest.example/removed");
        let added = seed_article(&conn, "subject", "https://interest.example/added", false);
        recompute_interest_boost(&conn).unwrap();
        let other = open_at(&dir.path().join("rrss.db")).unwrap();
        super::prepare::run_with(
            &conn,
            Some(super::prepare::Change::Enabled(interest, true)),
            || {
                other.execute("DELETE FROM chunks WHERE article_id=?1", [removed])?;
                put_title_vec(&other, added, &[1.0, 0.0, 0.0]);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(article_boost(&conn, removed), 0);
        assert_eq!(article_boost(&conn, added), DEFAULT_WEIGHT);
        assert_eq!(
            conn.query_row("SELECT article_id FROM article_interests", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            added
        );
    }

    #[test]
    fn foreground_rebase_budget_defers_without_partial_results() {
        let (dir, conn) = open_temp();
        let (article, interest) =
            seed_matching(&conn, "subject", "https://interest.example/budget");
        set_enabled(&conn, interest, false).unwrap();
        recompute_interest_boost(&conn).unwrap();
        let other = open_at(&dir.path().join("rrss.db")).unwrap();
        let result = super::prepare::run_with(
            &conn,
            Some(super::prepare::Change::Enabled(interest, true)),
            || {
                // Even one exceptionally large vector must not trigger unbounded
                // work while the writer is held; save only the configuration.
                other.execute(
                    "UPDATE chunks SET embedding=randomblob(1048576) WHERE article_id=?1",
                    [article],
                )?;
                Ok(())
            },
        );
        result.unwrap();
        assert_eq!(article_boost(&conn, article), 0);
        assert_eq!(stored_sig(&conn).as_deref(), Some("recheck"));
        assert_eq!(
            conn.query_row(
                "SELECT enabled FROM interests WHERE id=?1",
                [interest],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }

    fn interest_snapshot(conn: &Connection) -> serde_json::Value {
        let mut rows = Vec::new();
        for sql in [
            "SELECT json_array('interest',id,label,weight,enabled) FROM interests ORDER BY id",
            "SELECT json_array('article',id,interest_boost,json(interest_reasons_json)) FROM articles ORDER BY id",
            "SELECT json_array('hit',article_id,interest_id,weight) FROM article_interests ORDER BY article_id,interest_id",
        ] {
            let mut stmt = conn.prepare(sql).unwrap();
            rows.extend(
                stmt.query_map([], |r| r.get::<_, String>(0))
                    .unwrap()
                    .map(|r| serde_json::from_str::<serde_json::Value>(&r.unwrap()).unwrap()),
            );
        }
        rows.into()
    }

    #[test]
    fn interest_edits_match_legacy_baseline() {
        let (_dir, conn) = open_temp();
        let (article, interest) =
            seed_matching(&conn, "subject", "https://interest.example/matched");
        let other = seed_article(&conn, "other", "https://interest.example/other", false);
        put_title_vec(&conn, other, &[0.0, 1.0, 0.0]);
        assert_eq!((article, other, interest), (1, 2, 1));
        // Captured using the transaction-only implementation at dccf69af,
        // before deletion, with this same fixture. Includes stored reason JSON.
        assert_eq!(recompute_interest_boost(&conn).unwrap(), 1);
        assert_eq!(
            interest_snapshot(&conn),
            serde_json::json!([
                ["interest", 1, "Rust async", 2, 1],
                ["article", 1, 2, [{
                    "code": "interest_match", "component": "interest", "interest_id": 1,
                    "label": "Rust async", "params": {"pattern": "Rust async"}, "points": 2
                }]],
                ["article", 2, 0, null],
                ["hit", 1, 1, 2]
            ])
        );
        conn.execute("UPDATE interests SET weight=5 WHERE id=?1", [interest])
            .unwrap();
        recompute_interest_boost(&conn).unwrap();
        let weighted = serde_json::json!([
            ["interest", 1, "Rust async", 5, 1],
            ["article", 1, 5, [{
                "code": "interest_match", "component": "interest", "interest_id": 1,
                "label": "Rust async", "params": {"pattern": "Rust async"}, "points": 5
            }]],
            ["article", 2, 0, null],
            ["hit", 1, 1, 5]
        ]);
        assert_eq!(interest_snapshot(&conn), weighted);
        set_enabled(&conn, interest, false).unwrap();
        assert_eq!(
            interest_snapshot(&conn),
            serde_json::json!([
                ["interest", 1, "Rust async", 5, 0],
                ["article", 1, 0, null],
                ["article", 2, 0, null]
            ])
        );
        set_enabled(&conn, interest, true).unwrap();
        assert_eq!(interest_snapshot(&conn), weighted);
        delete_interest(&conn, interest).unwrap();
        assert_eq!(
            interest_snapshot(&conn),
            serde_json::json!([["article", 1, 0, null], ["article", 2, 0, null]])
        );
    }

    #[test]
    fn planned_boost_retries_relevant_changes_but_ignores_unrelated_writes() {
        let (dir, conn) = open_temp();
        let (article, interest) =
            seed_matching(&conn, "matching subject", "https://latency.example/story");
        let other = crate::db::open_at(&dir.path().join("rrss.db")).unwrap();
        let rounds = std::cell::Cell::new(0);
        super::prepare::run_with(
            &conn,
            Some(super::prepare::Change::Enabled(interest, true)),
            || {
                let round = rounds.get();
                rounds.set(round + 1);
                other.busy_timeout(std::time::Duration::ZERO)?;
                if round == 0 {
                    other.execute("UPDATE interests SET enabled=0 WHERE id=?1", [interest])?;
                } else {
                    crate::db::set_setting(&other, "close_action", "ask")?;
                }
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(rounds.get(), 2);
        assert_eq!(article_boost(&conn, article), DEFAULT_WEIGHT);
    }

    use super::*;
    use crate::db::{HIDE_THRESHOLD_KEY, PooledConnection, get_setting, open_at, set_setting};
    use crate::feeds::{SubscribeRequest, subscribe};
    use crate::parse::ParsedEntry;
    use crate::rag::{
        DueEmbedArticle, set_chunk_embedding, set_interest_embedding, sync_article_chunks,
    };
    use crate::rules::{Dimension, NewRule, Polarity, insert_rule};
    use rusqlite::params;
    use tempfile::tempdir;

    fn open_temp() -> (tempfile::TempDir, PooledConnection) {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        crate::vector_space::seed_active(&conn, "test-embed", 3);
        (dir, conn)
    }

    fn seed_article(conn: &Connection, title: &str, url: &str, like: bool) -> i64 {
        let feed = subscribe(
            conn,
            SubscribeRequest {
                url: format!("{url}/rss.xml"),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        if like {
            insert_rule(
                conn,
                NewRule::manual_global(Polarity::Like, Dimension::TitleKw, "rust"),
            )
            .unwrap();
        }
        crate::ingest::upsert_entries(
            conn,
            feed,
            &[ParsedEntry {
                guid: url.into(),
                canonical_url: Some(url.into()),
                title: title.into(),
                author: None,
                published_at: Some(1),
                content_html: Some("<p>body</p>".into()),
                hash: crate::parse::entry_hash(title, Some(url), Some("<p>body</p>")),
                tags: vec![],
            }],
            1,
        )
        .unwrap();
        conn.query_row(
            "SELECT id FROM articles WHERE canonical_url = ?1",
            [url],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn put_title_vec(conn: &Connection, article_id: i64, v: &[f32]) {
        let article = DueEmbedArticle {
            id: article_id,
            title: conn
                .query_row(
                    "SELECT title FROM articles WHERE id = ?1",
                    [article_id],
                    |r| r.get(0),
                )
                .unwrap(),
            summary: None,
            blocks_json: None,
            content_html: Some("<p>body</p>".into()),

            tier: 2,
        };
        let pending = sync_article_chunks(conn, &article, article.tier).unwrap();
        let chunk = pending
            .iter()
            .find(|c| {
                let kind: String = conn
                    .query_row("SELECT kind FROM chunks WHERE id = ?1", [c.id], |r| {
                        r.get(0)
                    })
                    .unwrap();
                kind == crate::chunk::TITLE_SUMMARY_KIND
            })
            .unwrap();
        set_chunk_embedding(conn, chunk.id, "test-embed", v).unwrap();
    }

    fn article_boost(conn: &Connection, id: i64) -> i64 {
        conn.query_row(
            "SELECT interest_boost FROM articles WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn article_reasons(conn: &Connection, id: i64) -> String {
        conn.query_row(
            "SELECT COALESCE(interest_reasons_json, '') FROM articles WHERE id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn hit_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM article_interests", [], |r| r.get(0))
            .unwrap()
    }

    fn total_changes(conn: &Connection) -> u64 {
        conn.total_changes()
    }

    fn title_chunk_id(conn: &Connection, article_id: i64) -> i64 {
        conn.query_row(
            "SELECT id FROM chunks WHERE article_id = ?1 AND kind = ?2",
            params![article_id, crate::chunk::TITLE_SUMMARY_KIND],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn body_chunk_id(conn: &Connection, article_id: i64) -> i64 {
        conn.query_row(
            "SELECT id FROM chunks WHERE article_id = ?1 AND kind = ?2 LIMIT 1",
            params![article_id, crate::chunk::BODY_KIND],
            |r| r.get(0),
        )
        .unwrap()
    }

    fn stored_sig(conn: &Connection) -> Option<String> {
        get_setting(conn, super::INTEREST_BOOST_SIG_KEY).unwrap()
    }

    fn seed_matching(conn: &Connection, title: &str, url: &str) -> (i64, i64) {
        let article = seed_article(conn, title, url, true);
        let iid = insert_interest(conn, "Rust async", "manual").unwrap();
        set_interest_embedding(conn, iid, &[1.0, 0.0, 0.0]).unwrap();
        put_title_vec(conn, article, &[1.0, 0.0, 0.0]);
        (article, iid)
    }

    #[test]
    fn boost_hits_similar_visible_skips_hidden() {
        let (_dir, conn) = open_temp();
        let focus = seed_article(&conn, "rust async runtime", "https://lab.example/a", true);
        insert_rule(
            &conn,
            NewRule::manual_global(Polarity::Dislike, Dimension::TitleKw, "promo"),
        )
        .unwrap();
        let hidden = seed_article(&conn, "promo sale junk", "https://ads.example/p", false);
        // hidden has dislike "promo" (-10) and maybe like rust (+10) depending on rules from first insert.
        // First insert_rule is global like rust; second is dislike promo. Hidden title contains both.
        let iid = insert_interest(&conn, "Rust async", "manual").unwrap();
        let proto = vec![1.0f32, 0.0, 0.0];
        set_interest_embedding(&conn, iid, &proto).unwrap();
        put_title_vec(&conn, focus, &[1.0, 0.0, 0.0]);
        put_title_vec(&conn, hidden, &[1.0, 0.0, 0.0]);
        let n = recompute_interest_boost(&conn).unwrap();
        assert!(n >= 1);
        let boost: i64 = conn
            .query_row(
                "SELECT interest_boost FROM articles WHERE id = ?1",
                [focus],
                |r| r.get(0),
            )
            .unwrap();
        assert!(boost > 0);
        let stored: serde_json::Value =
            serde_json::from_str(&article_reasons(&conn, focus)).unwrap();
        assert_eq!(stored[0]["component"], "interest");
        assert_eq!(stored[0]["code"], "interest_match");
        assert_eq!(
            stored[0]["params"],
            serde_json::json!({"pattern":"Rust async"})
        );
        assert_eq!(stored[0]["points"], boost);
        let tool =
            serde_json::to_value(parse_interest_reasons(Some(&article_reasons(&conn, focus))))
                .unwrap();
        assert_eq!(
            tool,
            serde_json::json!([{"interest_id":iid,"label":"Rust async","weight":boost}])
        );
        let label = "https://interest.example/notes?q=topic";
        conn.execute(
            "UPDATE interests SET label=?1 WHERE id=?2",
            params![label, iid],
        )
        .unwrap();
        recompute_interest_boost(&conn).unwrap();
        let updated: serde_json::Value =
            serde_json::from_str(&article_reasons(&conn, focus)).unwrap();
        assert_eq!(updated[0]["code"], "legacy_text");
        assert_eq!(
            updated[0]["params"],
            serde_json::json!({"text":label,"prefix":"interest"})
        );
        let hidden_boost: i64 = conn
            .query_row(
                "SELECT interest_boost FROM articles WHERE id = ?1",
                [hidden],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hidden_boost, 0);
        set_boost_enabled(&conn, false).unwrap();
        assert!(!boost_enabled(&conn).unwrap());
        let boost_kept: i64 = conn
            .query_row(
                "SELECT interest_boost FROM articles WHERE id = ?1",
                [focus],
                |r| r.get(0),
            )
            .unwrap();
        assert!(boost_kept > 0, "toggle must not wipe stored boost");
        crate::articles::set_focus_filter(&conn, crate::articles::FocusFilter::ShowAll).unwrap();
        let cards = crate::articles::list_focus_stream(&conn, 20, None).unwrap();
        let card = cards.iter().find(|c| c.id == focus).expect("focus card");
        assert_eq!(card.interest_boost, boost_kept);
        assert_eq!(card.rank_score(false), card.score);
    }

    #[test]
    fn toggling_boost_changes_order_without_clearing_column() {
        let (_dir, conn) = open_temp();
        let low = seed_article(&conn, "alpha rust", "https://lab.example/low", true);
        let high = seed_article(&conn, "beta rust", "https://lab.example/high", true);
        conn.execute(
            "UPDATE articles SET score = 10, interest_boost = 100 WHERE id = ?1",
            [low],
        )
        .unwrap();
        conn.execute(
            "UPDATE articles SET score = 20, interest_boost = 0 WHERE id = ?1",
            [high],
        )
        .unwrap();
        crate::articles::set_focus_filter(&conn, crate::articles::FocusFilter::ShowAll).unwrap();
        set_cluster_group_enabled(&conn, false).unwrap();
        set_boost_enabled(&conn, true).unwrap();
        let on = crate::articles::list_focus_stream(&conn, 20, None).unwrap();
        assert_eq!(on[0].id, low, "boost on: 10+100 should beat 20");
        set_boost_enabled(&conn, false).unwrap();
        let stored: i64 = conn
            .query_row(
                "SELECT interest_boost FROM articles WHERE id = ?1",
                [low],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, 100);
        let off = crate::articles::list_focus_stream(&conn, 20, None).unwrap();
        assert_eq!(off[0].id, high, "boost off: sort by score only");
    }

    #[test]
    fn ranking_note_when_boost_on_without_vectors() {
        let (_dir, conn) = open_temp();
        set_boost_enabled(&conn, true).unwrap();
        set_rerank_enabled(&conn, true).unwrap();
        assert!(!interest_ready(&conn).unwrap());
        let note = ranking_note(&conn).unwrap();
        assert_eq!(note, "interest_not_ready");
        set_boost_enabled(&conn, false).unwrap();
        set_rerank_enabled(&conn, false).unwrap();
        assert!(ranking_note(&conn).unwrap().is_empty());
    }

    #[test]
    fn recompute_skips_when_signature_unchanged() {
        let (dir, conn) = open_temp();
        let focus = seed_article(
            &conn,
            "rust async runtime",
            "https://lab.example/skip",
            true,
        );
        let iid = insert_interest(&conn, "Rust async", "manual").unwrap();
        set_interest_embedding(&conn, iid, &[1.0, 0.0, 0.0]).unwrap();
        put_title_vec(&conn, focus, &[1.0, 0.0, 0.0]);
        let first = recompute_interest_boost(&conn).unwrap();
        assert!(first >= 1);
        let changes = total_changes(&conn);
        let second = recompute_interest_boost(&conn).unwrap();
        assert_eq!(second, 0, "unchanged prototypes/embeddings should skip");
        assert_eq!(total_changes(&conn), changes, "skip must not write rows");
        let boost: i64 = conn
            .query_row(
                "SELECT interest_boost FROM articles WHERE id = ?1",
                [focus],
                |r| r.get(0),
            )
            .unwrap();
        assert!(boost > 0);

        // 无变化的核对只读：另一连接持有写锁时也能立即判定。
        let writer = open_at(&dir.path().join("rrss.db")).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        conn.busy_timeout(std::time::Duration::ZERO).unwrap();
        assert_eq!(recompute_interest_boost(&conn).unwrap(), 0);
        writer.execute_batch("ROLLBACK").unwrap();
        conn.busy_timeout(std::time::Duration::from_secs(30))
            .unwrap();

        let body_id: i64 = conn
            .query_row(
                "SELECT id FROM chunks WHERE article_id = ?1 AND kind = ?2 LIMIT 1",
                params![focus, crate::chunk::BODY_KIND],
                |r| r.get(0),
            )
            .unwrap();
        set_chunk_embedding(&conn, body_id, "test-embed", &[0.0, 1.0, 0.0]).unwrap();
        let after_body = total_changes(&conn);
        assert_eq!(
            recompute_interest_boost(&conn).unwrap(),
            0,
            "unselected body vector must not invalidate"
        );
        assert_eq!(total_changes(&conn), after_body);
        assert_eq!(article_boost(&conn, focus), boost);
    }

    #[test]
    fn same_length_title_vector_change_invalidates() {
        let (_dir, conn) = open_temp();
        let (focus, _) = seed_matching(&conn, "rust async runtime", "https://lab.example/inplace");
        assert!(recompute_interest_boost(&conn).unwrap() >= 1);
        assert!(article_boost(&conn, focus) > 0);

        set_chunk_embedding(
            &conn,
            title_chunk_id(&conn, focus),
            "test-embed",
            &[0.0, 1.0, 0.0],
        )
        .unwrap();
        let n = recompute_interest_boost(&conn).unwrap();
        assert_eq!(n, 0, "orthogonal same-length vector should not match");
        assert_eq!(article_boost(&conn, focus), 0);
        assert_eq!(hit_count(&conn), 0);
    }

    #[test]
    fn hidden_set_replacement_invalidates() {
        let (_dir, conn) = open_temp();
        let a = seed_article(&conn, "rust alpha", "https://lab.example/vis-a", true);
        let b = seed_article(&conn, "rust beta", "https://lab.example/vis-b", false);
        let c = seed_article(&conn, "rust gamma", "https://lab.example/vis-c", false);
        let iid = insert_interest(&conn, "Rust async", "manual").unwrap();
        set_interest_embedding(&conn, iid, &[1.0, 0.0, 0.0]).unwrap();
        put_title_vec(&conn, a, &[1.0, 0.0, 0.0]);
        put_title_vec(&conn, b, &[1.0, 0.0, 0.0]);
        put_title_vec(&conn, c, &[1.0, 0.0, 0.0]);
        conn.execute(
            "UPDATE articles SET score = 10 WHERE id IN (?1, ?2)",
            params![a, c],
        )
        .unwrap();
        conn.execute("UPDATE articles SET score = -10 WHERE id = ?1", [b])
            .unwrap();

        assert!(recompute_interest_boost(&conn).unwrap() >= 1);
        assert!(article_boost(&conn, a) > 0);
        assert_eq!(article_boost(&conn, b), 0);
        assert!(article_boost(&conn, c) > 0);

        conn.execute("UPDATE articles SET score = -10 WHERE id = ?1", [a])
            .unwrap();
        conn.execute("UPDATE articles SET score = 10 WHERE id = ?1", [b])
            .unwrap();
        assert!(recompute_interest_boost(&conn).unwrap() >= 1);
        assert_eq!(article_boost(&conn, a), 0);
        assert!(article_boost(&conn, b) > 0);
        assert!(article_boost(&conn, c) > 0);
    }

    #[test]
    fn label_weight_enabled_delete_and_threshold_invalidate() {
        let (_dir, conn) = open_temp();
        let (focus, iid) = seed_matching(&conn, "rust async runtime", "https://lab.example/proto");
        assert!(recompute_interest_boost(&conn).unwrap() >= 1);
        assert!(article_reasons(&conn, focus).contains("Rust async"));

        conn.execute(
            "UPDATE interests SET label = 'renamed' WHERE id = ?1",
            [iid],
        )
        .unwrap();
        assert!(recompute_interest_boost(&conn).unwrap() >= 1);
        assert!(article_reasons(&conn, focus).contains("renamed"));
        assert_eq!(recompute_interest_boost(&conn).unwrap(), 0);

        conn.execute("UPDATE interests SET weight = 9 WHERE id = ?1", [iid])
            .unwrap();
        assert!(recompute_interest_boost(&conn).unwrap() >= 1);
        let heavier = article_boost(&conn, focus);
        assert!(heavier > 0);

        set_setting(&conn, HIDE_THRESHOLD_KEY, "0").unwrap();
        assert!(
            recompute_interest_boost(&conn).unwrap() >= 1,
            "threshold is part of the fingerprint even if the visible set is unchanged"
        );
        assert_eq!(article_boost(&conn, focus), heavier);

        conn.execute("UPDATE interests SET enabled = 0 WHERE id = ?1", [iid])
            .unwrap();
        recompute_interest_boost(&conn).unwrap();
        assert_eq!(article_boost(&conn, focus), 0);
        assert_eq!(hit_count(&conn), 0);

        conn.execute("UPDATE interests SET enabled = 1 WHERE id = ?1", [iid])
            .unwrap();
        assert!(recompute_interest_boost(&conn).unwrap() >= 1);
        conn.execute("DELETE FROM interests WHERE id = ?1", [iid])
            .unwrap();
        recompute_interest_boost(&conn).unwrap();
        assert_eq!(article_boost(&conn, focus), 0);
        assert_eq!(hit_count(&conn), 0);
    }

    #[test]
    fn malformed_title_vector_clears_old_boost() {
        let (_dir, conn) = open_temp();
        let (focus, _) =
            seed_matching(&conn, "rust async runtime", "https://lab.example/malformed");
        assert!(recompute_interest_boost(&conn).unwrap() >= 1);
        assert!(article_boost(&conn, focus) > 0);

        conn.execute(
            "UPDATE chunks SET embedding = ?1 WHERE id = ?2",
            params![vec![0u8, 1, 2], title_chunk_id(&conn, focus)],
        )
        .unwrap();
        let n = recompute_interest_boost(&conn).unwrap();
        assert_eq!(n, 0);
        assert_eq!(article_boost(&conn, focus), 0);
        assert_eq!(article_reasons(&conn, focus), "");
        assert_eq!(hit_count(&conn), 0);
    }

    #[test]
    fn nan_empty_and_dim_mismatch_clear_old_boost_without_body_fallback() {
        let (_dir, conn) = open_temp();
        let (focus, _) = seed_matching(&conn, "rust async runtime", "https://lab.example/badvec");
        set_chunk_embedding(
            &conn,
            body_chunk_id(&conn, focus),
            "test-embed",
            &[1.0, 0.0, 0.0],
        )
        .unwrap();
        assert!(recompute_interest_boost(&conn).unwrap() >= 1);
        assert!(article_boost(&conn, focus) > 0);
        let title = title_chunk_id(&conn, focus);

        set_chunk_embedding(&conn, title, "test-embed", &[f32::NAN, 0.0, 0.0]).unwrap();
        assert_eq!(recompute_interest_boost(&conn).unwrap(), 0);
        assert_eq!(article_boost(&conn, focus), 0);
        assert_eq!(hit_count(&conn), 0);

        set_chunk_embedding(&conn, title, "test-embed", &[1.0, 0.0, 0.0]).unwrap();
        assert!(recompute_interest_boost(&conn).unwrap() >= 1);
        conn.execute(
            "UPDATE chunks SET embedding = ?1 WHERE id = ?2",
            params![Vec::<u8>::new(), title],
        )
        .unwrap();
        assert_eq!(recompute_interest_boost(&conn).unwrap(), 0);
        assert_eq!(article_boost(&conn, focus), 0);

        set_chunk_embedding(&conn, title, "test-embed", &[1.0, 0.0, 0.0]).unwrap();
        assert!(recompute_interest_boost(&conn).unwrap() >= 1);
        set_chunk_embedding(&conn, title, "test-embed", &[1.0, 0.0]).unwrap();
        assert_eq!(recompute_interest_boost(&conn).unwrap(), 0);
        assert_eq!(article_boost(&conn, focus), 0);
        assert_eq!(article_reasons(&conn, focus), "");
        assert_eq!(hit_count(&conn), 0);
    }

    #[test]
    fn missing_title_embedding_falls_back_to_body() {
        let (_dir, conn) = open_temp();
        let (focus, _) = seed_matching(&conn, "rust async runtime", "https://lab.example/fallback");
        let title = title_chunk_id(&conn, focus);
        let body = body_chunk_id(&conn, focus);
        set_chunk_embedding(&conn, body, "test-embed", &[0.0, 1.0, 0.0]).unwrap();
        assert!(recompute_interest_boost(&conn).unwrap() >= 1);
        assert!(article_boost(&conn, focus) > 0);

        conn.execute("UPDATE chunks SET embedding = NULL WHERE id = ?1", [title])
            .unwrap();
        assert_eq!(recompute_interest_boost(&conn).unwrap(), 0);
        assert_eq!(
            article_boost(&conn, focus),
            0,
            "NULL title must use orthogonal body, not keep old boost"
        );

        set_chunk_embedding(&conn, title, "test-embed", &[0.0, 1.0, 0.0]).unwrap();
        set_chunk_embedding(&conn, body, "test-embed", &[1.0, 0.0, 0.0]).unwrap();
        assert_eq!(recompute_interest_boost(&conn).unwrap(), 0);
        assert_eq!(article_boost(&conn, focus), 0, "title still preferred");

        conn.execute("UPDATE chunks SET embedding = NULL WHERE id = ?1", [title])
            .unwrap();
        assert!(recompute_interest_boost(&conn).unwrap() >= 1);
        assert!(
            article_boost(&conn, focus) > 0,
            "NULL title must fall back to matching body"
        );
    }

    #[test]
    fn recompute_failure_rolls_back() {
        let (_dir, conn) = open_temp();
        let (focus, iid) =
            seed_matching(&conn, "rust async runtime", "https://lab.example/rollback");
        assert!(recompute_interest_boost(&conn).unwrap() >= 1);
        let boost_before = article_boost(&conn, focus);
        let reasons_before = article_reasons(&conn, focus);
        let sig_before = stored_sig(&conn);
        let hits_before = hit_count(&conn);
        assert!(boost_before > 0);

        conn.execute_batch(
            "CREATE TEMP TRIGGER fail_interest_boost
             BEFORE UPDATE ON articles
             BEGIN
               SELECT RAISE(ABORT, 'forced failure');
             END;",
        )
        .unwrap();
        conn.execute(
            "UPDATE interests SET label = 'changed' WHERE id = ?1",
            [iid],
        )
        .unwrap();
        assert!(recompute_interest_boost(&conn).is_err());
        conn.execute_batch("DROP TRIGGER fail_interest_boost")
            .unwrap();

        assert_eq!(article_boost(&conn, focus), boost_before);
        assert_eq!(article_reasons(&conn, focus), reasons_before);
        assert_eq!(stored_sig(&conn), sig_before);
        assert_eq!(hit_count(&conn), hits_before);
    }

    #[test]
    fn interest_changes_reject_outer_transactions_without_changes() {
        let (_dir, conn) = open_temp();
        let (article, interest) =
            seed_matching(&conn, "subject", "https://interest.example/transaction");
        recompute_interest_boost(&conn).unwrap();
        let before = interest_snapshot(&conn);
        let sig_before = stored_sig(&conn);
        for commit in [false, true] {
            let tx = db::write_transaction(&conn).unwrap();
            set_chunk_embedding(
                &tx,
                title_chunk_id(&tx, article),
                "test-embed",
                &[0.0, 1.0, 0.0],
            )
            .unwrap();
            db::set_setting(&tx, "outer_marker", "kept").unwrap();
            let changes = tx.total_changes();
            for result in [
                recompute_interest_boost(&tx).map(|_| ()),
                set_enabled(&tx, interest, false),
                delete_interest(&tx, interest),
            ] {
                assert!(
                    matches!(result, Err(Error::Sqlite(rusqlite::Error::SqliteFailure(error, Some(message))))
                    if error.extended_code == rusqlite::ffi::SQLITE_MISUSE && message.contains("requires autocommit"))
                );
            }
            assert_eq!(tx.total_changes(), changes);
            assert_eq!(interest_snapshot(&tx), before);
            assert_eq!(stored_sig(&tx), sig_before);
            assert!(!tx.is_autocommit());
            if commit {
                tx.commit().unwrap();
                assert_eq!(
                    db::get_setting(&conn, "outer_marker").unwrap().as_deref(),
                    Some("kept")
                );
                assert_eq!(recompute_interest_boost(&conn).unwrap(), 0);
                assert_eq!(article_boost(&conn, article), 0);
            } else {
                tx.rollback().unwrap();
                assert_eq!(db::get_setting(&conn, "outer_marker").unwrap(), None);
                assert_eq!(interest_snapshot(&conn), before);
                assert_eq!(recompute_interest_boost(&conn).unwrap(), 0);
            }
        }
    }

    #[test]
    fn recompute_waits_for_a_concurrent_writer_instead_of_failing() {
        let (dir, conn) = open_temp();
        let (focus, _) = seed_matching(&conn, "rust async runtime", "https://lab.example/busy");
        // 另一条连接先拿写锁、稍后才提交。读后再升级写锁会因快照过期直接报忙；
        // 先拿写锁则在 busy_timeout 里等到对方提交。
        let other = open_at(&dir.path().join("rrss.db")).unwrap();
        other
            .execute_batch("BEGIN IMMEDIATE; UPDATE feeds SET title = 'touched';")
            .unwrap();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            other.execute_batch("COMMIT").unwrap();
        });
        let n = recompute_interest_boost(&conn).unwrap();
        writer.join().unwrap();
        assert!(n >= 1);
        assert!(article_boost(&conn, focus) > 0);
    }

    #[test]
    fn failed_recompute_also_rolls_back_the_interest_change() {
        let (_dir, conn) = open_temp();
        let (focus, iid) = seed_matching(&conn, "rust async runtime", "https://lab.example/atomic");
        assert!(recompute_interest_boost(&conn).unwrap() >= 1);
        conn.execute_batch(
            "CREATE TEMP TRIGGER fail_interest_boost
             BEFORE UPDATE ON articles
             BEGIN
               SELECT RAISE(ABORT, 'forced failure');
             END;",
        )
        .unwrap();
        assert!(set_enabled(&conn, iid, false).is_err());
        conn.execute_batch("DROP TRIGGER fail_interest_boost")
            .unwrap();
        let enabled: bool = conn
            .query_row("SELECT enabled FROM interests WHERE id = ?1", [iid], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(enabled, "enabled change rolled back with the boost");
        assert!(article_boost(&conn, focus) > 0);
    }
}
