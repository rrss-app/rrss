use std::collections::{BinaryHeap, HashMap, HashSet};

use rusqlite::{Connection, params_from_iter};

use super::{ArticleCard, ListScope};
use crate::{Result, rank::focus::SCORE_SQL};

#[cfg(test)]
mod interest_tests;
#[cfg(test)]
mod tests;

const SAME_FEED_COST: f64 = 2.0;
/// Preceding cards the same-feed cost looks back over.
const SAME_FEED_WINDOW: usize = 2;
/// Evaluation look-backs only, per card beyond the last two: above a one-point
/// gap of rounded scores and below an adjacent repeat.
const FARTHER_FEED_COST: f64 = 1.5;
const SIMILARITY_COST: f64 = 2.0;

#[cfg(feature = "evaluation-clock")]
thread_local! {
    static TOPIC_CALIBRATION: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
    static EXPLORATION: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
    static FEED_WINDOW: std::cell::Cell<usize> = const { std::cell::Cell::new(SAME_FEED_WINDOW) };
}

#[cfg(feature = "evaluation-clock")]
fn with_flag<V: Copy + 'static, T>(
    flag: &'static std::thread::LocalKey<std::cell::Cell<V>>,
    enabled: V,
    f: impl FnOnce() -> T,
) -> T {
    struct Restore<V: Copy + 'static>(&'static std::thread::LocalKey<std::cell::Cell<V>>, V);
    impl<V: Copy + 'static> Drop for Restore<V> {
        fn drop(&mut self) {
            self.0.set(self.1);
        }
    }
    let _restore = Restore(flag, flag.replace(enabled));
    f()
}

#[cfg(feature = "evaluation-clock")]
impl ListScope {
    /// Evaluation only: applies on this thread and restores even during unwind.
    pub fn with_topic_calibration<T>(enabled: bool, f: impl FnOnce() -> T) -> T {
        with_flag(&TOPIC_CALIBRATION, enabled, f)
    }

    /// Evaluation only: turns the daily exploration trial off on this thread.
    pub fn with_exploration<T>(enabled: bool, f: impl FnOnce() -> T) -> T {
        with_flag(&EXPLORATION, enabled, f)
    }

    /// Evaluation only: how many preceding cards the same-feed cost counts.
    pub fn with_same_feed_window<T>(window: usize, f: impl FnOnce() -> T) -> T {
        with_flag(&FEED_WINDOW, window, f)
    }
}

fn same_feed_window() -> usize {
    #[cfg(feature = "evaluation-clock")]
    return FEED_WINDOW.get();
    #[cfg(not(feature = "evaluation-clock"))]
    SAME_FEED_WINDOW
}

pub(super) fn exploration_enabled() -> bool {
    #[cfg(feature = "evaluation-clock")]
    return EXPLORATION.get();
    #[cfg(not(feature = "evaluation-clock"))]
    true
}

