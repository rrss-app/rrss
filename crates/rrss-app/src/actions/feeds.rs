//! Subscription, folder and OPML write orchestration.

use rrss_core::error::rules::RuleProblem;
use rrss_core::error::settings::SettingProblem;
use rrss_core::feeds::{self, SubscribeRequest};
use rrss_core::{db, opml};
use rusqlite::Connection;

use crate::views::SaveFeedArgs;

pub fn add_feed(conn: &Connection, url: &str) -> rrss_core::Result<i64> {
    feeds::subscribe(
        conn,
        SubscribeRequest {
            url: url.trim().to_string(),
            folder_id: None,
            as_rsshub: false,
            rsshub_instance_id: None,
        },
    )
}

/// 订阅行内编辑的复合保存：名称 / 地址 / 文件夹同一事务，任一失败整体回滚，
/// 成功后在同一事务内读取订阅树返回。
pub fn save_feed(
    conn: &Connection,
    args: &SaveFeedArgs,
) -> rrss_core::Result<crate::views::FeedTree> {
    let tx = db::write_transaction(conn)?;
    feeds::get_feed(&tx, args.id)?;
    if let Some(title) = &args.title {
        feeds::rename_feed(&tx, args.id, title.trim())?;
    }
    if let Some(url) = &args.url {
        feeds::update_feed_url(&tx, args.id, url.trim())?;
    }
    if let Some(folder_id) = args.folder_id {
        feeds::set_folder(&tx, args.id, Some(folder_id).filter(|id| *id > 0))?;
    }
    if let Some(retention) = &args.retention {
        let days = match retention.trim() {
            "follow" => None,
            "keep" => Some(0),
            n => Some(n.parse::<i64>().ok().filter(|d| *d > 0).ok_or_else(|| {
                rrss_core::Error::InvalidRule(RuleProblem::RetentionInvalid { value: n.into() })
            })?),
        };
        rrss_core::retention::set_feed_days(&tx, args.id, days)?;
    }
    if let Some(interval) = &args.interval {
        let minutes = match interval.trim() {
            "follow" => None,
            n => Some(n.parse::<i64>().map_err(|_| {
                rrss_core::Error::InvalidSetting(SettingProblem::IntegerRequired {
                    value: n.into(),
                })
            })?),
        };
        rrss_core::fetch_interval::set_feed_minutes(&tx, args.id, minutes)?;
    }
    if let Some(auto) = &args.auto_translate {
        let value = rrss_core::translations::FeedAuto::parse(auto).ok_or_else(|| {
            rrss_core::Error::InvalidSetting(SettingProblem::OptionInvalid {
                value: auto.trim().into(),
                options: ["follow", "always", "never"].map(String::from).to_vec(),
            })
        })?;
        rrss_core::translations::set_feed_auto(&tx, args.id, value)?;
    }
    let tree = crate::queries::feed_tree(&tx)?;
    tx.commit()?;
    Ok(tree)
}

pub fn unsubscribe(conn: &Connection, id: i64) -> rrss_core::Result<()> {
    feeds::unsubscribe(conn, id)?;
    tracing::info!(feed_id = id, "feed unsubscribed");
    Ok(())
}

/// 批量退订（源健康里的「退订长期失败的源」），一个写事务。
pub fn unsubscribe_many(
    conn: &Connection,
    ids: &[i64],
) -> rrss_core::Result<crate::views::FeedTree> {
    let tx = db::write_transaction(conn)?;
    for id in ids {
        feeds::unsubscribe(&tx, *id)?;
    }
    let tree = crate::queries::feed_tree(&tx)?;
    tx.commit()?;
    tracing::info!(count = ids.len(), "feeds unsubscribed");
    Ok(tree)
}

/// 忽略源健康提示：同一种状态 30 天内不再提示。
pub fn dismiss_feed_health(
    conn: &Connection,
    id: i64,
) -> rrss_core::Result<crate::views::FeedTree> {
    rrss_core::feed_health::dismiss(conn, id, feeds::now_ms())?;
    crate::queries::feed_tree(conn)
}

