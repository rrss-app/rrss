//! 简报（Digest）域：周期窗口存稿、选题、成稿输入打包与来源对齐。
//! 日志 target：`rrss_core::digest`。

use chrono::Datelike;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::ai_store::truncate_chars;
use crate::db::{get_setting, set_setting};
use crate::error::Result;
use crate::feeds::now_ms;

/// 自动生成时刻，从本地零点起的分钟数。缺省表示早上 8:00。
pub const DIGEST_GENERATE_MINUTES_KEY: &str = "digest_generate_minutes";
pub const DIGEST_GENERATE_DEFAULT_MINUTES: i32 = 8 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DigestSource {
    pub article_id: i64,
    pub title: String,
    pub feed_title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigestRow {
    pub id: i64,
    pub period: String,
    pub period_start: i64,
    pub body: String,
    pub items_json: String,
}

/// 读取自动生成时刻。缺省或无法识别时用早上 8:00，不把坏值当成零点。
pub fn digest_generate_minutes(conn: &Connection) -> Result<i32> {
    let Some(raw) = get_setting(conn, DIGEST_GENERATE_MINUTES_KEY)? else {
        return Ok(DIGEST_GENERATE_DEFAULT_MINUTES);
    };
    Ok(parse_digest_generate_minutes(&raw).unwrap_or(DIGEST_GENERATE_DEFAULT_MINUTES))
}

pub fn set_digest_generate_minutes(conn: &Connection, minutes: i32) -> Result<()> {
    let minutes = minutes.clamp(0, 24 * 60 - 1);
    set_setting(conn, DIGEST_GENERATE_MINUTES_KEY, &minutes.to_string())
}

pub fn parse_digest_generate_minutes(raw: &str) -> Option<i32> {
    let minutes: i32 = raw.trim().parse().ok()?;
    (0..24 * 60).contains(&minutes).then_some(minutes)
}

/// 当期是否已到自动生成时刻。`period_start_ms` 仍是本地零点，时刻加在这一天上。
pub fn digest_generate_due(period_start_ms: i64, now_ms: i64, minutes: i32) -> bool {
    now_ms >= digest_ready_ms(period_start_ms, minutes)
}

fn digest_ready_ms(period_start_ms: i64, minutes: i32) -> i64 {
    let minutes = minutes.clamp(0, 24 * 60 - 1);
    let date = chrono::DateTime::from_timestamp_millis(period_start_ms)
        .unwrap_or(chrono::DateTime::UNIX_EPOCH)
        .with_timezone(&chrono::Local)
        .date_naive();
    let clock = date
        .and_hms_opt((minutes / 60) as u32, (minutes % 60) as u32, 0)
        .expect("clamped clock");
    match clock.and_local_timezone(chrono::Local) {
        chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => {
            dt.timestamp_millis()
        }
        chrono::LocalResult::None => period_start_ms + i64::from(minutes) * 60_000,
    }
}

pub fn period_start_ms(period: &str) -> i64 {
    let today = chrono::Local::now().date_naive();
    let date = if period == "weekly" {
        let days = i64::from(today.weekday().num_days_from_monday());
        today - chrono::Duration::days(days)
    } else {
        today
    };
    naive_local_midnight_ms(date)
}

fn naive_local_midnight_ms(date: chrono::NaiveDate) -> i64 {
    let midnight = date.and_hms_opt(0, 0, 0).expect("midnight");
    match midnight.and_local_timezone(chrono::Local) {
        chrono::LocalResult::Single(dt) | chrono::LocalResult::Ambiguous(dt, _) => {
            dt.timestamp_millis()
        }
        chrono::LocalResult::None => chrono::Local::now().timestamp_millis(),
    }
}

pub fn latest_digest(conn: &Connection, period: &str) -> Result<Option<DigestRow>> {
    conn.query_row(
        "SELECT id, period, period_start, body, COALESCE(items_json, '[]') FROM digests
         WHERE period = ?1
         ORDER BY period_start DESC, id DESC
         LIMIT 1",
        [period],
        map_digest,
    )
    .optional()
    .map_err(Into::into)
}

pub fn digest_by_id(conn: &Connection, id: i64) -> Result<Option<DigestRow>> {
    conn.query_row(
        "SELECT id, period, period_start, body, COALESCE(items_json, '[]') FROM digests
         WHERE id = ?1",
        [id],
        map_digest,
    )
    .optional()
    .map_err(Into::into)
}

pub fn list_digests(conn: &Connection, period: &str, limit: i64) -> Result<Vec<DigestRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, period, period_start, body, COALESCE(items_json, '[]') FROM digests
         WHERE period = ?1
         ORDER BY period_start DESC, id DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![period, limit], map_digest)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn digest_date_label(period: &str, period_start: i64) -> String {
    let start = chrono::DateTime::from_timestamp_millis(period_start)
        .unwrap_or(chrono::DateTime::UNIX_EPOCH)
        .with_timezone(&chrono::Local)
        .date_naive();
    if period == "weekly" {
        let end = start + chrono::Duration::days(6);
        format!("{} – {}", format_month_day(start), format_month_day(end))
    } else {
        format_month_day(start)
    }
}

fn format_month_day(date: chrono::NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

pub fn insert_digest(
    conn: &Connection,
    period: &str,
    period_start: i64,
    body: &str,
    items_json: &str,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO digests (period, period_start, body, items_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (period, period_start) DO UPDATE SET
           body = excluded.body,
           items_json = excluded.items_json,
           created_at = excluded.created_at",
        params![period, period_start, body, items_json, now_ms()],
    )?;
    conn.query_row(
        "SELECT id FROM digests WHERE period = ?1 AND period_start = ?2",
        params![period, period_start],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

/// 关注 + 未读代表：标题 + 已有摘要截一句，不带全文。按此刻的日报范围取候选。
pub fn digest_pack_input(conn: &Connection, limit: i64) -> Result<String> {
    let window = DigestWindow::for_period("daily", now_ms(), period_start_ms("daily"), limit);
    let items = digest_candidates(conn, limit, &window)?;
    Ok(format_digest_items(&items))
}

#[derive(Debug, Clone)]
pub struct DigestItem {
    pub article_id: i64,
    pub title: String,
    pub summary: String,
    pub summary_json: Option<String>,
    pub cluster_id: Option<i64>,
    pub feed_title: String,
}

pub fn serialize_digest_items(items: &[DigestItem]) -> String {
    let sources: Vec<DigestSource> = items
        .iter()
        .map(|it| DigestSource {
            article_id: it.article_id,
            title: it.title.clone(),
            feed_title: it.feed_title.clone(),
        })
        .collect();
    serde_json::to_string(&sources).unwrap_or_else(|_| "[]".to_string())
}

pub fn serialize_digest_sources(sources: &[DigestSource]) -> String {
    serde_json::to_string(sources).unwrap_or_else(|_| "[]".to_string())
}

/// 正文 Markdown 里 `-` 列表条数（与前端 chip 序一致）。
pub fn count_digest_list_items(body: &str) -> usize {
    digest_list_item_texts(body).len()
}

fn digest_list_item_texts(body: &str) -> Vec<String> {
    let doc = rrss_render::markdown_to_document(body);
    doc.blocks
        .iter()
        .filter_map(|b| match b {
            rrss_render::Block::ListItem { inlines, .. } => {
                Some(rrss_render::inlines_to_plain(inlines))
            }
            _ => None,
        })
        .collect()
}

fn parse_json_article_id(value: &serde_json::Value) -> Option<i64> {
    if let Some(id) = value.as_i64() {
        return Some(id);
    }
    if let Some(id) = value.as_u64() {
        return i64::try_from(id).ok();
    }
    if let Some(s) = value.as_str() {
        return s.trim().parse().ok();
    }
    None
}

pub fn parse_digest_item_ns(value: &serde_json::Value) -> Vec<Vec<usize>> {
    value
        .get("item_refs")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|item| {
                    let mut out = Vec::new();
                    if let Some(ns) = item.get("ns").and_then(|v| v.as_array()) {
                        for n in ns {
                            let idx = if let Some(u) = n.as_u64() {
                                usize::try_from(u).ok()
                            } else {
                                n.as_i64().and_then(|i| usize::try_from(i).ok())
                            };
                            if let Some(i) = idx.filter(|x| *x > 0)
                                && !out.contains(&i)
                            {
                                out.push(i);
                            }
                        }
                    }
                    // 不在此截断：越界序号要先由调用方按编号表丢弃，再取前 3 个。
                    out
                })
                .collect()
        })
        .unwrap_or_default()
}

fn digest_line_from_json_value(item: &serde_json::Value) -> Option<Vec<DigestSource>> {
    if item.get("sources").is_some() {
        let sources = item
            .get("sources")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| {
                        let article_id = s.get("article_id").and_then(parse_json_article_id)?;
                        if article_id <= 0 {
                            return None;
                        }
                        let title = s
                            .get("title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let feed_title = s
                            .get("feed_title")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        Some(DigestSource {
                            article_id,
                            title,
                            feed_title,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Some(sources);
    }
    if item.get("article_id").is_some() {
        let article_id = item.get("article_id").and_then(parse_json_article_id)?;
        if article_id > 0 {
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let feed_title = item
                .get("feed_title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            return Some(vec![DigestSource {
                article_id,
                title,
                feed_title,
            }]);
        }
        return Some(vec![]);
    }
    None
}

fn digest_source_of(item: &DigestItem) -> DigestSource {
    DigestSource {
        article_id: item.article_id,
        title: item.title.clone(),
        feed_title: item.feed_title.clone(),
    }
}

fn serialize_digest_lines(lines: &[Vec<DigestSource>]) -> String {
    let wrapped: Vec<serde_json::Value> = lines
        .iter()
        .map(|sources| serde_json::json!({ "sources": sources }))
        .collect();
    serde_json::to_string(&wrapped).unwrap_or_else(|_| "[]".to_string())
}

/// 构建与正文列表项严格同序的 `items_json`（嵌套 `sources` 数组）。
/// `item_ns` 已在清洗前按原始列表项绑定；这里仅按成稿输入的序号表反查，
/// 不再用标题相似度补猜，也不把坏序号后的条目向前压缩。
pub fn build_digest_items_json_strict(
    body: &str,
    numbered_items: &[DigestItem],
    item_ns: &[Vec<usize>],
) -> String {
    let texts = digest_list_item_texts(body);
    if texts.is_empty() || numbered_items.is_empty() || item_ns.len() != texts.len() {
        return "[]".to_string();
    }

    let lines: Vec<Vec<DigestSource>> = item_ns
        .iter()
        .map(|refs| {
            let mut seen_ids = std::collections::HashSet::new();
            refs.iter()
                .filter(|n| **n > 0)
                .filter_map(|n| numbered_items.get(n.saturating_sub(1)))
                .filter(|item| seen_ids.insert(item.article_id))
                .take(3)
                .map(digest_source_of)
                .collect()
        })
        .collect();

    if lines.iter().all(|line| line.is_empty()) {
        return "[]".to_string();
    }
    serialize_digest_lines(&lines)
}

/// 读取时只使用成稿保存的来源快照。
///
/// v1 的槽位关系只能由正文列表项数量证明：数量一致时保留每个槽位，
/// 包括中间空槽；数量不一致时为每个正文条目返回空来源，避免把后续槽位前移。
/// 读取不会查文章表，也不会按标题相似度重新绑定。
pub fn align_digest_sources(body: &str, items_json: &str) -> Vec<Vec<DigestSource>> {
    let texts = digest_list_item_texts(body);
    if texts.is_empty() {
        return Vec::new();
    }

    let stored = parse_digest_items_json(items_json);
    align_digest_source_slots(texts.len(), &stored)
}

/// 使用同一份 IR 的列表项数对齐 v1 快照；解析和展示用例不必再次解析正文。
pub fn align_digest_source_slots(
    item_count: usize,
    stored: &[Vec<DigestSource>],
) -> Vec<Vec<DigestSource>> {
    if stored.len() != item_count {
        return vec![Vec::new(); item_count];
    }
    stored
        .iter()
        .map(|line| line.iter().filter(|s| s.article_id > 0).cloned().collect())
        .collect()
}

pub fn parse_digest_items_json(raw: &str) -> Vec<Vec<DigestSource>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return Vec::new();
    };
    let Some(arr) = val.as_array() else {
        return Vec::new();
    };
    arr.iter()
        .map(|item| digest_line_from_json_value(item).unwrap_or_default())
        .collect()
}

