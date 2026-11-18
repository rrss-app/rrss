//! Storage cleanup, retention, backup and restore orchestration.

use rrss_core::backup::{self, BackupKind};
use rrss_core::error::backup::BackupProblem;
use rrss_core::storage::{self, Cleanup};
use rrss_core::{ai_store, db};
use rusqlite::Connection;
use std::path::Path;

use crate::views::{BackupDoneView, RestoreCheckView};

/// 存储页的清理与压缩。`action` 取自 [`crate::views::StorageItemView::action`]；返回给用户看的结果。
pub fn storage_clean(
    conn: &Connection,
    db_path: &std::path::Path,
    action: &str,
) -> rrss_core::Result<crate::views::StorageMessage> {
    use crate::views::{StorageAction as A, StorageMessage as M};
    if action == "vacuum" {
        storage::vacuum(conn)?;
        return Ok(M::StorageCleaned {
            action: A::Vacuum,
            count: 0,
        });
    }
    if action == "logs" {
        crate::log::current()
            .ok_or_else(|| std::io::Error::other("logging is not initialized"))?
            .clear()?;
        return Ok(M::StorageCleaned {
            action: A::Logs,
            count: 0,
        });
    }
    let what =
        Cleanup::parse(action).ok_or_else(|| rrss_core::Error::UnknownCleanup(action.into()))?;
    let data_dir = db_path.parent().unwrap_or(std::path::Path::new("."));
    let model = ai_store::resolve_embed(conn)?.map(|e| e.model);
    let count = storage::clean(conn, data_dir, what, model.as_deref())?;
    let action = match what {
        Cleanup::AiCache => A::AiCache,
        Cleanup::RecommendationLogs => A::RecommendationLogs,
        Cleanup::Translations => A::Translations,
        Cleanup::Images => A::Images,
        Cleanup::Videos => A::Videos,
        Cleanup::UntrackedMedia => A::UntrackedMedia,
        Cleanup::OldVectors => A::OldVectors,
        Cleanup::LegacyTables => A::LegacyTables,
    };
    Ok(M::StorageCleaned { action, count })
}

/// 写进备份清单的程序版本。
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 手动完整备份到用户选的位置。
pub fn backup_create(
    conn: &Connection,
    data_dir: &Path,
    dest: &Path,
    include_secrets: bool,
) -> rrss_core::Result<BackupDoneView> {
    let manifest = backup::create(
        conn,
        data_dir,
        dest,
        BackupKind::Manual,
        APP_VERSION,
        include_secrets,
    )?;
    db::set_setting(
        conn,
        crate::queries::LAST_MANUAL_KEY,
        &manifest.created_at.to_string(),
    )?;
    Ok(BackupDoneView {
        path: dest.display().to_string(),
        bytes: storage::file_bytes(dest) as i64,
        summary: crate::views::BackupMessage::BackupCreated {
            count: manifest.counts.articles,
            feeds: manifest.counts.feeds,
            rules: manifest.counts.rules,
            threads: manifest.counts.threads,
            secrets_included: manifest.secrets_included,
        },
    })
}

/// 恢复第一步：解压到暂存目录并检查，给出备份内容请用户确认。
pub fn backup_inspect(data_dir: &Path, path: &Path) -> rrss_core::Result<RestoreCheckView> {
    let check = backup::inspect(path, data_dir)?;
    Ok(RestoreCheckView {
        path: path.display().to_string(),
        file_name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        created_at: check.manifest.created_at,
        schema_version: i64::from(check.schema_version),
        summary: crate::views::BackupMessage::BackupRestoreSummary {
            count: check.counts.articles,
            feeds: check.counts.feeds,
            rules: check.counts.rules,
            threads: check.counts.threads,
            secrets_included: check.manifest.secrets_included,
        },
    })
}

/// 恢复第二步：先把当前数据备份到自动备份里，再登记待办；调用方随后重启应用。
pub fn backup_restore(conn: &Connection, data_dir: &Path, path: &Path) -> rrss_core::Result<()> {
    backup::create_auto(conn, data_dir, BackupKind::BeforeRestore, APP_VERSION)?;
    backup::schedule_restore(data_dir, path)
}

