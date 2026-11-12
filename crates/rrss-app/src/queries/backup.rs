use std::path::Path;

use chrono::{Datelike, Local, TimeZone};
use rusqlite::Connection;

use rrss_core::backup::{self, BackupKind, Counts, Manifest};
use rrss_core::db;

use super::storage::{fmt_bytes, fmt_count};
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

pub fn counts_summary(c: &Counts) -> String {
    format!(
        "文章 {} 篇 · 订阅 {} 个 · 规则 {} 条 · 对话 {} 个",
        fmt_count(c.articles),
        fmt_count(c.feeds),
        fmt_count(c.rules),
        fmt_count(c.threads)
    )
}

fn kind_label(m: &Manifest) -> String {
    match m.kind {
        BackupKind::Manual => "手动备份".into(),
        BackupKind::Upgrade => format!("升级前自动备份 · v{}", m.schema_version),
        BackupKind::Scheduled => "定时备份".into(),
        BackupKind::BeforeRestore => "恢复前的数据".into(),
        BackupKind::BeforePrune => "清理旧文章前自动备份".into(),
    }
}

pub fn backup_page(conn: &Connection, data_dir: &Path) -> rrss_core::Result<BackupPage> {
    let last_manual = db::get_setting(conn, LAST_MANUAL_KEY)?
        .and_then(|v| v.parse::<i64>().ok())
        .map(|ms| format!("上次备份：{}。", fmt_when(ms)))
        .unwrap_or_else(|| "还没有手动备份过。".into());
    let schedule = backup_schedule(conn)?;
    let entries: Vec<BackupEntryView> = backup::list_auto(data_dir)
        .into_iter()
        .map(|e| match &e.manifest {
            Some(m) => BackupEntryView {
                path: e.path.display().to_string(),
                when: fmt_when(m.created_at),
                note: format!(
                    "{} · {} · {}",
                    kind_label(m),
                    fmt_bytes(e.bytes as i64),
                    if m.secrets_included {
                        "包含密钥"
                    } else {
                        "不含密钥"
                    }
                ),
                restorable: true,
            },
            None => BackupEntryView {
                path: e.path.display().to_string(),
                when: fmt_when(e.modified_ms),
                note: format!("无法读取的文件 · {}", fmt_bytes(e.bytes as i64)),
                restorable: false,
            },
        })
        .collect();
    let auto_summary = format!(
        "升级前自动 · 定时{}",
        match schedule.as_str() {
            "daily" => "每天",
            "weekly" => "每周",
            _ => "关闭",
        }
    );
    let counts = rrss_core::prefs::export_counts(conn)?;
    Ok(BackupPage {
        last_manual,
        schedule,
        auto_summary,
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
        assert_eq!(page.last_manual, "还没有手动备份过。");
        assert!(page.entries.is_empty());
        assert!(page.restore_notice.is_none());

        backup::create_auto(&conn, dir.path(), BackupKind::Scheduled, "t").unwrap();
        std::fs::write(backup::auto_dir(dir.path()).join("broken.zip"), b"x").unwrap();
        db::set_setting(&conn, SCHEDULE_KEY, "weekly").unwrap();
        let page = backup_page(&conn, dir.path()).unwrap();
        assert_eq!(page.schedule, "weekly");
        assert_eq!(page.auto_summary, "升级前自动 · 定时每周");
        assert_eq!(page.entries.len(), 2);
        let good = page.entries.iter().find(|e| e.restorable).unwrap();
        assert!(good.when.starts_with("今天 "));
        assert!(good.note.starts_with("定时备份 · "), "{}", good.note);
        assert!(page.entries.iter().any(|e| !e.restorable));
    }
}