const HOUR_MS: i64 = 3_600_000;
const DAY_MS: i64 = 24 * HOUR_MS;
/// 日报收最近 36 小时入库的未读文章：比一天略宽，早上生成时能带上前一天傍晚的更新。
pub const DAILY_WINDOW_MS: i64 = 36 * HOUR_MS;
/// 周报收最近 7 天入库的文章。
pub const WEEKLY_WINDOW_MS: i64 = 7 * DAY_MS;
/// 周报候选里已读文章最多占几分之一。
pub const WEEKLY_READ_SHARE: i64 = 4;

/// 简报候选的时间范围与已读规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DigestWindow {
    /// 只收这之后入库、发布时间也不早于它前一天的文章。
    pub since_ms: i64,
    /// 已读文章最多几篇；0 表示只收未读。
    pub read_limit: i64,
    /// 排除本期之前、时间范围内那几期日报已经用过的来源（日报窗口比一天宽，避免连着两期重复）。
    pub exclude_daily_before: Option<i64>,
}

impl DigestWindow {
    pub fn for_period(period: &str, now_ms: i64, period_start: i64, limit: i64) -> Self {
        if period == "weekly" {
            Self {
                since_ms: now_ms - WEEKLY_WINDOW_MS,
                read_limit: limit / WEEKLY_READ_SHARE,
                exclude_daily_before: None,
            }
        } else {
            Self {
                since_ms: now_ms - DAILY_WINDOW_MS,
                read_limit: 0,
                exclude_daily_before: Some(period_start),
            }
        }
    }
}

/// 简报候选：时间范围内的关注文章，先取已有摘要的，不足再补未摘要的关注篇与中性未读；周报另外带少量已读。
/// 排除点踩文章与已隐藏的噪声源，并按源做两轮限额，避免单一高频源垄断候选池。
pub fn digest_candidates(
    conn: &Connection,
    limit: i64,
    window: &DigestWindow,
) -> Result<Vec<DigestItem>> {
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let threshold: i64 = crate::db::get_setting(conn, crate::db::HIDE_THRESHOLD_KEY)?
        .and_then(|s| s.parse().ok())
        .unwrap_or(-10);
    let boost = i64::from(crate::interests::boost_enabled(conn)?);
    let hidden_feeds = crate::articles::focus_excluded_feeds(conn)?;
    let noisy_feeds = crate::feeds::noisy_feed_ids(conn)?;
    let exclude_daily_clusters = window.exclude_daily_before == Some(i64::MAX);
    let cluster_excl_sql = if exclude_daily_clusters {
        format!(
            " AND (a.cluster_id IS NULL OR a.cluster_id NOT IN (
                SELECT a2.cluster_id FROM articles a2
                 WHERE a2.cluster_id IS NOT NULL
                   AND a2.id IN (
                     SELECT CAST(j.value AS INTEGER) FROM digests d, json_tree(d.items_json) j
                      WHERE d.period = 'daily' AND d.period_start < ?5 AND d.period_start >= ?4 - {DAY_MS}
                        AND json_valid(d.items_json) AND j.key = 'article_id' AND j.type = 'integer')))"
        )
    } else {
        String::new()
    };

    let query = |filter: &str, n: i64| -> Result<Vec<(i64, DigestItem)>> {
        if n <= 0 {
            return Ok(Vec::new());
        }
        let mut bind: Vec<rusqlite::types::Value> = vec![
            n.into(),
            threshold.into(),
            boost.into(),
            window.since_ms.into(),
            window.exclude_daily_before.unwrap_or(i64::MIN).into(),
        ];
        let noisy_sql = crate::articles::exclude_feeds_sql_values(&mut bind, &hidden_feeds);
        let noisy_order = crate::articles::demote_feeds_order_sql(&mut bind, &noisy_feeds);
        let sql = format!(
            "SELECT a.id, a.feed_id, a.title, COALESCE(a.summary, ''), a.summary_json, a.cluster_id,
                    COALESCE(f.title, ''), a.blocks_json, a.content_html
             FROM articles a
             LEFT JOIN feeds f ON f.id = a.feed_id
             WHERE a.expires_at IS NULL AND a.score > ?2 AND a.pruned_at IS NULL
               AND a.created_at >= ?4
               AND COALESCE(a.published_at, a.created_at) >= ?4 - {DAY_MS}
               AND NOT EXISTS (SELECT 1 FROM feedback fb WHERE fb.article_id = a.id AND fb.kind = 'down')
               AND a.id NOT IN (
                 SELECT CAST(j.value AS INTEGER) FROM digests d, json_tree(d.items_json) j
                  WHERE d.period = 'daily' AND d.period_start < ?5 AND d.period_start >= ?4 - {DAY_MS}
                    AND json_valid(d.items_json) AND j.key = 'article_id' AND j.type = 'integer')
               {cluster_excl_sql}{noisy_sql}
               AND {filter}
             ORDER BY {noisy_order}(a.score + CASE WHEN ?3 THEN a.interest_boost ELSE 0 END) DESC,
                      COALESCE(a.published_at, 0) DESC
             LIMIT ?1"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(bind), |row| {
            let summary: String = row.get(3)?;
            let summary_json: Option<String> = row.get(4)?;
            let blocks_json: Option<String> = row.get(7)?;
            let content_html: Option<String> = row.get(8)?;
            let effective_summary = if summary.trim().is_empty()
                && summary_json
                    .as_deref()
                    .is_none_or(|raw| raw.trim().is_empty())
            {
                crate::summary_extract::extract_lead_snippet(
                    blocks_json.as_deref(),
                    content_html.as_deref(),
                    180,
                )
            } else {
                summary
            };
            Ok((
                row.get::<_, i64>(1)?,
                DigestItem {
                    article_id: row.get(0)?,
                    title: row.get(2)?,
                    summary: effective_summary,
                    summary_json,
                    cluster_id: row.get(5)?,
                    feed_title: row.get(6)?,
                },
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    };
    const FOCUS: &str = "(a.score + CASE WHEN ?3 THEN a.interest_boost ELSE 0 END) > 0";
    const SUMMARIZED: &str = "a.summary IS NOT NULL AND a.summary != ''";

    let per_feed_cap = ((limit / 5).max(3)) as usize;
    let diversify_group = |pool: Vec<(i64, DigestItem)>, cap: usize| -> Vec<DigestItem> {
        // 先按源分组（保持各源内部原有优先级顺序与源首次出现顺序），再轮询穿插取前 per_feed_cap 篇，
        // 让前排候选天然覆盖多源，避免单一高频源把前 12 位全占满；小库不足时再由溢出篇回填。
        let mut feed_order: Vec<i64> = Vec::new();
        let mut by_feed: std::collections::HashMap<i64, std::collections::VecDeque<DigestItem>> =
            std::collections::HashMap::new();
        for (feed_id, item) in pool {
            if !by_feed.contains_key(&feed_id) {
                feed_order.push(feed_id);
            }
            by_feed.entry(feed_id).or_default().push_back(item);
        }
        let mut out = Vec::with_capacity(cap);
        for _round in 0..per_feed_cap {
            let mut progressed = false;
            for &feed_id in &feed_order {
                if out.len() >= cap {
                    break;
                }
                if let Some(item) = by_feed.get_mut(&feed_id).and_then(|q| q.pop_front()) {
                    out.push(item);
                    progressed = true;
                }
            }
            if !progressed || out.len() >= cap {
                break;
            }
        }
        // 单源或极少源场景：前 per_feed_cap 轮没填满 cap 时，按原序继续回填剩余文章。
        while out.len() < cap {
            let mut progressed = false;
            for &feed_id in &feed_order {
                if out.len() >= cap {
                    break;
                }
                if let Some(item) = by_feed.get_mut(&feed_id).and_then(|q| q.pop_front()) {
                    out.push(item);
                    progressed = true;
                }
            }
            if !progressed {
                break;
            }
        }
        out
    };
    let diversify = |pool: Vec<(i64, DigestItem)>, cap: usize| -> Vec<DigestItem> {
        let (noisy_pool, clean_pool): (Vec<_>, Vec<_>) = pool
            .into_iter()
            .partition(|(feed_id, _)| noisy_feeds.contains(feed_id));
        let mut out = diversify_group(clean_pool, cap);
        if out.len() < cap && !noisy_pool.is_empty() {
            let remain = cap - out.len();
            out.extend(diversify_group(noisy_pool, remain));
        }
        out
    };

    // 周报的已读只收有摘要的关注文章，最多占四分之一。
    let read_pool = if window.read_limit > 0 {
        query(
            &format!("{FOCUS} AND a.read = 1 AND {SUMMARIZED}"),
            window.read_limit.saturating_mul(2),
        )?
    } else {
        Vec::new()
    };
    let read = diversify(read_pool, window.read_limit.max(0) as usize);
    let unread_limit = (limit - read.len() as i64).max(0) as usize;
    let fetch_cap = (unread_limit as i64).saturating_mul(3).max(unread_limit as i64);

    // 1. 优先取已有摘要的未读关注文章；2. 其次未摘要的未读关注文章；3. 最后中性未读兜底。
    let mut unread_pool: Vec<(i64, DigestItem)> = Vec::new();
    let mut seen: std::collections::HashSet<i64> = read.iter().map(|it| it.article_id).collect();
    for filter in [
        format!("{FOCUS} AND a.read = 0 AND {SUMMARIZED}"),
        format!("{FOCUS} AND a.read = 0"),
        "a.read = 0".to_string(),
    ] {
        if unread_pool.len() >= fetch_cap as usize {
            break;
        }
        for (feed_id, item) in query(&filter, fetch_cap)? {
            if seen.insert(item.article_id) {
                unread_pool.push((feed_id, item));
                if unread_pool.len() >= fetch_cap as usize {
                    break;
                }
            }
        }
    }
    let mut items = diversify(unread_pool, unread_limit);
    items.extend(read);
    Ok(items)
}

/// 单篇摘要里的关键事实（数字、主体、结论）。旧摘要没有这一项时为空。
pub fn summary_facts(json_str: Option<&str>) -> Vec<String> {
    json_str
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|v| v.get("facts").and_then(|f| f.as_array()).cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|f| {
            f.as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
        })
        .collect()
}

pub fn parse_summary_details(summary: &str, json_str: Option<&str>) -> (String, Vec<String>) {
    parse_summary_details_inner(summary, json_str, false)
}

/// Digest 打包用：去掉摘要里误带的分数/热度话术，减轻成稿「工具味」。
pub fn parse_summary_details_for_digest(
    summary: &str,
    json_str: Option<&str>,
) -> (String, Vec<String>) {
    parse_summary_details_inner(summary, json_str, true)
}

fn parse_summary_details_inner(
    summary: &str,
    json_str: Option<&str>,
    for_digest: bool,
) -> (String, Vec<String>) {
    let (ol, kp) = if let Some(raw) = json_str.map(str::trim).filter(|s| !s.is_empty()) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(raw) {
            let ol = val
                .get("one_liner")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string);
            let kp = val
                .get("key_points")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|item| {
                            item.as_str()
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .map(ToString::to_string)
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let ol = ol.unwrap_or_else(|| one_liner(summary));
            (ol, kp)
        } else {
            (one_liner(summary), Vec::new())
        }
    } else {
        (one_liner(summary), Vec::new())
    };
    if for_digest {
        (
            sanitize_digest_blurb(&ol),
            kp.into_iter()
                .map(|p| sanitize_digest_blurb(&p))
                .filter(|p| !p.is_empty())
                .collect(),
        )
    } else {
        (ol, kp)
    }
}

