use std::path::Path;

use rusqlite::Connection;

use rrss_core::ai_store;
use rrss_core::storage::{self, DbPart, DbUsage, MediaUsage};

use crate::views::*;

/// 大小的显示：1024 进制，与系统文件管理器一致。
pub fn fmt_bytes(bytes: i64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes.max(0) as f64;
    if b >= KB * KB * KB {
        format!("{:.2} GB", b / KB / KB / KB)
    } else if b >= 100.0 * KB * KB {
        format!("{:.0} MB", b / KB / KB)
    } else if b >= KB * KB {
        format!("{:.1} MB", b / KB / KB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else if bytes > 0 {
        format!("{bytes} B")
    } else {
        "0".into()
    }
}

pub fn fmt_count(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{out}") } else { out }
}

fn item(kind: StorageItemKind, hint: StorageMessage, bytes: i64) -> StorageItemView {
    StorageItemView {
        kind,
        hint,
        bytes,
        model: String::new(),
        tables: Vec::new(),
        free: false,
        action: None,
        passive: None,
        confirm: Vec::new(),
    }
}

impl StorageItemView {
    fn action(mut self, action: StorageAction, confirm: Vec<StorageMessage>) -> Self {
        self.action = Some(action);
        self.confirm = confirm;
        self
    }
    fn passive(mut self, passive: StoragePassive) -> Self {
        self.passive = Some(passive);
        self
    }
}

fn part(usage: &DbUsage, part: DbPart) -> (i64, i64) {
    usage
        .parts
        .iter()
        .find(|p| p.part == part)
        .map(|p| (p.bytes, p.rows.unwrap_or(0)))
        .unwrap_or((0, 0))
}

fn database_group(conn: &Connection, db_path: &Path, usage: &DbUsage) -> StorageGroupView {
    use StorageAction as A;
    use StorageItemKind as I;
    use StorageMessage as M;
    let db_file = storage::file_bytes(db_path) as i64;
    let wal = storage::file_bytes(&wal_path(db_path)) as i64;
    let mut items = Vec::new();
    let (bytes, count) = part(usage, DbPart::Articles);
    items.push(item(I::Articles, M::StorageArticles { count }, bytes));
    let (bytes, _) = part(usage, DbPart::SearchIndex);
    items.push(
        item(I::SearchIndex, M::StorageSearchIndex {}, bytes)
            .passive(StoragePassive::FollowsArticles),
    );
    let (bytes, count) = part(usage, DbPart::Vectors);
    let model = ai_store::resolve_embed(conn)
        .ok()
        .flatten()
        .map(|e| e.model);
    let old_count = storage::old_vector_count(conn).unwrap_or(0);
    let hint = match &model {
        None => M::StorageVectorsUnset { count },
        Some(_) if old_count > 0 => M::StorageVectorsOther { count, old_count },
        Some(_) => M::StorageVectorsCurrent { count },
    };
    let mut vectors = item(I::Vectors, hint, bytes);
    vectors.model = model.unwrap_or_default();
    if old_count > 0 {
        vectors = vectors.action(
            A::OldVectors,
            vec![
                M::StorageOldVectorsConfirm { count: old_count },
                M::StorageOldVectorsWarning {},
            ],
        );
    }
    items.push(vectors);
    let (bytes, count) = part(usage, DbPart::AiCache);
    let mut ai = item(I::AiCache, M::StorageAiCache { count }, bytes);
    if count > 0 {
        ai = ai.action(
            A::AiCache,
            vec![
                M::StorageAiCacheConfirm { count },
                M::StorageAiCacheWarning {},
            ],
        );
    }
    items.push(ai);
    let (bytes, count) = part(usage, DbPart::Translations);
    let mut tr = item(I::Translations, M::StorageTranslations { count }, bytes);
    if count > 0 {
        tr = tr.action(
            A::Translations,
            vec![
                M::StorageTranslationsConfirm { count },
                M::StorageTranslationsWarning {},
            ],
        );
    }
    items.push(tr);
    let (bytes, count) = part(usage, DbPart::RecommendationLogs);
    let mut logs = item(
        I::RecommendationLogs,
        M::StorageRecommendationLogs { count },
        bytes,
    );
    if count > 0 {
        logs = logs.action(
            A::RecommendationLogs,
            vec![
                M::StorageRecommendationLogsConfirm { count },
                M::StorageRecommendationLogsWarning {},
            ],
        );
    }
    items.push(logs);
    let (chat_bytes, threads) = part(usage, DbPart::Conversations);
    let (other_bytes, _) = part(usage, DbPart::Other);
    let (digest_bytes, periods) = part(usage, DbPart::Digests);
    items.push(
        item(
            I::Other,
            M::StorageOther {
                chat_bytes,
                threads,
                other_bytes,
                digest_bytes,
                periods,
            },
            chat_bytes + other_bytes + digest_bytes,
        )
        .passive(StoragePassive::ManageElsewhere),
    );
    let (bytes, _) = part(usage, DbPart::Legacy);
    let legacy = storage::legacy_tables(conn).unwrap_or_default();
    if !legacy.is_empty() {
        let mut legacy_item = item(I::Legacy, M::StorageLegacy {}, bytes).action(
            A::LegacyTables,
            vec![M::StorageLegacyConfirm {}, M::StorageLegacyWarning {}],
        );
        legacy_item.tables = legacy.into_iter().map(str::to_string).collect();
        items.push(legacy_item);
    }
    items.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    let available_bytes = db_path
        .parent()
        .and_then(storage::available_bytes)
        .map(|n| n as i64);
    let mut free = item(
        I::Free,
        M::StorageFree {
            required_bytes: db_file,
            available_bytes,
        },
        usage.free_bytes,
    );
    free.free = true;
    free = if usage.free_bytes > 0 {
        free.action(
            A::Vacuum,
            vec![
                M::StorageVacuumConfirm {
                    bytes: usage.free_bytes,
                },
                M::StorageVacuumSpace {
                    required_bytes: db_file,
                    available_bytes,
                },
                M::StorageVacuumWait {},
            ],
        )
    } else {
        free.passive(StoragePassive::None)
    };
    items.push(free);
    StorageGroupView {
        kind: StorageGroupKind::Database,
        bytes: db_file + wal,
        note: (wal > 0).then_some(M::StorageWal { bytes: wal }),
        items,
    }
}

fn media_group(media: &MediaUsage) -> StorageGroupView {
    use StorageItemKind as I;
    use StorageMessage as M;
    let mut items = vec![
        item(I::Images, M::StorageImages {}, media.image_bytes),
        item(I::Videos, M::StorageVideos {}, media.video_bytes),
    ];
    if media.image_bytes > 0 {
        items[0] = items[0].clone().action(
            StorageAction::Images,
            vec![
                M::StorageImagesConfirm {
                    bytes: media.image_bytes,
                },
                M::StorageImagesWarning {},
            ],
        );
    }
    if media.video_bytes > 0 {
        items[1] = items[1].clone().action(
            StorageAction::Videos,
            vec![
                M::StorageVideosConfirm {
                    bytes: media.video_bytes,
                },
                M::StorageVideosWarning {},
            ],
        );
    }
    if media.other_bytes > 0 {
        items.push(item(
            I::MediaOther,
            M::StorageMediaOther {},
            media.other_bytes,
        ));
    }
    if media.untracked_files > 0 {
        items.push(
            item(
                I::UntrackedMedia,
                M::StorageUntracked {
                    count: media.untracked_files,
                },
                media.untracked_bytes,
            )
            .action(
                StorageAction::UntrackedMedia,
                vec![
                    M::StorageUntrackedConfirm {
                        count: media.untracked_files,
                        bytes: media.untracked_bytes,
                    },
                    M::StorageUntrackedWarning {},
                ],
            ),
        );
    }
    items.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    let bytes = media.image_bytes + media.video_bytes + media.other_bytes + media.untracked_bytes;
    StorageGroupView {
        kind: StorageGroupKind::Media,
        bytes,
        note: None,
        items,
    }
}

fn engine_group(data_dir: &Path) -> StorageGroupView {
    let program = (storage::dir_bytes(&crate::pi::install::managed_bin_dir(data_dir))
        + storage::dir_bytes(&crate::pi::launch::pi_pkg_dir(data_dir))) as i64;
    let sessions = storage::dir_bytes(&data_dir.join("pi/sessions")) as i64;
    let mut items = Vec::new();
    if program > 0 {
        items.push(item(
            StorageItemKind::PiProgram,
            StorageMessage::StoragePiProgram {},
            program,
        ));
    }
    items.push(item(
        StorageItemKind::PiSessions,
        StorageMessage::StoragePiSessions {},
        sessions,
    ));
    items.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    StorageGroupView {
        kind: StorageGroupKind::Engine,
        bytes: program + sessions,
        note: None,
        items,
    }
}

fn backups_group(data_dir: &Path) -> Option<StorageGroupView> {
    let entries = rrss_core::backup::list_auto(data_dir);
    if entries.is_empty() {
        return None;
    }
    let bytes = entries.iter().map(|e| e.bytes as i64).sum();
    Some(StorageGroupView {
        kind: StorageGroupKind::Backups,
        bytes,
        note: None,
        items: vec![
            item(
                StorageItemKind::AutoBackups,
                StorageMessage::StorageAutoBackups {
                    count: entries.len() as i64,
                    keep: rrss_core::backup::AUTO_KEEP as i64,
                },
                bytes,
            )
            .passive(StoragePassive::ManageBackups),
        ],
    })
}

fn logs_group(log: Option<crate::views::LogStatusView>) -> Option<StorageGroupView> {
    use crate::views::LogUnavailableReason;
    let log = log?;
    let item = item(
        StorageItemKind::Logs,
        StorageMessage::StorageLogs {},
        log.total_bytes,
    );
    let item = if !log.can_clear {
        item.passive(match log.unavailable {
            Some(LogUnavailableReason::OtherInstance) => StoragePassive::OtherLogInstance,
            _ => StoragePassive::LogUnavailable,
        })
    } else if log.total_bytes > 0 {
        item.action(
            StorageAction::Logs,
            vec![
                StorageMessage::StorageLogsConfirm {},
                StorageMessage::StorageLogsWarning {},
            ],
        )
    } else {
        item
    };
    Some(StorageGroupView {
        kind: StorageGroupKind::Logs,
        bytes: log.total_bytes,
        note: Some(StorageMessage::StorageLogsLocation {}),
        items: vec![item],
    })
}

fn wal_path(db_path: &Path) -> std::path::PathBuf {
    let mut name = db_path.as_os_str().to_os_string();
    name.push("-wal");
    name.into()
}

pub fn storage_page(conn: &Connection, db_path: &Path) -> rrss_core::Result<StoragePage> {
    let data_dir = db_path.parent().unwrap_or(Path::new("."));
    let usage = storage::db_usage(conn)?;
    let media = storage::media_usage(conn, data_dir)?;
    let mut groups = vec![
        database_group(conn, db_path, &usage),
        media_group(&media),
        engine_group(data_dir),
    ];
    groups.extend(backups_group(data_dir));
    groups.extend(logs_group(
        crate::log::current().and_then(crate::log::Handle::status_view),
    ));
    let group_total = |kind| {
        groups
            .iter()
            .find(|g| g.kind == kind)
            .map(|g| g.items.iter().map(|i| i.bytes).sum::<i64>())
            .unwrap_or(0)
    };
    let database_bytes =
        storage::file_bytes(db_path) as i64 + storage::file_bytes(&wal_path(db_path)) as i64;
    let media_bytes = group_total(StorageGroupKind::Media);
    let engine_bytes = group_total(StorageGroupKind::Engine);
    let backup_bytes = group_total(StorageGroupKind::Backups);
    let log_bytes = group_total(StorageGroupKind::Logs);
    Ok(StoragePage {
        data_dir: data_dir.display().to_string(),
        total_bytes: Some(database_bytes + media_bytes + engine_bytes + backup_bytes + log_bytes),
        summary: Some(StorageMessage::StorageSummary {
            database_bytes,
            media_bytes,
            engine_bytes,
            backup_bytes,
            log_bytes,
        }),
        groups,
        scanned_at: rrss_core::feeds::now_ms(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes_and_counts() {
        assert_eq!(fmt_bytes(0), "0");
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(420 * 1024), "420 KB");
        assert_eq!(fmt_bytes(31 * 1024 * 1024 + 200_000), "31.2 MB");
        assert_eq!(fmt_bytes(612 * 1024 * 1024), "612 MB");
        assert_eq!(fmt_bytes(1_514_000_000), "1.41 GB");
        assert_eq!(fmt_count(38210), "38,210");
        assert_eq!(fmt_count(999), "999");
    }

    #[test]
    fn logs_group_offers_clearing_only_to_the_lock_owner() {
        use crate::views::{LogStatusView, LogUnavailableReason};
        let status = |can_clear, unavailable, total_bytes| LogStatusView {
            directory: "logs".into(),
            file: "logs/rrss.log".into(),
            unavailable,
            current_size: "1 KB".into(),
            total_size: "2 KB".into(),
            total_bytes,
            detailed: false,
            previous_unclean: None,
            can_clear,
        };
        assert!(logs_group(None).is_none());
        let owned = logs_group(Some(status(true, None, 2048))).unwrap();
        assert_eq!(
            (owned.kind, owned.items[0].action),
            (StorageGroupKind::Logs, Some(StorageAction::Logs))
        );
        let other = logs_group(Some(status(
            false,
            Some(LogUnavailableReason::OtherInstance),
            2048,
        )))
        .unwrap();
        assert!(other.items[0].action.is_none());
        assert!(other.items[0].passive == Some(StoragePassive::OtherLogInstance));
        let empty = logs_group(Some(status(true, None, 0))).unwrap();
        assert!(empty.items[0].action.is_none() && empty.items[0].passive.is_none());
    }

    #[test]
    fn page_lists_groups_and_offers_only_available_actions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rrss.db");
        let conn = rrss_core::db::open_at(&path).unwrap();
        conn.execute_batch(
            "INSERT INTO ai_cache (cache_key, value, created_at) VALUES ('k', 'v', 1);
             INSERT INTO feeds(id,url,created_at) VALUES(1,'https://test.invalid/rss',0);
             INSERT INTO articles(id,feed_id,title,created_at) VALUES(1,1,'article',0);
             INSERT INTO article_events(article_id,kind,at) VALUES(1,'open',0);
             CREATE TABLE feed_sub_records (id INTEGER PRIMARY KEY);",
        )
        .unwrap();
        let page = storage_page(&conn, &path).unwrap();
        let kinds: Vec<_> = page.groups.iter().map(|g| g.kind).collect();
        assert_eq!(
            kinds,
            [
                StorageGroupKind::Database,
                StorageGroupKind::Media,
                StorageGroupKind::Engine
            ]
        );
        let db = &page.groups[0];
        let find = |kind| db.items.iter().find(|i| i.kind == kind).unwrap();
        assert_eq!(
            find(StorageItemKind::AiCache).action,
            Some(StorageAction::AiCache)
        );
        assert!(matches!(
            find(StorageItemKind::AiCache).confirm[0],
            StorageMessage::StorageAiCacheConfirm { count: 1 }
        ));
        assert_eq!(
            find(StorageItemKind::Translations).action,
            None,
            "nothing to clear"
        );
        assert_eq!(
            find(StorageItemKind::RecommendationLogs).action,
            Some(StorageAction::RecommendationLogs)
        );
        assert!(matches!(
            find(StorageItemKind::RecommendationLogs).hint,
            StorageMessage::StorageRecommendationLogs { count: 1 }
        ));
        assert_eq!(
            find(StorageItemKind::Vectors).action,
            None,
            "no embed model configured"
        );
        assert_eq!(
            find(StorageItemKind::Legacy).action,
            Some(StorageAction::LegacyTables)
        );
        assert_eq!(
            find(StorageItemKind::SearchIndex).passive,
            Some(StoragePassive::FollowsArticles)
        );
        assert!(db.items.last().unwrap().free, "free space stays last");
        let media = &page.groups[1];
        assert!(
            media.items.iter().all(|i| i.action.is_none()),
            "empty caches have nothing to clear"
        );

        rrss_core::backup::create_auto(
            &conn,
            dir.path(),
            rrss_core::backup::BackupKind::Scheduled,
            "t",
        )
        .unwrap();
        let page = storage_page(&conn, &path).unwrap();
        let backups = page.groups.last().unwrap();
        assert_eq!(backups.kind, StorageGroupKind::Backups);
        assert!(matches!(
            backups.items[0].hint,
            StorageMessage::StorageAutoBackups { count: 1, .. }
        ));
        assert!(
            matches!(page.summary, Some(StorageMessage::StorageSummary { backup_bytes, .. }) if backup_bytes > 0)
        );
    }
}