/// Membership is independent of page size and exclusions. Only the bounded
/// active pool receives personalized scoring; explicit saved-item retrieval is
/// intentionally exhaustive. Shortlists and page exclusions operate afterwards.
pub(super) fn candidates(
    conn: &Connection,
    eligible: &str,
    group: bool,
    scope: ListScope,
    bind: &[i64],
    exclude_ids: &[i64],
    exclude_clusters: &[i64],
    explore_pool: Option<&mut Vec<i64>>,
) -> Result<Vec<(i64, i64)>> {
    let limit = bind[0].max(0) as usize;
    if scope == ListScope::Starred {
        let sql = format!(
            "SELECT a.id,a.feed_id,a.cluster_id,{SCORE_SQL} AS priority
             FROM articles a LEFT JOIN article_rank r ON r.article_id=a.id
             WHERE {eligible}
             ORDER BY priority DESC,COALESCE(a.published_at,0) DESC,a.id DESC"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(bind.iter()), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        shortlist(rows, limit, group, exclude_ids, exclude_clusters)
    } else {
        let (members, evidence) = super::focus_pool::members(conn, eligible, group, bind)?;
        if let Some(pool) = explore_pool {
            pool.extend(members.iter().map(|m| m.0));
        }
        let mut picked = shortlist(
            members.iter().copied().map(Ok),
            limit,
            group,
            exclude_ids,
            exclude_clusters,
        )?;
        #[cfg(feature = "evaluation-clock")]
        if !TOPIC_CALIBRATION.get() {
            return Ok(picked);
        }
        // A source shortlist alone cannot reach a second learned topic buried
        // in the same busy feed. This third lane only uses admitted members;
        // it does not expand the active pool or scan historical observations.
        if !evidence.is_empty() {
            let on = crate::db::rank_switches(conn)?;
            let epochs: HashSet<_> = evidence.values().filter_map(|e| e.epoch()).collect();
            let mut lanes = HashMap::new();
            let mut topics = BinaryHeap::new();
            let excluded: HashSet<_> = exclude_ids.iter().copied().collect();
            let excluded_clusters: HashSet<_> = exclude_clusters.iter().copied().collect();
            for (ordinal, &(id, _, cluster, priority)) in members.iter().enumerate() {
                if excluded.contains(&id) || cluster.is_some_and(|c| excluded_clusters.contains(&c))
                {
                    continue;
                }
                let Some(e) = evidence.get(&id) else { continue };
                let Some(key) = e.key(on, epochs.len() <= 1) else {
                    continue;
                };
                let depth = lanes.entry(key).or_insert(0_usize);
                *depth += 1;
                let item = (*depth, ordinal, id, priority);
                topics.push(item);
                if topics.len() > limit {
                    topics.pop();
                }
            }
            let mut ids: HashSet<_> = picked.iter().map(|(id, _)| *id).collect();
            ids.extend(topics.into_iter().map(|(_, _, id, _)| id));
            picked = members
                .iter()
                .filter(|m| ids.contains(&m.0))
                .map(|m| (m.0, m.3))
                .collect();
        }
        Ok(picked)
    }
}

/// Consume ordered rows lazily so saved-item retrieval can stop as soon as
/// neither shortlist can improve, without materializing the saved library.
fn shortlist(
    candidates: impl IntoIterator<Item = rusqlite::Result<(i64, i64, Option<i64>, i64)>>,
    limit: usize,
    group: bool,
    exclude_ids: &[i64],
    exclude_clusters: &[i64],
) -> Result<Vec<(i64, i64)>> {
    let excluded: HashSet<_> = exclude_ids.iter().copied().collect();
    let excluded_clusters: HashSet<_> = exclude_clusters.iter().copied().collect();
    let mut groups = HashSet::new();
    let mut feeds = HashMap::<i64, usize>::new();
    let mut leaders = Vec::new();
    let mut sources = BinaryHeap::new();
    let mut ordinal = 0_usize;
    for row in candidates {
        let (id, feed, cluster, priority) = row?;
        if group && cluster.is_some_and(|id| !groups.insert(id)) {
            continue;
        }
        if excluded.contains(&id) || cluster.is_some_and(|id| excluded_clusters.contains(&id)) {
            continue;
        }
        let position = feeds.entry(feed).or_default();
        *position += 1;
        if leaders.len() < limit {
            leaders.push((ordinal, id, priority));
        }
        // Ordinal preserves priority/date/id ties without sorting all sources.
        // The max-heap retains the smallest (source position, global ordinal).
        let candidate = (*position, ordinal, id, priority);
        if sources.len() < limit {
            sources.push(candidate);
        } else if let Some(mut worst) = sources.peek_mut() {
            if candidate < *worst {
                *worst = candidate;
            }
        }
        ordinal += 1;
        // Once both lists are full and every source winner is a first position,
        // later rows have position >= 1 and a larger ordinal: neither list can
        // improve. With fewer sources, keep scanning for their later positions.
        if leaders.len() == limit
            && sources.len() == limit
            && sources.peek().is_some_and(|(position, ..)| *position == 1)
        {
            break;
        }
    }
    leaders.extend(
        sources
            .into_iter()
            .map(|(_, order, id, priority)| (order, id, priority)),
    );
    leaders.sort_unstable();
    leaders.dedup();
    Ok(leaders
        .into_iter()
        .map(|(_, id, priority)| (id, priority))
        .collect())
}

struct Content {
    vector: Option<Vec<f32>>,
    tokens: HashSet<String>,
}

impl Content {
    fn similarity(&self, other: &Self) -> f64 {
        if let (Some(a), Some(b)) = (&self.vector, &other.vector) {
            return f64::from((1.0 - crate::rag::cosine_distance(a, b)).clamp(0.0, 1.0));
        }
        let common = self.tokens.intersection(&other.tokens).count();
        let union = self.tokens.len() + other.tokens.len() - common;
        common as f64 / union.max(1) as f64
    }
}

pub(super) fn select(
    conn: &Connection,
    candidates: Vec<(ArticleCard, i64)>,
    limit: usize,
    previous: &[i64],
    use_content: bool,
    group: bool,
) -> Result<Vec<ArticleCard>> {
    let on = crate::db::rank_switches(conn)?;
    // Saved-item retrieval is exhaustive, unlike the bounded recommendation
    // pool. Keep its original cost and ordering even after thousands of pages.
    let window = same_feed_window();
    let calibrate = super::list_scope(conn)? != ListScope::Starred;
    #[cfg(feature = "evaluation-clock")]
    let calibrate = calibrate && TOPIC_CALIBRATION.get();
    let ids: Vec<_> = if calibrate {
        candidates
            .iter()
            .map(|(card, _)| card.id)
            .chain(previous.iter().copied())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    } else {
        previous.iter().rev().take(window).copied().collect()
    };
    let mut stmt = conn.prepare(
        "SELECT a.id,a.feed_id,a.cluster_id,r.reasons_json,r.algo_version
         FROM json_each(?1) wanted JOIN articles a ON a.id=wanted.value
         LEFT JOIN article_rank r ON r.article_id=a.id WHERE a.expires_at IS NULL",
    )?;
    let context = stmt
        .query_map([serde_json::to_string(&ids).expect("integer ids")], |row| {
            let id: i64 = row.get(0)?;
            let feed: i64 = row.get(1)?;
            let cluster: Option<i64> = row.get(2)?;
            let version: Option<i64> = row.get(4)?;
            let evidence = if calibrate && version == Some(crate::rank::combine::ALGO_VERSION) {
                row.get::<_, Option<String>>(3)?
                    .as_deref()
                    .and_then(crate::rank::cache::topics)
            } else {
                None
            };
            Ok((id, (feed, cluster, evidence)))
        })?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;
    // Incremental writes can contain different vector partitions. Never treat
    // different representatives of the same changing partition as new interests.
    let epochs: HashSet<_> = context
        .values()
        .filter_map(|(_, _, e)| e.as_ref()?.epoch())
        .collect();
    let allocations: HashMap<_, _> = context
        .iter()
        .filter_map(|(id, (_, _, e))| {
            e.as_ref()
                .map(|e| (*id, e.allocation(on, epochs.len() <= 1)))
        })
        .collect();
    let mut covered = HashMap::new();
    let mut seen = HashSet::new();
    let mut groups = HashSet::new();
    for id in if calibrate { previous } else { &[] } {
        let Some((_, cluster, _)) = context.get(id) else {
            continue;
        };
        if !seen.insert(*id) || (group && cluster.is_some_and(|c| !groups.insert(c))) {
            continue;
        }
        if let Some(a) = allocations.get(id) {
            a.observe(&mut covered);
        }
    }
    let mut recent: std::collections::VecDeque<Option<i64>> = previous
        .iter()
        .rev()
        .take(window)
        .map(|id| context.get(id).map(|(feed, _, _)| *feed))
        .collect();
    let mut remaining = Vec::with_capacity(candidates.len());
    for (card, priority) in candidates {
        let content = if use_content {
            Some(Content {
                vector: crate::rag::title_embedding(conn, card.id)?,
                tokens: crate::simhash::tokenize(&format!("{} {}", card.title, card.summary))
                    .into_iter()
                    .collect(),
            })
        } else {
            None
        };
        remaining.push((card, priority, content, 0.0_f64));
    }
    let mut selected = Vec::with_capacity(limit.min(remaining.len()));
    while selected.len() < limit && !remaining.is_empty() {
        let mut best = 0;
        let mut best_score = f64::NEG_INFINITY;
        let mut scores = Vec::with_capacity(remaining.len());
        let mut costs = Vec::with_capacity(remaining.len());
        let mut unlearned = f64::NEG_INFINITY;
        for (i, (card, priority, _, similarity)) in remaining.iter().enumerate() {
            let repeats: f64 = recent
                .iter()
                .enumerate()
                .filter(|(_, feed)| **feed == Some(card.feed_id))
                .map(|(back, _)| {
                    if back < SAME_FEED_WINDOW {
                        SAME_FEED_COST
                    } else {
                        FARTHER_FEED_COST
                    }
                })
                .sum();
            let cost = repeats + SIMILARITY_COST * similarity;
            let score = *priority as f64 - cost;
            costs.push(cost);
            scores.push(score);
            if !allocations.get(&card.id).is_some_and(|a| a.eligible()) {
                unlearned = unlearned.max(*priority as f64);
            }
            // Equal adjusted scores keep the incoming priority / date / id order.
            if score > best_score {
                best = i;
                best_score = score;
            }
        }
        if allocations
            .get(&remaining[best].0.id)
            .is_some_and(|a| a.eligible())
        {
            // A common source/content cost does not distinguish topic choices
            // (both topics may belong to one feed). Only relative repetition
            // costs overlap with the same learned utility already discounted.
            let common_cost = remaining
                .iter()
                .enumerate()
                .filter(|(_, (card, priority, _, _))| {
                    *priority as f64 >= unlearned
                        && allocations.get(&card.id).is_some_and(|a| a.eligible())
                })
                .map(|(i, _)| costs[i])
                .fold(f64::INFINITY, f64::min);
            let mut marginal = f64::NEG_INFINITY;
            for (i, (card, priority, _, _)) in remaining.iter().enumerate() {
                let Some(a) = allocations.get(&card.id).filter(|a| a.eligible()) else {
                    continue;
                };
                // Calibration only redistributes learned-topic opportunities.
                // Unlearned items keep their baseline turn; a weaker calibrated
                // replacement may not leap over their substantive advantage.
                if (*priority as f64) < unlearned {
                    continue;
                }
                let discount = a.discount(&covered);
                let budget = a.total();
                let fraction = if budget > 0.0 {
                    ((budget - discount) / budget).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                // Source/content repetition and topic coverage can spend the
                // same utility. Fade only their overlapping relative part;
                // residual non-topic costs and common costs remain untouched.
                let overlap = budget.min((costs[i] - common_cost).max(0.0));
                let score = scores[i] - discount + overlap * (1.0 - fraction * fraction);
                if score > marginal {
                    best = i;
                    marginal = score;
                }
            }
        }
        let (card, _, content, _) = remaining.remove(best);
        if let Some(a) = allocations.get(&card.id) {
            a.observe(&mut covered);
        }
        recent.push_front(Some(card.feed_id));
        recent.truncate(window);
        if let Some(content) = content {
            for (_, _, other, max_similarity) in &mut remaining {
                if let Some(other) = other {
                    *max_similarity = max_similarity.max(content.similarity(other));
                }
            }
        }
        selected.push(card);
    }
    Ok(selected)
}