/// 去掉卡片摘要里常见的 HN 分、社区热度等不宜出现在简报正文的片段。
/// 摘要可能是任一种输出语言写的，中英两组规则都跑；中文规则碰不到英文，反之亦然。
pub fn sanitize_digest_blurb(text: &str) -> String {
    use regex::Regex;
    let mut s = text.trim().to_string();
    if s.is_empty() {
        return s;
    }
    let patterns = [
        r"（\s*\d+\s*分\s*）",
        r"\(\s*\d+\s*分\s*\)",
        r"[，,]?\s*社区(?:讨论)?(?:热烈|关注度(?:中等|较高)?|高关注)[^，。；]*",
        r"[，,]?\s*社区有简短悼念",
        r"[，,]?\s*获得社区高关注[^，。；]*",
        r"[，,]?\s*有用户(?:在社区)?分享[^，。；]*",
        r"[，,]?\s*有人(?:在社区)?分享[^，。；]*",
        r"[，,]?\s*网友(?:表示|分享|热议)[^，。；]*",
        r"(?i)\s*\(\s*\d+\s*(?:points?|comments?)\s*\)",
    ];
    for pat in patterns {
        if let Ok(re) = Regex::new(pat) {
            s = re.replace_all(&s, "").to_string();
        }
    }
    s.trim()
        .trim_end_matches(['，', ',', '；', ';'])
        .to_string()
}

/// 成稿后兜底：删掉模型偶发的覆盖率空话；二手转述行只在没有实质信息时才删。
/// 宁可漏删也不误删——正文信息量优先。
/// 清洗正文后同步保留仍存在的列表项来源槽位。
/// 只允许把清洗明确删除的列表项从 refs 中删除；任何无法证明的一一对应都返回空来源，
/// 不让后续条目向前挪位。
pub fn sanitize_digest_body_with_item_refs(
    body: &str,
    item_refs: &[Vec<usize>],
) -> (String, Vec<Vec<usize>>) {
    let before = digest_list_item_texts(body);
    let cleaned = sanitize_digest_body(body);
    let after = digest_list_item_texts(&cleaned);
    if before.len() != item_refs.len() || after.len() > before.len() {
        return (cleaned, Vec::new());
    }
    if after == before {
        return (cleaned, item_refs.to_vec());
    }
    // 只删不改时，文本互不相同才能唯一确定每条留下的是原来哪一条。
    let mut distinct = std::collections::HashSet::new();
    if !before.iter().all(|text| distinct.insert(text)) {
        return (cleaned, Vec::new());
    }

    let mut cursor = 0usize;
    let mut refs = Vec::with_capacity(after.len());
    for text in after {
        let Some(relative) = before[cursor..]
            .iter()
            .position(|candidate| candidate == &text)
        else {
            return (cleaned, Vec::new());
        };
        let index = cursor + relative;
        refs.push(item_refs[index].clone());
        cursor = index + 1;
    }
    (cleaned, refs)
}

/// 成稿后兜底：删掉模型偶发的覆盖率空话；二手转述行只在没有实质信息时才删。
/// 宁可漏删也不误删——正文信息量优先。中英两组规则都跑，英文只收评测里见过的写法。
pub fn sanitize_digest_body(body: &str) -> String {
    use regex::Regex;
    let always_drop = [
        r"^[-*•·\s]*(?:今日|本周|本期)[^。]{0,24}(?:还涵盖|还包括|另有)",
        r"(?:等综合快讯|涉及多领域|涵盖多个领域)",
        r"^[-*•·\s]*此外还(?:有|涵盖|包括)",
        r"(?i)^[-*•·\s]*(?:today|this week|this issue)\b[^.]{0,40}\balso (?:covers|covered|includes|included)\b",
        r"(?i)\b(?:spanning|covering) (?:many|multiple|various) (?:fields|areas|topics)\b",
    ];
    let drop_if_thin = [
        r"^[-*•·\s]*(?:有用户|有人)(?:在社区)?分享",
        r"^[-*•·\s]*网友(?:表示|分享|热议)",
        r"^[-*•·\s]*社区(?:讨论|热议|关注)",
        r"(?i)^[-*•·\s]*(?:a user|users|some users|someone|netizens|commenters)\b[^.]{0,20}\b(?:shared|said|noted|reported)\b",
        r"(?i)^[-*•·\s]*(?:the )?community (?:discussion|reaction|buzz)\b",
    ];
    let compile =
        |pats: &[&str]| -> Vec<Regex> { pats.iter().filter_map(|p| Regex::new(p).ok()).collect() };
    let always = compile(&always_drop);
    let thin = compile(&drop_if_thin);

    let mut kept = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            kept.push(String::new());
            continue;
        }
        if always.iter().any(|re| re.is_match(trimmed)) {
            continue;
        }
        // 含粗体主体或数字即视为有实质信息，保留。
        let has_substance = trimmed.contains("**") || trimmed.chars().any(|c| c.is_ascii_digit());
        if !has_substance && thin.iter().any(|re| re.is_match(trimmed)) {
            continue;
        }
        kept.push(line.to_string());
    }
    let mut out = kept.join("\n");
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    drop_empty_sections(out.trim())
}

/// 过滤后可能留下空的 `###` 小节，一并去掉。
fn drop_empty_sections(body: &str) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut kept: Vec<String> = Vec::new();
    let mut idx = 0;
    while idx < lines.len() {
        let line = lines[idx];
        if line.trim_start().starts_with('#') {
            let has_content = lines[idx + 1..]
                .iter()
                .take_while(|l| !l.trim_start().starts_with('#'))
                .any(|l| !l.trim().is_empty());
            if !has_content {
                idx += 1;
                continue;
            }
        }
        kept.push(line.to_string());
        idx += 1;
    }
    let mut out = kept.join("\n");
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    out.trim().to_string()
}

pub fn format_digest_items(items: &[DigestItem]) -> String {
    let mut groups: Vec<(Option<i64>, Vec<&DigestItem>)> = Vec::new();
    let mut seen_clusters: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();

    for item in items {
        if let Some(cid) = item.cluster_id {
            if let Some(&idx) = seen_clusters.get(&cid) {
                groups[idx].1.push(item);
                continue;
            }
            seen_clusters.insert(cid, groups.len());
            groups.push((Some(cid), vec![item]));
        } else {
            groups.push((None, vec![item]));
        }
    }

    let mut out = Vec::new();
    for (_cid, group_items) in groups {
        if group_items.len() > 1 {
            let first = group_items[0];
            let mut group_text = format!(
                "## Topic: {} ({} reports)",
                first.title,
                group_items.len()
            );
            let mut points_set = std::collections::HashSet::new();
            let mut all_points = Vec::new();

            for it in &group_items {
                let (ol, kp) = parse_summary_details(&it.summary, it.summary_json.as_deref());
                group_text.push_str(&format!("\n- {} | {}", it.title, ol));
                for point in kp {
                    let norm = point.trim().to_lowercase();
                    if points_set.insert(norm) && all_points.len() < 5 {
                        all_points.push(point);
                    }
                }
            }
            if !all_points.is_empty() {
                group_text.push_str("\n  Key points:");
                for p in all_points {
                    group_text.push_str(&format!("\n  * {p}"));
                }
            }
            out.push(group_text);
        } else if let Some(it) = group_items.first() {
            let (ol, kp) = parse_summary_details(&it.summary, it.summary_json.as_deref());
            let mut line = format!("- {} | {}", it.title, ol);
            if !kp.is_empty() {
                for p in kp.into_iter().take(3) {
                    line.push_str(&format!("\n  * {p}"));
                }
            }
            out.push(line);
        }
    }

    out.join("\n\n")
}

pub fn format_digest_input(
    items: &[DigestItem],
    noisy_titles: &[String],
    pick_max: usize,
) -> String {
    let catalog = format_digest_catalog(items);
    if catalog.is_empty() {
        return catalog;
    }
    let target = pick_max.min(items.len());
    let mut packed = format!(
        "{} candidates; pick {}.\n\n{}",
        items.len(),
        target,
        catalog
    );
    let mut seen_noisy = std::collections::HashSet::new();
    let short_noisy: Vec<String> = noisy_titles
        .iter()
        .map(|title| crate::cluster_summary::short_source_name(title))
        .filter(|name| !name.is_empty() && seen_noisy.insert(name.clone()))
        .collect();
    if !short_noisy.is_empty() {
        packed.push_str(
            "\n\nNoisy sources (lower priority when picking; never name them in the digest):\n",
        );
        for title in short_noisy {
            packed.push_str("- ");
            packed.push_str(&title);
            packed.push('\n');
        }
    }
    packed
}

fn format_catalog_entry(it: &DigestItem) -> String {
    let (ol, kp) = parse_summary_details_for_digest(&it.summary, it.summary_json.as_deref());
    let source = crate::cluster_summary::short_source_name(&it.feed_title);
    let source_tag = if source.is_empty() {
        String::new()
    } else {
        format!(" [{source}]")
    };
    let blurb = ol.trim();
    let mut line = if blurb.is_empty() || blurb == it.title.trim() {
        format!("[id:{}] {}{source_tag}", it.article_id, it.title)
    } else {
        format!("[id:{}] {}{source_tag} | {blurb}", it.article_id, it.title)
    };
    for p in kp {
        line.push_str(&format!("\n  * {p}"));
    }
    line
}

/// 选题轮：按 cluster 分组目录，每篇带 `[id:N]` 与来源短名，不把标题复读成摘要。
pub fn format_digest_catalog(items: &[DigestItem]) -> String {
    let mut groups: Vec<(Option<i64>, Vec<&DigestItem>)> = Vec::new();
    let mut cluster_idx: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();

    for item in items {
        if let Some(cid) = item.cluster_id {
            if let Some(&idx) = cluster_idx.get(&cid) {
                groups[idx].1.push(item);
                continue;
            }
            cluster_idx.insert(cid, groups.len());
            groups.push((Some(cid), vec![item]));
        } else {
            groups.push((None, vec![item]));
        }
    }

    let mut out = Vec::new();
    for (_cid, group) in groups {
        if group.len() > 1 {
            let first = group[0];
            let mut block = format!("Topic: {} ({} articles)", first.title, group.len());
            for it in group {
                block.push('\n');
                block.push_str(&format_catalog_entry(it));
            }
            out.push(block);
        } else if let Some(it) = group.first() {
            out.push(format_catalog_entry(it));
        }
    }
    out.join("\n\n")
}

