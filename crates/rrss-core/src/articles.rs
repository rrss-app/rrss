//! 文章列表与已读 / 收藏。日志 target：`rrss_core::articles`。

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};

use crate::db::{FOCUS_FILTER_KEY, HIDE_THRESHOLD_KEY, LIST_SCOPE_KEY};
use crate::error::{Error, Result};
use crate::feed_scope::FeedScope;
use crate::feeds::now_ms;
use crate::interests::{self, parse_interest_reasons};

// Replace the threshold marker with the query's bound placeholder.
pub(crate) const FOCUS_VISIBLE_SQL: &str =
    "(a.expires_at IS NULL AND (a.score > $threshold OR a.released_at IS NOT NULL))";

pub const DEFAULT_HIDE_THRESHOLD: i64 = -10;
pub const ROW_CARD: i32 = 0;
pub const ROW_HEADER: i32 = 1;
const MMR_LAMBDA: f32 = 0.7;
const FEED_MIX_CAP: usize = 3;
const FEED_MIX_OVERFETCH: i64 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusFilter {
    #[serde(rename = "focus")]
    FocusOnly,
    #[serde(rename = "hide_faded")]
    HideFaded,
    #[serde(rename = "all")]
    ShowAll,
}

impl FocusFilter {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FocusOnly => "focus",
            Self::HideFaded => "hide_faded",
            Self::ShowAll => "all",
        }
    }

    pub fn parse_str(value: &str) -> Self {
        match value {
            "focus" => Self::FocusOnly,
            "all" => Self::ShowAll,
            _ => Self::HideFaded,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleCard {
    pub id: i64,
    pub title: String,
    pub feed_title: String,
    pub published_at: Option<i64>,
    pub canonical_url: Option<String>,
    pub read: bool,
    pub starred: bool,
    pub score: i64,
    pub interest_boost: i64,
    pub rank_boost: i64,
    pub rank_reasons: Vec<crate::rank::combine::Reason>,
    pub summary: String,
    pub feed_id: i64,
    pub cluster_id: Option<i64>,
    pub cluster_size: i64,
    pub cluster_label: String,
    pub cluster_span: String,
    /// 只在标题行上：这组可见成员的来源（源 id 与名称），按成员先后去重。
    #[serde(default)]
    pub cluster_feeds: Vec<(i64, String)>,
    /// 只在标题行上：这组可见成员的 id，算综述之后新来了几篇。
    #[serde(default)]
    pub cluster_members: Vec<i64>,
    pub interest_label: String,
    pub interest_reason: Option<crate::rank::reasons::ReasonMessage>,
    pub noisy: bool,
    #[serde(default)]
    pub spot_check: bool,
    pub row_kind: i32,
}

impl ArticleCard {
    pub fn rank_score(&self, boost_on: bool) -> i64 {
        crate::rank::focus::score(self.score, self.interest_boost, boost_on, self.rank_boost)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArticleRow {
    pub id: i64,
    pub feed_id: i64,
    pub guid: Option<String>,
    pub canonical_url: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub published_at: Option<i64>,
    pub content_html: Option<String>,
    pub blocks_json: Option<String>,
    pub fulltext_md: Option<String>,
    pub read: bool,
    pub starred: bool,
    pub score: i64,
    pub reasons_json: Option<String>,
    pub feed_title: String,
    pub summary: Option<String>,
    pub summary_json: Option<String>,
    pub hash: Option<String>,
    pub interest_boost: i64,
    pub interest_reasons_json: Option<String>,
    pub cluster_id: Option<i64>,
    /// 按保留策略清掉正文的时间；`None` 表示正文完整。
    pub pruned_at: Option<i64>,
}

pub fn hide_threshold(conn: &Connection) -> Result<i64> {
    Ok(crate::db::get_setting(conn, HIDE_THRESHOLD_KEY)?
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_HIDE_THRESHOLD))
}

pub fn set_hide_threshold(conn: &Connection, value: i64) -> Result<()> {
    crate::db::set_setting(conn, HIDE_THRESHOLD_KEY, &value.to_string())
}

pub fn focus_summary_neutral_limit(conn: &Connection) -> Result<i64> {
    Ok(
        crate::db::get_setting(conn, crate::db::FOCUS_SUMMARY_NEUTRAL_LIMIT_KEY)?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
            .max(0),
    )
}

pub fn set_focus_summary_neutral_limit(conn: &Connection, value: i64) -> Result<()> {
    crate::db::set_setting(
        conn,
        crate::db::FOCUS_SUMMARY_NEUTRAL_LIMIT_KEY,
        &value.max(0).to_string(),
    )
}

pub fn focus_filter(conn: &Connection) -> Result<FocusFilter> {
    Ok(crate::db::get_setting(conn, FOCUS_FILTER_KEY)?
        .map(|s| FocusFilter::parse_str(&s))
        .unwrap_or(FocusFilter::HideFaded))
}

pub fn set_focus_filter(conn: &Connection, filter: FocusFilter) -> Result<()> {
    crate::db::set_setting(conn, FOCUS_FILTER_KEY, filter.as_str())
}

pub fn list_focus(conn: &Connection, limit: i64) -> Result<Vec<ArticleCard>> {
    list_focus_stream(conn, limit, None)
}

const PRELOAD_FOCUS: i64 = 12;
const PRELOAD_STARRED: i64 = 8;

/// 焦点高分 + 收藏，供正文配图预载。不含淡化条目。
pub fn list_preload_article_ids(conn: &Connection) -> Result<Vec<i64>> {
    let threshold = hide_threshold(conn)?;
    let boost_on = interests::boost_enabled(conn)?;
    let mut ids = Vec::new();
    let mut seen = HashSet::new();
    let focus = query_focus_cards(
        conn,
        FocusFilter::FocusOnly,
        threshold,
        boost_on,
        PRELOAD_FOCUS,
        &[],
        &[],
        &[],
        FeedScope::All,
    )?;
    for card in focus {
        if seen.insert(card.id) {
            ids.push(card.id);
        }
    }
    if ids.len() < 8 {
        let extra = query_focus_cards(
            conn,
            FocusFilter::HideFaded,
            threshold,
            boost_on,
            PRELOAD_FOCUS,
            &[],
            &[],
            &[],
            FeedScope::All,
        )?;
        for card in extra {
            if seen.insert(card.id) {
                ids.push(card.id);
            }
            if ids.len() >= PRELOAD_FOCUS as usize {
                break;
            }
        }
    }
    let mut stmt = conn.prepare(
        "SELECT id FROM articles WHERE expires_at IS NULL AND starred = 1
         ORDER BY COALESCE(published_at, 0) DESC, id DESC
         LIMIT ?1",
    )?;
    let starred = stmt.query_map([PRELOAD_STARRED], |row| row.get::<_, i64>(0))?;
    for id in starred {
        let id = id?;
        if seen.insert(id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListScope {
    All,
    Unread,
    Starred,
}

impl ListScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Unread => "unread",
            Self::Starred => "starred",
        }
    }

    pub fn parse_str(value: &str) -> Self {
        match value {
            "unread" => Self::Unread,
            "starred" => Self::Starred,
            _ => Self::All,
        }
    }

    pub(crate) fn filter_sql(self) -> &'static str {
        match self {
            Self::Unread => " AND a.read = 0",
            Self::Starred => " AND a.starred = 1",
            Self::All => "",
        }
    }
}

pub fn list_scope(conn: &Connection) -> Result<ListScope> {
    Ok(crate::db::get_setting(conn, LIST_SCOPE_KEY)?
        .map(|s| ListScope::parse_str(&s))
        .unwrap_or(ListScope::All))
}

pub fn set_list_scope(conn: &Connection, scope: ListScope) -> Result<()> {
    crate::db::set_setting(conn, LIST_SCOPE_KEY, scope.as_str())
}

pub const FOCUS_PAGE_SIZE: i64 = 40;

pub fn list_focus_stream(
    conn: &Connection,
    limit: i64,
    expanded_cluster_id: Option<i64>,
) -> Result<Vec<ArticleCard>> {
    Ok(list_focus_page(conn, limit, expanded_cluster_id, &[], &[], FeedScope::All)?.0)
}

/// 焦点栏目只接受全部订阅与文件夹。单个源没有栏目语义，按全部订阅查。
pub fn focus_column_scope(scope: FeedScope) -> FeedScope {
    match scope {
        FeedScope::Feed(_) => FeedScope::All,
        other => other,
    }
}

/// `scope_sql` 的占位符是匿名 `?`。焦点查询用编号参数，换成这次绑定的序号。
fn numbered_scope_sql(scope: FeedScope, index: usize) -> (String, Option<i64>) {
    let (sql, id) = crate::feed_scope::scope_sql(scope);
    (sql.replace('?', &format!("?{index}")), id)
}

/// 一页焦点卡片。`exclude_*` 给滚轮续查，避开 OFFSET 和首屏源混合窗口错位。
/// `scope` 过滤候选与簇内可见成员；全部订阅时 SQL 片段为空。
pub fn list_focus_page(
    conn: &Connection,
    limit: i64,
    expanded_cluster_id: Option<i64>,
    exclude_ids: &[i64],
    exclude_cluster_ids: &[i64],
    scope: FeedScope,
) -> Result<(Vec<ArticleCard>, bool)> {
    let scope = focus_column_scope(scope);
    let filter = focus_filter(conn)?;
    let threshold = hide_threshold(conn)?;
    let boost_on = interests::boost_enabled(conn)?;
    let rerank_on = interests::rerank_enabled(conn)?;
    let group_on = interests::cluster_group_enabled(conn)?;
    let hide_noisy = interests::hide_noisy_enabled(conn)?;
    let exclude_feeds: Vec<i64> = if hide_noisy {
        crate::feeds::noisy_feed_ids(conn)?.into_iter().collect()
    } else {
        Vec::new()
    };
    let want = limit.max(1);
    let first_page = exclude_ids.is_empty() && exclude_cluster_ids.is_empty();
    let fetch = if first_page {
        (want * FEED_MIX_OVERFETCH).max(want) + 1
    } else {
        want + 1
    };
    let mut cards = query_focus_cards(
        conn,
        filter,
        threshold,
        boost_on,
        fetch,
        exclude_ids,
        exclude_cluster_ids,
        &exclude_feeds,
        scope,
    )?;
    let fetched = cards.len();
    if first_page {
        cards = mix_by_feed(cards, want as usize, FEED_MIX_CAP);
    } else if cards.len() > want as usize {
        cards.truncate(want as usize);
    }
    let has_more = fetched > want as usize;
    let rerank_effective = rerank_on
        && (interests::interest_ready(conn).unwrap_or(false)
            || cards.iter().any(|c| c.score + c.interest_boost > 0));
    if rerank_effective {
        cards = mmr_rerank(conn, cards)?;
    }
    if group_on {
        cards = project_clusters(
            conn,
            cards,
            expanded_cluster_id,
            threshold,
            boost_on,
            exclude_cluster_ids,
            scope,
        )?;
    }
    if scope == FeedScope::All {
        crate::rank::spot_check::inject(
            conn,
            &mut cards,
            filter,
            threshold,
            &exclude_feeds,
            first_page,
        )?;
    }
    attach_noise(conn, &mut cards)?;
    Ok((cards, has_more))
}

fn query_focus_cards(
    conn: &Connection,
    filter: FocusFilter,
    threshold: i64,
    boost_on: bool,
    limit: i64,
    exclude_ids: &[i64],
    exclude_cluster_ids: &[i64],
    exclude_feed_ids: &[i64],
    scope: FeedScope,
) -> Result<Vec<ArticleCard>> {
    let boost = i64::from(boost_on);
    let exclude_sql = if exclude_ids.is_empty() {
        String::new()
    } else {
        let marks: String = (0..exclude_ids.len())
            .map(|i| format!("?{}", 4 + i))
            .collect::<Vec<_>>()
            .join(",");
        format!(" AND a.id NOT IN ({marks})")
    };
    let cluster_sql = if exclude_cluster_ids.is_empty() {
        String::new()
    } else {
        let start = 4 + exclude_ids.len();
        let marks: String = (0..exclude_cluster_ids.len())
            .map(|i| format!("?{}", start + i))
            .collect::<Vec<_>>()
            .join(",");
        format!(" AND (a.cluster_id IS NULL OR a.cluster_id NOT IN ({marks}))")
    };
    let noisy_sql = if exclude_feed_ids.is_empty() {
        String::new()
    } else {
        let start = 4 + exclude_ids.len() + exclude_cluster_ids.len();
        let marks: String = (0..exclude_feed_ids.len())
            .map(|i| format!("?{}", start + i))
            .collect::<Vec<_>>()
            .join(",");
        format!(" AND a.feed_id NOT IN ({marks})")
    };
    let list_sql = list_scope(conn)?.filter_sql();
    let scope_index = 4 + exclude_ids.len() + exclude_cluster_ids.len() + exclude_feed_ids.len();
    let (feed_sql, feed_id) = numbered_scope_sql(scope, scope_index);
    let rank_score = crate::rank::focus::SCORE_SQL;
    let member_score = crate::rank::focus::MEMBER_SQL;
    let visible = FOCUS_VISIBLE_SQL.replace("$threshold", "?2");
    let filter_sql = match filter {
        FocusFilter::FocusOnly => format!("{visible} AND {member_score} > 0"),
        FocusFilter::HideFaded => visible,
        FocusFilter::ShowAll => "1".into(),
    };
    let sql = format!(
        "SELECT a.id, a.title, COALESCE(f.title, f.url), a.published_at, a.canonical_url,
                a.read, a.starred, a.score, a.interest_boost, COALESCE(a.summary, ''),
                a.feed_id, a.cluster_id, a.interest_reasons_json, COALESCE(r.boost, 0), r.reasons_json
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         LEFT JOIN article_rank r ON r.article_id = a.id
         WHERE a.expires_at IS NULL AND {filter_sql}
           {list_sql}
           {feed_sql}
           {exclude_sql}
           {cluster_sql}
           {noisy_sql}
         ORDER BY {rank_score} DESC,
                  COALESCE(a.published_at, 0) DESC, a.id DESC
         LIMIT ?1"
    );
    let mut bind = vec![limit, threshold, boost];
    bind.extend_from_slice(exclude_ids);
    bind.extend_from_slice(exclude_cluster_ids);
    bind.extend_from_slice(exclude_feed_ids);
    if let Some(id) = feed_id {
        bind.push(id);
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(bind), map_card)?;
    let mut cards: Vec<ArticleCard> = rows.collect::<rusqlite::Result<_>>()?;
    for card in &mut cards {
        card.feed_title = crate::feeds::display_title(&card.feed_title, "");
    }
    Ok(cards)
}

/// 每个源先留 `cap` 条（保持原序），窗口未满再按分数从溢出里补。
fn mix_by_feed(cards: Vec<ArticleCard>, limit: usize, cap: usize) -> Vec<ArticleCard> {
    if limit == 0 || cards.len() <= limit {
        return cards;
    }
    let mut kept = Vec::new();
    let mut overflow = Vec::new();
    let mut counts: HashMap<i64, usize> = HashMap::new();
    for card in cards {
        let n = counts.entry(card.feed_id).or_insert(0);
        if *n < cap {
            *n += 1;
            kept.push(card);
            if kept.len() == limit {
                return kept;
            }
        } else {
            overflow.push(card);
        }
    }
    for card in overflow {
        if kept.len() >= limit {
            break;
        }
        kept.push(card);
    }
    kept
}

pub(crate) fn map_card(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArticleCard> {
    let reasons = parse_interest_reasons(row.get::<_, Option<String>>(12)?.as_deref());
    let interest_label = reasons
        .first()
        .map(|r| format!("接近你标记的兴趣：{}", r.label))
        .unwrap_or_default();
    Ok(ArticleCard {
        id: row.get(0)?,
        title: row.get(1)?,
        feed_title: row.get(2)?,
        published_at: row.get(3)?,
        canonical_url: row.get(4)?,
        read: row.get::<_, i64>(5)? != 0,
        starred: row.get::<_, i64>(6)? != 0,
        score: row.get(7)?,
        interest_boost: row.get(8)?,
        rank_boost: row.get(13)?,
        rank_reasons: row
            .get::<_, Option<String>>(14)?
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
        summary: row.get(9)?,
        feed_id: row.get(10)?,
        cluster_id: row.get(11)?,
        cluster_size: 1,
        cluster_label: String::new(),
        cluster_span: String::new(),
        cluster_feeds: Vec::new(),
        cluster_members: Vec::new(),
        interest_label,
        interest_reason: reasons.first().map(|reason| reason.message.clone()),
        noisy: false,
        spot_check: false,
        row_kind: ROW_CARD,
    })
}

fn mmr_rerank(conn: &Connection, cards: Vec<ArticleCard>) -> Result<Vec<ArticleCard>> {
    if cards.len() <= 1 {
        return Ok(cards);
    }
    struct Item {
        card: ArticleCard,
        rel: f32,
        vec: Option<Vec<f32>>,
        tokens: Vec<String>,
    }
    fn sim(a: &Item, b: &Item) -> f32 {
        if let (Some(va), Some(vb)) = (a.vec.as_deref(), b.vec.as_deref()) {
            return (1.0 - crate::rag::cosine_distance(va, vb)).clamp(0.0, 1.0);
        }
        jaccard(&a.tokens, &b.tokens)
    }
    let max_rel = cards
        .iter()
        .map(|c| (c.score + c.interest_boost).max(0) as f32)
        .fold(1.0f32, f32::max);
    let mut rest: Vec<Item> = Vec::new();
    for card in cards {
        let vec = crate::rag::title_embedding(conn, card.id)?;
        let tokens = crate::simhash::tokenize(&format!("{} {}", card.title, card.summary));
        let rel = (card.score + card.interest_boost).max(0) as f32 / max_rel;
        rest.push(Item {
            card,
            rel,
            vec,
            tokens,
        });
    }
    let mut selected: Vec<Item> = Vec::new();
    while !rest.is_empty() {
        let mut best_i = 0usize;
        let mut best_s = f32::NEG_INFINITY;
        for (i, item) in rest.iter().enumerate() {
            let sim_sel = selected.iter().map(|s| sim(item, s)).fold(0.0f32, f32::max);
            let score = MMR_LAMBDA * item.rel - (1.0 - MMR_LAMBDA) * sim_sel;
            if score > best_s {
                best_s = score;
                best_i = i;
            }
        }
        selected.push(rest.remove(best_i));
    }
    Ok(selected.into_iter().map(|i| i.card).collect())
}

fn jaccard(a: &[String], b: &[String]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let sa: HashSet<&str> = a.iter().map(String::as_str).collect();
    let sb: HashSet<&str> = b.iter().map(String::as_str).collect();
    let inter = sa.intersection(&sb).count() as f32;
    let union = sa.union(&sb).count() as f32;
    if union == 0.0 { 0.0 } else { inter / union }
}

fn project_clusters(
    conn: &Connection,
    cards: Vec<ArticleCard>,
    expanded: Option<i64>,
    threshold: i64,
    boost_on: bool,
    exclude_cluster_ids: &[i64],
    scope: FeedScope,
) -> Result<Vec<ArticleCard>> {
    let skip: HashSet<i64> = exclude_cluster_ids.iter().copied().collect();
    let mut wanted = Vec::new();
    let mut seen_want = HashSet::new();
    for card in &cards {
        let Some(cid) = card.cluster_id else {
            continue;
        };
        if skip.contains(&cid) && expanded != Some(cid) {
            continue;
        }
        if seen_want.insert(cid) {
            wanted.push(cid);
        }
    }
    let metas = crate::cluster::get_clusters(conn, &wanted)?;
    let members_map = visible_cluster_cards_map(conn, &wanted, threshold, scope)?;
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for card in cards {
        let Some(cid) = card.cluster_id else {
            out.push(card);
            continue;
        };
        if skip.contains(&cid) && expanded != Some(cid) {
            continue;
        }
        if seen.contains(&cid) {
            continue;
        }
        let Some(meta) = metas.get(&cid) else {
            out.push(card);
            continue;
        };
        if meta.member_count < 2 {
            out.push(card);
            continue;
        }
        seen.insert(cid);
        let mut members = members_map.get(&cid).cloned().unwrap_or_default();
        if members.len() < 2 {
            out.push(card);
            continue;
        }
        fill_cluster_members(&mut members, meta);
        out.push(cluster_header(meta, &members));
        if expanded == Some(cid) {
            out.extend(members);
        } else {
            let best = members
                .iter()
                .max_by_key(|c| (c.rank_score(boost_on), c.published_at.unwrap_or(0)))
                .cloned()
                .unwrap_or(card);
            // 折叠时只放代表文章；展开 / 收起只走标题行，不再另给「还有 N 篇」入口。
            out.push(best);
        }
    }
    Ok(out)
}

fn fill_cluster_members(members: &mut [ArticleCard], meta: &crate::cluster::ClusterMeta) {
    let span = format_span(meta.first_published_at, meta.last_published_at);
    for member in members {
        member.cluster_size = meta.member_count;
        member.cluster_label = meta.label.clone();
        member.cluster_span = span.clone();
    }
}

/// 焦点展开单个簇时要显示的可见成员。阈值、排序和簇字段与 `project_clusters` 的展开分支相同。
/// `scope` 与焦点页相同：只计范围内的可见成员，不足 2 篇则没有可展开的簇。
pub fn focus_cluster_members(
    conn: &Connection,
    cluster_id: i64,
    scope: FeedScope,
) -> Result<Vec<ArticleCard>> {
    let scope = focus_column_scope(scope);
    let threshold = hide_threshold(conn)?;
    let mut members = visible_cluster_cards_map(conn, &[cluster_id], threshold, scope)?
        .remove(&cluster_id)
        .unwrap_or_default();
    if members.len() < 2 {
        return Ok(Vec::new());
    }
    let Some(meta) = crate::cluster::get_cluster(conn, cluster_id)? else {
        return Ok(Vec::new());
    };
    if meta.member_count < 2 {
        return Ok(Vec::new());
    }
    fill_cluster_members(&mut members, &meta);
    Ok(members)
}

fn visible_cluster_cards_map(
    conn: &Connection,
    cluster_ids: &[i64],
    threshold: i64,
    scope: FeedScope,
) -> Result<HashMap<i64, Vec<ArticleCard>>> {
    let mut out: HashMap<i64, Vec<ArticleCard>> = HashMap::new();
    if cluster_ids.is_empty() {
        return Ok(out);
    }
    let marks: String = (1..=cluster_ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    let threshold_idx = cluster_ids.len() + 1;
    let (feed_sql, feed_id) = numbered_scope_sql(scope, threshold_idx + 1);
    let visible = FOCUS_VISIBLE_SQL.replace("$threshold", &format!("?{threshold_idx}"));
    let sql = format!(
        "SELECT a.id, a.title, COALESCE(f.title, f.url), a.published_at, a.canonical_url,
                a.read, a.starred, a.score, a.interest_boost, COALESCE(a.summary, ''),
                a.feed_id, a.cluster_id, a.interest_reasons_json, COALESCE(r.boost, 0), r.reasons_json
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         LEFT JOIN article_rank r ON r.article_id = a.id
         WHERE a.cluster_id IN ({marks}) AND {visible}{feed_sql}
         ORDER BY a.cluster_id, COALESCE(a.published_at, 0) ASC, a.id ASC"
    );
    let mut bind = cluster_ids.to_vec();
    bind.push(threshold);
    if let Some(id) = feed_id {
        bind.push(id);
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(bind), map_card)?;
    for row in rows {
        let card = row?;
        if let Some(cid) = card.cluster_id {
            out.entry(cid).or_default().push(card);
        }
    }
    Ok(out)
}

fn cluster_header(meta: &crate::cluster::ClusterMeta, members: &[ArticleCard]) -> ArticleCard {
    let size = members.len() as i64;
    let mut seen = HashSet::new();
    let cluster_feeds = members
        .iter()
        .filter(|m| seen.insert(m.feed_id))
        .map(|m| (m.feed_id, m.feed_title.clone()))
        .collect();
    ArticleCard {
        id: meta.id,
        title: meta.label.clone(),
        feed_title: format!("{size} 篇"),
        published_at: meta.first_published_at,
        canonical_url: None,
        read: true,
        starred: false,
        score: 0,
        interest_boost: 0,
        rank_boost: 0,
        rank_reasons: Vec::new(),
        summary: String::new(),
        feed_id: 0,
        cluster_id: Some(meta.id),
        cluster_size: size,
        cluster_label: meta.label.clone(),
        cluster_span: format_span(meta.first_published_at, meta.last_published_at),
        cluster_feeds,
        cluster_members: members.iter().map(|m| m.id).collect(),
        interest_label: String::new(),
        interest_reason: None,
        noisy: false,
        spot_check: false,
        row_kind: ROW_HEADER,
    }
}

/// 相近报道组尾「全部标为已读」：这组在范围内可见的未读一次标完，返回改动的 id。
/// 可见与范围的口径同 `focus_cluster_members`；被规则隐藏的成员不碰。
pub fn mark_cluster_read(conn: &Connection, cluster_id: i64, scope: FeedScope) -> Result<Vec<i64>> {
    let scope = focus_column_scope(scope);
    let visible = FOCUS_VISIBLE_SQL.replace("$threshold", "?2");
    let (feed_sql, feed_id) = numbered_scope_sql(scope, 3);
    let sql = format!(
        "UPDATE articles SET read = 1 WHERE id IN (
            SELECT a.id FROM articles a WHERE a.cluster_id = ?1 AND a.read = 0 AND {visible}{feed_sql}
         ) RETURNING id"
    );
    let mut bind = vec![cluster_id, hide_threshold(conn)?];
    if let Some(id) = feed_id {
        bind.push(id);
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(bind), |row| row.get(0))?;
    let mut ids: Vec<i64> = rows.collect::<rusqlite::Result<_>>()?;
    ids.sort_unstable();
    Ok(ids)
}

fn format_span(first: Option<i64>, last: Option<i64>) -> String {
    match (first, last) {
        (Some(a), Some(b)) if format_day(a) != format_day(b) => {
            format!("{} → {}", format_day(a), format_day(b))
        }
        (Some(a), _) | (_, Some(a)) => format_day(a),
        _ => String::new(),
    }
}

fn format_day(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| d.with_timezone(&chrono::Local).format("%m-%d").to_string())
        .unwrap_or_default()
}

fn attach_noise(conn: &Connection, cards: &mut [ArticleCard]) -> Result<()> {
    let mut feeds = HashSet::new();
    for card in cards.iter() {
        if card.row_kind == ROW_CARD {
            feeds.insert(card.feed_id);
        }
    }
    let noisy = crate::feeds::noisy_feed_ids_among(conn, &feeds.into_iter().collect::<Vec<_>>())?;
    for card in cards {
        if card.row_kind == ROW_CARD {
            card.noisy = noisy.contains(&card.feed_id);
        }
    }
    Ok(())
}

pub fn get_article(conn: &Connection, id: i64) -> Result<ArticleRow> {
    conn.query_row(
        "SELECT a.id, a.feed_id, a.guid, a.canonical_url, a.title, a.author, a.published_at,
                a.content_html, a.blocks_json, a.fulltext_md, a.read, a.starred, a.score, a.reasons_json,
                COALESCE(f.title, f.url), a.summary, a.summary_json, a.hash, a.interest_boost,
                a.interest_reasons_json, a.cluster_id, a.pruned_at
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         WHERE a.id = ?1",
        [id],
        |row| {
            Ok(ArticleRow {
                id: row.get(0)?,
                feed_id: row.get(1)?,
                guid: row.get(2)?,
                canonical_url: row.get(3)?,
                title: row.get(4)?,
                author: row.get(5)?,
                published_at: row.get(6)?,
                content_html: row.get(7)?,
                blocks_json: row.get(8)?,
                fulltext_md: row.get(9)?,
                read: row.get::<_, i64>(10)? != 0,
                starred: row.get::<_, i64>(11)? != 0,
                score: row.get(12)?,
                reasons_json: row.get(13)?,
                feed_title: row.get(14)?,
                summary: row.get(15)?,
                summary_json: row.get(16)?,
                hash: row.get(17)?,
                interest_boost: row.get(18)?,
                interest_reasons_json: row.get(19)?,
                cluster_id: row.get(20)?,
                pruned_at: row.get(21)?,
            })
        },
    )
    .optional()?
    .map(|mut row| {
        row.feed_title = crate::feeds::display_title(&row.feed_title, "");
        row
    })
    .ok_or(Error::ArticleNotFound)
}

/// Missing articles remain errors at action boundaries; background guards simply skip them.
pub fn is_temporary(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT expires_at IS NOT NULL FROM articles WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(false))
}

pub fn collect_article(conn: &Connection, id: i64) -> Result<()> {
    let changed = collect_existing_article(conn, id)?;
    if changed == 0
        && !conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM articles WHERE id=?1)",
            [id],
            |r| r.get::<_, bool>(0),
        )?
    {
        return Err(Error::ArticleNotFound);
    }
    Ok(())
}