pub fn backup_cancel_restore(data_dir: &Path) -> rrss_core::Result<()> {
    backup::cancel_restore(data_dir)
}

pub fn dismiss_restore_notice(data_dir: &Path) -> rrss_core::Result<()> {
    backup::clear_restore_result(data_dir)
}

pub fn set_backup_schedule(conn: &Connection, value: &str) -> rrss_core::Result<()> {
    if !matches!(value, "off" | "daily" | "weekly") {
        return Err(rrss_core::Error::Backup(BackupProblem::ScheduleUnknown {
            schedule: value.into(),
        }));
    }
    db::set_setting(conn, crate::queries::SCHEDULE_KEY, value)
}

/// 定时备份到期就生成一份；未开启或未到期时返回 `None`。刷新后的整理作业调用。
pub fn run_scheduled_backup(
    conn: &Connection,
    data_dir: &Path,
    now_ms: i64,
) -> rrss_core::Result<Option<std::path::PathBuf>> {
    const DAY: i64 = 24 * 3600 * 1000;
    // 刷新时间有抖动，提前一小时也算到期，免得每天的备份一天天往后推。
    const SLACK: i64 = 3600 * 1000;
    let every = match crate::queries::backup_schedule(conn)?.as_str() {
        "daily" => DAY,
        "weekly" => 7 * DAY,
        _ => return Ok(None),
    };
    if let Some(last) = backup::latest_auto(data_dir, BackupKind::Scheduled)
        && now_ms - last < every - SLACK
    {
        return Ok(None);
    }
    backup::create_auto(conn, data_dir, BackupKind::Scheduled, APP_VERSION).map(Some)
}

pub fn set_retention_days(conn: &Connection, days: i64) -> rrss_core::Result<()> {
    rrss_core::retention::set_global_days(conn, days)
}

const RETENTION_LAST_RUN: &str = "retention_last_run";
const SIGNALS_LAST_RUN: &str = "signals_retention_last_run";

/// 执行一轮保留清理：第一次清理前先自动备份，之后每批一个写事务。
/// `force` 为假时一天最多一次（刷新后的整理作业调用）；返回清理篇数与是否做了备份。
pub fn run_retention(
    conn: &Connection,
    data_dir: &Path,
    now_ms: i64,
    force: bool,
) -> rrss_core::Result<(i64, bool)> {
    use rrss_core::retention;
    const DAY: i64 = 24 * 3600 * 1000;
    let last_signals_run =
        db::get_setting(conn, SIGNALS_LAST_RUN)?.and_then(|v| v.parse::<i64>().ok());
    if force || last_signals_run.is_none_or(|last| now_ms.saturating_sub(last) >= DAY - 3600 * 1000)
    {
        rrss_core::signals::prune(conn, now_ms)?;
        db::set_setting(conn, SIGNALS_LAST_RUN, &now_ms.to_string())?;
    }
    let expired = retention::delete_expired(conn, now_ms)?;
    if !retention::active(conn)? {
        return Ok((expired, false));
    }
    if !force {
        let last = db::get_setting(conn, RETENTION_LAST_RUN)?
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        if now_ms - last < DAY - 3600 * 1000 {
            return Ok((expired, false));
        }
    }
    let mut backed_up = false;
    if !retention::ever_pruned(conn)? && !retention::due_batch(conn, now_ms, 1)?.is_empty() {
        backup::create_auto(conn, data_dir, BackupKind::BeforePrune, APP_VERSION)?;
        backed_up = true;
    }
    let mut total = expired;
    loop {
        let tx = db::write_transaction(conn)?;
        let ids = retention::due_batch(&tx, now_ms, retention::BATCH)?;
        if ids.is_empty() {
            break;
        }
        total += retention::prune_articles(&tx, &ids, now_ms)?;
        tx.commit()?;
        rrss_core::media::flush_removed_files(conn)?;
    }
    db::set_setting(conn, RETENTION_LAST_RUN, &now_ms.to_string())?;
    Ok((total, backed_up))
}

