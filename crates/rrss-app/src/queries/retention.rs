use rusqlite::Connection;

use rrss_core::retention;

use super::storage::{fmt_bytes, fmt_count};
use crate::views::*;

/// 存储页「保留策略」。预览要按条件扫一遍文章和引用，只在打开存储页时读。
pub fn retention_page(conn: &Connection, now_ms: i64) -> rrss_core::Result<RetentionPage> {
    let days = retention::global_days(conn)?;
    let custom_feeds: i64 = conn.query_row(
        "SELECT COUNT(*) FROM feeds WHERE retention_days IS NOT NULL",
        [],
        |r| r.get(0),
    )?;
    let active = retention::active(conn)?;
    let preview = if active {
        retention::preview(conn, now_ms)?
    } else {
        retention::Preview::default()
    };
    let first = !retention::ever_pruned(conn)?;
    let scope = if days > 0 {
        format!("入库满 {days} 天")
    } else {
        "单独设了保留天数的源里过期".into()
    };
    let mut confirm = vec![
        format!(
            "清理 {} 篇{}的文章的正文、向量、译文和配图。",
            fmt_count(preview.articles),
            scope
        ),
        "标题、链接、阅读记录和赞踩保留，仍能按标题搜到；打开时可以重新抓取正文。".into(),
    ];
    if first {
        confirm.push("这是第一次清理，会先自动备份全部数据。".into());
    }
    Ok(RetentionPage {
        days,
        default_days: retention::DEFAULT_DAYS,
        custom_feeds,
        preview: if !active {
            String::new()
        } else if preview.articles > 0 {
            format!("将清理 {} 篇", fmt_count(preview.articles))
        } else {
            "目前没有要清理的文章".into()
        },
        preview_hint: if preview.articles > 0 {
            format!(
                "预计释放约 {}，清理后可压缩数据库还给磁盘。每天刷新后自动执行{}。",
                fmt_bytes(preview.bytes),
                if first {
                    "；第一次执行前会自动备份"
                } else {
                    ""
                }
            )
        } else if active {
            "每天刷新后自动检查一次。".into()
        } else {
            String::new()
        },
        can_run: preview.articles > 0,
        confirm_title: "立即清理旧文章正文".into(),
        confirm,
        confirm_ok: if first {
            "备份并清理".into()
        } else {
            "清理".into()
        },
    })
}
