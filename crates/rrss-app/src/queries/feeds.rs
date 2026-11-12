use rusqlite::{Connection, OptionalExtension};

use rrss_core::feed_health::{self, FeedHealth, HealthKind};
use rrss_core::feeds;

use crate::views::*;

pub fn feed_tree(conn: &Connection) -> rrss_core::Result<FeedTree> {
    let listed = feeds::list_feeds(conn)?;
    let icons = feed_icon_hrefs(conn)?;
    let views = feeds::list_folder_views(conn).unwrap_or_default();
    let noisy = feeds::noisy_feed_ids(conn).unwrap_or_default();
    let retention = feed_retention(conn)?;
    let health = feed_health::feed_health(conn, feeds::now_ms()).unwrap_or_default();
    // 只有手动收录源要这个标记：没有文章时订阅树不显示它。
    let empty_manual: Option<i64> = conn
        .query_row(
            "SELECT id FROM feeds f WHERE kind = 'manual'
             AND NOT EXISTS (SELECT 1 FROM articles a WHERE a.feed_id = f.id)",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let mut combo = vec!["未分类".to_string()];
    let rows: Vec<FeedView> = listed
        .into_iter()
        .map(|feed| FeedView {
            id: feed.id,
            icon_href: icons.get(&feed.id).cloned().unwrap_or_default(),
            title: feeds::display_title(feed.title.as_deref().unwrap_or(""), &feed.url),
            url: feed.url,
            error: feed
                .error
                .map(|e| feeds::humanize_refresh_error(&e))
                .unwrap_or_default(),
            noisy: noisy.contains(&feed.id),
            folder_id: feed.folder_id.unwrap_or(0),
            retention_days: retention.get(&feed.id).copied().flatten(),
            health: health
                .get(&feed.id)
                .map(|h| h.kind.as_str().to_string())
                .unwrap_or_default(),
            health_label: health.get(&feed.id).map(health_label).unwrap_or_default(),
            health_detail: health.get(&feed.id).map(health_detail).unwrap_or_default(),
            position: feed.position,
            empty: empty_manual == Some(feed.id),
        })
        .collect();
    // `list_feeds` 已按 (position, id) 排好，筛出来的子列表保持这个顺序。
    let uncategorized: Vec<FeedView> = rows.iter().filter(|r| r.folder_id == 0).cloned().collect();
    let folders: Vec<FolderView> = views
        .iter()
        .map(|v| {
            combo.push(v.label.clone());
            FolderView {
                id: v.id,
                name: v.name.clone(),
                label: v.label.clone(),
                indent: v.indent,
                position: v.position,
                feeds: rows
                    .iter()
                    .filter(|r| r.folder_id == v.id)
                    .cloned()
                    .collect(),
            }
        })
        .collect();
    Ok(FeedTree {
        folders,
        uncategorized,
        folder_combo: combo,
    })
}

pub(super) fn feed_icon_hrefs(
    conn: &Connection,
) -> rrss_core::Result<std::collections::HashMap<i64, String>> {
    if !rrss_core::feed_icon::enabled(conn)? {
        return Ok(Default::default());
    }
    let mut stmt = conn.prepare("SELECT feed_id, id, local_path FROM media WHERE feed_id IS NOT NULL AND local_path IS NOT NULL")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut icons = std::collections::HashMap::new();
    for row in rows {
        let (feed, media, path) = row?;
        if std::path::Path::new(&path).is_file() {
            icons.insert(feed, super::media::media_href(media));
        }
    }
    Ok(icons)
}

/// 单个源的本地图标；阅读器打开一篇只查这一行，不扫全部源。
pub(super) fn feed_icon_href(conn: &Connection, feed_id: i64) -> rrss_core::Result<String> {
    if !rrss_core::feed_icon::enabled(conn)? {
        return Ok(String::new());
    }
    let row: Option<(i64, String)> = conn
        .query_row(
            "SELECT id, local_path FROM media WHERE feed_id = ?1 AND local_path IS NOT NULL",
            [feed_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row
        .filter(|(_, path)| std::path::Path::new(path).is_file())
        .map(|(media, _)| super::media::media_href(media))
        .unwrap_or_default())
}

fn feed_retention(
    conn: &Connection,
) -> rrss_core::Result<std::collections::HashMap<i64, Option<i64>>> {
    let mut stmt = conn.prepare("SELECT id, retention_days FROM feeds")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

fn health_label(h: &FeedHealth) -> String {
    match h.kind {
        HealthKind::Failing => format!("连续失败 {} 次 · {} 天", h.fail_count, h.failing_days),
        HealthKind::Stale => format!("{} 天没有新文章", h.quiet_days),
        HealthKind::Noisy => "多数内容被隐藏".into(),
    }
}

fn health_detail(h: &FeedHealth) -> String {
    let success = h
        .last_success_at
        .map(|t| format!("最近成功：{}", super::fmt_when(t)))
        .unwrap_or_else(|| "还没有成功抓取过".into());
    match h.kind {
        HealthKind::Failing => format!(
            "最近错误：{}\n{success}",
            h.error
                .as_deref()
                .map(feeds::humanize_refresh_error)
                .unwrap_or_default()
        ),
        HealthKind::Stale => {
            let pace = h
                .avg_interval_days
                .map(|d| format!("平时约 {} 天一篇；", d.round().max(1.0)))
                .unwrap_or_default();
            let last = h
                .last_article_at
                .map(|t| format!("最近一篇入库于 {}。", super::fmt_when(t)))
                .unwrap_or_else(|| "订阅以来还没有入库过文章。".into());
            let fetch = if h.error.is_some() {
                "最近一次抓取失败"
            } else {
                "抓取正常"
            };
            format!("{pace}{last}\n{fetch}，{success}")
        }
        HealthKind::Noisy => format!(
            "近 {} 天大部分文章被规则隐藏或被踩。可以退订，或在规则页调整。",
            feeds::NOISE_WINDOW_DAYS
        ),
    }
}