pub fn create_folder(
    conn: &Connection,
    name: &str,
    parent_folder_id: Option<i64>,
) -> rrss_core::Result<i64> {
    feeds::create_folder_first(conn, name.trim(), parent_folder_id)
}

pub fn move_tree_items(
    conn: &Connection,
    items: Vec<String>,
    parent_folder_id: Option<i64>,
    before: Option<String>,
) -> rrss_core::Result<Vec<crate::views::TreeLayout>> {
    let items = items
        .iter()
        .map(|item| rrss_core::tree_order::TreeItem::parse(item))
        .collect::<rrss_core::Result<Vec<_>>>()?;
    let before = before
        .as_deref()
        .map(rrss_core::tree_order::TreeItem::parse)
        .transpose()?;
    let layout = rrss_core::tree_order::move_tree_items(conn, &items, parent_folder_id, before)?;
    Ok(layout.entries.into_iter().map(Into::into).collect())
}

pub fn restore_tree_layout(
    conn: &Connection,
    layout: Vec<crate::views::TreeLayout>,
) -> rrss_core::Result<()> {
    let entries = layout
        .iter()
        .map(TryInto::try_into)
        .collect::<rrss_core::Result<Vec<_>>>()?;
    rrss_core::tree_order::restore_tree_layout(conn, &rrss_core::tree_order::TreeLayout { entries })
}

pub fn rename_folder(conn: &Connection, id: i64, name: &str) -> rrss_core::Result<()> {
    feeds::rename_folder(conn, id, name.trim())
}

pub fn delete_folder(conn: &Connection, id: i64) -> rrss_core::Result<()> {
    feeds::delete_folder(conn, id)?;
    tracing::info!(folder_id = id, "folder deleted");
    Ok(())
}

pub fn import_opml(conn: &Connection, xml: &str) -> rrss_core::Result<opml::ImportReport> {
    opml::import_opml(conn, xml)
}