/// 成稿轮：正文摘录 + 已清洗的摘要与关键事实。按【事件】分组，组内用〈序号〉分篇。
#[derive(Debug, Clone)]
pub struct DigestDeepInput {
    pub prompt: String,
    /// 与 prompt 中〈序号〉完全同序的文章表；落库反查只能使用这张表。
    pub numbered_items: Vec<DigestItem>,
}

/// 按 id 读出文章的简报条目；已删除的文章略过，保持传入顺序。
pub fn digest_items_by_ids(conn: &Connection, ids: &[i64]) -> Result<Vec<DigestItem>> {
    let mut stmt = conn.prepare(
        "SELECT a.id, a.title, COALESCE(a.summary, ''), a.summary_json, a.cluster_id,
                COALESCE(f.title, ''), a.blocks_json, a.content_html
         FROM articles a LEFT JOIN feeds f ON f.id = a.feed_id WHERE a.id = ?1",
    )?;
    let mut out = Vec::new();
    for id in ids {
        let row = stmt
            .query_row([id], |row| {
                let summary: String = row.get(2)?;
                let summary_json: Option<String> = row.get(3)?;
                let blocks_json: Option<String> = row.get(6)?;
                let content_html: Option<String> = row.get(7)?;
                let effective_summary = if summary.trim().is_empty()
                    && summary_json
                        .as_deref()
                        .is_none_or(|raw| raw.trim().is_empty())
                {
                    crate::summary_extract::extract_lead_snippet(
                        blocks_json.as_deref(),
                        content_html.as_deref(),
                        180,
                    )
                } else {
                    summary
                };
                Ok(DigestItem {
                    article_id: row.get(0)?,
                    title: row.get(1)?,
                    summary: effective_summary,
                    summary_json,
                    cluster_id: row.get(4)?,
                    feed_title: row.get(5)?,
                })
            })
            .optional()?;
        out.extend(row);
    }
    Ok(out)
}

/// 一篇文章的一句话、要点与关键事实，已清洗分数 / 热度话术；不复读标题。
pub(crate) fn brief_lines(item: &DigestItem) -> String {
    let (ol, kp) = parse_summary_details_for_digest(&item.summary, item.summary_json.as_deref());
    let facts: Vec<String> = summary_facts(item.summary_json.as_deref())
        .into_iter()
        .map(|f| sanitize_digest_blurb(&f))
        .filter(|f| !f.is_empty() && !kp.contains(f))
        .collect();
    let mut block = String::new();
    if !ol.is_empty() && ol.trim() != item.title.trim() {
        block.push_str(&format!("\n{ol}\n"));
    }
    if !kp.is_empty() {
        for p in kp {
            block.push_str(&format!("\n- {p}"));
        }
        block.push('\n');
    }
    if !facts.is_empty() {
        block.push_str("\nKey facts:");
        for f in facts {
            block.push_str(&format!("\n- {f}"));
        }
        block.push('\n');
    }
    block
}

/// 当文章正文摘录已包含完整短文时，去掉与正文原句重复的一句话 / 要点 / 事实；无正式摘要时跳过重复导语。
fn brief_lines_for_deep(item: &DigestItem, excerpt: &str) -> String {
    let excerpt_trim = excerpt.trim();
    if item
        .summary_json
        .as_deref()
        .is_none_or(|raw| raw.trim().is_empty())
        && !excerpt_trim.is_empty()
    {
        let lead = item.summary.trim();
        let lead_prefix = lead.trim_end_matches(" …").trim_end_matches('…').trim();
        if lead.is_empty()
            || lead == "[No body text]"
            || lead == item.title.trim()
            || (!lead_prefix.is_empty() && excerpt_trim.contains(lead_prefix))
        {
            return String::new();
        }
    }
    if excerpt_trim.is_empty() || excerpt_trim.contains('…') {
        return brief_lines(item);
    }
    let norm_excerpt: String = excerpt_trim
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let repeats_excerpt = |s: &str| {
        let norm = s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        !norm.is_empty() && norm_excerpt.contains(&norm)
    };
    let (ol, kp) = parse_summary_details_for_digest(&item.summary, item.summary_json.as_deref());
    let kp: Vec<String> = kp.into_iter().filter(|p| !repeats_excerpt(p)).collect();
    let facts: Vec<String> = summary_facts(item.summary_json.as_deref())
        .into_iter()
        .map(|f| sanitize_digest_blurb(&f))
        .filter(|f| !f.is_empty() && !kp.contains(f) && !repeats_excerpt(f))
        .collect();
    let mut block = String::new();
    if !ol.is_empty() && ol.trim() != item.title.trim() && !repeats_excerpt(&ol) {
        block.push_str(&format!("\n{ol}\n"));
    }
    if !kp.is_empty() {
        for p in kp {
            block.push_str(&format!("\n- {p}"));
        }
        block.push('\n');
    }
    if !facts.is_empty() {
        block.push_str("\nKey facts:");
        for f in facts {
            block.push_str(&format!("\n- {f}"));
        }
        block.push('\n');
    }
    block
}

/// 余额回收（water-filling）：短文只占实际需要的预算，剩余空间回流给同批较长文章（不超过单篇上限）。
fn allocate_waterfill_budgets(demands: &[usize], total: usize, cap: usize) -> Vec<usize> {
    let n = demands.len();
    if n == 0 || total == 0 || cap == 0 {
        return vec![0; n];
    }
    let capped: Vec<usize> = demands.iter().map(|&d| d.min(cap)).collect();
    let mut alloc = vec![0usize; n];
    let mut remaining_budget = total;
    let mut active: Vec<usize> = (0..n).collect();
    while !active.is_empty() && remaining_budget > 0 {
        let share = remaining_budget / active.len();
        if share == 0 {
            for &idx in &active {
                if remaining_budget == 0 {
                    break;
                }
                if alloc[idx] < capped[idx] {
                    alloc[idx] += 1;
                    remaining_budget -= 1;
                }
            }
            break;
        }
        let mut next_active = Vec::new();
        let mut freed = false;
        for &idx in &active {
            let need = capped[idx].saturating_sub(alloc[idx]);
            if need <= share {
                alloc[idx] += need;
                remaining_budget = remaining_budget.saturating_sub(need);
                freed = true;
            } else {
                next_active.push(idx);
            }
        }
        if !freed {
            let per = remaining_budget / next_active.len();
            for &idx in &next_active {
                let take = per.min(capped[idx].saturating_sub(alloc[idx]));
                alloc[idx] += take;
                remaining_budget = remaining_budget.saturating_sub(take);
            }
            break;
        }
        active = next_active;
    }
    alloc
}