/// Chat snapshots survive article deletion; collect only rows that still exist.
pub(crate) fn collect_existing_article(conn: &Connection, id: i64) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE articles SET expires_at=NULL WHERE id=?1 AND expires_at IS NOT NULL",
        [id],
    )?)
}

pub fn set_read(conn: &Connection, id: i64, read: bool) -> Result<()> {
    let n = conn.execute(
        "UPDATE articles SET read = ?1 WHERE id = ?2",
        params![i64::from(read), id],
    )?;
    if n == 0 {
        Err(Error::ArticleNotFound)
    } else {
        Ok(())
    }
}

pub fn set_starred(conn: &Connection, id: i64, starred: bool) -> Result<()> {
    let n = conn.execute(
        "UPDATE articles SET starred = ?1 WHERE id = ?2",
        params![i64::from(starred), id],
    )?;
    if n == 0 {
        Err(Error::ArticleNotFound)
    } else {
        Ok(())
    }
}

pub fn set_blocks_json(conn: &Connection, id: i64, json: &str) -> Result<()> {
    let n = conn.execute(
        "UPDATE articles SET blocks_json = ?1, embed_tier = 0 WHERE id = ?2",
        params![json, id],
    )?;
    if n == 0 {
        Err(Error::ArticleNotFound)
    } else {
        Ok(())
    }
}