pub fn export_opml(conn: &Connection) -> rrss_core::Result<String> {
    opml::export_opml(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_temp() -> (tempfile::TempDir, rrss_core::db::PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("feeds.db")).unwrap();
        (dir, conn)
    }

    #[test]
    fn folder_move_can_be_undone_and_invalid_restore_keeps_the_tree() {
        let (_dir, conn) = open_temp();
        let folder = create_folder(&conn, "Reading", None).unwrap();
        let feed = add_feed(&conn, "https://feeds.example/feed.xml").unwrap();
        let tree = || serde_json::to_value(crate::queries::feed_tree(&conn).unwrap()).unwrap();
        let before = tree();
        let layout = crate::actions::move_tree_items(
            &conn,
            vec![format!("feed:{feed}")],
            Some(folder),
            None,
        )
        .unwrap();
        assert_eq!(
            find_feed(&crate::queries::feed_tree(&conn).unwrap(), feed).folder_id,
            folder
        );
        let moved = tree();
        let mut invalid = layout.clone();
        invalid.push(crate::views::TreeLayout {
            item: "unknown:1".into(),
            parent_folder_id: None,
            position: 0,
        });
        assert!(matches!(
            crate::actions::restore_tree_layout(&conn, invalid),
            Err(rrss_core::Error::InvalidTreeOrder(_))
        ));
        assert_eq!(tree(), moved);
        crate::actions::restore_tree_layout(&conn, layout).unwrap();
        assert_eq!(tree(), before);
    }

    #[test]
    fn feed_tree_carries_retention_and_health_and_dismiss_hides_it() {
        let (_dir, conn) = open_temp();
        let day = 86_400_000;
        let now = rrss_core::feeds::now_ms();
        conn.execute(
            "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://a/rss', ?1)",
            [now - 100 * day],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO feeds (id, url, created_at) VALUES (2, 'https://b/rss', ?1)",
            [now - 100 * day],
        )
        .unwrap();
        conn.execute(
            "UPDATE feeds SET fail_count = 12, failing_since = ?1, error = 'HTTP 404' WHERE id = 1",
            [now - 9 * day],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO articles (feed_id, title, created_at) VALUES (2, 't', ?1)",
            [now - day],
        )
        .unwrap();
        let tree = save_feed(
            &conn,
            &SaveFeedArgs {
                id: 2,
                title: None,
                url: None,
                folder_id: None,
                retention: Some("keep".into()),
                interval: None,
                auto_translate: None,
            },
        )
        .unwrap();
        let a = find_feed(&tree, 1);
        let wire = serde_json::to_value(&a.health).unwrap();
        assert_eq!(wire["code"], "feed_health_failing");
        assert_eq!(wire["params"]["fail_count"], 12);
        assert_eq!(wire["params"]["failing_days"], 9);
        assert!(wire["params"]["last_success_at"].is_null());
        let b = find_feed(&tree, 2);
        assert!(b.health.is_none());
        assert_eq!(b.retention_days, Some(0));
        assert!(
            save_feed(
                &conn,
                &SaveFeedArgs {
                    id: 2,
                    title: None,
                    url: None,
                    folder_id: None,
                    retention: Some("abc".into()),
                    interval: None,
                    auto_translate: None,
                }
            )
            .is_err()
        );
        let tree = save_feed(
            &conn,
            &SaveFeedArgs {
                id: 2,
                title: None,
                url: None,
                folder_id: None,
                retention: Some("30".into()),
                interval: None,
                auto_translate: None,
            },
        )
        .unwrap();
        assert_eq!(find_feed(&tree, 2).retention_days, Some(30));
        assert_eq!(find_feed(&tree, 2).interval_minutes, None);

        let interval = |value: &str| SaveFeedArgs {
            id: 2,
            title: Some("renamed".into()),
            url: None,
            folder_id: None,
            retention: None,
            interval: Some(value.into()),
            auto_translate: None,
        };
        let tree = save_feed(&conn, &interval("5")).unwrap();
        assert_eq!(find_feed(&tree, 2).interval_minutes, Some(5));
        for bad in ["0", "abc"] {
            assert!(save_feed(&conn, &interval(bad)).is_err());
        }
        let tree = save_feed(&conn, &interval("follow")).unwrap();
        assert_eq!(find_feed(&tree, 2).interval_minutes, None);
        let translate = |value: &str| SaveFeedArgs {
            id: 2,
            title: None,
            url: None,
            folder_id: None,
            retention: None,
            interval: None,
            auto_translate: Some(value.into()),
        };
        let tree = save_feed(&conn, &translate("never")).unwrap();
        assert_eq!(find_feed(&tree, 2).auto_translate.as_deref(), Some("never"));
        assert!(save_feed(&conn, &translate("sometimes")).is_err());
        let tree = save_feed(&conn, &translate("follow")).unwrap();
        assert_eq!(find_feed(&tree, 2).auto_translate, None);
        // Never fetched: no times yet. After a fetch both times are exposed.
        assert_eq!(
            (
                find_feed(&tree, 2).last_fetched_at,
                find_feed(&tree, 2).next_fetch_at
            ),
            (None, None)
        );
        conn.execute(
            "UPDATE feeds SET last_fetched_at = 1000, next_fetch_at = 3601000 WHERE id = 2",
            [],
        )
        .unwrap();
        let tree = crate::queries::feed_tree(&conn).unwrap();
        assert_eq!(
            (
                find_feed(&tree, 2).last_fetched_at,
                find_feed(&tree, 2).next_fetch_at
            ),
            (Some(1000), Some(3_601_000))
        );

        let tree = dismiss_feed_health(&conn, 1).unwrap();
        assert!(find_feed(&tree, 1).health.is_none());
        let tree = unsubscribe_many(&conn, &[1, 2]).unwrap();
        assert!(tree.uncategorized.is_empty());
    }

    #[test]
    fn save_feed_writes_title_url_and_folder_together_and_returns_the_tree() {
        let (_dir, conn) = open_temp();
        let id = seed_feed(&conn, "https://news.example.com/a.xml");
        let folder = create_folder(&conn, "科技", None).unwrap();
        let tree = save_feed(
            &conn,
            &SaveFeedArgs {
                id,
                title: Some(" 自定义名 ".into()),
                url: Some("https://news.example.com/b.xml".into()),
                folder_id: Some(folder),
                retention: None,
                interval: None,
                auto_translate: None,
            },
        )
        .unwrap();
        let row = find_feed(&tree, id);
        assert_eq!(row.title, "自定义名");
        assert_eq!(row.url, "https://news.example.com/b.xml");
        assert_eq!(row.folder_id, folder);
        assert!(tree.uncategorized.iter().all(|f| f.id != id));

        // `None` 字段不动；`Some(0)` 移出文件夹。
        let tree = save_feed(
            &conn,
            &SaveFeedArgs {
                id,
                title: None,
                url: None,
                folder_id: Some(0),
                retention: None,
                interval: None,
                auto_translate: None,
            },
        )
        .unwrap();
        let row = find_feed(&tree, id);
        assert_eq!(row.title, "自定义名");
        assert_eq!(row.url, "https://news.example.com/b.xml");
        assert_eq!(row.folder_id, 0);
    }

    #[test]
    fn save_feed_rolls_back_title_and_url_when_the_folder_is_missing() {
        let (_dir, conn) = open_temp();
        let id = seed_feed(&conn, "https://news.example.com/a.xml");
        let before = find_feed(&crate::queries::feed_tree(&conn).unwrap(), id);
        let err = save_feed(
            &conn,
            &SaveFeedArgs {
                id,
                title: Some("不该落库".into()),
                url: Some("https://news.example.com/changed.xml".into()),
                folder_id: Some(4040),
                retention: None,
                interval: None,
                auto_translate: None,
            },
        )
        .unwrap_err();
        assert!(!err.to_string().is_empty());
        let after = find_feed(&crate::queries::feed_tree(&conn).unwrap(), id);
        assert_eq!(after.title, before.title);
        assert_eq!(after.url, before.url);
        assert_eq!(after.folder_id, 0);
    }

    #[test]
    fn save_feed_rolls_back_title_when_the_url_duplicates_another_feed() {
        let (_dir, conn) = open_temp();
        let id = seed_feed(&conn, "https://news.example.com/a.xml");
        seed_feed(&conn, "https://news.example.com/taken.xml");
        let err = save_feed(
            &conn,
            &SaveFeedArgs {
                id,
                title: Some("不该落库".into()),
                url: Some("https://news.example.com/taken.xml".into()),
                folder_id: None,
                retention: None,
                interval: None,
                auto_translate: None,
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, rrss_core::error::Error::DuplicateFeed(_)),
            "{err}"
        );
        let after = find_feed(&crate::queries::feed_tree(&conn).unwrap(), id);
        assert_ne!(after.title, "不该落库");
        assert_eq!(after.url, "https://news.example.com/a.xml");
    }

    #[test]
    fn save_feed_rejects_a_missing_feed() {
        let (_dir, conn) = open_temp();
        let err = save_feed(
            &conn,
            &SaveFeedArgs {
                id: 77,
                title: Some("x".into()),
                url: None,
                folder_id: None,
                retention: None,
                interval: None,
                auto_translate: None,
            },
        )
        .unwrap_err();
        assert!(
            matches!(err, rrss_core::error::Error::FeedNotFound),
            "{err}"
        );
    }

    fn seed_feed(conn: &Connection, url: &str) -> i64 {
        rrss_core::feeds::subscribe(
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

    fn find_feed(tree: &crate::views::FeedTree, id: i64) -> crate::views::FeedView {
        tree.uncategorized
            .iter()
            .chain(tree.folders.iter().flat_map(|f| f.feeds.iter()))
            .find(|f| f.id == id)
            .cloned()
            .expect("feed in tree")
    }
}