fn format_article_clock(ms: Option<i64>) -> String {
    ms.and_then(chrono::DateTime::from_timestamp_millis)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// 精简版来源块：用于满载周报超出总预算时压缩「Source articles」，只留标题与去重后的关键事实。
fn compact_source_block(n: usize, item: &DigestItem) -> String {
    let facts: Vec<String> = summary_facts(item.summary_json.as_deref())
        .into_iter()
        .map(|f| sanitize_digest_blurb(&f))
        .filter(|f| !f.is_empty())
        .collect();
    let mut block = format!("〈{n}〉{}", item.title);
    if !facts.is_empty() {
        for f in facts.into_iter().take(3) {
            block.push_str(&format!("\n- {f}"));
        }
    }
    block
}

/// 周报成稿输入：本周日报条目（末尾标来源序号）、来源文章的摘要与关键事实、日报未收的补充文章（带正文摘录）。
/// 没有日报时只有补充文章。编号表先列日报来源、再列补充文章，与输入中的〈序号〉同序。
/// 总量按 `InputTier` 约束：满载一周日报时自动压缩来源摘要并按余额回收分配补充正文预算。
pub fn format_weekly_input(
    conn: &Connection,
    daily: &[crate::digest_sources::DailyDigestLine],
    supplements: &[DigestItem],
    tier: crate::summary_extract::InputTier,
) -> Result<DigestDeepInput> {
    let mut numbered_items: Vec<DigestItem> = Vec::new();
    let mut number: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
    let mut ids: Vec<i64> = Vec::new();
    for line in daily {
        for id in &line.article_ids {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }
    }
    for item in digest_items_by_ids(conn, &ids)? {
        let id = item.article_id;
        numbered_items.push(item);
        number.insert(id, numbered_items.len());
    }

    let mut daily_lines = Vec::new();
    for line in daily {
        let refs: Vec<String> = line
            .article_ids
            .iter()
            .filter_map(|id| number.get(id))
            .map(|n| format!("〈{n}〉"))
            .collect();
        if refs.is_empty() {
            continue;
        }
        let section = if line.section.is_empty() {
            String::new()
        } else {
            format!(" · {}", line.section)
        };
        daily_lines.push(format!(
            "【{}{section}】{} {}",
            line.day,
            line.text,
            refs.join("")
        ));
    }

    // 补充文章的摘录最多占档位总量的三分之一（有日报时），用余额回收分配给较长文章；总预算受档位约束。
    let weekly_total_budget = tier.digest_total_chars();
    let share = if daily_lines.is_empty() {
        tier.digest_total_chars()
    } else {
        tier.digest_total_chars() / 3
    };
    let mut extra_rows = Vec::new();
    for item in supplements {
        if number.contains_key(&item.article_id) {
            continue;
        }
        numbered_items.push(item.clone());
        let n = numbered_items.len();
        number.insert(item.article_id, n);
        let (title, blocks_json, content_html, published_at) =
            article_digest_source(conn, item.article_id)?;
        let demand = crate::summary_extract::body_weighted_len(
            blocks_json.as_deref(),
            content_html.as_deref(),
        );
        extra_rows.push((n, item, title, blocks_json, content_html, published_at, demand));
    }
    let demands: Vec<usize> = extra_rows.iter().map(|r| r.6).collect();
    let budgets = allocate_waterfill_budgets(&demands, share, tier.digest_item_chars());
    let mut extra = Vec::new();
    for ((n, item, title, blocks_json, content_html, published_at, _), item_budget) in
        extra_rows.into_iter().zip(budgets)
    {
        let excerpt = crate::summary_extract::extract_body_excerpt(
            blocks_json.as_deref(),
            content_html.as_deref(),
            item_budget,
        );
        let clock = format_article_clock(published_at);
        let header = if clock.is_empty() {
            format!("〈{n}〉{title}")
        } else {
            format!("〈{n}〉{title} ({clock})")
        };
        let mut block = format!("{header}\n{}", brief_lines_for_deep(item, &excerpt));
        if !excerpt.trim().is_empty() {
            block.push_str(&format!("\n{excerpt}"));
        }
        extra.push(block.trim_end().to_string());
    }

    let mut parts = Vec::new();
    if !daily_lines.is_empty() {
        let daily_section = format!("## Daily digest items\n\n{}", daily_lines.join("\n"));
        let daily_units = crate::summary_extract::weighted_len(&daily_section);
        let extra_units: usize = extra
            .iter()
            .map(|e| crate::summary_extract::weighted_len(e))
            .sum();
        let source_budget = weekly_total_budget
            .saturating_sub(daily_units)
            .saturating_sub(extra_units);

        let daily_source_count = numbered_items.len() - extra.len();
        let full_sources: Vec<String> = numbered_items[..daily_source_count]
            .iter()
            .enumerate()
            .map(|(i, item)| {
                format!("〈{}〉{}\n{}", i + 1, item.title, brief_lines(item))
                    .trim_end()
                    .to_string()
            })
            .collect();
        let full_units: usize = full_sources
            .iter()
            .map(|s| crate::summary_extract::weighted_len(s) + 2)
            .sum();
        let sources = if full_units <= source_budget {
            full_sources
        } else {
            let compact: Vec<String> = numbered_items[..daily_source_count]
                .iter()
                .enumerate()
                .map(|(i, item)| compact_source_block(i + 1, item))
                .collect();
            let compact_units: usize = compact
                .iter()
                .map(|s| crate::summary_extract::weighted_len(s) + 2)
                .sum();
            if compact_units <= source_budget {
                compact
            } else {
                // 极端满载：只保留编号与标题，保证每个来源号都能对上而不撑爆上下文。
                numbered_items[..daily_source_count]
                    .iter()
                    .enumerate()
                    .map(|(i, item)| format!("〈{}〉{}", i + 1, item.title))
                    .collect()
            }
        };
        parts.push(daily_section);
        parts.push(format!("## Source articles\n\n{}", sources.join("\n\n")));
    }

    if !extra.is_empty() {
        let heading = if daily_lines.is_empty() {
            "## Supplementary articles"
        } else {
            "## Supplementary articles (not in the daily digests)"
        };
        parts.push(format!("{heading}\n\n{}", extra.join("\n\n---\n\n")));
    }
    if daily_lines.is_empty() && extra.is_empty() {
        return Ok(DigestDeepInput {
            prompt: String::new(),
            numbered_items: Vec::new(),
        });
    }
    Ok(DigestDeepInput {
        prompt: format!(
            "Daily digest items this week: {}; supplementary articles: {}. Cite with the 〈n〉 numbers.\n\n{}",
            daily_lines.len(),
            extra.len(),
            parts.join("\n\n")
        ),
        numbered_items,
    })
}

/// 摘录预算按输入档位与余额回收分配：短文用完即止，余量回流给同批长文（单篇不超过档位上限）。
pub fn format_digest_deep_input(
    conn: &Connection,
    items: &[DigestItem],
    tier: crate::summary_extract::InputTier,
) -> Result<DigestDeepInput> {
    let mut groups: Vec<Vec<&DigestItem>> = Vec::new();
    let mut cluster_idx: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
    for item in items {
        if let Some(cid) = item.cluster_id {
            if let Some(&idx) = cluster_idx.get(&cid) {
                groups[idx].push(item);
                continue;
            }
            cluster_idx.insert(cid, groups.len());
            groups.push(vec![item]);
        } else {
            groups.push(vec![item]);
        }
    }

    let mut ordered: Vec<(usize, &DigestItem, String, Option<String>, Option<String>, Option<i64>, usize)> =
        Vec::with_capacity(items.len());
    for (group_idx, group) in groups.iter().enumerate() {
        for item in group {
            let (title, blocks_json, content_html, published_at) =
                article_digest_source(conn, item.article_id)?;
            let demand = crate::summary_extract::body_weighted_len(
                blocks_json.as_deref(),
                content_html.as_deref(),
            );
            ordered.push((
                group_idx,
                *item,
                title,
                blocks_json,
                content_html,
                published_at,
                demand,
            ));
        }
    }
    let demands: Vec<usize> = ordered.iter().map(|r| r.6).collect();
    let budgets =
        allocate_waterfill_budgets(&demands, tier.digest_total_chars(), tier.digest_item_chars());

    let mut global_idx = 0usize;
    let mut numbered_items = Vec::with_capacity(ordered.len());
    let mut group_parts: Vec<Vec<String>> = groups
        .iter()
        .enumerate()
        .map(|(i, group)| {
            let n = group.len();
            let header = if n > 1 {
                format!("【Event {}】{} articles", i + 1, n)
            } else {
                format!("【Event {}】", i + 1)
            };
            vec![header]
        })
        .collect();

    for ((group_idx, item, title, blocks_json, content_html, published_at, _), item_budget) in
        ordered.into_iter().zip(budgets)
    {
        global_idx += 1;
        numbered_items.push(item.clone());
        let excerpt = crate::summary_extract::extract_body_excerpt(
            blocks_json.as_deref(),
            content_html.as_deref(),
            item_budget,
        );
        let clock = format_article_clock(published_at);
        let header = if clock.is_empty() {
            format!("〈{global_idx}〉{title}")
        } else {
            format!("〈{global_idx}〉{title} ({clock})")
        };
        let mut block = format!("{header}\n{}", brief_lines_for_deep(item, &excerpt));
        if !excerpt.trim().is_empty() {
            block.push_str(&format!("\n{excerpt}"));
        }
        group_parts[group_idx].push(block.trim_end().to_string());
    }

    let event_blocks: Vec<String> = group_parts
        .into_iter()
        .map(|parts| parts.join("\n\n---\n\n"))
        .collect();

    if event_blocks.is_empty() {
        return Ok(DigestDeepInput {
            prompt: String::new(),
            numbered_items,
        });
    }
    Ok(DigestDeepInput {
        prompt: format!(
            "This issue: {} events ({} articles in all). Cite with the global 〈n〉 numbers.\n\n{}",
            event_blocks.len(),
            numbered_items.len(),
            event_blocks.join("\n\n")
        ),
        numbered_items,
    })
}

pub fn parse_digest_picks(value: &serde_json::Value) -> Vec<i64> {
    value
        .get("picks")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|p| p.get("article_id").and_then(|id| id.as_i64()))
                .collect()
        })
        .unwrap_or_default()
}

/// 校验模型返回的 id；不足 `max` 时不补满，仅 0 篇时应急取 `pick_min`。
pub fn apply_digest_picks(
    items: &[DigestItem],
    pick_ids: &[i64],
    max: usize,
    pick_min: usize,
) -> Vec<DigestItem> {
    let allowed: std::collections::HashSet<i64> = items.iter().map(|i| i.article_id).collect();
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let cap = max.min(items.len());
    for id in pick_ids {
        if !allowed.contains(id) || !seen.insert(*id) {
            continue;
        }
        if let Some(it) = items.iter().find(|i| i.article_id == *id) {
            out.push(it.clone());
        }
        if out.len() >= cap {
            return out;
        }
    }
    if out.is_empty() {
        let emergency = pick_min.min(items.len());
        for it in items.iter().take(emergency) {
            out.push(it.clone());
        }
    }
    out
}

