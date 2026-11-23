//! 文章卡片、可见性、相近报道分组与已读 / 收藏。焦点页的候选与排列在 `rrss-focus`。
//! 日志 target：`rrss_core::articles`。

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};

use crate::db::{FOCUS_FILTER_KEY, HIDE_THRESHOLD_KEY, LIST_SCOPE_KEY};
use crate::error::{Error, Result};
use crate::feed_scope::FeedScope;
use crate::feeds::now_ms;
use crate::interests::{self, parse_interest_reasons};

/// 规则可见性：未过期，且规则分高于隐藏阈值或已被用户放行。焦点页、时间线、相近报道与摘要挑选共用；
/// 用时把 `$threshold` 换成这次查询绑定的占位符。
pub const VISIBLE_SQL: &str =
    "(a.expires_at IS NULL AND (a.score > $threshold OR a.released_at IS NOT NULL))";

pub const DEFAULT_HIDE_THRESHOLD: i64 = -10;
pub const ROW_CARD: i32 = 0;
pub const ROW_HEADER: i32 = 1;

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
    pub rank_reasons: Vec<crate::reasons::RankReason>,
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
    pub interest_reason: Option<crate::reasons::ReasonMessage>,
    pub noisy: bool,
    #[serde(default)]
    pub spot_check: bool,
    #[serde(default)]
    pub explore: bool,
    pub row_kind: i32,
}

impl ArticleCard {
    pub fn rank_score(&self, boost_on: bool) -> i64 {
        crate::rank_score::score(self.score, self.interest_boost, boost_on, self.rank_boost)
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

    pub fn filter_sql(self) -> &'static str {
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

/// 焦点栏目只接受全部订阅与文件夹。单个源没有栏目语义，按全部订阅查。
pub fn focus_column_scope(scope: FeedScope) -> FeedScope {
    match scope {
        FeedScope::Feed(_) => FeedScope::All,
        other => other,
    }
}

/// 已提交的文章 ID 水位。删除文章不会降低水位，从未入库时为 0。
pub fn ingestion_watermark(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE((SELECT seq FROM sqlite_sequence WHERE name='articles'), 0)",
        [],
        |row| row.get(0),
    )?)
}

/// `scope_sql` 的占位符是匿名 `?`。焦点查询用编号参数，换成这次绑定的序号。
pub fn numbered_scope_sql(scope: FeedScope, index: usize) -> (String, Option<i64>) {
    let (sql, id) = crate::feed_scope::scope_sql(scope);
    (sql.replace('?', &format!("?{index}")), id)
}

/// Feed exclusions shared by Focus candidates, standalone cards and groups.
pub fn focus_excluded_feeds(conn: &Connection) -> Result<Vec<i64>> {
    if interests::hide_noisy_enabled(conn)? {
        Ok(crate::feeds::noisy_feed_ids(conn)?.into_iter().collect())
    } else {
        Ok(Vec::new())
    }
}

pub fn exclude_feeds_sql(bind: &mut Vec<i64>, ids: &[i64]) -> String {
    if ids.is_empty() {
        return String::new();
    }
    let first = bind.len() + 1;
    bind.extend_from_slice(ids);
    let marks = (first..=bind.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(",");
    format!(" AND a.feed_id NOT IN ({marks})")
}

pub fn query_ranked_cards(
    conn: &Connection,
    ranked: &str,
    bind: impl rusqlite::Params,
) -> Result<Vec<(ArticleCard, i64)>> {
    let sql = format!(
        "WITH picked AS ({ranked})
         SELECT a.id, a.title, COALESCE(f.title, f.url), a.published_at, a.canonical_url,
                a.read, a.starred, a.score, a.interest_boost, COALESCE(a.summary, ''),
                a.feed_id, a.cluster_id, a.interest_reasons_json, COALESCE(r.boost, 0),
                r.reasons_json, picked.priority
         FROM picked JOIN articles a ON a.id=picked.id JOIN feeds f ON f.id=a.feed_id
         LEFT JOIN article_rank r ON r.article_id=a.id
         ORDER BY picked.priority DESC,picked.position,COALESCE(a.published_at,0) DESC,a.id DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(bind, |r| Ok((map_card(r)?, r.get(15)?)))?;
    let mut cards: Vec<(ArticleCard, i64)> = rows.collect::<rusqlite::Result<_>>()?;
    for (card, _) in &mut cards {
        card.feed_title = crate::feeds::display_title(&card.feed_title, "");
    }
    Ok(cards)
}

pub fn map_card(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArticleCard> {
    let reasons = parse_interest_reasons(row.get::<_, Option<String>>(12)?.as_deref());
    let interest_label = reasons.first().map(|r| r.label.clone()).unwrap_or_default();
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
            .map(|s| crate::reasons::rank_reasons(&s))
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
        explore: false,
        row_kind: ROW_CARD,
    })
}

pub fn project_clusters(
    conn: &Connection,
    cards: Vec<ArticleCard>,
    expanded: Option<i64>,
    threshold: i64,
    exclude_cluster_ids: &[i64],
    scope: FeedScope,
    excluded_feeds: &[i64],
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
    let members_map = visible_cluster_cards_map(
        conn,
        &wanted,
        threshold,
        scope,
        list_scope(conn)?,
        excluded_feeds,
    )?;
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
                .into_iter()
                .find(|m| m.id == card.id)
                .unwrap_or(card);
            // 折叠时只放代表文章；展开 / 收起只走标题行，不再另给「还有 N 篇」入口。
            out.push(best);
        }
    }
    Ok(out)
}

