//! Bounded admission for ordinary recommendations, before pagination exclusions.
//! This is a derived query, not a retention policy or a persisted recommendation
//! session. Fixed database state and time windows produce the same finite pool.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use rusqlite::{Connection, params, params_from_iter, types::Value};

use rrss_core::{Result, feeds::now_ms, rank_score::SCORE_SQL};

const DAY: i64 = 86_400_000;
const RECENT_DAYS: i64 = 30;
const READ_DAYS: i64 = 7;
const RECOVERY_DAYS: i64 = 180;
const RECENT_SEEDS: usize = 4096;
const RECOVERY_SEEDS: usize = 512;
const PER_FEED_NEWEST: usize = 256;
const PER_FEED_QUALITY: usize = 64;
const PER_FEED_RECOVERY_SEEDS: usize = 32;
const PER_FEED: usize = 160;
const PER_FEED_RECOVERY: usize = 2;
const TOTAL: usize = 2048;
const RECOVERY_TOTAL: usize = 64;
const READ_COST: i64 = 14;
const EXPOSURE_COST: i64 = 3;

// Keep this expression identical to the admission index in schema/migrations.
// First ingestion caps future dates and missing dates; re-fetches never rejuvenate
// an article. Genuine backdated publications retain their original age.
const DATE_SQL: &str =
    "(CASE WHEN a.published_at>0 THEN MIN(a.published_at,a.created_at) ELSE a.created_at END)";

// Lowest source depth first, then cached relevance/date/id. The heap discards the
// worst key, so one prolific source cannot consume the global seed budget.
type SeedKey = (Reverse<usize>, i64, i64, i64, i64);
type SeedHeap = BinaryHeap<Reverse<SeedKey>>;
pub(super) type Member = (i64, i64, Option<i64>, i64);

fn retain(heap: &mut SeedHeap, key: SeedKey, limit: usize) {
    heap.push(Reverse(key));
    if heap.len() > limit {
        heap.pop();
    }
}