fn article_digest_source(
    conn: &Connection,
    article_id: i64,
) -> Result<(String, Option<String>, Option<String>, Option<i64>)> {
    conn.query_row(
        "SELECT title, blocks_json, content_html, published_at FROM articles WHERE id = ?1",
        [article_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )
    .map_err(Into::into)
}

pub fn one_liner(summary: &str) -> String {
    let trimmed = summary.trim();
    let end = trimmed
        .char_indices()
        .find(|(_, c)| matches!(c, '。' | '！' | '？' | '.' | '!' | '?'))
        .map(|(i, c)| i + c.len_utf8());
    let sentence = end.map(|i| &trimmed[..i]).unwrap_or(trimmed);
    truncate_chars(sentence.trim(), 80)
}

fn map_digest(row: &rusqlite::Row<'_>) -> rusqlite::Result<DigestRow> {
    Ok(DigestRow {
        id: row.get(0)?,
        period: row.get(1)?,
        period_start: row.get(2)?,
        body: row.get(3)?,
        items_json: row.get(4)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{PooledConnection, open_at};
    use tempfile::tempdir;

    fn open_temp() -> (tempfile::TempDir, PooledConnection) {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        (dir, conn)
    }

    #[test]
    fn digest_generate_waits_until_the_configured_local_clock() {
        use chrono::TimeZone;
        let monday = chrono::Local
            .with_ymd_and_hms(2026, 9, 28, 0, 0, 0)
            .single()
            .expect("monday midnight");
        let start = monday.timestamp_millis();
        let before = chrono::Local
            .with_ymd_and_hms(2026, 9, 28, 7, 59, 0)
            .single()
            .expect("before eight")
            .timestamp_millis();
        let at_eight = chrono::Local
            .with_ymd_and_hms(2026, 9, 28, 8, 0, 0)
            .single()
            .expect("eight")
            .timestamp_millis();
        assert!(!digest_generate_due(start, before, 8 * 60));
        assert!(digest_generate_due(start, at_eight, 8 * 60));
        assert!(digest_generate_due(start, before, 0));
        let tuesday_morning = chrono::Local
            .with_ymd_and_hms(2026, 9, 29, 7, 0, 0)
            .single()
            .expect("tuesday")
            .timestamp_millis();
        assert!(digest_generate_due(start, tuesday_morning, 8 * 60));
    }

    #[test]
    fn digest_generate_minutes_default_to_eight_and_round_trip() {
        let (_dir, conn) = open_temp();
        assert_eq!(digest_generate_minutes(&conn).unwrap(), 8 * 60);
        set_digest_generate_minutes(&conn, 7 * 60 + 30).unwrap();
        assert_eq!(digest_generate_minutes(&conn).unwrap(), 7 * 60 + 30);
        set_digest_generate_minutes(&conn, 24 * 60).unwrap();
        assert_eq!(digest_generate_minutes(&conn).unwrap(), 24 * 60 - 1);
        set_digest_generate_minutes(&conn, -5).unwrap();
        assert_eq!(digest_generate_minutes(&conn).unwrap(), 0);
        set_setting(&conn, DIGEST_GENERATE_MINUTES_KEY, "nope").unwrap();
        assert_eq!(digest_generate_minutes(&conn).unwrap(), 8 * 60);
        assert_eq!(parse_digest_generate_minutes(" 480 "), Some(480));
        assert_eq!(parse_digest_generate_minutes("1440"), None);
    }

    #[test]
    fn digest_date_label_pins_local_calendar() {
        use chrono::TimeZone;
        let daily = chrono::Local
            .with_ymd_and_hms(2026, 8, 27, 0, 0, 0)
            .single()
            .expect("daily midnight");
        assert_eq!(
            digest_date_label("daily", daily.timestamp_millis()),
            "2026-08-27"
        );
        let weekly = chrono::Local
            .with_ymd_and_hms(2026, 8, 25, 0, 0, 0)
            .single()
            .expect("week start");
        assert_eq!(
            digest_date_label("weekly", weekly.timestamp_millis()),
            "2026-08-25 – 2026-08-31"
        );
    }

    #[test]
    fn insert_digest_overwrites_same_period_start() {
        let (_dir, conn) = open_temp();
        let start = 1_700_000_000_000;
        let items = r#"[{"article_id":1,"title":"A","feed_title":"HN"}]"#;
        let id1 = insert_digest(&conn, "daily", start, "first", "[]").unwrap();
        let id2 = insert_digest(&conn, "daily", start, "second", items).unwrap();
        assert_eq!(id1, id2);
        let daily = list_digests(&conn, "daily", 60).unwrap();
        assert_eq!(daily.len(), 1);
        assert_eq!(daily[0].body, "second");
        assert_eq!(daily[0].items_json, items);
        insert_digest(&conn, "weekly", start, "week", "[]").unwrap();
        assert!(
            list_digests(&conn, "daily", 60)
                .unwrap()
                .iter()
                .all(|d| d.period == "daily")
        );
        assert_eq!(list_digests(&conn, "weekly", 60).unwrap()[0].body, "week");
    }

    #[test]
    fn digest_pack_uses_one_liner_not_full_html() {
        let (_dir, conn) = open_temp();
        assert!(digest_pack_input(&conn, 10).unwrap().is_empty());
        insert_digest(
            &conn,
            "daily",
            period_start_ms("daily"),
            "hello digest",
            "[]",
        )
        .unwrap();
        let d = latest_digest(&conn, "daily").unwrap().unwrap();
        assert_eq!(d.body, "hello digest");
        assert_eq!(one_liner("第一句。后面还有很多字。"), "第一句。");
    }

    #[test]
    fn digest_candidates_follow_period_window() {
        let (_dir, conn) = open_temp();
        let now = 1_900_000_000_000_i64;
        let today = now - 8 * HOUR_MS;
        conn.execute(
            "INSERT INTO feeds (id, url, title, kind, created_at) VALUES (1, 'https://ex.test/f', 'f', 'rss', 1)",
            [],
        )
        .unwrap();
        // (id, 入库几小时前, 已读, 分数)
        let rows = [
            (1, 2, 0, 5),   // 今天未读
            (2, 30, 0, 5),  // 昨天傍晚未读：日报窗口内
            (3, 40, 0, 5),  // 超出日报 36 小时
            (4, 3, 1, 5),   // 今天已读
            (5, 20, 0, 5),  // 昨天未读，但已进上一期日报
            (6, 100, 1, 8), // 本周已读
            (7, 120, 1, 6), // 本周已读
            (8, 130, 1, 4), // 本周已读
            (9, 200, 0, 5), // 超出周报 7 天
        ];
        for (id, hours, read, score) in rows {
            conn.execute(
                "INSERT INTO articles (id, feed_id, title, hash, created_at, published_at, read, score, summary)
                 VALUES (?1, 1, ?2, ?3, ?4, ?4, ?5, ?6, '摘要。')",
                rusqlite::params![id, format!("t{id}"), format!("h{id}"), now - hours * HOUR_MS, read, score],
            )
            .unwrap();
        }
        // 旧文章刚入库（新订阅回填）：发布时间太早，不算。
        conn.execute(
            "INSERT INTO articles (id, feed_id, title, hash, created_at, published_at, score, summary)
             VALUES (10, 1, 't10', 'h10', ?1, ?2, 5, '摘要。')",
            rusqlite::params![now - HOUR_MS, now - 30 * DAY_MS],
        )
        .unwrap();
        insert_digest(
            &conn,
            "daily",
            today - DAY_MS,
            "昨天",
            r#"{"v":2,"sources":[{"article_id":5}]}"#,
        )
        .unwrap();

        let ids = |period: &str, limit: i64| -> Vec<i64> {
            let window = DigestWindow::for_period(period, now, today, limit);
            let mut ids: Vec<i64> = digest_candidates(&conn, limit, &window)
                .unwrap()
                .iter()
                .map(|i| i.article_id)
                .collect();
            ids.sort_unstable();
            ids
        };
        assert_eq!(ids("daily", 40), vec![1, 2]);
        // 周报：未读 + 已读，已读最多四分之一（limit 8 → 2 篇，按分数取 6、7）；上一期日报用过的不排除。
        assert_eq!(ids("weekly", 8), vec![1, 2, 3, 5, 6, 7]);
        // 重新生成当天日报时，不排除当天这期自己的来源。
        insert_digest(
            &conn,
            "daily",
            today,
            "今天",
            r#"{"v":2,"sources":[{"article_id":1}]}"#,
        )
        .unwrap();
        assert_eq!(ids("daily", 40), vec![1, 2]);
    }

    #[test]
    fn weekly_input_uses_daily_lines_sources_and_supplements() {
        use crate::digest_sources::{prepare_digest_draft, week_daily_lines};
        use crate::summary_extract::InputTier;
        let (_dir, conn) = open_temp();
        conn.execute(
            "INSERT INTO feeds (id, url, title, kind, created_at) VALUES (1, 'https://ex.test/f', 'F', 'rss', 1)",
            [],
        )
        .unwrap();
        for (id, title) in [
            (1, "芯片发布"),
            (2, "芯片跑分"),
            (3, "监管新规"),
            (9, "补充长文"),
        ] {
            conn.execute(
                "INSERT INTO articles (id, feed_id, title, hash, created_at, summary, summary_json, content_html)
                 VALUES (?1, 1, ?2, ?3, 1, '摘要。', ?4, '<p>补充正文，写着 42 项改进。</p>')",
                rusqlite::params![
                    id,
                    title,
                    format!("h{id}"),
                    format!(r#"{{"summary":"s","one_liner":"{title}一句话","key_points":[],"facts":["{title}事实"]}}"#)
                ],
            )
            .unwrap();
        }
        let day = |n: i64| 1_900_000_000_000 + n * DAY_MS;
        let write = |start: i64, sections: serde_json::Value, numbered: &[DigestItem]| {
            let draft =
                prepare_digest_draft(
                    &serde_json::json!({ "sections": sections }),
                    numbered,
                    crate::ai_store::OutputLanguage::ZhCn,
                )
                .unwrap();
            insert_digest(&conn, "daily", start, &draft.body, &draft.items_json).unwrap();
        };
        let items = digest_items_by_ids(&conn, &[1, 2, 3]).unwrap();
        write(
            day(1),
            serde_json::json!([{"title": "硬件", "items": [{"text": "**Acme** 发布 X2 芯片", "refs": [1]}]}]),
            &items[..1],
        );
        write(
            day(2),
            serde_json::json!([
                {"title": "硬件", "items": [{"text": "X2 跑分出炉", "refs": [1]}]},
                {"title": "政策", "items": [{"text": "监管新规征求意见", "refs": [2]}]}
            ]),
            &items[1..],
        );
        // 范围外的一期不算。
        write(
            day(-5),
            serde_json::json!([{"title": "旧", "items": [{"text": "旧闻", "refs": [1]}]}]),
            &items[..1],
        );

        let daily = week_daily_lines(&conn, day(0), day(3)).unwrap();
        assert_eq!(daily.len(), 3);
        assert_eq!(daily[0].text, "**Acme** 发布 X2 芯片");
        assert_eq!(daily[0].section, "硬件");
        assert_eq!(daily[2].article_ids, vec![3]);

        let supplements = digest_items_by_ids(&conn, &[9]).unwrap();
        let input = format_weekly_input(&conn, &daily, &supplements, InputTier::Medium).unwrap();
        let ids: Vec<i64> = input.numbered_items.iter().map(|i| i.article_id).collect();
        assert_eq!(ids, vec![1, 2, 3, 9]);
        let prompt = &input.prompt;
        assert!(
            prompt.starts_with("Daily digest items this week: 3; supplementary articles: 1."),
            "{prompt}"
        );
        assert!(prompt.contains("】**Acme** 发布 X2 芯片 〈1〉"), "{prompt}");
        assert!(
            prompt.contains("· 政策】监管新规征求意见 〈3〉"),
            "{prompt}"
        );
        assert!(
            prompt.contains("## Source articles\n\n〈1〉芯片发布\n\n芯片发布一句话"),
            "{prompt}"
        );
        assert!(
            prompt.contains("## Supplementary articles (not in the daily digests)\n\n〈4〉补充长文"),
            "{prompt}"
        );
        assert!(prompt.contains("42 项改进"), "{prompt}");
        assert!(!prompt.contains("旧闻"));

        // 没有日报时只有补充文章。
        let only = format_weekly_input(&conn, &[], &supplements, InputTier::Medium).unwrap();
        assert!(
            only.prompt.contains("## Supplementary articles\n\n〈1〉补充长文"),
            "{}",
            only.prompt
        );
        assert!(
            format_weekly_input(&conn, &[], &[], InputTier::Low)
                .unwrap()
                .prompt
                .is_empty()
        );
    }

    #[test]
    fn deep_input_budget_follows_tier_and_lists_facts() {
        use crate::summary_extract::InputTier;
        let (_dir, conn) = open_temp();
        conn.execute(
            "INSERT INTO feeds (id, url, title, kind, created_at) VALUES (1, 'https://ex.test/f', 'f', 'rss', 1)",
            [],
        )
        .unwrap();
        let body: String = (0..60)
            .map(|i| format!("<p>第 {i} 段。{}</p>", "正文内容".repeat(40)))
            .collect();
        conn.execute(
            "INSERT INTO articles (id, feed_id, title, hash, created_at, content_html) VALUES (1, 1, '长文', 'h', 1, ?1)",
            [&body],
        )
        .unwrap();
        let mut item = digest_item(1, "长文", "F");
        item.summary_json = Some(
            r#"{"summary":"s","one_liner":"一句话","key_points":["要点"],"facts":["售价 499 美元","要点"]}"#.into(),
        );
        let items = vec![item];
        let low = format_digest_deep_input(&conn, &items, InputTier::Low)
            .unwrap()
            .prompt;
        let high = format_digest_deep_input(&conn, &items, InputTier::High)
            .unwrap()
            .prompt;
        assert!(low.chars().count() < 1_800, "{}", low.chars().count());
        assert!(high.chars().count() > 5_000);
        assert!(low.contains("Key facts:\n- 售价 499 美元"));
        assert_eq!(
            low.matches("- 要点").count(),
            1,
            "facts repeating a key point are dropped"
        );
    }

    #[test]
    fn digest_catalog_lists_article_ids() {
        let items = vec![DigestItem {
            article_id: 42,
            title: "Rust 1.90".into(),
            summary: "摘要".into(),
            summary_json: Some(
                r#"{"summary":"x","one_liner":"稳定版发布","key_points":["新语法"]}"#.into(),
            ),
            cluster_id: Some(9),
            feed_title: "HN".into(),
        }];
        let packed = format_digest_catalog(&items);
        assert!(packed.contains("[id:42]"));
        assert!(packed.contains("稳定版发布"));
        assert!(!packed.contains("cluster"));
    }

    #[test]
    fn sanitize_digest_blurb_strips_scores_and_heat_phrases() {
        assert_eq!(
            sanitize_digest_blurb("先进核燃料量产，社区讨论热烈（137 分）。"),
            "先进核燃料量产。"
        );
        assert_eq!(
            sanitize_digest_blurb("Pnpm 12.0 大版本，社区关注度中等（24 分）"),
            "Pnpm 12.0 大版本"
        );
        assert_eq!(
            sanitize_digest_blurb("空投机制变化，有用户在社区分享早年参与经历。"),
            "空投机制变化。"
        );
        assert_eq!(
            parse_summary_details_for_digest(
                "x",
                Some(
                    r#"{"one_liner":"离线地图获高关注（228 分）","key_points":["社区有简短悼念"]}"#
                ),
            )
            .0,
            "离线地图获高关注"
        );
        let (_, kp) = parse_summary_details_for_digest(
            "x",
            Some(r#"{"one_liner":"标题","key_points":["要点，社区讨论热烈"]}"#),
        );
        assert_eq!(kp, vec!["要点"]);
    }

    #[test]
    fn sanitize_digest_body_drops_filler_lines() {
        let raw = "### 科技\n\n- **Rust 1.90** 稳定版发布，引入新语法。\n- 有用户在社区分享空投经历。\n- 今日资讯还涵盖派早报等综合快讯，涉及多领域技术产品与社区动态。\n";
        let cleaned = sanitize_digest_body(raw);
        assert!(cleaned.contains("Rust 1.90"));
        assert!(!cleaned.contains("有用户在社区"));
        assert!(!cleaned.contains("派早报"));
        assert!(!cleaned.contains("涉及多领域"));
    }

    #[test]
    fn english_output_gets_the_same_cleanup() {
        assert_eq!(
            sanitize_digest_blurb("Rust 1.90 ships a new borrow checker (412 points)"),
            "Rust 1.90 ships a new borrow checker"
        );
        let raw = "### Tech\n\n- **Rust 1.90** ships with a new borrow checker.\n- Users shared stories about the airdrop.\n- Today's digest also covers roundups spanning many fields.\n";
        let cleaned = sanitize_digest_body(raw);
        assert!(cleaned.contains("**Rust 1.90**"));
        assert!(!cleaned.contains("Users shared"));
        assert!(!cleaned.contains("also covers"));
        // 有粗体主体或数字的转述行留着。
        let kept = sanitize_digest_body("### Security\n\n- A user shared packet captures from the **Cloudflare** outage.\n");
        assert!(kept.contains("Cloudflare"));
    }

    #[test]
    fn sanitize_digest_body_keeps_lines_with_substance() {
        let raw = "### 安全\n\n- 网友分享了 **Cloudflare** 宕机期间的抓包数据，指向 BGP 撤销。\n";
        let cleaned = sanitize_digest_body(raw);
        assert!(cleaned.contains("Cloudflare"));
        assert!(cleaned.contains("### 安全"));
    }

    #[test]
    fn sanitize_digest_body_removes_sections_left_empty() {
        let raw = "### 科技\n\n- **Rust 1.90** 发布。\n\n### 社区\n\n- 网友热议某话题。\n";
        let cleaned = sanitize_digest_body(raw);
        assert!(cleaned.contains("Rust 1.90"));
        assert!(!cleaned.contains("### 社区"));
    }

    #[test]
    fn digest_input_states_pick_target() {
        let items = vec![
            DigestItem {
                article_id: 1,
                title: "a".into(),
                summary: "s".into(),
                summary_json: None,
                cluster_id: None,
                feed_title: String::new(),
            },
            DigestItem {
                article_id: 2,
                title: "b".into(),
                summary: "s".into(),
                summary_json: None,
                cluster_id: None,
                feed_title: String::new(),
            },
        ];
        let packed = format_digest_input(&items, &[], 5);
        assert!(packed.contains("2 candidates; pick 2."));
        assert!(format_digest_input(&[], &[], 5).is_empty());
    }

    #[test]
    fn apply_digest_picks_validates_without_fill() {
        let items = vec![
            DigestItem {
                article_id: 1,
                title: "a".into(),
                summary: "s".into(),
                summary_json: None,
                cluster_id: None,
                feed_title: String::new(),
            },
            DigestItem {
                article_id: 2,
                title: "b".into(),
                summary: "s".into(),
                summary_json: None,
                cluster_id: None,
                feed_title: String::new(),
            },
        ];
        let picked = apply_digest_picks(&items, &[99, 2], 2, 6);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].article_id, 2);
        let emergency = apply_digest_picks(&items, &[], 2, 6);
        assert_eq!(emergency.len(), 2);
    }

    #[test]
    fn parse_digest_picks_reads_article_ids() {
        let v = serde_json::json!({
            "picks": [
                {"article_id": 3, "reason": "重要"},
                {"article_id": 7, "reason": "趋势"}
            ]
        });
        assert_eq!(parse_digest_picks(&v), vec![3, 7]);
    }

    #[test]
    fn serialize_and_parse_digest_items_json() {
        let items = vec![DigestItem {
            article_id: 42,
            title: "Amazon 收购 DuckLabs".into(),
            summary: String::new(),
            summary_json: None,
            cluster_id: None,
            feed_title: "HN".into(),
        }];
        let json = serialize_digest_items(&items);
        assert!(json.contains("Amazon"));
        let parsed = parse_digest_items_json(&json);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].len(), 1);
        assert_eq!(parsed[0][0].article_id, 42);
        assert_eq!(parsed[0][0].feed_title, "HN");
        assert!(parse_digest_items_json("").is_empty());
        assert!(parse_digest_items_json("not json").is_empty());
        assert_eq!(parse_digest_items_json(r#"[{"title":"x"}]"#), vec![vec![]]);
    }

    fn digest_item(id: i64, title: &str, feed: &str) -> DigestItem {
        DigestItem {
            article_id: id,
            title: title.into(),
            summary: String::new(),
            summary_json: None,
            cluster_id: None,
            feed_title: feed.into(),
        }
    }

    fn align_with_stored(body: &str, stored: &[Vec<DigestSource>]) -> Vec<Vec<DigestSource>> {
        let raw = serde_json::to_string(
            &stored
                .iter()
                .map(|line| serde_json::json!({ "sources": line }))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        align_digest_sources(body, &raw)
    }

    #[test]
    fn align_digest_sources_does_not_fill_empty_snapshot_from_extra() {
        let body = "- 历史期第一条\n";
        let items = align_digest_sources(body, "[]");
        assert_eq!(items, vec![Vec::<DigestSource>::new()]);
    }

    #[test]
    fn align_digest_sources_does_not_align_when_slot_count_differs() {
        let stored = vec![vec![DigestSource {
            article_id: 1,
            title: "历史第一条".into(),
            feed_title: "旧源".into(),
        }]];
        let body = "- 历史第一条\n- 历史第二条\n";
        let items = align_with_stored(body, &stored);
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(Vec::is_empty));
    }

    #[test]
    fn align_digest_sources_keeps_snapshot_slots_without_title_rematch() {
        let stored = vec![
            vec![DigestSource {
                article_id: 1,
                title: "English headline".into(),
                feed_title: "旧源".into(),
            }],
            vec![],
        ];
        let body = "- 中文改写后的正文\n- 没有来源的正文\n";
        let items = align_with_stored(body, &stored);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0][0].article_id, 1);
        assert!(items[1].is_empty());
    }

    #[test]
    fn align_digest_sources_preserves_bad_middle_slot_position() {
        let stored = vec![
            vec![DigestSource {
                article_id: 1,
                title: "第一条".into(),
                feed_title: "源一".into(),
            }],
            vec![],
            vec![DigestSource {
                article_id: 3,
                title: "第三条".into(),
                feed_title: "源三".into(),
            }],
        ];
        let body = "- 第一条\n- 第二条\n- 第三条\n";
        let items = align_with_stored(body, &stored);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0][0].article_id, 1);
        assert!(items[1].is_empty());
        assert_eq!(items[2][0].article_id, 3);
    }

    #[test]
    fn align_digest_sources_does_not_recover_dropped_title_from_library() {
        let stored = vec![vec![DigestSource {
            article_id: 1,
            title: "Stripe acquires Clerky".into(),
            feed_title: "HN".into(),
        }]];
        let body = "- **Stripe** 收购 Clerky。\n- 喜马拉雅跨界流域的冰川湖溃决洪水风险研究发表。\n";
        let items = align_with_stored(body, &stored);
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(Vec::is_empty));
    }

    #[test]
    fn align_digest_sources_keeps_stored_order_instead_of_rematching_titles() {
        let stored = vec![
            vec![DigestSource {
                article_id: 1,
                title: "Show HN: a tracking site".into(),
                feed_title: "Hacker News: Front Page".into(),
            }],
            vec![DigestSource {
                article_id: 2,
                title: "GLM-5.3-Flash performance".into(),
                feed_title: "智谱".into(),
            }],
        ];
        let body = "- **智谱** 发布了 GLM-5.3-Flash。\n- Show HN: a tracking site 上线。\n";
        let items = align_with_stored(body, &stored);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0][0].article_id, 1);
        assert_eq!(items[1][0].article_id, 2);
    }

    #[test]
    fn align_digest_sources_ignores_library_when_stored_empty() {
        let body = "- **Stripe** 收购 Clerky。\n";
        let items = align_digest_sources(body, "[]");
        assert_eq!(items, vec![Vec::<DigestSource>::new()]);
    }

    #[test]
    fn parse_digest_items_json_preserves_invalid_slot_position() {
        let raw = r#"[
            {"sources":[{"article_id":1,"title":"A","feed_title":"F1"}]},
            {"unexpected":"bad slot"},
            {"sources":[{"article_id":3,"title":"C","feed_title":"F3"}]}
        ]"#;
        let parsed = parse_digest_items_json(raw);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0][0].article_id, 1);
        assert!(parsed[1].is_empty());
        assert_eq!(parsed[2][0].article_id, 3);
    }

    #[test]
    fn align_digest_sources_trusts_stored_when_line_count_matches() {
        let stored = vec![
            vec![DigestSource {
                article_id: 1,
                title: "Rust release".into(),
                feed_title: "HN".into(),
            }],
            vec![DigestSource {
                article_id: 2,
                title: "Python news".into(),
                feed_title: "TC".into(),
            }],
        ];
        let body = "- Rust release ships\n- Python news today\n";
        let items = align_with_stored(body, &stored);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].len(), 1);
        assert_eq!(items[0][0].article_id, 1);
        assert_eq!(items[1].len(), 1);
        assert_eq!(items[1][0].article_id, 2);
    }

    #[test]
    fn format_digest_deep_input_numbered_items_follow_prompt_order() {
        let (_dir, conn) = open_temp();
        conn.execute(
            "INSERT INTO feeds (id, url, title, kind, created_at) VALUES (1, 'https://ex.test/f', 'f', 'rss', 1)",
            [],
        )
        .unwrap();
        for id in 1..=3 {
            conn.execute(
                "INSERT INTO articles (id, feed_id, title, hash, created_at) VALUES (?1, 1, ?2, ?3, 1)",
                rusqlite::params![id, format!("title{id}"), format!("h{id}")],
            )
            .unwrap();
        }
        let items = vec![
            digest_item(1, "Alpha", "F"),
            digest_item(3, "Beta", "F"),
            digest_item(2, "Alpha follow-up", "F"),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, mut item)| {
            item.cluster_id = if index == 0 || index == 2 {
                Some(5)
            } else {
                None
            };
            item
        })
        .collect::<Vec<_>>();
        let out =
            format_digest_deep_input(&conn, &items, crate::summary_extract::InputTier::Medium)
                .unwrap();
        assert_eq!(
            out.numbered_items
                .iter()
                .map(|item| item.article_id)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert!(out.prompt.contains("〈2〉title2"));
        assert!(out.prompt.contains("〈3〉title3"));
    }

    #[test]
    fn strict_digest_items_json_uses_numbered_mapping_without_title_guess() {
        let numbered = vec![
            digest_item(1, "Alpha", "F1"),
            digest_item(2, "Beta", "F2"),
            digest_item(3, "Gamma", "F3"),
        ];
        let body = "- completely rewritten first\n- completely rewritten second\n";
        let json = build_digest_items_json_strict(body, &numbered, &[vec![3], vec![1]]);
        let items = parse_digest_items_json(&json);
        assert_eq!(
            items
                .iter()
                .map(|line| line[0].article_id)
                .collect::<Vec<_>>(),
            vec![3, 1]
        );
    }

    #[test]
    fn sanitize_digest_body_keeps_refs_after_deleted_middle_item() {
        let body = "- **Alpha** 事实。\n- 有用户在社区分享空话。\n- **Gamma** 事实。\n";
        let (cleaned, refs) =
            sanitize_digest_body_with_item_refs(body, &[vec![1], vec![2], vec![3]]);
        assert!(!cleaned.contains("空话"));
        assert_eq!(refs, vec![vec![1], vec![3]]);
    }

    #[test]
    fn format_digest_deep_input_groups_by_cluster() {
        let (_dir, conn) = open_temp();
        conn.execute(
            "INSERT INTO feeds (id, url, title, kind, created_at) VALUES (1, 'https://ex.test/f', 'f', 'rss', 1)",
            [],
        )
        .unwrap();
        for id in 1..=3 {
            conn.execute(
                "INSERT INTO articles (id, feed_id, title, hash, created_at) VALUES (?1, 1, ?2, ?3, 1)",
                rusqlite::params![id, format!("title{id}"), format!("h{id}")],
            )
            .unwrap();
        }
        let items = vec![
            DigestItem {
                article_id: 1,
                title: "A".into(),
                summary: "body a".into(),
                summary_json: None,
                cluster_id: Some(5),
                feed_title: "F".into(),
            },
            DigestItem {
                article_id: 3,
                title: "C".into(),
                summary: "body c".into(),
                summary_json: None,
                cluster_id: None,
                feed_title: "F".into(),
            },
            DigestItem {
                article_id: 2,
                title: "B".into(),
                summary: "body b".into(),
                cluster_id: Some(5),
                feed_title: "F".into(),
                summary_json: None,
            },
        ];
        let out =
            format_digest_deep_input(&conn, &items, crate::summary_extract::InputTier::Medium)
                .unwrap();
        assert!(out.prompt.contains("【"));
        assert!(out.prompt.contains("〈1〉"));
        assert!(out.prompt.contains("〈2〉"));
        assert!(out.prompt.contains("〈3〉"));
        assert_eq!(
            out.numbered_items
                .iter()
                .map(|item| item.article_id)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn digest_pack_formats_cluster_groups_and_key_points() {
        let items = vec![
            DigestItem {
                article_id: 1,
                title: "Rust 1.90 发布".into(),
                summary: "Rust 1.90 带来了新特性。".into(),
                summary_json: Some(
                    r#"{"summary":"正文摘要","one_liner":"Rust 1.90 稳定版发布","key_points":["引入新语法","提升编译速度"]}"#.into(),
                ),
                cluster_id: Some(101),
                feed_title: "Rust Blog".into(),
            },
            DigestItem {
                article_id: 2,
                title: "Rust 1.90 更新解读".into(),
                summary: "深度解析 1.90 变更。".into(),
                summary_json: Some(
                    r#"{"summary":"正文解读","one_liner":"解析新版本性能优化","key_points":["提升编译速度","修复若干回归问题"]}"#.into(),
                ),
                cluster_id: Some(101),
                feed_title: "Rust Blog".into(),
            },
            DigestItem {
                article_id: 3,
                title: "独立文章标题".into(),
                summary: "独立报道摘要。".into(),
                summary_json: None,
                cluster_id: None,
                feed_title: String::new(),
            },
        ];

        let packed = format_digest_items(&items);
        assert!(packed.contains("## Topic: Rust 1.90 发布 (2 reports)"));
        assert!(packed.contains("- Rust 1.90 发布 | Rust 1.90 稳定版发布"));
        assert!(packed.contains("- Rust 1.90 更新解读 | 解析新版本性能优化"));
        assert!(packed.contains("Key points:"));
        assert!(packed.contains("* 引入新语法"));
        assert!(packed.contains("* 提升编译速度"));
        assert!(packed.contains("* 修复若干回归问题"));
        assert!(packed.contains("- 独立文章标题 | 独立报道摘要。"));
    }

    #[test]
    fn digest_candidates_exclude_disliked_noisy_diversify_feeds_and_attach_leads() {
        let (_dir, conn) = open_temp();
        let now = 1_900_000_000_000_i64;
        let today = now - 8 * HOUR_MS;
        for (id, title) in [
            (1, "Alpha Wire - Daily"),
            (2, "Beta Lab"),
            (3, "Noisy Feed"),
        ] {
            conn.execute(
                "INSERT INTO feeds (id, url, title, kind, created_at) VALUES (?1, ?2, ?3, 'rss', 1)",
                rusqlite::params![id, format!("https://f{id}.example/rss"), title],
            )
            .unwrap();
        }
        // 源 1 连发 10 篇高分无摘要文章；源 2 发 3 篇；源 3 是隐藏噪声源。
        for i in 1..=10 {
            conn.execute(
                "INSERT INTO articles (id, feed_id, title, hash, created_at, published_at, score, content_html)
                 VALUES (?1, 1, ?2, ?3, ?4, ?4, 20, ?5)",
                rusqlite::params![
                    i,
                    format!("Wire story {i}"),
                    format!("hw{i}"),
                    now - i * 60_000,
                    format!("<p>Concrete wire lead for story {i} with 42 units.</p>")
                ],
            )
            .unwrap();
        }
        for i in 11..=13 {
            conn.execute(
                "INSERT INTO articles (id, feed_id, title, hash, created_at, published_at, score, content_html)
                 VALUES (?1, 2, ?2, ?3, ?4, ?4, 5, '<p>Beta deep report with benchmark data.</p>')",
                rusqlite::params![i, format!("Beta story {i}"), format!("hb{i}"), now - i * 60_000],
            )
            .unwrap();
        }
        // 点踩 1 篇源 2 的文章。
        conn.execute(
            "INSERT INTO feedback (article_id, kind, created_at) VALUES (13, 'down', ?1)",
            [now],
        )
        .unwrap();
        // 源 3 触发噪声源且开启隐藏噪声源。
        conn.execute(
            "INSERT INTO articles (id, feed_id, title, hash, created_at, published_at, score, content_html)
             VALUES (14, 3, 'Noisy top', 'hn14', ?1, ?1, 50, '<p>Body.</p>')",
            [now - 10_000],
        )
        .unwrap();
        for i in 0..crate::feeds::NOISE_MIN_ARTICLES {
            conn.execute(
                "INSERT INTO articles (feed_id, title, hash, created_at, published_at, score)
                 VALUES (3, ?1, ?2, ?3, ?3, -20)",
                rusqlite::params![format!("noise {i}"), format!("hnoise{i}"), now - 10_000],
            )
            .unwrap();
        }
        crate::interests::set_hide_noisy_enabled(&conn, true).unwrap();

        let window = DigestWindow::for_period("daily", now, today, 5);
        let items = digest_candidates(&conn, 5, &window).unwrap();
        assert_eq!(items.len(), 5);
        assert!(
            items.iter().all(|i| i.article_id != 13),
            "disliked article must be excluded"
        );
        assert!(
            items.iter().all(|i| i.article_id != 14),
            "hidden noisy feed must be excluded"
        );
        // limit=5 时单源上限为 max(5/5, 3) = 3，因此源 2 的 2 篇也能进入候选，不被源 1 刷满。
        let from_beta = items.iter().filter(|i| i.feed_title == "Beta Lab").count();
        assert_eq!(from_beta, 2);

        // 选题目录带短源名、带导语，不复读标题。
        let packed = format_digest_input(&items, &["Alpha Wire - Daily".into()], 4);
        assert!(packed.contains("[Alpha Wire]"), "{packed}");
        assert!(packed.contains("[Beta Lab]"), "{packed}");
        assert!(packed.contains("Concrete wire lead"), "{packed}");
        assert!(packed.contains("- Alpha Wire\n"), "{packed}");
        assert!(!packed.contains("Wire story 1 | Wire story 1"), "{packed}");
    }

    #[test]
    fn weekly_supplements_skip_clusters_already_in_daily_digests() {
        let (_dir, conn) = open_temp();
        let now = 1_900_000_000_000_i64;
        conn.execute(
            "INSERT INTO feeds (id, url, title, kind, created_at) VALUES (1, 'https://a.example/f', 'F', 'rss', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO clusters (id, label, member_count, created_at) VALUES (9, 'Launch', 2, 1)",
            [],
        )
        .unwrap();
        for (id, cluster) in [(1, Some(9)), (2, Some(9)), (3, None)] {
            conn.execute(
                "INSERT INTO articles (id, feed_id, title, hash, created_at, published_at, score, cluster_id, summary)
                 VALUES (?1, 1, ?2, ?3, ?4, ?4, 5, ?5, '摘要。')",
                rusqlite::params![id, format!("t{id}"), format!("h{id}"), now - 2 * HOUR_MS, cluster],
            )
            .unwrap();
        }
        insert_digest(
            &conn,
            "daily",
            now - DAY_MS,
            "日报",
            r#"{"v":2,"items":[{"section":0,"refs":[{"article_id":1,"title":"t1","feed_title":"F"}]}]}"#,
        )
        .unwrap();
        let mut window = DigestWindow::for_period("weekly", now, now - DAY_MS, 30);
        window.exclude_daily_before = Some(i64::MAX);
        let items = digest_candidates(&conn, 30, &window).unwrap();
        let ids: Vec<i64> = items.iter().map(|i| i.article_id).collect();
        assert_eq!(ids, vec![3]);
    }

    #[test]
    fn waterfill_reclaims_short_article_surplus_for_long_articles() {
        // 总预算 1000，单篇上限 600，4 篇需求 [100, 100, 800, 800]：
        // 平均分是每篇 250，水填后短文只拿 100，长文各拿 400（而非被压到 250）。
        let alloc = allocate_waterfill_budgets(&[100, 100, 800, 800], 1000, 600);
        assert_eq!(alloc, vec![100, 100, 400, 400]);

        // 单篇上限 300 时，即使总预算很大也不超过上限。
        let capped = allocate_waterfill_budgets(&[50, 900], 1000, 300);
        assert_eq!(capped, vec![50, 300]);
    }

    #[test]
    fn weekly_input_compacts_sources_when_busy_week_exceeds_budget() {
        use crate::digest_sources::DailyDigestLine;
        use crate::summary_extract::InputTier;
        let (_dir, conn) = open_temp();
        conn.execute(
            "INSERT INTO feeds (id, url, title, kind, created_at) VALUES (1, 'https://a.example/f', 'F', 'rss', 1)",
            [],
        )
        .unwrap();
        let mut daily = Vec::new();
        for id in 1..=70 {
            let long_kp = "要点详述。".repeat(25);
            let json = serde_json::json!({
                "summary": "摘要。",
                "one_liner": format!("文章 {id} 一句话。{}", "细节。".repeat(30)),
                "key_points": [&long_kp, &long_kp, &long_kp],
                "facts": [format!("事实 {id}：增长 42%")]
            })
            .to_string();
            conn.execute(
                "INSERT INTO articles (id, feed_id, title, hash, created_at, published_at, summary, summary_json)
                 VALUES (?1, 1, ?2, ?3, 1, 1, '摘要。', ?4)",
                rusqlite::params![id, format!("大事件 {id}"), format!("hw{id}"), json],
            )
            .unwrap();
            daily.push(DailyDigestLine {
                day: format!("2026-11-{:02}", 10 + (id % 7)),
                section: "科技".into(),
                text: format!("**项目 {id}** 发布版本，带来显著变化。"),
                article_ids: vec![id],
            });
        }
        let low = format_weekly_input(&conn, &daily, &[], InputTier::Low).unwrap();
        // 满载 70 条日报下触发精简，不再把每篇的 3 条长要点全量塞进低档周报。
        assert!(low.prompt.contains("〈1〉大事件 1"), "{}", low.prompt);
        assert!(low.prompt.contains("〈70〉大事件 70"));
        assert!(!low.prompt.contains("要点详述。要点详述。"));
        assert!(
            crate::summary_extract::weighted_len(&low.prompt)
                <= InputTier::Low.digest_total_chars() * 3 / 2 + 500
        );
    }
}
