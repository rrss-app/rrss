//! Conversation thread, folder and user-message write orchestration.

use rrss_core::db;
use rrss_core::feeds::now_ms;
use rrss_core::threads::{self, MessageRole};
use rusqlite::Connection;
use std::path::Path;

use crate::views::CitationView;

fn ordinary_thread(conn: &Connection, thread_id: i64) -> anyhow::Result<threads::Thread> {
    let thread = threads::get_thread(conn, thread_id)?;
    anyhow::ensure!(
        thread.anchor_kind.is_none(),
        crate::ui_error::chat::ChatUiError::ThreadAnchored.failure("anchored thread")
    );
    Ok(thread)
}

fn update_thread(
    conn: &Connection,
    thread_id: i64,
    sql: &str,
    value: impl rusqlite::ToSql,
) -> anyhow::Result<()> {
    let tx = db::write_transaction(conn)?;
    ordinary_thread(&tx, thread_id)?;
    tx.execute(sql, rusqlite::params![value, thread_id])?;
    tx.commit()?;
    Ok(())
}

const THREAD_TITLE_MAX: usize = 80;
const THREAD_FOLDER_NAME_MAX: usize = 40;

pub fn rename_thread(conn: &Connection, thread_id: i64, title: &str) -> anyhow::Result<()> {
    let title = title.trim();
    anyhow::ensure!(
        title.chars().count() <= THREAD_TITLE_MAX,
        crate::ui_error::chat::ChatUiError::ThreadTitleTooLong { max: THREAD_TITLE_MAX }.failure("thread title too long")
    );
    update_thread(
        conn,
        thread_id,
        "UPDATE threads SET title=?1 WHERE id=?2",
        (!title.is_empty()).then_some(title),
    )
}

pub fn set_thread_folder(
    conn: &Connection,
    thread_id: i64,
    folder_id: Option<i64>,
) -> anyhow::Result<()> {
    let tx = db::write_transaction(conn)?;
    ordinary_thread(&tx, thread_id)?;
    if let Some(id) = folder_id {
        tx.query_row(
            "SELECT id FROM thread_folders WHERE id=?1",
            [id],
            |_| Ok(()),
        )?;
    }
    tx.execute(
        "UPDATE threads SET folder_id=?1 WHERE id=?2",
        rusqlite::params![folder_id, thread_id],
    )?;
    tx.commit()?;
    Ok(())
}

pub fn set_thread_pinned(conn: &Connection, thread_id: i64, pinned: bool) -> anyhow::Result<()> {
    update_thread(
        conn,
        thread_id,
        "UPDATE threads SET pinned=?1 WHERE id=?2",
        pinned,
    )
}

pub fn archive_thread(conn: &Connection, thread_id: i64) -> anyhow::Result<()> {
    update_thread(
        conn,
        thread_id,
        "UPDATE threads SET archived_at=?1 WHERE id=?2",
        now_ms(),
    )
}

pub fn unarchive_thread(conn: &Connection, thread_id: i64) -> anyhow::Result<()> {
    update_thread(
        conn,
        thread_id,
        "UPDATE threads SET archived_at=?1 WHERE id=?2",
        None::<i64>,
    )
}

/// 调用方在整个删除期间持有 ChatRuntime 的删除守卫，防止同时发送。
pub fn delete_thread(conn: &Connection, thread_id: i64, data_dir: &Path) -> anyhow::Result<()> {
    let tx = db::write_transaction(conn)?;
    let thread = ordinary_thread(&tx, thread_id)?;
    // 所有应用连接均开启 foreign_keys，messages 通过 ON DELETE CASCADE 删除。
    tx.execute("DELETE FROM threads WHERE id=?1", [thread_id])?;
    if let Some(file) = thread.pi_session_file {
        let sessions = crate::pi::workspace::WorkspacePaths::new(data_dir).sessions;
        // canonicalize 同时挡住 .. 和指向托管目录外的符号链接；文件不存在视为已清理。
        let canonical = |path: &Path| -> std::io::Result<Option<std::path::PathBuf>> {
            match path.canonicalize() {
                Ok(path) => Ok(Some(path)),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(err) => Err(err),
            }
        };
        let remove = || -> std::io::Result<()> {
            let file = Path::new(&file);
            if let (Some(root), Some(path), Some(parent)) = (
                canonical(&sessions)?,
                canonical(file)?,
                file.parent().map(canonical).transpose()?.flatten(),
            ) && path.starts_with(&root)
                && path != root
                && parent.starts_with(&root)
            {
                match std::fs::remove_file(file) {
                    Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
                    _ => {}
                }
            }
            Ok(())
        };
        remove().map_err(|err| {
            crate::ui_error::chat::ChatUiError::SessionFileDeleteFailed.failure(format!(
                "session file delete failed: {}",
                rrss_core::logsafe::text(&err.to_string(), 200)
            ))
        })?;
    }
    tx.commit()?;
    Ok(())
}

