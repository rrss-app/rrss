//! 订阅树顺序：同一父级里文件夹与源共用 `position`。
//!
//! `move_tree_items` 在一个写事务里改父级并重新连续编号，返回移动前这些父级的布局，供撤销写回。

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::atomic_write;
use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreeItem {
    Folder(i64),
    Feed(i64),
}

impl TreeItem {
    pub fn parse(raw: &str) -> Result<Self> {
        let invalid =
            || Error::InvalidTreeOrder("订阅树项应为 folder:<正整数> 或 feed:<正整数>".into());
        let (kind, id) = raw.split_once(':').ok_or_else(invalid)?;
        if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(invalid());
        }
        let id: i64 = id.parse().map_err(|_| invalid())?;
        match kind {
            "folder" if id > 0 => Ok(Self::Folder(id)),
            "feed" if id > 0 => Ok(Self::Feed(id)),
            _ => Err(invalid()),
        }
    }

    pub fn key(self) -> String {
        match self {
            Self::Folder(id) => format!("folder:{id}"),
            Self::Feed(id) => format!("feed:{id}"),
        }
    }

    /// 所在表、指向父文件夹的列与 id。两张表其余写法相同。
    fn row(self) -> (&'static str, &'static str, i64) {
        match self {
            Self::Folder(id) => ("folders", "parent_id", id),
            Self::Feed(id) => ("feeds", "folder_id", id),
        }
    }

    fn not_found(self) -> Error {
        match self {
            Self::Folder(_) => Error::FolderNotFound,
            Self::Feed(_) => Error::FeedNotFound,
        }
    }

    /// 同一 position 时文件夹在前，再按 id。
    fn sort_key(self) -> (u8, i64) {
        match self {
            Self::Folder(id) => (0, id),
            Self::Feed(id) => (1, id),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreePlacement {
    pub item: TreeItem,
    pub parent: Option<i64>,
    pub position: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeLayout {
    pub entries: Vec<TreePlacement>,
}

pub fn move_tree_items(
    conn: &Connection,
    items: &[TreeItem],
    parent: Option<i64>,
    before: Option<TreeItem>,
) -> Result<TreeLayout> {
    atomic_write(conn, |conn| move_within(conn, items, parent, before))
}

pub fn restore_tree_layout(conn: &Connection, layout: &TreeLayout) -> Result<()> {
    atomic_write(conn, |conn| {
        for slot in &layout.entries {
            let (table, parent_col, id) = slot.item.row();
            conn.execute(
                &format!("UPDATE {table} SET {parent_col}=?1, position=?2 WHERE id=?3"),
                params![slot.parent, slot.position, id],
            )?;
        }
        Ok(())
    })
}

/// 新项放在父级末尾：`max(position)+1`，空父级为 0。
pub(crate) fn end_position(conn: &Connection, parent: Option<i64>) -> Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(position), -1) + 1 FROM (
            SELECT position FROM folders WHERE parent_id IS ?1
            UNION ALL
            SELECT position FROM feeds WHERE folder_id IS ?1
        )",
        [parent],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

/// 同父级已有项的 position 加 1，给最前面的新文件夹让出 0。
pub(crate) fn shift_front(conn: &Connection, parent: Option<i64>) -> Result<()> {
    for (table, parent_col) in [("folders", "parent_id"), ("feeds", "folder_id")] {
        conn.execute(
            &format!("UPDATE {table} SET position = position + 1 WHERE {parent_col} IS ?1"),
            [parent],
        )?;
    }
    Ok(())
}

fn move_within(
    conn: &Connection,
    items: &[TreeItem],
    parent: Option<i64>,
    before: Option<TreeItem>,
) -> Result<TreeLayout> {
    if items.is_empty() {
        return Err(Error::InvalidTreeOrder("没有要移动的项".into()));
    }
    if items.iter().collect::<HashSet<_>>().len() != items.len() {
        return Err(Error::InvalidTreeOrder("移动列表有重复项".into()));
    }
    if let Some(id) = parent {
        parent_of(conn, TreeItem::Folder(id))?;
    }
    let mut parents = vec![parent];
    for item in items {
        let origin = parent_of(conn, *item)?;
        if !parents.contains(&origin) {
            parents.push(origin);
        }
    }
    let moved_folders: HashSet<i64> = items
        .iter()
        .filter_map(|item| match item {
            TreeItem::Folder(id) => Some(*id),
            TreeItem::Feed(_) => None,
        })
        .collect();
    if !moved_folders.is_empty() && enters_moved_folder(conn, parent, &moved_folders)? {
        return Err(Error::InvalidTreeOrder("不能把文件夹移进自身或子孙".into()));
    }
    if let Some(marker) = before
        && (items.contains(&marker) || parent_of(conn, marker)? != parent)
    {
        return Err(Error::InvalidTreeOrder("插入位置不在目标父级".into()));
    }
    let mut entries = Vec::new();
    for group in &parents {
        entries.extend(children(conn, *group)?);
    }
    for item in items {
        let (table, parent_col, id) = item.row();
        conn.execute(
            &format!("UPDATE {table} SET {parent_col}=?1 WHERE id=?2"),
            params![parent, id],
        )?;
    }
    for group in &parents {
        let mut order: Vec<TreeItem> = children(conn, *group)?
            .into_iter()
            .map(|row| row.item)
            .filter(|item| !items.contains(item))
            .collect();
        if *group == parent {
            let index = match before {
                Some(marker) => order
                    .iter()
                    .position(|item| *item == marker)
                    .ok_or_else(|| Error::InvalidTreeOrder("插入位置不在目标父级".into()))?,
                None => order.len(),
            };
            order.splice(index..index, items.iter().copied());
        }
        for (position, item) in order.into_iter().enumerate() {
            let (table, _, id) = item.row();
            conn.execute(
                &format!("UPDATE {table} SET position=?1 WHERE id=?2"),
                params![position as i64, id],
            )?;
        }
    }
    Ok(TreeLayout { entries })
}

/// 某父级下的文件夹与源，按 (position, 文件夹在前, id) 排好。
fn children(conn: &Connection, parent: Option<i64>) -> Result<Vec<TreePlacement>> {
    let mut out = Vec::new();
    for (table, parent_col) in [("folders", "parent_id"), ("feeds", "folder_id")] {
        let mut stmt = conn.prepare(&format!(
            "SELECT id, position FROM {table} WHERE {parent_col} IS ?1"
        ))?;
        for row in stmt.query_map(params![parent], |row| Ok((row.get(0)?, row.get(1)?)))? {
            let (id, position) = row?;
            let item = if table == "folders" {
                TreeItem::Folder(id)
            } else {
                TreeItem::Feed(id)
            };
            out.push(TreePlacement {
                item,
                parent,
                position,
            });
        }
    }
    out.sort_by_key(|row| (row.position, row.item.sort_key()));
    Ok(out)
}

fn parent_of(conn: &Connection, item: TreeItem) -> Result<Option<i64>> {
    let (table, parent_col, id) = item.row();
    conn.query_row(
        &format!("SELECT {parent_col} FROM {table} WHERE id=?1"),
        [id],
        |row| row.get(0),
    )
    .optional()?
    .ok_or_else(|| item.not_found())
}

fn enters_moved_folder(
    conn: &Connection,
    start: Option<i64>,
    moved: &HashSet<i64>,
) -> Result<bool> {
    let mut current = start;
    for _ in 0..10_000 {
        let Some(id) = current else {
            return Ok(false);
        };
        if moved.contains(&id) {
            return Ok(true);
        }
        current = parent_of(conn, TreeItem::Folder(id))?;
    }
    Err(Error::InvalidTreeOrder("文件夹层级过深".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_at;
    use crate::feeds::{SubscribeRequest, create_folder_first, set_folder, subscribe};
    use rusqlite::Connection;

    fn open() -> (tempfile::TempDir, crate::db::PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_at(&dir.path().join("tree.db")).unwrap();
        (dir, conn)
    }

    fn seed(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO folders(id, parent_id, name, position, created_at) VALUES
                (1, NULL, 'A', 0, 0),
                (2, NULL, 'B', 1, 0),
                (3, 1, 'A1', 0, 0),
                (4, 3, 'A1a', 0, 0);
             INSERT INTO feeds(id, url, folder_id, position, created_at) VALUES
                (1, 'https://a.example/rss', NULL, 2, 0),
                (2, 'https://b.example/rss', 1, 1, 0),
                (3, 'https://c.example/rss', NULL, 3, 0);",
        )
        .unwrap();
    }

    fn placed(conn: &Connection) -> Vec<(Option<i64>, String, i64)> {
        let mut rows = Vec::new();
        for (sql, prefix) in [
            ("SELECT parent_id, id, position FROM folders", "folder"),
            ("SELECT folder_id, id, position FROM feeds", "feed"),
        ] {
            let mut stmt = conn.prepare(sql).unwrap();
            let iter = stmt
                .query_map([], |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                })
                .unwrap();
            for row in iter {
                let (parent, id, position) = row.unwrap();
                rows.push((parent, format!("{prefix}:{id}"), position));
            }
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)).then(a.1.cmp(&b.1)));
        rows
    }

    #[test]
    fn v36_keeps_each_parents_previous_display_order() {
        let conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "foreign_keys", true).unwrap();
        conn.execute_batch(include_str!("../../../testdata/schema/v33.sql"))
            .unwrap();
        conn.execute_batch(
            "INSERT INTO folders(id, parent_id, name, position, created_at) VALUES
                (1, NULL, 'A', 5, 0),
                (2, NULL, 'B', 1, 0),
                (4, NULL, 'D', 1, 0),
                (3, 1, 'C', 9, 0);
             INSERT INTO feeds(id, url, folder_id, created_at) VALUES
                (10, 'https://root-b.example/rss', NULL, 0),
                (2, 'https://root-a.example/rss', NULL, 0),
                (8, 'https://child-b.example/rss', 1, 0),
                (7, 'https://child-a.example/rss', 1, 0);",
        )
        .unwrap();
        let expected = legacy_display_order(&conn);
        conn.pragma_update(None, "user_version", 33).unwrap();
        crate::db::migrations::migrate(&conn).unwrap();
        assert_eq!(placed(&conn), expected);
    }

    fn legacy_display_order(conn: &Connection) -> Vec<(Option<i64>, String, i64)> {
        let folders: Vec<(i64, Option<i64>)> = conn
            .prepare("SELECT id, parent_id FROM folders ORDER BY position, id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let feeds: Vec<(i64, Option<i64>)> = conn
            .prepare("SELECT id, folder_id FROM feeds ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let mut groups: std::collections::BTreeMap<Option<i64>, Vec<String>> =
            std::collections::BTreeMap::new();
        for (id, parent) in folders {
            groups
                .entry(parent)
                .or_default()
                .push(format!("folder:{id}"));
        }
        for (id, parent) in feeds {
            groups.entry(parent).or_default().push(format!("feed:{id}"));
        }
        let mut rows = Vec::new();
        for (parent, items) in groups {
            for (position, item) in items.into_iter().enumerate() {
                rows.push((parent, item, position as i64));
            }
        }
        rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.2.cmp(&b.2)).then(a.1.cmp(&b.1)));
        rows
    }

    #[test]
    fn move_into_folder_appends() {
        let (_dir, conn) = open();
        seed(&conn);
        move_tree_items(&conn, &[TreeItem::Feed(3)], Some(2), None).unwrap();
        assert_eq!(
            placed(&conn),
            vec![
                (None, "folder:1".into(), 0),
                (None, "folder:2".into(), 1),
                (None, "feed:1".into(), 2),
                (Some(1), "folder:3".into(), 0),
                (Some(1), "feed:2".into(), 1),
                (Some(2), "feed:3".into(), 0),
                (Some(3), "folder:4".into(), 0),
            ]
        );
    }

    #[test]
    fn move_places_item_before_marker() {
        let (_dir, conn) = open();
        seed(&conn);
        move_tree_items(
            &conn,
            &[TreeItem::Folder(2)],
            None,
            Some(TreeItem::Folder(1)),
        )
        .unwrap();
        assert_eq!(
            placed(&conn)
                .into_iter()
                .filter(|(parent, _, _)| parent.is_none())
                .collect::<Vec<_>>(),
            vec![
                (None, "folder:2".into(), 0),
                (None, "folder:1".into(), 1),
                (None, "feed:1".into(), 2),
                (None, "feed:3".into(), 3),
            ]
        );
    }

    #[test]
    fn move_to_root_front_uses_current_first_item() {
        let (_dir, conn) = open();
        seed(&conn);
        move_tree_items(&conn, &[TreeItem::Feed(3)], None, Some(TreeItem::Folder(1))).unwrap();
        assert_eq!(
            placed(&conn)
                .into_iter()
                .filter(|(parent, _, _)| parent.is_none())
                .collect::<Vec<_>>(),
            vec![
                (None, "feed:3".into(), 0),
                (None, "folder:1".into(), 1),
                (None, "folder:2".into(), 2),
                (None, "feed:1".into(), 3),
            ]
        );
    }

    #[test]
    fn root_feed_can_sit_ahead_of_a_folder() {
        let (_dir, conn) = open();
        seed(&conn);
        move_tree_items(&conn, &[TreeItem::Feed(1)], None, Some(TreeItem::Folder(2))).unwrap();
        assert_eq!(
            placed(&conn)
                .into_iter()
                .filter(|(parent, _, _)| parent.is_none())
                .collect::<Vec<_>>(),
            vec![
                (None, "folder:1".into(), 0),
                (None, "feed:1".into(), 1),
                (None, "folder:2".into(), 2),
                (None, "feed:3".into(), 3),
            ]
        );
    }

    #[test]
    fn folder_cannot_move_into_itself_or_a_descendant() {
        let (_dir, conn) = open();
        seed(&conn);
        let before = placed(&conn);
        let into_self = move_tree_items(&conn, &[TreeItem::Folder(1)], Some(1), None);
        let into_child = move_tree_items(&conn, &[TreeItem::Folder(1)], Some(4), None);
        assert!(
            matches!(into_self, Err(Error::InvalidTreeOrder(_)))
                && matches!(into_child, Err(Error::InvalidTreeOrder(_)))
                && placed(&conn) == before
        );
    }

    #[test]
    fn before_must_belong_to_the_target_parent() {
        let (_dir, conn) = open();
        seed(&conn);
        let before = placed(&conn);
        let moved = move_tree_items(&conn, &[TreeItem::Feed(3)], None, Some(TreeItem::Feed(2)));
        assert!(matches!(moved, Err(Error::InvalidTreeOrder(_))) && placed(&conn) == before);
    }

    #[test]
    fn restore_puts_back_the_layout_from_before_the_move() {
        let (_dir, conn) = open();
        seed(&conn);
        let original = placed(&conn);
        let layout = move_tree_items(&conn, &[TreeItem::Feed(3)], Some(2), None).unwrap();
        move_tree_items(&conn, &[TreeItem::Feed(1)], Some(2), None).unwrap();
        restore_tree_layout(&conn, &layout).unwrap();
        assert_eq!(placed(&conn), original);
    }

    #[test]
    fn create_folder_first_goes_first_and_new_feeds_go_last() {
        let (_dir, conn) = open();
        let a = create_folder_first(&conn, "A", None).unwrap();
        let b = create_folder_first(&conn, "B", None).unwrap();
        let feed = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        let child = create_folder_first(&conn, "C", Some(a)).unwrap();
        set_folder(&conn, feed, Some(a)).unwrap();
        assert_eq!(
            placed(&conn),
            vec![
                (None, format!("folder:{b}"), 0),
                (None, format!("folder:{a}"), 1),
                (Some(a), format!("folder:{child}"), 0),
                (Some(a), format!("feed:{feed}"), 1),
            ]
        );
    }
}