fn fill_cluster_members(members: &mut [ArticleCard], meta: &crate::cluster::ClusterMeta) {
    let span = format_span(meta.first_published_at, meta.last_published_at);
    let size = members.len() as i64;
    for member in members {
        member.cluster_size = size;
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
    cluster_members_in_scope(
        conn,
        cluster_id,
        scope,
        list_scope(conn)?,
        &focus_excluded_feeds(conn)?,
    )
}

pub(crate) fn cluster_members_in_scope(
    conn: &Connection,
    cluster_id: i64,
    scope: FeedScope,
    list_scope: ListScope,
    excluded_feeds: &[i64],
) -> Result<Vec<ArticleCard>> {
    let scope = focus_column_scope(scope);
    let threshold = hide_threshold(conn)?;
    let mut members = visible_cluster_cards_map(
        conn,
        &[cluster_id],
        threshold,
        scope,
        list_scope,
        excluded_feeds,
    )?
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
    list_scope: ListScope,
    excluded_feeds: &[i64],
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
    let visible = VISIBLE_SQL.replace("$threshold", &format!("?{threshold_idx}"));
    let list_sql = list_scope.filter_sql();
    let mut bind = cluster_ids.to_vec();
    bind.push(threshold);
    bind.extend(feed_id);
    let noisy_sql = exclude_feeds_sql(&mut bind, excluded_feeds);
    let sql = format!(
        "SELECT a.id, a.title, COALESCE(f.title, f.url), a.published_at, a.canonical_url,
                a.read, a.starred, a.score, a.interest_boost, COALESCE(a.summary, ''),
                a.feed_id, a.cluster_id, a.interest_reasons_json, COALESCE(r.boost, 0), r.reasons_json
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         LEFT JOIN article_rank r ON r.article_id = a.id
         WHERE a.cluster_id IN ({marks}) AND {visible}{feed_sql}{list_sql}{noisy_sql}
         ORDER BY a.cluster_id, COALESCE(a.published_at, 0) ASC, a.id ASC"
    );
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
        // The header has no single source; the UI words the size itself.
        feed_title: String::new(),
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
        explore: false,
        row_kind: ROW_HEADER,
    }
}

/// 相近报道组尾「全部标为已读」：这组在范围内可见的未读一次标完，返回改动的 id。
/// 可见与范围的口径同 `focus_cluster_members`；被规则隐藏的成员不碰。
pub fn mark_cluster_read(conn: &Connection, cluster_id: i64, scope: FeedScope) -> Result<Vec<i64>> {
    let scope = focus_column_scope(scope);
    let visible = VISIBLE_SQL.replace("$threshold", "?2");
    let (feed_sql, feed_id) = numbered_scope_sql(scope, 3);
    let list_sql = list_scope(conn)?.filter_sql();
    let mut bind = vec![cluster_id, hide_threshold(conn)?];
    bind.extend(feed_id);
    let noisy_sql = exclude_feeds_sql(&mut bind, &focus_excluded_feeds(conn)?);
    let sql = format!(
        "UPDATE articles SET read = 1 WHERE id IN (
            SELECT a.id FROM articles a WHERE a.cluster_id = ?1 AND a.read = 0 AND {visible}{feed_sql}{list_sql}{noisy_sql}
         ) RETURNING id"
    );
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

pub fn attach_noise(conn: &Connection, cards: &mut [ArticleCard]) -> Result<()> {
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

pub struct CitationMetadata {
    pub title: String,
    pub feed_title: String,
    pub canonical_url: Option<String>,
    pub temporary: bool,
}

pub fn citation_metadata(conn: &Connection, id: i64) -> Result<CitationMetadata> {
    conn.query_row(
        "SELECT a.title, COALESCE(f.title, f.url), a.canonical_url, a.expires_at IS NOT NULL
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         WHERE a.id = ?1",
        [id],
        |row| {
            Ok(CitationMetadata {
                title: row.get(0)?,
                feed_title: crate::feeds::display_title(&row.get::<_, String>(1)?, ""),
                canonical_url: row.get(2)?,
                temporary: row.get(3)?,
            })
        },
    )
    .optional()?
    .ok_or(Error::ArticleNotFound)
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
    crate::db::atomic_write(conn, |conn| {
        let n = conn.execute(
            "UPDATE articles SET read = ?1 WHERE id = ?2",
            params![i64::from(read), id],
        )?;
        if n == 0 {
            Err(Error::ArticleNotFound)
        } else {
            Ok(())
        }
    })
}

pub fn set_starred(conn: &Connection, id: i64, starred: bool) -> Result<()> {
    crate::db::atomic_write(conn, |conn| {
        let n = conn.execute(
            "UPDATE articles SET starred = ?1 WHERE id = ?2",
            params![i64::from(starred), id],
        )?;
        if n == 0 {
            Err(Error::ArticleNotFound)
        } else {
            Ok(())
        }
    })
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
    let visible = VISIBLE_SQL.replace("$threshold", "?2");
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
        let visible = VISIBLE_SQL.replace("$threshold", "?1");
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
            published_at: Some(now_ms() - 60_000),
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
            let map =
                visible_cluster_cards_map(&conn, &[7], 0, FeedScope::All, ListScope::All, &[])
                    .unwrap();
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
}
