//! 跨源近重复聚类。入库不硬删。日志 target：`rrss_core::cluster`。

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

use crate::ai_store::truncate_chars;
use crate::error::Result;
use crate::feeds::now_ms;
use crate::rag::{self, cosine_distance};
use crate::simhash::{fingerprint_article, hamming};

pub const SIMHASH_HAMMING: u32 = 3;
// Similarity (not distance): never loosen the former distance < 0.15 rule.
pub const VECTOR_MIN_SIMILARITY: f32 = 0.85;
pub const CALIBRATION_QUANTILE: f64 = 0.99;
pub const CALIBRATION_MIN_ARTICLES: usize = 32;
pub const CALIBRATION_SAMPLE_ARTICLES: usize = 128;
pub const CALIBRATION_KEY_PREFIX: &str = "cluster_similarity_v2";
pub const ALGORITHM_VERSION_KEY: &str = "cluster_algorithm_version";
pub const ALGORITHM_VERSION: &str = "2";
pub const EVENT_WINDOW_DAYS: i64 = 3;
pub const TITLE_MIN_SHARED: usize = 2;
pub const TITLE_MIN_OVERLAP: f32 = 0.5;
pub const CLUSTER_WINDOW_DAYS: i64 = 14;
pub const BACKFILL_BATCH: i64 = 200;
const LABEL_CHARS: usize = 48;
const MS_PER_DAY: i64 = 86_400_000;

#[derive(Debug, Clone)]
struct ArticleFinger {
    id: i64,
    title: String,
    canonical_url: Option<String>,
    simhash: Option<i64>,
    published_at: Option<i64>,
    cluster_id: Option<i64>,
}

/// Rebuild only recent permanent articles; article IDs and persisted source/citation snapshots stay intact.
pub fn rebuild_if_needed(conn: &Connection) -> Result<bool> {
    // 常态版本已一致：只读核对，不为此拿写锁；需要重建时在写事务里再核对一次。
    if conn.is_autocommit()
        && crate::db::get_setting(conn, ALGORITHM_VERSION_KEY)?.as_deref()
            == Some(ALGORITHM_VERSION)
    {
        return Ok(false);
    }
    let window = now_ms() - CLUSTER_WINDOW_DAYS * MS_PER_DAY;
    // 指纹只取决于标题与正文：先在读快照里算好，锁内核对两者未变后直接使用。
    // 只留内容摘要，不在内存里多存一份正文。
    let precomputed: HashMap<i64, (u64, i64)> = if conn.is_autocommit() {
        crate::db::read_snapshot(conn, |snapshot| {
            let mut stmt = snapshot.prepare(WINDOW_ROWS_SQL)?;
            let mut rows = stmt.query([window])?;
            let mut out = HashMap::new();
            while let Some(row) = rows.next()? {
                let (id, title, html): (i64, String, Option<String>) =
                    (row.get(0)?, row.get(2)?, row.get(3)?);
                let fp = fingerprint_article(&title, html.as_deref());
                out.insert(id, (content_key(&title, html.as_deref()), fp));
            }
            Ok(out)
        })?
    } else {
        HashMap::new()
    };
    crate::db::atomic_write(conn, |conn| {
        if crate::db::get_setting(conn, ALGORITHM_VERSION_KEY)?.as_deref()
            == Some(ALGORITHM_VERSION)
        {
            return Ok(false);
        }
        let rows = window_rows(conn, window)?;
        conn.execute(
            "UPDATE articles SET simhash=NULL, cluster_id=NULL
            WHERE expires_at IS NULL AND COALESCE(published_at,created_at)>=?1",
            [window],
        )?;
        for cid in rows.iter().filter_map(|r| r.1).collect::<HashSet<_>>() {
            // Keep empty IDs reserved: an already-open focus expansion must never target a new event.
            if member_count(conn, cid)? == 0 {
                conn.execute(
                    "UPDATE clusters SET member_count=0, representative_id=NULL,
                    first_published_at=NULL,last_published_at=NULL,
                    summary_json=NULL,summary_members=NULL,summary_at=NULL WHERE id=?1",
                    [cid],
                )?;
            } else {
                refresh_cluster(conn, cid)?;
            }
        }
        for (id, _, title, html) in &rows {
            let fp = match precomputed.get(id) {
                Some((key, fp)) if *key == content_key(title, html.as_deref()) => *fp,
                _ => fingerprint_article(title, html.as_deref()),
            };
            conn.execute(
                "UPDATE articles SET simhash=?1 WHERE id=?2",
                params![fp, id],
            )?;
        }
        assign_articles_inner(conn, &rows.iter().map(|r| r.0).collect::<Vec<_>>())?;
        merge_close_clusters_inner(conn)?;
        crate::db::set_setting(conn, ALGORITHM_VERSION_KEY, ALGORITHM_VERSION)?;
        Ok(true)
    })
}

type WindowRow = (i64, Option<i64>, String, Option<String>);

/// 窗口内的长期文章：ID、所属簇、标题与正文。
const WINDOW_ROWS_SQL: &str = "SELECT id, cluster_id, title, content_html FROM articles
    WHERE expires_at IS NULL AND COALESCE(published_at,created_at)>=?1 ORDER BY id";

