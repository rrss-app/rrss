//! 焦点页组装：候选池与排列、抽查与探索。卡片读取、可见性与相近报道的分组投影在 core。

use std::collections::HashSet;

use rrss_core::articles::{
    ArticleCard, FocusFilter, ListScope, VISIBLE_SQL, attach_noise, focus_column_scope,
    focus_filter, hide_threshold, list_scope, numbered_scope_sql, project_clusters,
    query_ranked_cards,
};
use rrss_core::feed_scope::FeedScope;
use rrss_core::feeds::now_ms;
use rrss_core::{Result, interests};
use rusqlite::{Connection, params_from_iter};

mod order;
mod pool;
#[cfg(test)]
mod tests;

pub(crate) use order::SAME_FEED_WINDOW;

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
        None,
        None,
    )?;
    for (card, _) in focus {
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
            None,
            None,
        )?;
        for (card, _) in extra {
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

pub const FOCUS_PAGE_SIZE: i64 = 40;

pub fn list_focus_stream(
    conn: &Connection,
    limit: i64,
    expanded_cluster_id: Option<i64>,
) -> Result<Vec<ArticleCard>> {
    Ok(list_focus_page(conn, limit, expanded_cluster_id, &[], &[], FeedScope::All)?.0)
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
        rrss_core::feeds::noisy_feed_ids(conn)?
            .into_iter()
            .collect()
    } else {
        Vec::new()
    };
    let want = limit.max(1);
    let first_page = exclude_ids.is_empty() && exclude_cluster_ids.is_empty();
    rrss_core::db::read_snapshot(conn, |conn| {
        let settings = crate::store::settings(conn)?;
        let exploring = first_page
            && scope == FeedScope::All
            && list_scope(conn)? != ListScope::Starred
            && settings.fitted;
        let mut explore_pool = Vec::new();
        let candidates = query_focus_cards(
            conn,
            filter,
            threshold,
            boost_on,
            want.saturating_add(1),
            exclude_ids,
            if group_on { exclude_cluster_ids } else { &[] },
            &exclude_feeds,
            scope,
            Some((group_on, &settings)),
            exploring.then_some(&mut explore_pool),
        )?;
        let has_more = candidates.len() > want as usize;
        let rerank_effective = rerank_on
            && (interests::interest_ready(conn).unwrap_or(false)
                || candidates
                    .iter()
                    .any(|(c, _)| c.score > 0 || (boost_on && c.interest_boost > 0)));
        let mut cards = order::select(
            conn,
            &settings,
            candidates,
            want as usize,
            exclude_ids,
            rerank_effective,
            group_on,
        )?;
        if group_on {
            cards = project_clusters(
                conn,
                cards,
                expanded_cluster_id,
                threshold,
                exclude_cluster_ids,
                scope,
            )?;
        }
        if scope == FeedScope::All {
            crate::spot_check::inject(
                conn,
                &mut cards,
                filter,
                threshold,
                &exclude_feeds,
                first_page,
            )?;
        }
        if exploring && crate::tuning::current().exploration {
            crate::explore::inject(conn, &mut cards, &explore_pool, threshold, now_ms())?;
        }
        attach_noise(conn, &mut cards)?;
        Ok((cards, has_more))
    })
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
    // 焦点页：是否分组，以及这次请求读到的排序开关；预载不经候选池，传 None。
    group_for_page: Option<(bool, &crate::store::Settings)>,
    explore_pool: Option<&mut Vec<i64>>,
) -> Result<Vec<(ArticleCard, i64)>> {
    let boost = i64::from(boost_on);
    let noisy_sql = if exclude_feed_ids.is_empty() {
        String::new()
    } else {
        let start = 4;
        let marks: String = (0..exclude_feed_ids.len())
            .map(|i| format!("?{}", start + i))
            .collect::<Vec<_>>()
            .join(",");
        format!(" AND a.feed_id NOT IN ({marks})")
    };
    let list_scope = list_scope(conn)?;
    let list_sql = list_scope.filter_sql();
    let scope_index = 4 + exclude_feed_ids.len();
    let (feed_sql, feed_id) = numbered_scope_sql(scope, scope_index);
    let rank_score = rrss_core::rank_score::SCORE_SQL;
    let member_score = rrss_core::rank_score::MEMBER_SQL;
    let visible = VISIBLE_SQL.replace("$threshold", "?2");
    let filter_sql = match filter {
        FocusFilter::FocusOnly => format!("{visible} AND {member_score} > 0"),
        FocusFilter::HideFaded => visible,
        FocusFilter::ShowAll => "1".into(),
    };
    let eligible = format!(
        "a.expires_at IS NULL AND {filter_sql}
           {list_sql}
           {feed_sql}
           {noisy_sql}"
    );
    let mut bind = vec![limit, threshold, boost];
    bind.extend_from_slice(exclude_feed_ids);
    if let Some(id) = feed_id {
        bind.push(id);
    }
    if let Some((group, settings)) = group_for_page {
        // Candidate selection and card loading used to be one SQL statement.
        // Preserve that snapshot even when a background writer changes articles.
        return rrss_core::db::read_snapshot(conn, |conn| {
            let picked = order::candidates(
                conn,
                settings,
                &eligible,
                group,
                list_scope,
                &bind,
                exclude_ids,
                exclude_cluster_ids,
                explore_pool,
            )?;
            query_ranked_cards(
                conn,
                "SELECT json_extract(value,'$[0]') AS id,
                        json_extract(value,'$[1]') AS priority,CAST(key AS INTEGER) AS position FROM json_each(?1)",
                [serde_json::to_string(&picked).expect("integer candidate pairs serialize")],
            )
        });
    }
    let ranked = format!(
        "SELECT a.id, {rank_score} AS priority,0 AS position FROM articles a
         LEFT JOIN article_rank r ON r.article_id=a.id WHERE {eligible}
         ORDER BY priority DESC, COALESCE(a.published_at,0) DESC, a.id DESC LIMIT ?1"
    );
    query_ranked_cards(conn, &ranked, params_from_iter(bind))
}