pub fn set_fulltext(
    conn: &Connection,
    id: i64,
    fulltext_md: &str,
    blocks_json: &str,
) -> Result<()> {
    let n = conn.execute(
        // 手动抓回正文后不再算已清理；仍按入库时间参与下一次保留判断。
        "UPDATE articles SET fulltext_md = ?1, blocks_json = ?2, pruned_at = NULL,
         web_extracted_at = NULL, embed_tier = 0 WHERE id = ?3",
        params![fulltext_md, blocks_json, id],
    )?;
    if n == 0 {
        Err(Error::ArticleNotFound)
    } else {
        Ok(())
    }
}

/// 写入摘要，并把 `summary_hash` 设为当前 `articles.hash`（正文未变则不重跑）。
pub fn set_summary(conn: &Connection, id: i64, summary: &str) -> Result<()> {
    set_summary_and_json(conn, id, summary, None)
}

/// 写入摘要与结构化 JSON，并更新 `summary_hash`。
pub fn set_summary_and_json(
    conn: &Connection,
    id: i64,
    summary: &str,
    summary_json: Option<&str>,
) -> Result<()> {
    let n = conn.execute(
        "UPDATE articles SET summary = ?1, summary_json = ?2, summary_hash = hash,
         embed_tier = 0 WHERE id = ?3",
        params![summary, summary_json, id],
    )?;
    if n == 0 {
        Err(Error::ArticleNotFound)
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ArticleToolView {
    pub id: i64,
    pub title: String,
    pub feed_title: String,
    pub url: String,
    pub published_at: Option<i64>,
    pub score: i64,
    pub text: String,
}

/// Pi `get_article`：元数据 + summary，否则剥标签正文前 1200 字。
pub fn article_for_tool(conn: &Connection, id: i64) -> Result<ArticleToolView> {
    let row = get_article(conn, id)?;
    let text = row
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            crate::ai_store::truncate_chars(
                &crate::score::strip_html(row.content_html.as_deref().unwrap_or("")),
                1200,
            )
        });
    Ok(ArticleToolView {
        id: row.id,
        title: row.title,
        feed_title: row.feed_title,
        url: row.canonical_url.unwrap_or_default(),
        published_at: row.published_at,
        score: row.score,
        text,
    })
}

