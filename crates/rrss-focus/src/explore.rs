//! One daily trial of a rarely exposed source, drawn from the admitted Focus pool.
use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, params};

use super::{evidence::DAY_MS, labels, spot_check};
use rrss_core::signals::engagement;
use rrss_core::{
    Result,
    articles::{self, ArticleCard},
    signals,
};

// SplitMix64 supplies a fixed, platform-independent stream from the daily seed.
// Integer Gamma shapes are sums of independent exponential samples; their ratio
// is Beta(1 + reads, 1 + skips), without another RNG/distribution dependency.
fn beta(mut seed: u64, reads: usize, skips: usize) -> f64 {
    let mut gamma = |shape| {
        (0..shape)
            .map(|_| {
                seed = seed.wrapping_add(0x9e3779b97f4a7c15);
                let mut z = seed;
                z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
                z ^= z >> 31;
                let u = ((z >> 12) as f64 + 0.5) / (1_u64 << 52) as f64;
                -u.ln()
            })
            .sum::<f64>()
    };
    let a = gamma(1 + reads);
    a / (a + gamma(1 + skips))
}

pub(crate) fn inject(
    conn: &Connection,
    cards: &mut Vec<ArticleCard>,
    pool: &[i64],
    threshold: i64,
    now: i64,
) -> Result<()> {
    if cards.len() < 4 || pool.is_empty() {
        return Ok(());
    }
    let visible = articles::VISIBLE_SQL.replace("$threshold", "?2");
    let mut stmt = conn.prepare(&format!(
        "SELECT a.id,a.feed_id FROM json_each(?1) pool JOIN articles a ON a.id=pool.value
         WHERE a.read=0 AND {visible}
         AND NOT EXISTS(SELECT 1 FROM article_events e WHERE e.article_id=a.id AND e.kind='down' AND e.at<=?3)
         AND NOT EXISTS(SELECT 1 FROM feedback f WHERE f.article_id=a.id AND f.kind='down' AND f.created_at<=?3)
         ORDER BY COALESCE(a.published_at,a.created_at) DESC,a.id DESC"
    ))?;
    let candidates = stmt
        .query_map(
            params![serde_json::to_string(pool).unwrap(), threshold, now],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let present: HashSet<_> = cards
        .iter()
        .flat_map(|c| std::iter::once(c.id).chain(c.cluster_members.iter().copied()))
        .collect();
    let mut newest = HashMap::new();
    for (id, feed) in candidates
        .into_iter()
        .filter(|(id, _)| !present.contains(id))
    {
        newest.entry(feed).or_insert(id);
    }
    if newest.is_empty() {
        return Ok(());
    }
    let start = now - 30 * DAY_MS;
    let exposures = signals::rows(
        conn,
        "SELECT a.id,a.feed_id,MIN(e.shown_at),COUNT(*)
         FROM exposures e JOIN articles a ON a.id=e.article_id
         WHERE e.surface='focus' AND e.variant IN ('baseline','explore') AND a.expires_at IS NULL
         AND e.shown_at>=?1 AND e.shown_at<?2 GROUP BY a.id,a.feed_id",
        start,
        now,
        |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, usize>(3)?,
            ))
        },
    )?;
    let mut stats = HashMap::<i64, (usize, usize, usize)>::new();
    for (_, feed, _, count) in &exposures {
        stats.entry(*feed).or_default().0 += count;
    }
    newest.retain(|feed, _| stats.get(feed).is_none_or(|s| s.0 < 10));
    if newest.is_empty() {
        return Ok(());
    }
    let sessions = signals::load_session_summaries(conn, start, now)?;
    let history = engagement::merge_daily_sessions(
        &signals::load_session_summaries(conn, start - 30 * DAY_MS, start)?,
        0,
    );
    let originals: Vec<_> = signals::load_events(conn, start, now)?
        .into_iter()
        .filter(|e| e.kind == signals::EventKind::OpenOriginal)
        .collect();
    let mut reads = HashMap::<i64, i64>::new();
    for v in labels::evidence(&originals, &sessions, &history, 0)
        .into_iter()
        .filter(|v| v.weight > 0.0)
    {
        reads
            .entry(v.article_id)
            .and_modify(|at| *at = (*at).max(v.at))
            .or_insert(v.at);
    }
    for (id, feed, shown, _) in exposures {
        let s = stats.entry(feed).or_default();
        if reads.get(&id).is_some_and(|at| *at >= shown) {
            s.1 += 1;
        } else {
            s.2 += 1;
        }
    }
    let day = now.div_euclid(DAY_MS).to_string();
    let selected = newest
        .into_iter()
        .map(|(feed, id)| {
            let (_, reads, skips) = stats.get(&feed).copied().unwrap_or_default();
            (
                beta(spot_check::daily_hash(feed, &day), reads, skips),
                feed,
                id,
            )
        })
        .max_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    if let Some((_, _, id)) = selected {
        let mut card = articles::query_ranked_cards(
            conn,
            "SELECT ?1 AS id,0 AS priority,0 AS position",
            [id],
        )?
        .pop()
        .unwrap()
        .0;
        card.explore = true;
        spot_check::insert(cards, card, spot_check::daily_hash(0, &day), 4, 14);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