fn thread_folder_name(name: &str) -> anyhow::Result<&str> {
    let name = name.trim();
    anyhow::ensure!(
        !name.is_empty() && name.chars().count() <= THREAD_FOLDER_NAME_MAX,
        crate::ui_error::chat::ChatUiError::FolderNameInvalid { max: THREAD_FOLDER_NAME_MAX }.failure("thread folder name invalid")
    );
    Ok(name)
}

pub fn create_thread_folder(
    conn: &Connection,
    name: &str,
) -> anyhow::Result<crate::views::ThreadFolderView> {
    let name = thread_folder_name(name)?;
    let tx = db::write_transaction(conn)?;
    tx.execute(
        "INSERT INTO thread_folders(name, position, created_at)
        SELECT ?1, COALESCE(MAX(position), -1)+1, ?2 FROM thread_folders",
        rusqlite::params![name, now_ms()],
    )?;
    let id = tx.last_insert_rowid();
    let position = tx.query_row(
        "SELECT position FROM thread_folders WHERE id=?1",
        [id],
        |r| r.get(0),
    )?;
    tx.commit()?;
    Ok(crate::views::ThreadFolderView {
        id,
        name: name.into(),
        position,
        collapsed: false,
    })
}

pub fn rename_thread_folder(conn: &Connection, folder_id: i64, name: &str) -> anyhow::Result<()> {
    let name = thread_folder_name(name)?;
    let n = conn.execute(
        "UPDATE thread_folders SET name=?1 WHERE id=?2",
        rusqlite::params![name, folder_id],
    )?;
    anyhow::ensure!(n > 0, crate::ui_error::chat::ChatUiError::FolderNotFound.failure("thread folder not found"));
    Ok(())
}

pub fn delete_thread_folder(conn: &Connection, folder_id: i64) -> anyhow::Result<()> {
    let tx = db::write_transaction(conn)?;
    // 显式置空，即使调用方的连接未开启外键，也不会留下悬空归类。
    tx.execute(
        "UPDATE threads SET folder_id=NULL WHERE folder_id=?1",
        [folder_id],
    )?;
    let n = tx.execute("DELETE FROM thread_folders WHERE id=?1", [folder_id])?;
    anyhow::ensure!(n > 0, crate::ui_error::chat::ChatUiError::FolderNotFound.failure("thread folder not found"));
    tx.commit()?;
    Ok(())
}