#[derive(Debug, Clone)]
pub struct DueSummary {
    pub hash: String,
    pub id: i64,
    pub title: String,
    pub content_html: Option<String>,
    pub blocks_json: Option<String>,
    pub score: i64,
    pub starred: bool,
}

/// 关注条目且摘要缺失或正文已变。中性 / 隐藏不入队；兴趣可把中性抬进关注。
/// `focus_summary_neutral_limit` > 0 时额外对最近 N 条中性未读做摘要。
pub fn due_focus_summaries(conn: &Connection, limit: i64) -> Result<Vec<DueSummary>> {
    due_focus_summaries_excluding(conn, limit, &[])
}

/// Failed input versions are excluded before LIMIT, leaving room for healthy work.
pub fn due_focus_summaries_excluding(
    conn: &Connection,
    limit: i64,
    excluded: &[(i64, String)],
) -> Result<Vec<DueSummary>> {
    let excluded = serde_json::to_string(excluded).expect("summary exclusions");
    let cutoff = now_ms() - crate::ai_store::FOCUS_WINDOW_DAYS * 86_400_000;
    let threshold = hide_threshold(conn)?;
    let boost = i64::from(interests::boost_enabled(conn)?);
    let visible = FOCUS_VISIBLE_SQL.replace("$threshold", "?2");
    let mut stmt = conn.prepare(&format!(
        "SELECT id, title, content_html, blocks_json, score, starred, COALESCE(hash,'') FROM articles a
         WHERE {visible} AND pruned_at IS NULL AND created_at >= ?5
           AND (score + CASE WHEN ?3 THEN interest_boost ELSE 0 END) > 0
           AND NOT EXISTS (SELECT 1 FROM json_each(?4) e
               WHERE json_extract(e.value,'$[0]')=a.id AND json_extract(e.value,'$[1]')=COALESCE(a.hash,''))
           AND (summary IS NULL OR summary = ''
                OR summary_hash IS NULL OR summary_hash != hash)
         ORDER BY (score + CASE WHEN ?3 THEN interest_boost ELSE 0 END) DESC, id DESC
         LIMIT ?1",
    ))?;
    let rows = stmt.query_map(params![limit, threshold, boost, excluded, cutoff], |row| {
        Ok(DueSummary {
            hash: row.get(6)?,
            id: row.get(0)?,
            title: row.get(1)?,
            content_html: row.get(2)?,
            blocks_json: row.get(3)?,
            score: row.get(4)?,
            starred: row.get::<_, i64>(5)? != 0,
        })
    })?;
    let mut out = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    let extra = focus_summary_neutral_limit(conn)?;
    if extra > 0 && (out.len() as i64) < limit {
        let remain = (limit - out.len() as i64).min(extra);
        let seen: HashSet<i64> = out.iter().map(|d| d.id).collect();
        let visible = FOCUS_VISIBLE_SQL.replace("$threshold", "?1");
        let mut extra_stmt = conn.prepare(&format!(
            "SELECT id, title, content_html, blocks_json, score, starred, COALESCE(hash,'') FROM articles a
             WHERE {visible} AND (score + CASE WHEN ?5 THEN interest_boost ELSE 0 END) = 0
               AND read = 0 AND pruned_at IS NULL AND created_at >= ?4
               AND NOT EXISTS (SELECT 1 FROM json_each(?3) e
                   WHERE json_extract(e.value,'$[0]')=a.id AND json_extract(e.value,'$[1]')=COALESCE(a.hash,''))
               AND (summary IS NULL OR summary = ''
                    OR summary_hash IS NULL OR summary_hash != hash)
             ORDER BY COALESCE(published_at, 0) DESC, id DESC
             LIMIT ?2",
        ))?;
        let extras =
            extra_stmt.query_map(params![threshold, remain, excluded, cutoff, boost], |row| {
                Ok(DueSummary {
                    hash: row.get(6)?,
                    id: row.get(0)?,
                    title: row.get(1)?,
                    content_html: row.get(2)?,
                    blocks_json: row.get(3)?,
                    score: row.get(4)?,
                    starred: row.get::<_, i64>(5)? != 0,
                })
            })?;
        for row in extras {
            let item = row?;
            if seen.contains(&item.id) {
                continue;
            }
            out.push(item);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{PooledConnection, open_at};
    use crate::feed_scope::FeedScope;
    use crate::feeds::{SubscribeRequest, subscribe};
    use crate::parse::ParsedEntry;
    use crate::rules::{Dimension, NewRule, Polarity, insert_rule};
    use rusqlite::Connection;
    use tempfile::tempdir;

    fn open_temp() -> (tempfile::TempDir, PooledConnection) {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        (dir, conn)
    }

    fn seed(conn: &Connection) {
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
        insert_rule(
            conn,
            NewRule::manual_global(Polarity::Like, Dimension::TitleKw, "focus"),
        )
        .unwrap();
        insert_rule(
            conn,
            NewRule::manual_global(Polarity::Dislike, Dimension::TitleKw, "noise"),
        )
        .unwrap();
        let entries = [
            entry("focus piece", "https://news.example.com/a"),
            entry("neutral piece", "https://news.example.com/b"),
            entry("noise piece", "https://news.example.com/c"),
        ];
        crate::ingest::upsert_entries(conn, feed, &entries, now_ms()).unwrap();
    }

    fn entry(title: &str, url: &str) -> ParsedEntry {
        ParsedEntry {
            guid: url.into(),
            canonical_url: Some(url.into()),
            title: title.into(),
            author: None,
            published_at: Some(1),
            content_html: Some("<p>x</p>".into()),
            hash: crate::parse::entry_hash(title, Some(url), Some("<p>x</p>")),
            tags: vec![],
        }
    }

    #[test]
    fn released_cluster_members_and_summary_counts_share_visibility() {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_at(&dir.path().join("hidden.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://hidden.invalid/feed',0);
            INSERT INTO articles(id,feed_id,title,score,interest_boost,cluster_id,created_at) VALUES
                (1,1,'visible',1,0,7,0), (2,1,'released',-10,20,7,0),
                (3,1,'hidden',-10,20,7,0), (4,1,'neutral',0,0,NULL,0);",
        )
        .unwrap();
        conn.execute("UPDATE articles SET created_at=?1", [now_ms()])
            .unwrap();
        set_hide_threshold(&conn, 0).unwrap();
        set_focus_summary_neutral_limit(&conn, 10).unwrap();
        let check = |expected: &[i64], count| {
            let map = visible_cluster_cards_map(&conn, &[7], 0, FeedScope::All).unwrap();
            assert_eq!(map[&7].iter().map(|a| a.id).collect::<Vec<_>>(), expected);
            assert_eq!(due_focus_summaries(&conn, 10).unwrap().len() as i64, count);
        };
        check(&[1], 1);
        conn.execute("UPDATE articles SET released_at=1 WHERE id IN (2,4)", [])
            .unwrap();
        check(&[1, 2], 3);
        conn.execute("UPDATE articles SET released_at=NULL", [])
            .unwrap();
        check(&[1], 1);
    }

    #[test]
    fn learned_boost_changes_order_only_never_visibility_or_membership() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("rank.db")).unwrap();
        conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://rank.invalid/feed',0);
            INSERT INTO articles(id,feed_id,title,score,interest_boost,created_at,starred) VALUES
            (1,1,'hidden',-10,0,0,0),(2,1,'neutral',0,0,0,0),(3,1,'liked',10,0,0,1),(4,1,'liked second',10,0,0,1);").unwrap();
        let ids = || {
            list_focus_page(&conn, 40, None, &[], &[], FeedScope::All)
                .unwrap()
                .0
                .into_iter()
                .map(|c| c.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(), vec![4, 3, 2]);
        conn.execute_batch("INSERT INTO article_rank VALUES(1,1000,'[]',2,0),(2,100,'[]',2,0),(3,10,'[]',2,0),(4,-100,'[]',2,0);").unwrap();
        assert_eq!(ids(), vec![2, 3, 4]);
        set_focus_filter(&conn, FocusFilter::FocusOnly).unwrap();
        assert_eq!(ids(), vec![3, 4]); // neutral +100 不能变成关注，liked -100 不能掉出。
        crate::db::set_setting(&conn, crate::db::LIST_SCOPE_KEY, "starred").unwrap();
        assert_eq!(ids(), vec![3, 4]);
        conn.execute("UPDATE articles SET read=1 WHERE id=3", [])
            .unwrap();
        crate::db::set_setting(&conn, crate::db::LIST_SCOPE_KEY, "unread").unwrap();
        assert_eq!(ids(), vec![4]);
    }

    #[test]
    fn slider_filters() {
        let (_dir, conn) = open_temp();
        seed(&conn);
        assert_eq!(focus_filter(&conn).unwrap(), FocusFilter::HideFaded);
        let faded = list_focus(&conn, 10).unwrap();
        let titles: Vec<_> = faded.iter().map(|c| c.title.as_str()).collect();
        assert_eq!(titles, vec!["focus piece", "neutral piece"]);

        set_focus_filter(&conn, FocusFilter::FocusOnly).unwrap();
        let focus = list_focus(&conn, 10).unwrap();
        assert_eq!(focus.len(), 1);
        assert_eq!(focus[0].title, "focus piece");
        assert_eq!(focus[0].score, 10);

        set_focus_filter(&conn, FocusFilter::ShowAll).unwrap();
        let all = list_focus(&conn, 10).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].title, "focus piece");
        assert_eq!(all[2].title, "noise piece");
        assert_eq!(all[2].score, -10);
    }

    #[test]
    fn due_focus_summaries_filters_age_visibility_and_effective_score() {
        let (_dir, conn) = open_temp();
        seed(&conn);
        let now = now_ms();
        let cutoff = now - crate::ai_store::FOCUS_WINDOW_DAYS * 86_400_000;
        let feed: i64 = conn
            .query_row("SELECT id FROM feeds LIMIT 1", [], |r| r.get(0))
            .unwrap();
        for (id, score, boost, created, expired, pruned) in [
            (100, 10, 0, cutoff - 1, false, false),
            (101, 10, 0, cutoff + 60_000, false, false),
            (102, 0, 5, now, false, false),
            (103, -10, 20, now, false, false),
            (104, 10, 0, now, true, false),
            (105, 10, 0, now, false, true),
            (106, 0, 0, cutoff - 1, false, false),
            (107, -1, 0, now, false, false),
        ] {
            conn.execute("INSERT INTO articles(id,feed_id,title,score,interest_boost,created_at,expires_at,pruned_at,hash)
                VALUES(?1,?2,'candidate',?3,?4,?5,?6,?7,?1)",
                params![id,feed,score,boost,created,expired.then_some(1),pruned.then_some(1)]).unwrap();
        }
        let due = due_focus_summaries(&conn, 20).unwrap();
        assert_eq!(due.len(), 3);
        assert!(due.iter().any(|a| a.id == 101));
        assert!(due.iter().any(|a| a.id == 102));
        for article in due {
            set_summary(&conn, article.id, "摘要").unwrap();
        }
        assert!(due_focus_summaries(&conn, 20).unwrap().is_empty());
        set_focus_summary_neutral_limit(&conn, 1).unwrap();
        let neutral = due_focus_summaries(&conn, 20).unwrap();
        assert_eq!(neutral.len(), 1);
        assert_eq!(neutral[0].title, "neutral piece");
    }

    #[test]
    fn cooled_summary_versions_are_excluded_before_the_batch_limit() {
        let (_dir, conn) = open_temp();
        seed(&conn);
        conn.execute(
            "UPDATE articles SET score=5 WHERE title='neutral piece'",
            [],
        )
        .unwrap();
        let first = due_focus_summaries(&conn, 1).unwrap().remove(0);
        let excluded = [(first.id, first.hash.clone())];
        let next = due_focus_summaries_excluding(&conn, 1, &excluded).unwrap();
        assert_eq!(next.len(), 1);
        assert_ne!(next[0].id, first.id);
        conn.execute(
            "UPDATE articles SET hash='new-body' WHERE id=?1",
            [first.id],
        )
        .unwrap();
        assert_eq!(
            due_focus_summaries_excluding(&conn, 1, &excluded).unwrap()[0].id,
            first.id
        );
    }

    #[test]
    fn list_focus_groups_cross_feed_cluster() {
        let (_dir, conn) = open_temp();
        let a = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://a.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        let b = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://b.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        let url = "https://news.example.com/library";
        crate::ingest::upsert_entries(&conn, a, &[entry("市政厅新图书馆开放", url)], 1).unwrap();
        crate::ingest::upsert_entries(&conn, b, &[entry("市政厅开放新图书馆", url)], 1).unwrap();
        set_focus_filter(&conn, FocusFilter::ShowAll).unwrap();
        let rows = list_focus(&conn, 10).unwrap();
        assert!(
            rows.iter().any(|r| r.row_kind == ROW_HEADER),
            "expected cluster header, got {:?}",
            rows.iter()
                .map(|r| (r.row_kind, r.title.clone()))
                .collect::<Vec<_>>()
        );
        let cid = rows
            .iter()
            .find(|r| r.row_kind == ROW_HEADER)
            .and_then(|r| r.cluster_id)
            .unwrap();
        // 折叠时只有标题行和代表文章，没有第二个展开入口。
        let collapsed: Vec<i32> = rows
            .iter()
            .filter(|r| r.cluster_id == Some(cid))
            .map(|r| r.row_kind)
            .collect();
        assert_eq!(collapsed, vec![ROW_HEADER, ROW_CARD]);
        let expanded = list_focus_stream(&conn, 10, Some(cid)).unwrap();
        let members = expanded
            .iter()
            .filter(|r| r.row_kind == ROW_CARD && r.cluster_id == Some(cid))
            .count();
        assert_eq!(members, 2);
        assert!(
            expanded
                .iter()
                .all(|r| r.row_kind == ROW_CARD || r.row_kind == ROW_HEADER)
        );
    }

    #[test]
    fn preload_ids_cover_focus_and_starred() {
        let (_dir, conn) = open_temp();
        seed(&conn);
        let ids = list_preload_article_ids(&conn).unwrap();
        let titles: Vec<String> = ids
            .iter()
            .map(|id| get_article(&conn, *id).unwrap().title)
            .collect();
        assert!(titles.iter().any(|t| t.contains("focus")));
        assert!(!titles.iter().any(|t| t.contains("noise")));
        let noise_id: i64 = conn
            .query_row(
                "SELECT id FROM articles WHERE title LIKE '%noise%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        set_starred(&conn, noise_id, true).unwrap();
        let ids = list_preload_article_ids(&conn).unwrap();
        assert!(ids.contains(&noise_id));
    }

    #[test]
    fn mix_keeps_quiet_feed_when_loud_feed_floods() {
        let (_dir, conn) = open_temp();
        set_focus_filter(&conn, FocusFilter::ShowAll).unwrap();
        crate::interests::set_cluster_group_enabled(&conn, false).unwrap();
        let loud = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://loud.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        let quiet = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://quiet.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        let loud_entries: Vec<ParsedEntry> = (1..=20)
            .map(|i| {
                let mut e = entry(
                    &format!("loud {i:02}"),
                    &format!("https://loud.example.com/{i}"),
                );
                e.published_at = Some(1_000 + i);
                e
            })
            .collect();
        crate::ingest::upsert_entries(&conn, loud, &loud_entries, 1).unwrap();
        crate::ingest::upsert_entries(
            &conn,
            quiet,
            &[
                {
                    let mut e = entry("quiet alpha", "https://quiet.example.com/a");
                    e.published_at = Some(1);
                    e
                },
                {
                    let mut e = entry("quiet beta", "https://quiet.example.com/b");
                    e.published_at = Some(2);
                    e
                },
            ],
            1,
        )
        .unwrap();

        let mixed = list_focus(&conn, 10).unwrap();
        assert_eq!(mixed.len(), 10);
        let quiet_n = mixed.iter().filter(|c| c.feed_id == quiet).count();
        assert!(
            quiet_n >= 2,
            "quiet feed should survive mix, got {:?}",
            mixed.iter().map(|c| c.title.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn focus_page_then_more_does_not_repeat_ids() {
        let (_dir, conn) = open_temp();
        seed(&conn);
        set_focus_filter(&conn, FocusFilter::ShowAll).unwrap();
        crate::interests::set_cluster_group_enabled(&conn, false).unwrap();
        let feed: i64 = conn
            .query_row("SELECT id FROM feeds LIMIT 1", [], |r| r.get(0))
            .unwrap();
        for i in 0..12 {
            let title = format!("extra {i}");
            let url = format!("https://news.example.com/extra/{i}");
            crate::ingest::upsert_entries(
                &conn,
                feed,
                &[ParsedEntry {
                    guid: url.clone(),
                    canonical_url: Some(url.clone()),
                    title: title.clone(),
                    author: None,
                    published_at: Some(10 + i),
                    content_html: Some("<p>x</p>".into()),
                    hash: crate::parse::entry_hash(&title, Some(&url), Some("<p>x</p>")),
                    tags: vec![],
                }],
                1,
            )
            .unwrap();
        }
        let (first, has_more) = list_focus_page(&conn, 5, None, &[], &[], FeedScope::All).unwrap();
        assert!(has_more);
        assert_eq!(first.len(), 5);
        let seen: Vec<i64> = first
            .iter()
            .filter(|c| c.row_kind == ROW_CARD)
            .map(|c| c.id)
            .collect();
        let (second, second_more) =
            list_focus_page(&conn, 5, None, &seen, &[], FeedScope::All).unwrap();
        let again: Vec<i64> = second
            .iter()
            .filter(|c| c.row_kind == ROW_CARD)
            .map(|c| c.id)
            .collect();
        assert!(!again.is_empty());
        assert!(second_more);
        for id in &again {
            assert!(!seen.contains(id), "id {id} repeated across pages");
        }
        let mut all = seen;
        all.extend(again);
        let (third, third_more) =
            list_focus_page(&conn, 5, None, &all, &[], FeedScope::All).unwrap();
        assert_eq!(third.len(), 5);
        assert!(!third_more);
        for card in &third {
            assert!(!all.contains(&card.id));
        }
    }

    #[test]
    fn hide_noisy_drops_flagged_feed() {
        let (_dir, conn) = open_temp();
        insert_rule(
            &conn,
            NewRule::manual_global(Polarity::Dislike, Dimension::TitleKw, "ad"),
        )
        .unwrap();
        let feed = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://ads.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        let now = crate::feeds::now_ms();
        let entries: Vec<ParsedEntry> = (0..8)
            .map(|i| {
                let url = format!("https://ads.example.com/{i}");
                let mut item = entry(&format!("ad promo {i}"), &url);
                item.published_at = Some(now);
                item
            })
            .collect();
        crate::ingest::upsert_entries(&conn, feed, &entries, now).unwrap();
        crate::articles::set_focus_filter(&conn, FocusFilter::ShowAll).unwrap();
        crate::interests::set_cluster_group_enabled(&conn, false).unwrap();
        let shown = list_focus(&conn, 20).unwrap();
        assert!(shown.iter().any(|c| c.noisy));
        crate::interests::set_hide_noisy_enabled(&conn, true).unwrap();
        let hidden = list_focus(&conn, 20).unwrap();
        assert!(hidden.is_empty(), "got {:?}", hidden.len());
    }

    #[test]
    fn cluster_header_lists_sources_and_group_read_respects_scope_and_rules() {
        let (_dir, conn) = open_temp();
        conn.execute_batch(
            "INSERT INTO folders(id,parent_id,name,created_at) VALUES (1,NULL,'in',0),(2,NULL,'out',0);
             INSERT INTO feeds(id,url,title,folder_id,created_at) VALUES
             (10,'https://a.example/rss','甲',1,0),(11,'https://b.example/rss','乙',1,0),
             (12,'https://c.example/rss','丙',2,0);
             INSERT INTO clusters(id,label,member_count,created_at) VALUES (7,'同题',5,0);
             INSERT INTO articles(id,feed_id,title,score,published_at,cluster_id,read,created_at) VALUES
             (1,10,'甲一',5,1000,7,0,1),(2,11,'乙一',5,2000,7,0,1),(3,10,'甲二',5,3000,7,1,1),
             (4,12,'丙一',5,4000,7,0,1),(5,11,'被隐藏',-100,5000,7,0,1);",
        )
        .unwrap();
        let header = |scope| {
            list_focus_page(&conn, 40, None, &[], &[], scope)
                .unwrap()
                .0
                .into_iter()
                .find(|card| card.row_kind == ROW_HEADER)
                .unwrap()
        };
        // 按成员发布先后去重；被规则隐藏的成员不算来源。
        assert_eq!(
            header(FeedScope::All).cluster_feeds,
            vec![
                (10, "甲".to_string()),
                (11, "乙".to_string()),
                (12, "丙".to_string())
            ]
        );
        assert_eq!(
            header(FeedScope::Folder(1)).cluster_feeds,
            vec![(10, "甲".to_string()), (11, "乙".to_string())]
        );
        // 只标范围内可见的未读：已读、隐藏、范围外的都不动。
        assert_eq!(
            mark_cluster_read(&conn, 7, FeedScope::Folder(1)).unwrap(),
            vec![1, 2]
        );
        let read = |id: i64| -> i64 {
            conn.query_row("SELECT read FROM articles WHERE id=?1", [id], |r| r.get(0))
                .unwrap()
        };
        assert_eq!((read(4), read(5)), (0, 0));
        assert_eq!(
            mark_cluster_read(&conn, 7, FeedScope::All).unwrap(),
            vec![4]
        );
        assert!(
            mark_cluster_read(&conn, 7, FeedScope::All)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn folder_scope_limits_cards_clusters_and_skips_spot_checks() {
        let (_dir, conn) = open_temp();
        conn.execute_batch(
            "INSERT INTO folders(id,parent_id,name,created_at) VALUES
             (1,NULL,'parent',0),(2,1,'child',0),(3,NULL,'other',0);
             INSERT INTO feeds(id,url,folder_id,created_at) VALUES
             (10,'https://in.example/rss',1,0),(11,'https://child.example/rss',2,0),
             (12,'https://out.example/rss',3,0);
             INSERT INTO clusters(id,label,member_count,created_at) VALUES
             (7,'同题',3,0),(8,'只剩一篇',2,0);
             INSERT INTO articles(id,feed_id,title,score,published_at,cluster_id,created_at) VALUES
             (1,10,'in parent',5,3000,7,1),(2,11,'in child',5,2000,7,1),
             (3,12,'outside cluster',5,4000,7,1),(4,10,'solo in',5,1000,8,1),
             (5,12,'solo out',5,5000,8,1),(6,12,'plain out',5,6000,NULL,1);",
        )
        .unwrap();
        crate::db::set_setting(&conn, crate::db::HIDDEN_SPOT_CHECK_RATE_KEY, "5").unwrap();
        let now = crate::feeds::now_ms();
        conn.execute(
            "WITH RECURSIVE ids(id) AS (SELECT 100 UNION ALL SELECT id+1 FROM ids WHERE id<500)
             INSERT INTO articles(id,feed_id,title,score,created_at)
             SELECT id,10,'hidden',-10,?1 FROM ids",
            [now],
        )
        .unwrap();
        let page = |scope| {
            list_focus_page(&conn, 40, Some(7), &[], &[], scope)
                .unwrap()
                .0
        };
        let cards = |rows: &[ArticleCard]| {
            rows.iter()
                .filter(|card| card.row_kind == ROW_CARD)
                .map(|card| card.id)
                .collect::<Vec<_>>()
        };
        let scoped = page(FeedScope::Folder(1));
        assert_eq!(cards(&scoped), vec![2, 1, 4]);
        let headers: Vec<_> = scoped
            .iter()
            .filter(|card| card.row_kind == ROW_HEADER)
            .collect();
        assert_eq!(
            headers
                .iter()
                .map(|card| (card.cluster_id, card.cluster_size))
                .collect::<Vec<_>>(),
            vec![(Some(7), 2)]
        );
        assert_eq!(
            focus_cluster_members(&conn, 7, FeedScope::Folder(1))
                .unwrap()
                .into_iter()
                .map(|card| card.id)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        let all = page(FeedScope::All);
        assert!(
            all.iter().any(|card| card.spot_check) && scoped.iter().all(|card| !card.spot_check)
        );
        assert_eq!(cards(&page(FeedScope::Feed(12))), cards(&all));
    }
}