/// 存储页「立即清理」。
pub fn retention_clean_now(
    conn: &Connection,
    data_dir: &Path,
) -> rrss_core::Result<crate::views::RetentionMessage> {
    let (count, backed_up) = run_retention(conn, data_dir, rrss_core::feeds::now_ms(), true)?;
    Ok(crate::views::RetentionMessage::RetentionCleaned { count, backed_up })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::open_temp;

    #[test]
    fn manual_backup_records_time_and_restore_is_scheduled_after_safety_backup() {
        let (dir, conn) = open_temp();
        let dest = dir.path().join("out/manual.zip");
        let done = backup_create(&conn, dir.path(), &dest, false).unwrap();
        assert!(matches!(
            done.summary,
            crate::views::BackupMessage::BackupCreated { count: 0, .. }
        ));
        assert!(
            crate::queries::backup_page(&conn, dir.path())
                .unwrap()
                .last_manual_at
                .is_some()
        );

        let check = backup_inspect(dir.path(), &dest).unwrap();
        assert_eq!(check.file_name, "manual.zip");
        assert_eq!(check.schema_version, i64::from(db::SCHEMA_VERSION));
        backup_restore(&conn, dir.path(), &dest).unwrap();
        let entries = backup::list_auto(dir.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].manifest.as_ref().unwrap().kind,
            BackupKind::BeforeRestore
        );
        assert!(dir.path().join("restore-pending.json").is_file());
        backup_cancel_restore(dir.path()).unwrap();
        assert!(!dir.path().join("restore-pending.json").exists());
    }

    #[test]
    fn scheduled_backup_runs_only_when_due() {
        let (dir, conn) = open_temp();
        let now = rrss_core::feeds::now_ms();
        assert!(
            run_scheduled_backup(&conn, dir.path(), now)
                .unwrap()
                .is_none()
        );
        assert!(set_backup_schedule(&conn, "hourly").is_err());
        set_backup_schedule(&conn, "daily").unwrap();
        assert!(
            run_scheduled_backup(&conn, dir.path(), now)
                .unwrap()
                .is_some()
        );
        assert!(
            run_scheduled_backup(&conn, dir.path(), now + 3600 * 1000)
                .unwrap()
                .is_none()
        );
        assert!(
            run_scheduled_backup(&conn, dir.path(), now + 47 * 1800 * 1000)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn signals_retention_is_fixed_and_independent_of_body_retention() {
        let (dir, conn) = open_temp();
        let now = 1_800_000_000_000;
        let cutoff = now - rrss_core::signals::RETENTION_MS;
        conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://test.invalid/rss',0);
            INSERT INTO articles(id,feed_id,title,content_html,created_at) VALUES(1,1,'t','<p>body</p>',0);").unwrap();
        let insert = |at: i64| {
            conn.execute("INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
                VALUES(1,'focus',0,0,0,0,'baseline',?1,?2)", rusqlite::params![at.to_string(),at]).unwrap();
            conn.execute(
                "INSERT INTO article_events(article_id,kind,at) VALUES(1,'open',?1)",
                [at],
            )
            .unwrap();
            conn.execute("INSERT INTO read_sessions(article_id,opened_at,active_ms,idle_ms,read_chars,skimmed_chars,total_chars,max_block,reached_end,translated,ended_by,via)
                VALUES(1,?1,0,0,0,0,0,0,0,0,'back','focus')", [at]).unwrap();
        };
        let counts = || {
            ["exposures", "article_events", "read_sessions"].map(|t| {
                conn.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap()
            })
        };
        insert(cutoff - 1);
        insert(cutoff);
        insert(now - 100 * 24 * 3600 * 1000);
        assert_eq!(
            run_retention(&conn, dir.path(), now, false).unwrap(),
            (0, false)
        );
        assert_eq!(counts(), [2; 3]);
        assert!(backup::list_auto(dir.path()).is_empty());
        assert_eq!(
            conn.query_row("SELECT content_html FROM articles", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "<p>body</p>"
        );
        insert(cutoff - 2);
        run_retention(&conn, dir.path(), now + 1000, false).unwrap();
        assert_eq!(
            counts(),
            [3; 3],
            "daily throttle applies while body retention is off"
        );
        run_retention(&conn, dir.path(), now, true).unwrap();
        assert_eq!(counts(), [2; 3], "force bypasses log throttle");
        set_retention_days(&conn, 90).unwrap();
        run_retention(&conn, dir.path(), now, true).unwrap();
        assert_eq!(
            counts(),
            [2; 3],
            "body retention does not shorten log retention"
        );
        set_retention_days(&conn, 0).unwrap();
        run_retention(&conn, dir.path(), now + 24 * 3600 * 1000, false).unwrap();
        assert_eq!(
            counts(),
            [1; 3],
            "logs keep pruning after body retention is disabled"
        );
    }

    #[test]
    fn retention_backs_up_once_prunes_in_batches_and_runs_daily() {
        let (dir, conn) = open_temp();
        let now = rrss_core::feeds::now_ms();
        let old = now - 200 * 24 * 3600 * 1000;
        conn.execute(
            "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://a/rss', 0)",
            [],
        )
        .unwrap();
        for i in 0..(rrss_core::retention::BATCH + 5) {
            conn.execute(
                "INSERT INTO articles (feed_id, guid, title, content_html, created_at) VALUES (1, ?1, 't', '<p>x</p>', ?2)",
                rusqlite::params![format!("g{i}"), old],
            )
            .unwrap();
        }
        assert_eq!(
            run_retention(&conn, dir.path(), now, false).unwrap(),
            (0, false),
            "off by default"
        );
        set_retention_days(&conn, 90).unwrap();
        let page = crate::queries::retention_page(&conn, now).unwrap();
        assert!(matches!(
            page.preview,
            Some(crate::views::RetentionMessage::RetentionPreview { count: 205 })
        ));
        assert!(page.first_run);
        assert!(matches!(
            page.confirm[2],
            crate::views::RetentionMessage::RetentionBackup {}
        ));

        let (n, backed_up) = run_retention(&conn, dir.path(), now, false).unwrap();
        assert_eq!((n, backed_up), (rrss_core::retention::BATCH + 5, true));
        assert_eq!(
            backup::list_auto(dir.path())[0]
                .manifest
                .as_ref()
                .unwrap()
                .kind,
            BackupKind::BeforePrune
        );
        conn.execute(
            "INSERT INTO articles (feed_id, guid, title, content_html, created_at) VALUES (1, 'late', 't', '<p>x</p>', ?1)",
            [old],
        )
        .unwrap();
        assert_eq!(
            run_retention(&conn, dir.path(), now + 3600 * 1000, false).unwrap(),
            (0, false),
            "once a day"
        );
        let msg = retention_clean_now(&conn, dir.path()).unwrap();
        assert!(matches!(
            msg,
            crate::views::RetentionMessage::RetentionCleaned {
                count: 1,
                backed_up: false
            }
        ));
        assert_eq!(
            backup::list_auto(dir.path()).len(),
            1,
            "no second pre-prune backup"
        );
        let page = crate::queries::retention_page(&conn, now).unwrap();
        assert!(!page.can_run);
        assert!(!page.first_run);
    }

    #[test]
    fn storage_clean_reports_and_rejects_unknown_actions() {
        let (dir, conn) = open_temp();
        let path = dir.path().join("t.db");
        conn.execute(
            "INSERT INTO ai_cache (cache_key, value, created_at) VALUES ('k', 'v', 1)",
            [],
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(storage_clean(&conn, &path, "ai_cache").unwrap()).unwrap(),
            serde_json::json!({ "code": "storage_cleaned", "params": { "action": "ai_cache", "count": 1 } })
        );
        assert_eq!(
            serde_json::to_value(storage_clean(&conn, &path, "recommendation_logs").unwrap())
                .unwrap(),
            serde_json::json!({ "code": "storage_cleaned", "params": { "action": "recommendation_logs", "count": 0 } })
        );
        assert_eq!(
            serde_json::to_value(storage_clean(&conn, &path, "vacuum").unwrap()).unwrap(),
            serde_json::json!({ "code": "storage_cleaned", "params": { "action": "vacuum", "count": 0 } })
        );
        assert!(matches!(
            storage_clean(&conn, &path, "drop_everything"),
            Err(rrss_core::Error::UnknownCleanup(_))
        ));
    }
}
