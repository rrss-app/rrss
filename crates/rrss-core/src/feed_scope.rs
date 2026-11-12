//! 阶段 23「范围浏览 S1」：订阅范围 `all` / `folder:<id>` / `feed:<id>`。

use rusqlite::{Connection, OptionalExtension};

use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedScope {
    All,
    Folder(i64),
    Feed(i64),
}

impl FeedScope {
    /// `all`、`folder:<id>`、`feed:<id>`；其余输入都是全部订阅。
    pub fn parse_str(value: &str) -> Self {
        if value == "all" {
            return Self::All;
        }
        if let Some(id) = value.strip_prefix("folder:").and_then(|s| s.parse().ok()) {
            return Self::Folder(id);
        }
        if let Some(id) = value.strip_prefix("feed:").and_then(|s| s.parse().ok()) {
            return Self::Feed(id);
        }
        Self::All
    }

    pub fn as_string(self) -> String {
        match self {
            Self::All => "all".to_string(),
            Self::Folder(id) => format!("folder:{id}"),
            Self::Feed(id) => format!("feed:{id}"),
        }
    }
}

/// 文件夹或源已经不在库里时退回全部订阅。
pub fn resolve(conn: &Connection, scope: FeedScope) -> Result<FeedScope> {
    let (sql, id) = match scope {
        FeedScope::All => return Ok(scope),
        FeedScope::Folder(id) => ("SELECT 1 FROM folders WHERE id = ?1", id),
        FeedScope::Feed(id) => ("SELECT 1 FROM feeds WHERE id = ?1", id),
    };
    let found = conn.query_row(sql, [id], |_| Ok(())).optional()?;
    Ok(if found.is_some() {
        scope
    } else {
        FeedScope::All
    })
}

// 从该文件夹起沿 parent_id 向下收集子孙。UNION 丢掉已经见过的 id，环不会无限展开。
const FOLDER_SQL: &str = " AND a.feed_id IN (
    SELECT id FROM feeds WHERE folder_id IN (
        WITH RECURSIVE tree(id) AS (
            SELECT id FROM folders WHERE id = ?
            UNION
            SELECT f.id FROM folders f JOIN tree t ON f.parent_id = t.id
        )
        SELECT id FROM tree
    )
)";

/// 拼进 `WHERE` 的范围条件，时间线、计数、标已读和隐藏列表共用。
/// 全部订阅时片段为空；文件夹或源时以 ` AND ` 开头，并含一个 `?`。
/// 拼入该片段的整条 SQL 只用匿名 `?`，按文本出现顺序用 `params_from_iter` 绑定。
pub(crate) fn scope_sql(scope: FeedScope) -> (&'static str, Option<i64>) {
    match scope {
        FeedScope::All => ("", None),
        FeedScope::Feed(id) => (" AND a.feed_id = ?", Some(id)),
        FeedScope::Folder(id) => (FOLDER_SQL, Some(id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("schema.sql")).unwrap();
        conn
    }

    #[test]
    fn scope_text_round_trips_and_invalid_is_all() {
        for (text, scope) in [
            ("all", FeedScope::All),
            ("folder:4", FeedScope::Folder(4)),
            ("feed:9", FeedScope::Feed(9)),
        ] {
            assert_eq!(FeedScope::parse_str(text), scope);
            assert_eq!(scope.as_string(), text);
        }
        for text in [
            "",
            "folder",
            "feed:",
            "folder:x",
            "nope",
            "ALL",
            "folder:1a",
        ] {
            assert_eq!(FeedScope::parse_str(text), FeedScope::All);
        }
    }

    #[test]
    fn resolve_deleted_scope_falls_back_to_all() {
        let conn = mem();
        conn.execute_batch(
            "INSERT INTO folders (id, name, created_at) VALUES (1, 'f', 0);
             INSERT INTO feeds (id, url, folder_id, created_at)
             VALUES (2, 'https://a.example/rss', 1, 0);",
        )
        .unwrap();
        assert_eq!(
            resolve(&conn, FeedScope::Folder(1)).unwrap(),
            FeedScope::Folder(1)
        );
        assert_eq!(
            resolve(&conn, FeedScope::Feed(2)).unwrap(),
            FeedScope::Feed(2)
        );
        assert_eq!(resolve(&conn, FeedScope::All).unwrap(), FeedScope::All);
        conn.execute("DELETE FROM folders WHERE id = 1", [])
            .unwrap();
        conn.execute("DELETE FROM feeds WHERE id = 2", []).unwrap();
        assert_eq!(
            resolve(&conn, FeedScope::Folder(1)).unwrap(),
            FeedScope::All
        );
        assert_eq!(resolve(&conn, FeedScope::Feed(2)).unwrap(), FeedScope::All);
    }
}