pub(super) fn members(
    conn: &Connection,
    eligible: &str,
    group: bool,
    bind: &[i64],
) -> Result<(Vec<Member>, HashMap<i64, crate::topics::Lane>)> {
    let now = now_ms();
    let mut args = bind.to_vec();
    let start = args.len() + 1;
    // These numbered parameters follow the caller's rule/scope bindings.
    let (feed, since, until, read_since, now_index) =
        (start, start + 1, start + 2, start + 3, start + 4);
    args.extend([0, now - RECENT_DAYS * DAY, now, now - READ_DAYS * DAY, now]);
    let member_score = rrss_core::rank_score::MEMBER_SQL;
    let select = format!(
        "SELECT a.id,{member_score},{DATE_SQL} FROM articles a
         WHERE a.feed_id=?{feed} AND {DATE_SQL}>=?{since} AND {DATE_SQL}<=?{until}
         AND a.created_at<=?{now_index} AND (a.read=0 OR {DATE_SQL}>=?{read_since}) AND {eligible}"
    );
    let mut newest = conn.prepare(&format!(
        "{select} ORDER BY {DATE_SQL} DESC,a.id DESC LIMIT {PER_FEED_NEWEST}"
    ))?;
    // This lane sorts only cached rule/interest scores in the date-index range;
    // it never probes learning scores or exposures over the historical library.
    let mut quality = conn.prepare(&format!(
        "{select} AND {member_score}>0 ORDER BY {member_score} DESC,{DATE_SQL} DESC,a.id DESC
         LIMIT {PER_FEED_QUALITY}"
    ))?;
    let mut recovery = conn.prepare(&format!(
        "SELECT a.id,{member_score},{DATE_SQL} FROM articles a
         WHERE a.feed_id=?{feed} AND {DATE_SQL}>=?{since} AND {DATE_SQL}<?{until}
         AND a.created_at<=?{now_index} AND a.read=0 AND {member_score}>=0 AND {eligible}
         ORDER BY {member_score} DESC,{DATE_SQL} DESC,a.id DESC LIMIT {PER_FEED_RECOVERY_SEEDS}"
    ))?;
    // Include hidden/read/expired rows: filtering a busy feed is not dormancy.
    let mut recent_exists = conn.prepare(&format!(
        "SELECT EXISTS(SELECT 1 FROM articles a WHERE a.feed_id=?1
         AND {DATE_SQL}>=?2 AND {DATE_SQL}<=?3)"
    ))?;
    let feeds = conn
        .prepare("SELECT id FROM feeds ORDER BY id")?
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    // Fitted ranking: each feed keeps only its shelf life, scaled by its
    // publishing rate; quiet feeds keep 180 days and leave after repeated
    // unread showings. No separate recovery lane under this policy.
    let shelf = matches!(
        crate::freshness::policy(crate::store::fitted_enabled(conn)?),
        crate::freshness::Policy::Shelf { .. }
    );
    let mut rate_count = conn.prepare(&format!(
        "SELECT COUNT(*) FROM articles a WHERE a.feed_id=?1 AND {DATE_SQL}>=?2 AND {DATE_SQL}<=?3"
    ))?;
    let mut rate_first = conn.prepare(&format!(
        "SELECT MIN({DATE_SQL}) FROM articles a WHERE a.feed_id=?1 AND {DATE_SQL}<=?2"
    ))?;
    let mut quiet = HashSet::new();
    let mut recent_heap = SeedHeap::new();
    let mut recovery_heap = SeedHeap::new();
    let mut dormant = HashSet::new();
    for feed_id in feeds {
        args[start - 1] = feed_id;
        if shelf {
            use crate::freshness::{
                EXEMPT_LOOKBACK_DAYS, RATE_DAYS, exempt, per_day, window_for_rate,
            };
            let count: i64 =
                rate_count.query_row(params![feed_id, now - RATE_DAYS * DAY, now], |r| r.get(0))?;
            let first: Option<i64> = rate_first.query_row(params![feed_id, now], |r| r.get(0))?;
            let rate = per_day(count.max(0) as usize, first.map_or(0, |f| now - f));
            args[start] = if exempt(rate) {
                quiet.insert(feed_id);
                now - EXEMPT_LOOKBACK_DAYS * DAY
            } else {
                now - window_for_rate(rate)
            };
        }
        let mut source = HashMap::new();
        for stmt in [&mut newest, &mut quality] {
            let rows = stmt.query_map(params_from_iter(args.iter()), |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?;
            for row in rows {
                let (id, score, at) = row?;
                source.insert(id, (score, at, id));
            }
        }
        let mut source: Vec<_> = source.into_values().collect();
        source.sort_unstable_by(|a, b| b.cmp(a));
        for (depth, (score, at, id)) in source.into_iter().enumerate() {
            retain(
                &mut recent_heap,
                (Reverse(depth), score, at, id, feed_id),
                RECENT_SEEDS,
            );
        }
        if shelf {
            continue;
        }
        args[start] = now - RECOVERY_DAYS * DAY;
        args[start + 1] = now - RECENT_DAYS * DAY;
        let rows = recovery.query_map(params_from_iter(args.iter()), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        for (depth, row) in rows.enumerate() {
            let (id, score, at) = row?;
            retain(
                &mut recovery_heap,
                (Reverse(depth), score, at, id, feed_id),
                RECOVERY_SEEDS,
            );
        }
        args[start] = now - RECENT_DAYS * DAY;
        args[start + 1] = now;
        if !recent_exists.query_row([feed_id, now - RECENT_DAYS * DAY, now], |row| {
            row.get::<_, bool>(0)
        })? {
            dormant.insert(feed_id);
        }
    }
    let seeds: Vec<_> = recent_heap
        .into_iter()
        .map(|Reverse((_, _, _, id, _))| (id, false))
        .chain(
            recovery_heap
                .into_iter()
                .map(|Reverse((_, _, _, id, _))| (id, true)),
        )
        .collect();
    if seeds.is_empty() {
        return Ok((Vec::new(), HashMap::new()));
    }
    let mut values: Vec<Value> = bind.iter().copied().map(Value::Integer).collect();
    let seeds_index = values.len() + 1;
    values.push(Value::Text(
        serde_json::to_string(&seeds).expect("integer seeds serialize"),
    ));
    let since = values.len() + 1;
    let until = since + 1;
    values.extend([Value::Integer(now - 7 * DAY), Value::Integer(now)]);
    let lane_sql = crate::topics::LANE_SQL;
    let version = crate::combine::ALGO_VERSION;
    let sql = format!(
        "SELECT a.id,a.feed_id,a.cluster_id,{SCORE_SQL},{DATE_SQL},a.read,
                json_extract(seed.value,'$[1]'),a.score,a.interest_boost,COALESCE(r.boost,0),
                EXISTS(SELECT 1 FROM exposures e WHERE e.article_id=a.id
                  AND e.surface='focus' AND e.variant='baseline'
                  AND e.shown_at BETWEEN ?{since} AND ?{until}
                  GROUP BY e.shown_at/{DAY} LIMIT 1 OFFSET 2),
                CASE WHEN json_extract(seed.value,'$[1]') THEN EXISTS(
                  SELECT 1 FROM exposures e WHERE e.article_id=a.id
                  AND e.surface='focus' AND e.variant='baseline' AND e.shown_at<=?{until}
                ) ELSE 0 END,
                CASE WHEN json_extract(seed.value,'$[1]') THEN r.reasons_json END,
                CASE WHEN r.algo_version={version} AND substr(r.reasons_json,1,1)='{{'
                     AND json_valid(r.reasons_json) THEN {lane_sql} END
         FROM json_each(?{seeds_index}) seed CROSS JOIN articles a
         LEFT JOIN article_rank r ON r.article_id=a.id WHERE a.id=json_extract(seed.value,'$[0]')"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(values.iter()), |row| {
        let id: i64 = row.get(0)?;
        let feed: i64 = row.get(1)?;
        let cluster: Option<i64> = row.get(2)?;
        let relevance: i64 = row.get(3)?;
        let at: i64 = row.get(4)?;
        let read: bool = row.get(5)?;
        let recovery: bool = row.get(6)?;
        let repeated: bool = row.get(10)?;
        let seen: bool = row.get(11)?;
        // Old neutral articles normally have a freshness penalty. Ignore only
        // that identified component for recovery admission, never for ordering.
        // Missing/invalid reasons do not excuse an unexplained negative score.
        let cached: Option<String> = row.get(12)?;
        let reasons = if recovery {
            cached
                .as_deref()
                .map(crate::cache::reasons)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let freshness: i64 = reasons
            .iter()
            .filter(|r| r.component == crate::combine::Component::Freshness)
            .map(|r| r.points.min(0))
            .sum();
        let preference = relevance - freshness;
        let negative = row.get::<_, i64>(7)? < 0
            || (bind[2] != 0 && row.get::<_, i64>(8)? < 0)
            || row.get::<_, i64>(9)? - freshness < 0
            || reasons
                .iter()
                .any(|r| r.component != crate::combine::Component::Freshness && r.points < 0);
        let neutral_recovery = recovery && preference == 0;
        let eligible = if recovery {
            !seen && (preference > 0 || (neutral_recovery && !negative && dormant.contains(&feed)))
        } else {
            read || !repeated || (relevance > 0 && !quiet.contains(&feed))
        };
        let priority = relevance
            - if read {
                READ_COST
            } else if repeated {
                EXPOSURE_COST
            } else {
                0
            };
        // json_extract returns native numbers/booleans for malformed scalar
        // lanes. Ignore those metadata values without failing the whole page.
        let lane = match row.get_ref(13)? {
            rusqlite::types::ValueRef::Text(_) => row.get::<_, Option<String>>(13)?,
            _ => None,
        };
        Ok(eligible.then_some((
            priority,
            at,
            id,
            feed,
            cluster,
            recovery,
            neutral_recovery,
            lane,
        )))
    })?;
    let mut ranked = rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    ranked.sort_unstable_by(|a, b| b.cmp(a));
    let positive_recovery: HashSet<_> =
        ranked.iter().filter(|r| r.5 && !r.6).map(|r| r.3).collect();
    let mut groups = HashSet::new();
    let mut counts = HashMap::<(i64, bool), usize>::new();
    let mut total_recent = 0;
    let mut total_recovery = 0;
    let mut members = Vec::new();
    let mut topics = HashMap::new();
    for (priority, _, id, feed, cluster, recovery, neutral, lane) in ranked {
        if neutral && positive_recovery.contains(&feed) {
            continue;
        }
        let count = counts.entry((feed, recovery)).or_default();
        let cap = if neutral {
            1
        } else if recovery {
            PER_FEED_RECOVERY
        } else {
            PER_FEED
        };
        if *count >= cap
            || (recovery && total_recovery >= RECOVERY_TOTAL)
            || (!recovery && total_recent >= TOTAL - RECOVERY_TOTAL)
        {
            continue;
        }
        if group && cluster.is_some_and(|cid| !groups.insert(cid)) {
            continue;
        }
        *count += 1;
        if recovery {
            total_recovery += 1;
        } else {
            total_recent += 1;
        }
        members.push((id, feed, cluster, priority));
        if let Some(lane) = lane.as_deref().and_then(crate::topics::Lane::decode) {
            topics.insert(id, lane);
        }
    }
    Ok((members, topics))
}
