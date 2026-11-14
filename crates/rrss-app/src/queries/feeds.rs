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
            error: feed.error.map(|e| crate::ui_error::feeds::stored_error(&e)),
            noisy: noisy.contains(&feed.id),
            folder_id: feed.folder_id.unwrap_or(0),
            retention_days: retention.get(&feed.id).copied().flatten(),
            health: health.get(&feed.id).map(health_notice),
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

fn health_notice(h: &FeedHealth) -> crate::ui_error::feeds::FeedHealthNotice {
    use crate::ui_error::feeds::FeedHealthNotice;
    match h.kind {
        HealthKind::Failing => FeedHealthNotice::Failing {
            fail_count: h.fail_count,
            failing_days: h.failing_days,
            last_success_at: h.last_success_at,
        },
        HealthKind::Stale => FeedHealthNotice::Stale {
            quiet_days: h.quiet_days,
            avg_interval_days: h.avg_interval_days,
            failed: h.error.is_some(),
            last_article_at: h.last_article_at,
            last_success_at: h.last_success_at,
        },
        HealthKind::Noisy => FeedHealthNotice::Noisy {
            window_days: feeds::NOISE_WINDOW_DAYS,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tree_reads_old_text_and_new_error_records_and_carries_raw_health_data() {
        let dir = tempfile::tempdir().unwrap();
        let conn = rrss_core::db::open_at(&dir.path().join("rrss.db")).unwrap();
        let now = feeds::now_ms();
        let record = feeds::refresh_error_json(&rrss_core::Error::Http {
            status: 503,
            message: String::new(),
        });
        for (id, error) in [(1, "fixture legacy failure"), (2, record.as_str())] {
            conn.execute(
                "INSERT INTO feeds(id,url,created_at,error,fail_count,failing_since,last_success_at)
                 VALUES(?1,?2,?3,?4,12,?5,?6)",
                rusqlite::params![id, format!("https://feed{id}.example/rss"), now - 100 * 86_400_000, error,
                    now - 9 * 86_400_000, now - 10 * 86_400_000],
            ).unwrap();
        }
        let tree = feed_tree(&conn).unwrap();
        let old = tree.uncategorized.iter().find(|feed| feed.id == 1).unwrap();
        assert_eq!(old.error.as_ref().unwrap().code(), "legacy_text");
        assert_eq!(old.error.as_ref().unwrap().detail, "fixture legacy failure");
        let new = tree.uncategorized.iter().find(|feed| feed.id == 2).unwrap();
        assert_eq!(new.error.as_ref().unwrap().code(), "feed_refresh_http");
        let wire = serde_json::to_value(new).unwrap();
        assert_eq!(wire["error"]["params"], json!({ "status": 503 }));
        assert_eq!(wire["health"]["code"], "feed_health_failing");
        assert_eq!(
            wire["health"]["params"],
            json!({ "fail_count": 12, "failing_days": 9, "last_success_at": now - 10 * 86_400_000 })
        );
        assert!(wire.get("health_label").is_none());
        assert!(wire.get("health_detail").is_none());
        let persisted: String = conn
            .query_row("SELECT error FROM feeds WHERE id=1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(persisted, "fixture legacy failure");
    }
}
