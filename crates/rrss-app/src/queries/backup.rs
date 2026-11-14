use std::path::Path;

use chrono::{Datelike, Local, TimeZone};
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

/// 「今天 09:12」「9 月 20 日 22:40」「2025 年 9 月 20 日」。
pub fn fmt_when(ms: i64) -> String {
    let Some(t) = Local.timestamp_millis_opt(ms).single() else {
        return String::new();
    };
    let today = Local::now().date_naive();
    let day = t.date_naive();
    let clock = t.format("%H:%M");
    if day == today {
        format!("今天 {clock}")
    } else if today.pred_opt() == Some(day) {
        format!("昨天 {clock}")
    } else if day.year() == today.year() {
        format!("{} 月 {} 日 {clock}", day.month(), day.day())
    } else {
        format!("{} 年 {} 月 {} 日", day.year(), day.month(), day.day())
    }
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
        restore_notice: backup::restore_result(data_dir).map(|r| RestoreNoticeView {
            ok: r.ok,
            message: r.message,
        }),
        rules: counts.rules,
        interests: counts.interests,
        memories: counts.memories,
    })
}

/// 手动备份的默认文件名。
pub fn default_backup_name() -> String {
    format!("rrss-backup-{}.zip", Local::now().format("%Y-%m-%d-%H%M"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_times_relative_to_today() {
        let now = Local::now();
        assert!(fmt_when(now.timestamp_millis()).starts_with("今天 "));
        let yesterday = now - chrono::Duration::days(1);
        assert!(fmt_when(yesterday.timestamp_millis()).starts_with("昨天 "));
        let old = Local.with_ymd_and_hms(2001, 9, 20, 22, 40, 0).unwrap();
        assert_eq!(fmt_when(old.timestamp_millis()), "2001 年 9 月 20 日");
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