fn window_rows(conn: &Connection, window: i64) -> Result<Vec<WindowRow>> {
    let mut stmt = conn.prepare(WINDOW_ROWS_SQL)?;
    Ok(stmt
        .query_map([window], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

/// 同一进程内比较标题与正文是否变化。
fn content_key(title: &str, html: Option<&str>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    (title, html).hash(&mut hasher);
    hasher.finish()
}

/// Random articles give a bounded random-pair background distribution (at most 8128 pairs).
/// Cache per space for one day; changed population or insufficient data invalidates it.
fn calibrated_similarity(conn: &Connection, window: i64) -> Result<f32> {
    let calibration = calibrate(conn, window)?;
    calibration.persist(conn)?;
    Ok(calibration.threshold)
}

/// 只读当天的空间校准缓存；不检查样本规模、不重新校准或写缓存。
pub fn cached_similarity(conn: &Connection, space_id: i64) -> Result<f32> {
    let key = format!("{CALIBRATION_KEY_PREFIX}:{space_id}");
    if let Some(raw) = crate::db::get_setting(conn, &key)?
        && let Ok((day, _, threshold)) = serde_json::from_str::<(i64, usize, f32)>(&raw)
        && day == now_ms() / MS_PER_DAY
        && threshold.is_finite()
        && (VECTOR_MIN_SIMILARITY..=1.0).contains(&threshold)
    {
        return Ok(threshold);
    }
    Ok(VECTOR_MIN_SIMILARITY)
}

/// 校准结果与对缓存的改动；只读快照里先算，需要时再单独写回缓存。
struct Calibration {
    threshold: f32,
    // Some(Some(值)) 写入缓存，Some(None) 删除缓存。
    cache: Option<(String, Option<String>)>,
}

impl Calibration {
    fn persist(&self, conn: &Connection) -> Result<()> {
        match &self.cache {
            Some((key, Some(value))) => crate::db::set_setting(conn, key, value),
            Some((key, None)) => {
                conn.execute("DELETE FROM settings WHERE key=?1", [key])?;
                Ok(())
            }
            None => Ok(()),
        }
    }
}

/// 样本不足时清掉旧缓存；没有缓存就不必写。
fn stale_cache(conn: &Connection, key: String) -> Result<Option<(String, Option<String>)>> {
    Ok(crate::db::get_setting(conn, &key)?.map(|_| (key, None)))
}

fn calibrate(conn: &Connection, window: i64) -> Result<Calibration> {
    let fixed = |threshold| Calibration {
        threshold,
        cache: None,
    };
    let Some(space) = crate::vector_space::active(conn)? else {
        return Ok(fixed(VECTOR_MIN_SIMILARITY));
    };
    let mut stmt = conn.prepare("SELECT a.id, COUNT(*) OVER() FROM articles a WHERE a.expires_at IS NULL
        AND COALESCE(a.published_at,a.created_at)>=?1 AND EXISTS(SELECT 1 FROM chunks c
            WHERE c.article_id=a.id AND c.kind='title_summary' AND c.space_id=?2 AND c.embedding IS NOT NULL)
        ORDER BY random() LIMIT ?3")?;
    let ids = stmt
        .query_map(
            params![window, space.id, CALIBRATION_SAMPLE_ARTICLES],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, usize>(1)?)),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let population = ids.first().map_or(0, |r| r.1);
    let key = format!("{CALIBRATION_KEY_PREFIX}:{}", space.id);
    let day = now_ms() / MS_PER_DAY;
    if ids.len() >= CALIBRATION_MIN_ARTICLES
        && let Some(raw) = crate::db::get_setting(conn, &key)?
        && let Ok((cached_day, count, threshold)) = serde_json::from_str::<(i64, usize, f32)>(&raw)
        && cached_day == day
        && count == population
        && threshold.is_finite()
        && (VECTOR_MIN_SIMILARITY..=1.0).contains(&threshold)
    {
        return Ok(fixed(threshold));
    }
    let vectors = ids
        .iter()
        .take(CALIBRATION_SAMPLE_ARTICLES)
        .map(|(id, _)| rag::title_embedding(conn, *id))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .filter(|v| !v.is_empty() && v.iter().all(|x| x.is_finite()) && v.iter().any(|x| *x != 0.0))
        .collect::<Vec<_>>();
    if vectors.len() < CALIBRATION_MIN_ARTICLES {
        return Ok(Calibration {
            threshold: VECTOR_MIN_SIMILARITY,
            cache: stale_cache(conn, key)?,
        });
    }
    let mut similarities = Vec::new();
    for (i, a) in vectors.iter().enumerate() {
        for b in &vectors[i + 1..] {
            let similarity = 1.0 - cosine_distance(a, b);
            if similarity.is_finite() {
                similarities.push(similarity);
            }
        }
    }
    if similarities.len() < CALIBRATION_MIN_ARTICLES * (CALIBRATION_MIN_ARTICLES - 1) / 2 {
        return Ok(Calibration {
            threshold: VECTOR_MIN_SIMILARITY,
            cache: stale_cache(conn, key)?,
        });
    }
    similarities.sort_unstable_by(f32::total_cmp);
    let index = ((similarities.len() - 1) as f64 * CALIBRATION_QUANTILE).ceil() as usize;
    let threshold = similarities[index].clamp(VECTOR_MIN_SIMILARITY, 1.0);
    tracing::debug!(
        space_id = space.id,
        threshold,
        pairs = similarities.len(),
        "calibrated cluster similarity"
    );
    Ok(Calibration {
        threshold,
        cache: Some((key, Some(format!("[{day},{population},{threshold}]")))),
    })
}

fn titles_overlap(a: &str, b: &str) -> bool {
    let a = crate::simhash::content_tokens(a);
    let b = crate::simhash::content_tokens(b);
    let shared = a.intersection(&b).count();
    shared >= TITLE_MIN_SHARED && shared as f32 >= a.len().min(b.len()) as f32 * TITLE_MIN_OVERLAP
}

pub fn backfill_simhash(conn: &Connection, limit: i64) -> Result<u32> {
    let mut stmt = conn.prepare(
        "SELECT id, title, content_html FROM articles
         WHERE expires_at IS NULL AND simhash IS NULL
         ORDER BY id DESC
         LIMIT ?1",
    )?;
    let rows: Vec<(i64, String, Option<String>)> = stmt
        .query_map([limit], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut n = 0u32;
    for (id, title, html) in rows {
        let fp = fingerprint_article(&title, html.as_deref());
        conn.execute(
            "UPDATE articles SET simhash = ?1 WHERE id = ?2",
            params![fp, id],
        )?;
        n += 1;
    }
    if n > 0 {
        tracing::info!(count = n, "backfilled simhash");
    }
    Ok(n)
}

pub fn assign_unclustered(conn: &Connection, limit: i64) -> Result<u32> {
    let ids: Vec<i64> = {
        let mut stmt = conn.prepare(
            "SELECT id FROM articles WHERE expires_at IS NULL AND cluster_id IS NULL ORDER BY id DESC LIMIT ?1",
        )?;
        stmt.query_map([limit], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    assign_articles(conn, &ids)?;
    let n = ids.len() as u32;
    if n > 0 {
        tracing::info!(count = n, "assigned clusters");
    }
    Ok(n)
}

/// 同一入库批次复用候选指纹。保持逐篇算法的 URL 优先、时间下界和距离阈值。
/// 缓存只含不可变的指纹/日期，cluster_id 每次命中后重新读取，避免批次内陈旧关联。
pub fn assign_articles(conn: &Connection, article_ids: &[i64]) -> Result<()> {
    if article_ids.is_empty() {
        return Ok(());
    }
    crate::db::atomic_write(conn, |conn| assign_articles_inner(conn, article_ids))
}

fn assign_articles_inner(conn: &Connection, article_ids: &[i64]) -> Result<()> {
    let now = now_ms();
    let mut window_start = i64::MAX;
    for &id in article_ids {
        if crate::articles::is_temporary(conn, id)? {
            continue;
        }
        let article = load_finger(conn, id)?;
        window_start = window_start
            .min(article.published_at.unwrap_or(now) - CLUSTER_WINDOW_DAYS * MS_PER_DAY);
    }
    let candidates = {
        let mut stmt = conn.prepare(
            "SELECT id, simhash, COALESCE(published_at, created_at) FROM articles
             WHERE expires_at IS NULL AND simhash IS NOT NULL AND simhash != 0
               AND COALESCE(published_at, created_at) >= ?1
             ORDER BY id",
        )?;
        stmt.query_map([window_start], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for &id in article_ids {
        if crate::articles::is_temporary(conn, id)? {
            continue;
        }
        let article = load_finger(conn, id)?;
        if let Some(cid) = article.cluster_id {
            refresh_cluster(conn, cid)?;
            continue;
        }
        if let Some(url) = article.canonical_url.as_deref().filter(|u| !u.is_empty())
            && let Some(cid) = cluster_by_url(conn, id, url)?
        {
            join_cluster(conn, id, cid, "url", 0.0)?;
            continue;
        }
        let start = article.published_at.unwrap_or(now) - CLUSTER_WINDOW_DAYS * MS_PER_DAY;
        let mut matches: Vec<_> = article
            .simhash
            .filter(|s| *s != 0)
            .map_or_else(Vec::new, |fp| {
                candidates
                    .iter()
                    .filter(|(other_id, _, time)| *other_id != id && *time >= start)
                    .map(|(other_id, other_fp, _)| (hamming(fp, *other_fp), *other_id))
                    .filter(|(distance, _)| *distance <= SIMHASH_HAMMING)
                    .collect()
            });
        matches.sort_unstable();
        // 本篇尚未归簇，与候选的来源核对只取决于候选所在的簇；扫描期间簇成员不变，
        // 同簇只核对一次。模板化正文让大量候选指纹相近时，避免逐个候选跑整簇查询。
        let mut verdicts = HashMap::new();
        let mut best = None;
        for (distance, other_id) in matches {
            let cluster: Option<i64> = conn
                .prepare_cached("SELECT cluster_id FROM articles WHERE id=?1")?
                .query_row([other_id], |r| r.get(0))?;
            let compatible = match cluster.and_then(|cid| verdicts.get(&cid).copied()) {
                Some(known) => known,
                None => {
                    let ok = compatible_sources(conn, id, other_id)?;
                    if let Some(cid) = cluster {
                        verdicts.insert(cid, ok);
                    }
                    ok
                }
            };
            if compatible {
                best = Some((distance, other_id));
                break;
            }
        }
        if let Some((distance, other_id)) = best {
            let other = load_finger(conn, other_id)?;
            let cid = match other.cluster_id {
                Some(cid) => cid,
                None => create_cluster(conn, &other)?,
            };
            join_cluster(conn, id, cid, "simhash", distance as f32)?;
        } else {
            create_cluster(conn, &article)?;
        }
    }
    Ok(())
}

pub fn assign_article(conn: &Connection, article_id: i64) -> Result<i64> {
    if crate::articles::is_temporary(conn, article_id)? {
        return Ok(0);
    }
    assign_articles(conn, &[article_id])?;
    Ok(load_finger(conn, article_id)?.cluster_id.unwrap_or(0))
}

/// 窗口内代表标题向量相近的簇两两合并。合并在一个写事务里（已在事务内则用 SAVEPOINT）。
///
/// 先在读快照里找有没有满足条件的簇对：多数轮次一对都没有，不必拿写锁。
/// 找到后在写锁里按原算法完整重做，结果与一直持锁时相同；快照之后才出现的
/// 簇对留给下一轮。
pub fn merge_close_clusters(conn: &Connection) -> Result<u32> {
    if conn.is_autocommit() {
        let (found, calibration) = crate::db::read_snapshot(conn, |snapshot| {
            let window = now_ms() - CLUSTER_WINDOW_DAYS * MS_PER_DAY;
            let calibration = calibrate(snapshot, window)?;
            let reps = window_representatives(snapshot, window)?;
            Ok((
                any_close_pair(snapshot, &reps, calibration.threshold)?,
                calibration,
            ))
        })?;
        if !found {
            if calibration.cache.is_some() {
                crate::db::atomic_write(conn, |tx| calibration.persist(tx))?;
            }
            return Ok(0);
        }
    }
    crate::db::atomic_write(conn, merge_close_clusters_inner)
}

type Representative = (i64, Vec<f32>, ArticleFinger);

/// 窗口内完全落在窗口里的簇及其代表的标题向量。
fn window_representatives(conn: &Connection, window: i64) -> Result<Vec<Representative>> {
    // 窗口内没有任何当前空间的向量时，逐簇查代表向量必然全空。
    let Some(space) = crate::vector_space::active(conn)? else {
        return Ok(Vec::new());
    };
    let any_vector: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM chunks c JOIN articles a ON a.id=c.article_id
         WHERE c.space_id=?2 AND c.embedding IS NOT NULL
           AND COALESCE(a.published_at,a.created_at)>=?1)",
        params![window, space.id],
        |r| r.get(0),
    )?;
    if !any_vector {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT DISTINCT a.cluster_id
         FROM articles a
         WHERE a.expires_at IS NULL AND a.cluster_id IS NOT NULL
           AND COALESCE(a.published_at, a.created_at) >= ?1
           AND NOT EXISTS(SELECT 1 FROM articles old WHERE old.cluster_id=a.cluster_id
               AND COALESCE(old.published_at,old.created_at) < ?1)
         ORDER BY a.cluster_id",
    )?;
    let ids: Vec<i64> = stmt
        .query_map([window], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut reps = Vec::new();
    for cid in ids {
        let Some(rep_id) = representative_id(conn, cid)? else {
            continue;
        };
        let Some(vec) = rag::title_embedding(conn, rep_id)? else {
            continue;
        };
        reps.push((cid, vec, load_finger(conn, rep_id)?));
    }
    Ok(reps)
}

/// 两个簇满足合并条件时返回代表标题向量的距离。
fn close_pair(
    conn: &Connection,
    a: &Representative,
    b: &Representative,
    threshold: f32,
) -> Result<Option<f32>> {
    let distance = cosine_distance(&a.1, &b.1);
    if !distance.is_finite()
        || 1.0 - distance <= threshold
        || !titles_overlap(&a.2.title, &b.2.title)
    {
        return Ok(None);
    }
    let span: i64 = conn.query_row(
        "SELECT MAX(COALESCE(published_at,created_at))-
        MIN(COALESCE(published_at,created_at)) FROM articles WHERE cluster_id IN (?1,?2)",
        params![a.0, b.0],
        |r| r.get(0),
    )?;
    Ok(
        (span <= EVENT_WINDOW_DAYS * MS_PER_DAY && compatible_sources(conn, a.2.id, b.2.id)?)
            .then_some(distance),
    )
}

fn any_close_pair(conn: &Connection, reps: &[Representative], threshold: f32) -> Result<bool> {
    for (i, a) in reps.iter().enumerate() {
        for b in &reps[i + 1..] {
            if close_pair(conn, a, b, threshold)?.is_some() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn merge_close_clusters_inner(conn: &Connection) -> Result<u32> {
    let window = now_ms() - CLUSTER_WINDOW_DAYS * MS_PER_DAY;
    let threshold = calibrated_similarity(conn, window)?;
    let mut reps = window_representatives(conn, window)?;
    let mut merged = 0u32;
    let mut skip = std::collections::HashSet::new();
    for i in 0..reps.len() {
        if skip.contains(&reps[i].0) {
            continue;
        }
        for j in (i + 1)..reps.len() {
            if skip.contains(&reps[j].0) {
                continue;
            }
            if let Some(distance) = close_pair(conn, &reps[i], &reps[j], threshold)? {
                let survivor = merge_clusters(conn, reps[i].0, reps[j].0, distance, threshold)?;
                merged += 1;
                let k = if survivor == reps[i].0 { i } else { j };
                let rep_id =
                    representative_id(conn, survivor)?.expect("merged cluster has members");
                reps[k].2 = load_finger(conn, rep_id)?;
                reps[k].1 = rag::title_embedding(conn, rep_id)?.unwrap_or_default();
                if survivor == reps[i].0 {
                    skip.insert(reps[j].0);
                } else {
                    // i 已并入 j，簇已删除；之后由 j 继续和后面的簇比较。
                    skip.insert(reps[i].0);
                    break;
                }
            }
        }
    }
    if merged > 0 {
        tracing::info!(count = merged, "merged clusters by title vector");
    }
    Ok(merged)
}

/// Compare all members, not only the representative: A -> B -> A must not bypass the rule.
fn compatible_sources(conn: &Connection, a: i64, b: i64) -> Result<bool> {
    Ok(!conn.query_row(
        "WITH members AS (SELECT id,feed_id,canonical_url FROM articles
            WHERE id IN (?1,?2) OR cluster_id IN
                (SELECT cluster_id FROM articles WHERE id IN (?1,?2)))
         SELECT EXISTS(SELECT 1 FROM members a JOIN members b ON a.feed_id=b.feed_id AND a.id<b.id
           WHERE NULLIF(a.canonical_url,'') IS NULL OR NULLIF(b.canonical_url,'') IS NULL
                OR a.canonical_url != b.canonical_url)",
        params![a, b],
        |r| r.get::<_, bool>(0),
    )?)
}

fn cluster_by_url(conn: &Connection, article_id: i64, url: &str) -> Result<Option<i64>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM articles WHERE expires_at IS NULL AND canonical_url=?1 AND id!=?2
         ORDER BY cluster_id IS NULL, id",
    )?;
    let ids = stmt.query_map(params![url, article_id], |r| r.get::<_, i64>(0))?;
    for id in ids {
        let id = id?;
        if compatible_sources(conn, article_id, id)? {
            let other = load_finger(conn, id)?;
            return Ok(Some(match other.cluster_id {
                Some(cid) => cid,
                None => create_cluster(conn, &other)?,
            }));
        }
    }
    Ok(None)
}

fn create_cluster(conn: &Connection, article: &ArticleFinger) -> Result<i64> {
    let label = truncate_chars(&article.title, LABEL_CHARS);
    conn.execute(
        "INSERT INTO clusters (
            label, representative_id, member_count,
            first_published_at, last_published_at, created_at
         ) VALUES (?1, ?2, 1, ?3, ?3, ?4)",
        params![label, article.id, article.published_at, now_ms()],
    )?;
    let cid = conn.last_insert_rowid();
    conn.execute(
        "UPDATE articles SET cluster_id = ?1 WHERE id = ?2",
        params![cid, article.id],
    )?;
    tracing::debug!(article_id = article.id, cluster_id = cid, step = "new", distance = ?Option::<f32>::None, "assigned cluster");
    Ok(cid)
}

fn join_cluster(
    conn: &Connection,
    article_id: i64,
    cluster_id: i64,
    step: &str,
    distance: f32,
) -> Result<()> {
    conn.execute(
        "UPDATE articles SET cluster_id = ?1 WHERE id = ?2",
        params![cluster_id, article_id],
    )?;
    tracing::debug!(article_id, cluster_id, step, distance, "assigned cluster");
    refresh_cluster(conn, cluster_id)
}

fn merge_clusters(conn: &Connection, a: i64, b: i64, distance: f32, threshold: f32) -> Result<i64> {
    if a == b {
        return Ok(a);
    }
    let count_a = member_count(conn, a)?;
    let count_b = member_count(conn, b)?;
    let (survivor, loser) = if count_a >= count_b { (a, b) } else { (b, a) };
    for article_id in cluster_members(conn, loser)? {
        tracing::debug!(
            article_id,
            cluster_id = survivor,
            step = "vector",
            distance,
            threshold,
            "assigned cluster"
        );
    }
    conn.execute(
        "UPDATE articles SET cluster_id = ?1 WHERE cluster_id = ?2",
        params![survivor, loser],
    )?;
    conn.execute("DELETE FROM clusters WHERE id = ?1", [loser])?;
    refresh_cluster(conn, survivor)?;
    Ok(survivor)
}

fn refresh_cluster(conn: &Connection, cluster_id: i64) -> Result<()> {
    let (n, first, last): (i64, Option<i64>, Option<i64>) = conn.query_row(
        "SELECT COUNT(*), MIN(published_at), MAX(published_at)
         FROM articles WHERE cluster_id = ?1",
        [cluster_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    if n == 0 {
        conn.execute("DELETE FROM clusters WHERE id = ?1", [cluster_id])?;
        return Ok(());
    }
    let (rep_id, title): (i64, String) = conn.query_row(
        "SELECT id, title FROM articles
         WHERE cluster_id = ?1
         ORDER BY score DESC, COALESCE(published_at, 0) DESC, id DESC
         LIMIT 1",
        [cluster_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let label = truncate_chars(&title, LABEL_CHARS);
    conn.execute(
        "UPDATE clusters SET label = ?1, representative_id = ?2, member_count = ?3,
                first_published_at = ?4, last_published_at = ?5
         WHERE id = ?6",
        params![label, rep_id, n, first, last, cluster_id],
    )?;
    Ok(())
}

fn member_count(conn: &Connection, cluster_id: i64) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM articles WHERE cluster_id = ?1",
        [cluster_id],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn representative_id(conn: &Connection, cluster_id: i64) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT representative_id FROM clusters WHERE id = ?1",
        [cluster_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

fn load_finger(conn: &Connection, article_id: i64) -> Result<ArticleFinger> {
    conn.query_row(
        "SELECT id, title, canonical_url, simhash, COALESCE(published_at, created_at), score, cluster_id
         FROM articles WHERE id = ?1",
        [article_id],
        |row| {
            Ok(ArticleFinger {
                id: row.get(0)?,
                title: row.get(1)?,
                canonical_url: row.get(2)?,
                simhash: row.get(3)?,
                published_at: row.get(4)?,
                cluster_id: row.get(6)?,
            })
        },
    )
    .map_err(Into::into)
}

#[derive(Debug, Clone)]
pub struct ClusterMeta {
    pub id: i64,
    pub label: String,
    pub representative_id: Option<i64>,
    pub member_count: i64,
    pub first_published_at: Option<i64>,
    pub last_published_at: Option<i64>,
}

fn map_cluster_meta(row: &rusqlite::Row<'_>) -> rusqlite::Result<ClusterMeta> {
    Ok(ClusterMeta {
        id: row.get(0)?,
        label: row.get(1)?,
        representative_id: row.get(2)?,
        member_count: row.get(3)?,
        first_published_at: row.get(4)?,
        last_published_at: row.get(5)?,
    })
}

pub fn get_cluster(conn: &Connection, id: i64) -> Result<Option<ClusterMeta>> {
    conn.query_row(
        "SELECT id, label, representative_id, member_count, first_published_at, last_published_at
         FROM clusters WHERE id = ?1",
        [id],
        map_cluster_meta,
    )
    .optional()
    .map_err(Into::into)
}

pub fn get_clusters(conn: &Connection, ids: &[i64]) -> Result<HashMap<i64, ClusterMeta>> {
    let mut out = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let marks: String = (1..=ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT id, label, representative_id, member_count, first_published_at, last_published_at
         FROM clusters WHERE id IN ({marks})"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(ids.iter()), map_cluster_meta)?;
    for row in rows {
        let meta = row?;
        out.insert(meta.id, meta);
    }
    Ok(out)
}

pub fn cluster_members(conn: &Connection, cluster_id: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT id FROM articles
         WHERE cluster_id = ?1
         ORDER BY COALESCE(published_at, 0) ASC, id ASC",
    )?;
    let rows = stmt.query_map([cluster_id], |row| row.get(0))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{PooledConnection, open_at};
    use crate::feeds::{SubscribeRequest, subscribe};
    use crate::parse::ParsedEntry;
    use tempfile::tempdir;

    fn open_temp() -> (tempfile::TempDir, PooledConnection) {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        (dir, conn)
    }

    fn feed(conn: &Connection, url: &str) -> i64 {
        subscribe(
            conn,
            SubscribeRequest {
                url: url.into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap()
    }

    fn entry(title: &str, url: &str, html: &str) -> ParsedEntry {
        ParsedEntry {
            guid: url.into(),
            canonical_url: Some(url.into()),
            title: title.into(),
            author: None,
            published_at: Some(now_ms()),
            content_html: Some(html.into()),
            hash: crate::parse::entry_hash(title, Some(url), Some(html)),
            tags: vec![],
        }
    }

    #[test]
    fn batch_assignment_matches_serial_with_live_cluster_membership() {
        fn seeded() -> (tempfile::TempDir, PooledConnection) {
            let (dir, conn) = open_temp();
            let fid = feed(&conn, "https://compare.test/feed");
            let now = now_ms();
            for id in 1..=36i64 {
                // 精确重复、等距候选、零/空指纹、过期/未来/无发布日期。
                let fp = match id % 6 {
                    0 => None,
                    1 => Some(0),
                    2 => Some(0x1234i64),
                    3 => Some(0x1235),
                    4 => Some(-1),
                    _ => Some(0x76543210),
                };
                let published = match id % 5 {
                    0 => None,
                    1 => Some(now - 40 * MS_PER_DAY),
                    2 => Some(now + MS_PER_DAY),
                    _ => Some(now),
                };
                let url = if id % 7 == 0 {
                    Some("https://same.test/story".to_string())
                } else if id % 4 == 0 {
                    None
                } else {
                    Some(format!("https://test/a{id}"))
                };
                conn.execute("INSERT INTO articles (id,feed_id,title,hash,canonical_url,simhash,published_at,created_at,score)
                    VALUES (?1,?2,?3,?3,?4,?5,?6,?7,?8)",
                    params![id, fid, format!("article{id}"), url, fp, published, now, id % 3]).unwrap();
            }
            (dir, conn)
        }
        fn snapshot(conn: &Connection) -> (Vec<(i64, Option<i64>)>, Vec<String>) {
            let articles = conn
                .prepare("SELECT id,cluster_id FROM articles ORDER BY id")
                .unwrap()
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            let clusters = conn.prepare("SELECT json_array(id,label,representative_id,member_count) FROM clusters ORDER BY id").unwrap()
                .query_map([], |r| r.get(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
            (articles, clusters)
        }
        let (_a, serial) = seeded();
        let (_b, batch) = seeded();
        // 同时覆盖已有簇、批次中尚未处理但可作为候选的文章、重复处理 ID。
        assign_article(&serial, 36).unwrap();
        assign_article(&batch, 36).unwrap();
        let mut ids: Vec<_> = (1..=36).collect();
        ids.extend([1, 3, 36]);
        for &id in &ids {
            assign_article(&serial, id).unwrap();
        }
        assign_articles(&batch, &ids).unwrap();
        assert_eq!(snapshot(&serial), snapshot(&batch));
    }

    #[test]
    fn failed_batch_assignment_rolls_back_all_cluster_writes() {
        let (_dir, conn) = open_temp();
        let fid = feed(&conn, "https://rollback.test/feed");
        for id in 1..=2 {
            conn.execute(
                "INSERT INTO articles(id,feed_id,title,hash,created_at) VALUES (?1,?2,'x',?1,1)",
                params![id, fid],
            )
            .unwrap();
        }
        conn.execute_batch(
            "CREATE TEMP TRIGGER fail_cluster BEFORE UPDATE OF cluster_id ON articles
            WHEN NEW.id=2 BEGIN SELECT RAISE(ABORT,'injected'); END;",
        )
        .unwrap();
        assert!(assign_articles(&conn, &[1, 2]).is_err());
        let clusters: i64 = conn
            .query_row("SELECT count(*) FROM clusters", [], |r| r.get(0))
            .unwrap();
        let assigned: i64 = conn
            .query_row(
                "SELECT count(*) FROM articles WHERE cluster_id IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!((clusters, assigned), (0, 0));
        conn.execute_batch("DROP TRIGGER fail_cluster").unwrap();
        assign_articles(&conn, &[1, 2]).unwrap();
    }

    #[test]
    fn same_canonical_url_clusters_across_feeds() {
        let (_dir, conn) = open_temp();
        let a = feed(&conn, "https://a.example.com/rss.xml");
        let b = feed(&conn, "https://b.example.com/rss.xml");
        let html = "<p>市民可凭证件借阅。</p>";
        let url = "https://news.example.com/library";
        crate::ingest::upsert_entries(&conn, a, &[entry("市政厅新图书馆开放", url, html)], 1)
            .unwrap();
        crate::ingest::upsert_entries(&conn, b, &[entry("市政厅开放新图书馆", url, html)], 1)
            .unwrap();
        let ids: Vec<(i64, Option<i64>)> = {
            let mut stmt = conn
                .prepare("SELECT id, cluster_id FROM articles ORDER BY id")
                .unwrap();
            stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0].1, ids[1].1);
        assert!(ids[0].1.is_some());
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM articles", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 2);
    }

    #[test]
    fn same_feed_cannot_join_directly_or_through_another_feed() {
        let (_dir, conn) = open_temp();
        seed_similar_clusters(&conn, &[1, 1, 1]);
        conn.execute(
            "UPDATE articles SET feed_id=(SELECT feed_id FROM articles WHERE id=1001),
            title='Git infrastructure' WHERE id=1003",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE articles SET title='Java Copilot developers' WHERE id=1001",
            [],
        )
        .unwrap();
        conn.execute("UPDATE articles SET cluster_id=NULL, simhash=123", [])
            .unwrap();
        conn.execute("DELETE FROM clusters", []).unwrap();
        assign_articles(&conn, &[1001, 1002, 1003]).unwrap();
        let a = load_finger(&conn, 1001).unwrap().cluster_id;
        assert_eq!(a, load_finger(&conn, 1002).unwrap().cluster_id);
        assert_ne!(a, load_finger(&conn, 1003).unwrap().cluster_id);
        // Force the same title/vector evidence too: source conflicts must still block vector chaining.
        conn.execute("UPDATE articles SET title='Copilot Java development'", [])
            .unwrap();
        assert_eq!(merge_close_clusters(&conn).unwrap(), 0);
        // A URL hit through the other source must not smuggle in a conflicting same-source URL.
        conn.execute(
            "UPDATE articles SET canonical_url='https://news.test/shared' WHERE id IN (1002,1003)",
            [],
        )
        .unwrap();
        conn.execute("UPDATE articles SET cluster_id=NULL WHERE id=1003", [])
            .unwrap();
        assign_article(&conn, 1003).unwrap();
        assert_ne!(a, load_finger(&conn, 1003).unwrap().cluster_id);
    }

    #[test]
    fn same_feed_same_url_still_clusters() {
        let (_dir, conn) = open_temp();
        seed_similar_clusters(&conn, &[1, 1]);
        conn.execute("UPDATE articles SET cluster_id=NULL, simhash=0,
            feed_id=(SELECT feed_id FROM articles WHERE id=1001), canonical_url='https://news.test/same'", []).unwrap();
        assign_articles(&conn, &[1001, 1002]).unwrap();
        assert_eq!(
            load_finger(&conn, 1001).unwrap().cluster_id,
            load_finger(&conn, 1002).unwrap().cluster_id
        );
    }

    #[test]
    fn short_excerpts_with_shared_source_footer_do_not_cluster() {
        let (_dir, conn) = open_temp();
        for (n, title) in [(1, "Storage"), (2, "Copilot")] {
            let fid = feed(&conn, &format!("https://source{n}.test/rss"));
            let footer = "The post Weekly developer update appeared first on Technology News for Software Engineers Worldwide.";
            crate::ingest::upsert_entries(
                &conn,
                fid,
                &[entry(
                    title,
                    &format!("https://source{n}.test/article"),
                    &format!("<p>{title} released.</p><p>{footer}</p>"),
                )],
                1,
            )
            .unwrap();
        }
        let a = load_finger(&conn, 1).unwrap();
        let b = load_finger(&conn, 2).unwrap();
        assert_eq!((a.simhash, b.simhash), (Some(0), Some(0)));
        assert_ne!(a.cluster_id, b.cluster_id);
    }

    #[test]
    fn near_title_simhash_clusters() {
        let (_dir, conn) = open_temp();
        let a = feed(&conn, "https://a.example.com/rss.xml");
        let b = feed(&conn, "https://b.example.com/rss.xml");
        let html = "<p>市民可凭证件到新馆借阅图书，开放时间为每日上午九时。</p>";
        crate::ingest::upsert_entries(
            &conn,
            a,
            &[entry(
                "市政厅新图书馆今日开放",
                "https://a.example.com/lib",
                html,
            )],
            1,
        )
        .unwrap();
        crate::ingest::upsert_entries(
            &conn,
            b,
            &[entry(
                "市政厅新图书馆开放",
                "https://b.example.com/lib",
                html,
            )],
            1,
        )
        .unwrap();
        let cids: Vec<Option<i64>> = {
            let mut stmt = conn
                .prepare("SELECT cluster_id FROM articles ORDER BY id")
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        assert_eq!(cids[0], cids[1], "near-duplicate titles should cluster");
    }

    /// 三个窗口内的簇，代表标题向量相同；`counts` 是各簇成员数。返回簇 id。
    fn seed_similar_clusters(conn: &Connection, counts: &[i64]) -> Vec<i64> {
        let now = now_ms();
        crate::vector_space::seed_active(conn, "m", 3);
        let blob = rag::f32s_to_le(&[1.0, 0.0, 0.0]);
        let mut ids = Vec::new();
        let mut article = 0;
        for (c, &count) in counts.iter().enumerate() {
            let cid = c as i64 + 100;
            conn.execute(
                "INSERT INTO clusters (id, label, member_count, created_at) VALUES (?1, 'x', ?2, ?3)",
                rusqlite::params![cid, count, now],
            )
            .unwrap();
            for _ in 0..count {
                article += 1;
                let feed_id = feed(conn, &format!("https://merge{article}.example/rss.xml"));
                conn.execute(
                    "INSERT INTO articles (id, feed_id, title, hash, created_at, published_at, cluster_id)
                     VALUES (?1, ?2, 'library opening event', ?3, ?4, ?4, ?5)",
                    rusqlite::params![article + 1000, feed_id, format!("h{article}"), now, cid],
                )
                .unwrap();
                conn.execute(
                    "INSERT INTO chunks (article_id, ordinal, kind, text, embedding, model, space_id)
                     VALUES (?1, 0, 'title_summary', 'library opening event', ?2, 'm', (SELECT id FROM vector_spaces WHERE state = 'active'))",
                    rusqlite::params![article + 1000, blob],
                )
                .unwrap();
            }
            ids.push(cid);
        }
        conn.execute(
            "UPDATE clusters SET representative_id =
               (SELECT MIN(id) FROM articles WHERE cluster_id = clusters.id)",
            [],
        )
        .unwrap();
        ids
    }

    #[test]
    fn calibrated_threshold_tracks_space_distribution_and_rejects_high_baseline() {
        let (_dir, conn) = open_temp();
        const POPULATION: usize = CALIBRATION_SAMPLE_ARTICLES + 4;
        seed_similar_clusters(&conn, &vec![1; POPULATION]);
        let window = now_ms() - CLUSTER_WINDOW_DAYS * MS_PER_DAY;
        // Orthogonal background: the old 0.85 minimum is sufficient.
        conn.execute("UPDATE vector_spaces SET dims=?1", [POPULATION as i64 + 1])
            .unwrap();
        for i in 0..POPULATION {
            let mut v = vec![0.0; POPULATION + 1];
            v[i + 1] = 1.0;
            conn.execute(
                "UPDATE chunks SET embedding=?1 WHERE article_id=?2",
                params![rag::f32s_to_le(&v), 1001 + i as i64],
            )
            .unwrap();
        }
        assert_eq!(
            calibrated_similarity(&conn, window).unwrap(),
            VECTOR_MIN_SIMILARITY
        );
        let old_space = crate::vector_space::active(&conn).unwrap().unwrap().id;
        conn.execute("UPDATE vector_spaces SET state='retired'", [])
            .unwrap();
        crate::vector_space::seed_active(&conn, "high-baseline", (POPULATION + 1) as i64);
        let new_space = crate::vector_space::active(&conn).unwrap().unwrap().id;
        assert_ne!(old_space, new_space);
        for i in 0..POPULATION {
            let mut v = vec![0.0; POPULATION + 1];
            v[0] = 0.99_f32.sqrt();
            v[i + 1] = 0.01_f32.sqrt();
            // Two articles have lower similarity than the unrelated background, but still exceed 0.85.
            if i < 2 {
                v[0] = 0.95_f32.sqrt();
                v[i + 1] = 0.05_f32.sqrt();
            }
            conn.execute(
                "UPDATE chunks SET embedding=?1,space_id=?2 WHERE article_id=?3",
                params![rag::f32s_to_le(&v), new_space, 1001 + i as i64],
            )
            .unwrap();
        }
        let high = calibrated_similarity(&conn, window).unwrap();
        assert!(high > 0.98);
        let cached =
            crate::db::get_setting(&conn, &format!("{CALIBRATION_KEY_PREFIX}:{new_space}"))
                .unwrap()
                .unwrap();
        let (_, population, cached_threshold): (i64, usize, f32) =
            serde_json::from_str(&cached).unwrap();
        assert_eq!(population, POPULATION);
        assert_eq!(cached_threshold, high);
        assert_eq!(calibrated_similarity(&conn, window).unwrap(), high);
        assert_eq!(merge_close_clusters(&conn).unwrap(), 0);
        assert!(
            crate::db::get_setting(&conn, &format!("{CALIBRATION_KEY_PREFIX}:{new_space}"))
                .unwrap()
                .is_some()
        );
        conn.execute("DELETE FROM chunks WHERE article_id>1002", [])
            .unwrap();
        assert_eq!(
            calibrated_similarity(&conn, window).unwrap(),
            VECTOR_MIN_SIMILARITY
        );
    }

    #[test]
    fn vector_merge_requires_event_time_and_substantive_title_overlap() {
        let (_dir, conn) = open_temp();
        let ids = seed_similar_clusters(&conn, &[1, 1]);
        conn.execute(
            "UPDATE articles SET published_at=published_at-?1 WHERE id=1001",
            [EVENT_WINDOW_DAYS * MS_PER_DAY + 1],
        )
        .unwrap();
        assert_eq!(merge_close_clusters(&conn).unwrap(), 0);
        conn.execute(
            "UPDATE articles SET published_at=created_at, title='The and with' WHERE id=1001",
            [],
        )
        .unwrap();
        assert_eq!(merge_close_clusters(&conn).unwrap(), 0);
        conn.execute(
            "UPDATE articles SET title='library opening downtown',score=10 WHERE id=1001",
            [],
        )
        .unwrap();
        assert_eq!(merge_close_clusters(&conn).unwrap(), 1);
        assert_eq!(representative_id(&conn, ids[0]).unwrap(), Some(1001));
    }

    #[test]
    fn rounds_without_close_pairs_do_not_take_the_write_lock() {
        let (dir, conn) = open_temp();
        seed_similar_clusters(&conn, &[1, 1]);
        conn.execute("UPDATE articles SET title='The and with' WHERE id=1001", [])
            .unwrap();
        crate::db::set_setting(&conn, ALGORITHM_VERSION_KEY, ALGORITHM_VERSION).unwrap();
        // 第一轮写入校准缓存；之后的常态轮次只读。
        assert_eq!(merge_close_clusters(&conn).unwrap(), 0);
        let writer = open_at(&dir.path().join("rrss.db")).unwrap();
        writer.execute_batch("BEGIN IMMEDIATE").unwrap();
        conn.busy_timeout(std::time::Duration::ZERO).unwrap();
        assert_eq!(merge_close_clusters(&conn).unwrap(), 0);
        assert!(!rebuild_if_needed(&conn).unwrap());
        writer.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn merged_away_cluster_stops_comparing_and_count_is_exact() {
        let (_dir, conn) = open_temp();
        // 第一个簇最小，会并入第二个；之后不应再拿它和第三个比较。
        let ids = seed_similar_clusters(&conn, &[1, 2, 1]);
        assert_eq!(merge_close_clusters(&conn).unwrap(), 2);
        let left: Vec<i64> = conn
            .prepare("SELECT id FROM clusters ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(left, [ids[1]]);
        assert_eq!(cluster_members(&conn, ids[1]).unwrap().len(), 4);
    }

    #[test]
    fn failed_merge_rolls_back_the_whole_round() {
        let (_dir, conn) = open_temp();
        let ids = seed_similar_clusters(&conn, &[1, 2, 1]);
        // 第二次删簇时失败：第一次合并也必须回滚。
        conn.execute_batch(
            "CREATE TEMP TABLE merge_deletes (n INTEGER);
             CREATE TEMP TRIGGER fail_second_merge
             BEFORE DELETE ON clusters
             BEGIN
               INSERT INTO merge_deletes VALUES (1);
               SELECT RAISE(ABORT, 'forced failure')
               WHERE (SELECT COUNT(*) FROM merge_deletes) > 1;
             END;",
        )
        .unwrap();
        assert!(merge_close_clusters(&conn).is_err());
        let clusters: i64 = conn
            .query_row("SELECT COUNT(*) FROM clusters", [], |r| r.get(0))
            .unwrap();
        assert_eq!(clusters, 3);
        for (cid, n) in ids.iter().zip([1, 2, 1]) {
            assert_eq!(cluster_members(&conn, *cid).unwrap().len(), n);
        }
    }
}
