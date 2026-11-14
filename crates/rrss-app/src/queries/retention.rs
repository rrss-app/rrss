use rusqlite::Connection;

use rrss_core::retention;

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
    let mut confirm = vec![
        RetentionMessage::RetentionConfirm {
            count: preview.articles,
            days,
        },
        RetentionMessage::RetentionPreserve {},
    ];
    if first {
        confirm.push(RetentionMessage::RetentionBackup {});
    }
    Ok(RetentionPage {
        days,
        default_days: retention::DEFAULT_DAYS,
        custom_feeds,
        preview: if !active {
            None
        } else if preview.articles > 0 {
            Some(RetentionMessage::RetentionPreview {
                count: preview.articles,
            })
        } else {
            Some(RetentionMessage::RetentionPreviewEmpty {})
        },
        preview_hint: if preview.articles > 0 {
            Some(RetentionMessage::RetentionPreviewHint {
                bytes: preview.bytes,
                first,
            })
        } else if active {
            Some(RetentionMessage::RetentionDaily {})
        } else {
            None
        },
        can_run: preview.articles > 0,
        first_run: first,
        confirm,
    })
}
