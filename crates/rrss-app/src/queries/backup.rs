use std::path::Path;

use chrono::Local;
use rusqlite::Connection;

use rrss_core::backup::{self, BackupKind};
use rrss_core::db;

use crate::views::*;

pub const SCHEDULE_KEY: &str = "backup_schedule";
pub const LAST_MANUAL_KEY: &str = "backup_last_manual";

/// 定时备份：off / daily / weekly，默认关（阶段 20 D7）。
pub fn backup_schedule(conn: &Connection) -> rrss_core::Result<String> {
    let value = db::get_setting(conn, SCHEDULE_KEY)?.unwrap_or_default();
    Ok(match value.as_str() {
        "daily" | "weekly" => value,
        _ => "off".into(),
    })
}

pub fn backup_page(conn: &Connection, data_dir: &Path) -> rrss_core::Result<BackupPage> {
    let last_manual_at =
        db::get_setting(conn, LAST_MANUAL_KEY)?.and_then(|v| v.parse::<i64>().ok());
    let schedule = backup_schedule(conn)?;
    let entries = backup::list_auto(data_dir)
        .into_iter()
        .map(|e| {
            let (created_at, kind, schema_version, secrets_included) = match e.manifest.as_ref() {
                Some(m) => (
                    m.created_at,
                    match m.kind {
                        BackupKind::Manual => BackupEntryKind::Manual,
                        BackupKind::Upgrade => BackupEntryKind::Upgrade,
                        BackupKind::Scheduled => BackupEntryKind::Scheduled,
                        BackupKind::BeforeRestore => BackupEntryKind::BeforeRestore,
                        BackupKind::BeforePrune => BackupEntryKind::BeforePrune,
                    },
                    m.schema_version,
                    Some(m.secrets_included),
                ),
                None => (e.modified_ms, BackupEntryKind::Unreadable, 0, None),
            };
            BackupEntryView {
                path: e.path.display().to_string(),
                created_at,
                kind,
                schema_version: i64::from(schema_version),
                bytes: e.bytes as i64,
                secrets_included,
                restorable: e.manifest.is_some(),
            }
        })
        .collect();
    let counts = rrss_core::prefs::export_counts(conn)?;
    Ok(BackupPage {
        last_manual_at,
        schedule,
        entries,
        restore_notice: backup::restore_result(data_dir).map(restore_notice),
        rules: counts.rules,
        interests: counts.interests,
        memories: counts.memories,
    })
}

fn restore_notice(result: backup::RestoreResult) -> RestoreNoticeView {
    use backup::RestoreOutcome as O;
    let message = match result.outcome {
        Some(O::Restored { name }) => BackupMessage::RestoreDone { name },
        Some(O::ExitTimeout { name, seconds }) => {
            BackupMessage::RestoreExitTimeout { name, seconds }
        }
        Some(O::StagingIncomplete { name }) => BackupMessage::RestoreStagingIncomplete { name },
        Some(O::RolledBack { name }) => BackupMessage::RestoreRolledBack { name },
        Some(O::Partial {
            name,
            old_dir,
            kept_dir,
        }) => BackupMessage::RestorePartial {
            name,
            old_dir,
            kept_dir,
        },
        None => BackupMessage::RestoreLegacy {
            text: result.message,
        },
    };
    RestoreNoticeView {
        ok: result.ok,
        message,
    }
}

/// 手动备份的默认文件名。
pub fn default_backup_name() -> String {
    format!("rrss-backup-{}.zip", Local::now().format("%Y-%m-%d-%H%M"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_notices_keep_old_sentences_and_code_new_outcomes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("restore-result.json");
        std::fs::write(&file, r#"{"ok":true,"message":"old sentence","at":1}"#).unwrap();
        let old = restore_notice(backup::restore_result(dir.path()).unwrap());
        assert!(
            matches!(old.message, BackupMessage::RestoreLegacy { text } if text == "old sentence")
        );
        std::fs::write(
            &file,
            r#"{"ok":false,"outcome":{"code":"exit_timeout","name":"a.zip","seconds":10},"at":1}"#,
        )
        .unwrap();
        let new = restore_notice(backup::restore_result(dir.path()).unwrap());
        let wire = serde_json::to_value(&new.message).unwrap();
        assert_eq!(wire["code"], "restore_exit_timeout");
        assert_eq!(wire["params"]["seconds"], 10);
    }

    #[test]
    fn page_lists_auto_backups_and_schedule() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("rrss.db")).unwrap();
        let page = backup_page(&conn, dir.path()).unwrap();
        assert_eq!(page.schedule, "off");
        assert_eq!(page.last_manual_at, None);
        assert!(page.entries.is_empty());
        assert!(page.restore_notice.is_none());

        backup::create_auto(&conn, dir.path(), BackupKind::Scheduled, "t").unwrap();
        std::fs::write(backup::auto_dir(dir.path()).join("broken.zip"), b"x").unwrap();
        db::set_setting(&conn, SCHEDULE_KEY, "weekly").unwrap();
        let page = backup_page(&conn, dir.path()).unwrap();
        assert_eq!(page.schedule, "weekly");
        assert_eq!(page.entries.len(), 2);
        let good = page.entries.iter().find(|e| e.restorable).unwrap();
        assert!(good.created_at > 0);
        assert_eq!(good.kind, BackupEntryKind::Scheduled);
        assert!(page.entries.iter().any(|e| !e.restorable));
    }
}