pub fn reorder_thread_folders(conn: &Connection, ids: &[i64]) -> anyhow::Result<()> {
    let tx = db::write_transaction(conn)?;
    let existing: std::collections::HashSet<_> = crate::queries::thread_folders(&tx)?
        .into_iter()
        .map(|f| f.id)
        .collect();
    let requested: std::collections::HashSet<_> = ids.iter().copied().collect();
    anyhow::ensure!(
        ids.len() == existing.len() && requested == existing,
        crate::ui_error::chat::ChatUiError::FolderOrderInvalid.failure("thread folder order invalid")
    );
    for (position, id) in ids.iter().enumerate() {
        tx.execute(
            "UPDATE thread_folders SET position=?1 WHERE id=?2",
            rusqlite::params![position as i64, id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub fn set_thread_folder_collapsed(
    conn: &Connection,
    folder_id: i64,
    collapsed: bool,
) -> anyhow::Result<()> {
    let n = conn.execute(
        "UPDATE thread_folders SET collapsed=?1 WHERE id=?2",
        rusqlite::params![collapsed, folder_id],
    )?;
    anyhow::ensure!(n > 0, crate::ui_error::chat::ChatUiError::FolderNotFound.failure("thread folder not found"));
    Ok(())
}

pub fn new_thread(conn: &Connection, title: Option<&str>) -> rrss_core::Result<i64> {
    Ok(threads::create_thread(conn, title)?.id)
}

pub fn insert_user_message(
    conn: &Connection,
    thread_id: i64,
    text: &str,
    citations: &[CitationView],
) -> rrss_core::Result<i64> {
    let snaps: Vec<rrss_core::CitationSnap> = citations
        .iter()
        .map(|c| rrss_core::CitationSnap {
            kind: c.kind.clone(),
            article_id: c.article_id,
            title: c.title.clone(),
            feed_title: c.feed_title.clone(),
            url: c.url.clone(),
            excerpt: c.excerpt.clone(),
            block_id: c.block_id,
        })
        .collect();
    let json = if snaps.is_empty() {
        None
    } else {
        serde_json::to_string(&snaps).ok()
    };
    Ok(threads::insert_message(conn, thread_id, MessageRole::User, text, json.as_deref())?.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::create_folder;

    fn open_temp() -> (tempfile::TempDir, rrss_core::db::PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.db");
        let conn = db::open_at(&path).unwrap();
        (dir, conn)
    }

    #[test]
    fn thread_folder_deletion_unfiles_threads_even_without_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        let id = new_thread(&conn, None).unwrap();
        let folder = create_thread_folder(&conn, "  工作  ").unwrap();
        assert_eq!(folder.name, "工作");
        assert!(create_thread_folder(&conn, "  ").is_err());
        assert!(rename_thread_folder(&conn, folder.id, &"字".repeat(41)).is_err());
        rename_thread_folder(&conn, folder.id, "  资料  ").unwrap();
        set_thread_folder_collapsed(&conn, folder.id, true).unwrap();
        let folders = crate::queries::thread_folders(&conn).unwrap();
        assert_eq!(folders[0].name, "资料");
        assert!(folders[0].collapsed);
        set_thread_folder(&conn, id, Some(folder.id)).unwrap();
        assert_eq!(
            threads::get_thread(&conn, id).unwrap().folder_id,
            Some(folder.id)
        );
        assert!(set_thread_folder(&conn, id, Some(folder.id + 1)).is_err());
        conn.pragma_update(None, "foreign_keys", "OFF").unwrap();
        delete_thread_folder(&conn, folder.id).unwrap();
        assert_eq!(threads::get_thread(&conn, id).unwrap().folder_id, None);
        assert!(crate::queries::thread_folders(&conn).unwrap().is_empty());
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    }

    #[test]
    fn archived_threads_are_separate_and_sorted_by_archive_time() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        let a = new_thread(&conn, None).unwrap();
        let b = new_thread(&conn, None).unwrap();
        for id in [a, b] {
            insert_user_message(&conn, id, "first", &[]).unwrap();
            archive_thread(&conn, id).unwrap();
        }
        conn.execute("UPDATE threads SET archived_at=1 WHERE id=?1", [b])
            .unwrap();
        let list = crate::queries::thread_list(&conn).unwrap();
        assert!(list.threads.is_empty());
        assert_eq!(list.archived_count, 2);
        assert_eq!(
            crate::queries::archived_threads(&conn)
                .unwrap()
                .iter()
                .map(|t| t.id)
                .collect::<Vec<_>>(),
            [a, b]
        );
        unarchive_thread(&conn, a).unwrap();
        assert_eq!(crate::queries::threads(&conn).unwrap()[0].id, a);
        assert_eq!(
            crate::queries::thread_list(&conn).unwrap().archived_count,
            1
        );
    }

    #[test]
    fn thread_rename_trims_counts_characters_and_empty_restores_automatic_title() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        let id = new_thread(&conn, None).unwrap();
        insert_user_message(&conn, id, "自动标题", &[]).unwrap();
        let title = "字".repeat(80);
        rename_thread(&conn, id, &format!("  {title}  ")).unwrap();
        assert_eq!(crate::queries::threads(&conn).unwrap()[0].title, title);
        assert!(rename_thread(&conn, id, &"字".repeat(81)).is_err());
        rename_thread(&conn, id, "新对话").unwrap();
        assert_eq!(crate::queries::threads(&conn).unwrap()[0].title, "新对话");
        rename_thread(&conn, id, "  ").unwrap();
        assert_eq!(threads::get_thread(&conn, id).unwrap().title, None);
        assert_eq!(crate::queries::threads(&conn).unwrap()[0].title, "自动标题");
    }

    #[test]
    fn thread_folder_order_requires_every_id_once() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        let a = create_thread_folder(&conn, "A").unwrap().id;
        let b = create_thread_folder(&conn, "B").unwrap().id;
        for ids in [vec![a], vec![a, a], vec![a, b + 1]] {
            assert!(reorder_thread_folders(&conn, &ids).is_err());
            assert_eq!(crate::queries::thread_folders(&conn).unwrap()[0].id, a);
        }
        reorder_thread_folders(&conn, &[b, a]).unwrap();
        let folders = crate::queries::thread_folders(&conn).unwrap();
        assert_eq!(
            folders
                .iter()
                .map(|f| (f.id, f.position))
                .collect::<Vec<_>>(),
            [(b, 0), (a, 1)]
        );
    }

    #[test]
    fn anchored_threads_reject_all_ordinary_thread_actions() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        let id = threads::get_or_create_anchored_thread(&conn, "追问", "digest", 1)
            .unwrap()
            .id;
        assert!(rename_thread(&conn, id, "标题").is_err());
        assert!(set_thread_folder(&conn, id, None).is_err());
        assert!(set_thread_pinned(&conn, id, true).is_err());
        assert!(archive_thread(&conn, id).is_err());
        assert!(unarchive_thread(&conn, id).is_err());
        assert!(delete_thread(&conn, id, dir.path()).is_err());
        assert!(threads::get_thread(&conn, id).is_ok());
    }

    #[test]
    fn thread_deletion_only_removes_managed_session_files() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("t.db")).unwrap();
        let sessions = crate::pi::workspace::WorkspacePaths::new(dir.path()).sessions;
        std::fs::create_dir_all(&sessions).unwrap();
        let outside = dir.path().join("outside.jsonl");
        std::fs::write(&outside, "fixture").unwrap();
        // 只有 Unix 会追加符号链接一项。
        #[cfg_attr(not(unix), allow(unused_mut))]
        let mut files = vec![
            outside.clone(),
            sessions.join("../outside.jsonl"),
            sessions.join("missing.jsonl"),
        ];
        std::fs::write(&files[1], "fixture").unwrap();
        #[cfg(unix)]
        {
            let symlink = sessions.join("linked.jsonl");
            std::os::unix::fs::symlink(&outside, &symlink).unwrap();
            files.push(symlink);
        }
        for file in files {
            let id = new_thread(&conn, None).unwrap();
            threads::set_pi_session_file(&conn, id, Some(file.to_str().unwrap())).unwrap();
            delete_thread(&conn, id, dir.path()).unwrap();
        }
        assert!(outside.exists());
        assert!(sessions.join("../outside.jsonl").exists());
        let blocked = sessions.join("not-a-file");
        std::fs::create_dir(&blocked).unwrap();
        let id = new_thread(&conn, None).unwrap();
        threads::set_pi_session_file(&conn, id, Some(blocked.to_str().unwrap())).unwrap();
        assert!(delete_thread(&conn, id, dir.path()).is_err());
        assert!(
            threads::get_thread(&conn, id).is_ok(),
            "file failure rolls back the deletion"
        );
    }

    #[test]
    fn thread_and_folder_roundtrip() {
        let (_dir, conn) = open_temp();
        let folder = create_folder(&conn, "科技", None).unwrap();
        assert!(folder > 0);
        let tid = new_thread(&conn, Some("新对话")).unwrap();
        assert!(tid > 0);
        let mid = insert_user_message(&conn, tid, "hello", &[]).unwrap();
        assert!(mid > 0);
    }

    #[test]
    fn user_message_snapshots_survive_until_thread_deletion() {
        let (dir, conn) = open_temp();
        let thread_id = new_thread(&conn, Some("Reading notes")).unwrap();
        let citation = CitationView {
            kind: "span".into(),
            article_id: 7,
            title: "A local reader".into(),
            feed_title: "Example Journal".into(),
            url: "https://journal.example/local-reader".into(),
            excerpt: "Keep the selected passage".into(),
            block_id: 3,
        };
        let message_id = insert_user_message(
            &conn,
            thread_id,
            "Explain this passage",
            std::slice::from_ref(&citation),
        )
        .unwrap();
        let page = crate::queries::messages(&conn, thread_id, None, None).unwrap();
        assert_eq!(page.messages.len(), 1);
        let message = &page.messages[0];
        assert_eq!(message.id, message_id);
        assert_eq!(message.role, "user");
        assert_eq!(message.text, "Explain this passage");
        assert_eq!(
            serde_json::to_value(&message.citations).unwrap(),
            serde_json::to_value([citation]).unwrap()
        );

        let sessions = crate::pi::workspace::WorkspacePaths::new(dir.path()).sessions;
        std::fs::create_dir_all(&sessions).unwrap();
        let managed = sessions.join("session.jsonl");
        std::fs::write(&managed, "fixture").unwrap();
        threads::set_pi_session_file(&conn, thread_id, Some(managed.to_str().unwrap())).unwrap();
        delete_thread(&conn, thread_id, dir.path()).unwrap();
        assert!(!managed.exists());
        assert!(threads::get_thread(&conn, thread_id).is_err());
        assert_eq!(
            conn.query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM messages WHERE thread_id=?1",
                [thread_id],
                |row| row.get(0),
            )
            .unwrap(),
            0
        );
    }
}
