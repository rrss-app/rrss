//! 读 SQLite，返回前端视图。无 UI 类型。

pub mod ai;
pub mod backup;
pub mod chat;
pub mod diagnostics;
pub mod feeds;
pub mod focus;
mod hidden;
pub mod media;
pub mod memories;
pub mod network;
pub mod pi;
pub mod reader;
pub mod retention;
mod rule_stats;
pub mod rules;
pub mod search;
pub mod storage;
pub mod timeline;
pub mod vector_space;
pub mod window;

pub use ai::*;
pub use backup::*;
pub use chat::*;
pub use diagnostics::*;
pub use feeds::*;
pub use focus::*;
pub use hidden::*;
pub use media::*;
pub use memories::*;
pub use network::*;
pub use pi::*;
pub use reader::*;
pub use retention::*;
pub use rule_stats::*;
pub use rules::*;
pub use search::*;
pub use storage::*;
pub use timeline::*;
pub use vector_space::*;
pub use window::*;

#[cfg(test)]
mod tests {
    use super::{
        digest, digest_banner_text, digest_summaries, focus_page, media_href,
        media_href_with_fingerprint, media_id_from_path,
    };
    use rrss_core::feed_scope::FeedScope;
    use rrss_render::{Block, Inline};

    #[test]
    fn media_href_matches_tauri_2_platform() {
        // 各平台的来源写法在 `platform` 里逐个测；这里只看地址拼接。
        let origin = rrss_core::platform::Os::current().protocol_origin("rrss");
        assert_eq!(media_href(7), format!("{origin}/media/7"));
        assert_eq!(
            media_href_with_fingerprint(7, "abc123def456"),
            format!("{origin}/media/7?v=abc123def456")
        );
    }

    #[test]
    fn media_id_from_common_paths() {
        assert_eq!(media_id_from_path("/media/12"), Some(12));
        assert_eq!(media_id_from_path("media/12"), Some(12));
        assert_eq!(media_id_from_path("/localhost/media/12"), Some(12));
        assert_eq!(media_id_from_path("/media/12?cache=1"), Some(12));
        assert_eq!(media_id_from_path("/other/12"), None);
    }

    #[test]
    fn digest_markdown_becomes_ir_not_one_paragraph() {
        let dir = tempfile::tempdir().unwrap();
        let conn = rrss_core::db::open_at(&dir.path().join("rrss.db")).unwrap();
        rrss_core::ai_store::insert_digest(
            &conn,
            "daily",
            1,
            "### 焦点\n\nHello **world**\n\n- item a\n- item b\n",
            "[]",
        )
        .unwrap();
        let rows = digest_summaries(&conn, "daily");
        assert_eq!(rows.len(), 1);
        let view = digest(&conn, rows[0].id).unwrap();
        let doc: rrss_render::Document =
            serde_json::from_value(view.document.clone().expect("document")).unwrap();
        assert!(
            doc.blocks
                .iter()
                .any(|b| matches!(b, Block::Heading { level: 3, .. })),
            "heading: {:?}",
            doc.blocks
        );
        assert!(
            doc.blocks
                .iter()
                .any(|b| matches!(b, Block::ListItem { .. })),
            "list: {:?}",
            doc.blocks
        );
        let has_strong = doc.blocks.iter().any(|b| match b {
            Block::Paragraph { inlines } | Block::Heading { inlines, .. } => {
                inlines.iter().any(|i| matches!(i, Inline::Strong { .. }))
            }
            _ => false,
        });
        assert!(has_strong, "strong missing: {:?}", doc.blocks);

        let page = focus_page(&conn, None, &[], &[], false, FeedScope::All).unwrap();
        assert!(!page.digest_banner.contains("###"));
        assert!(!page.digest_banner.contains("**"));
        assert!(
            page.digest_banner.starts_with("今日简报"),
            "{}",
            page.digest_banner
        );
        assert!(!page.ranking.interest_ready);
        assert!(
            page.ranking.ranking_note.contains("兴趣未就绪"),
            "{}",
            page.ranking.ranking_note
        );
    }

    #[test]
    fn digests_filters_by_period_and_covers_same_window() {
        let dir = tempfile::tempdir().unwrap();
        let conn = rrss_core::db::open_at(&dir.path().join("rrss.db")).unwrap();
        rrss_core::ai_store::insert_digest(&conn, "daily", 1_756_224_000_000, "daily-a", "[]")
            .unwrap();
        rrss_core::ai_store::insert_digest(&conn, "weekly", 1_756_051_200_000, "weekly-a", "[]")
            .unwrap();
        let daily = digest_summaries(&conn, "daily");
        assert_eq!(daily.len(), 1);
        assert_eq!(daily[0].period, "daily");
        assert_eq!(daily[0].period_start, 1_756_224_000_000);
        assert_eq!(
            daily[0].date_label,
            rrss_core::ai_store::digest_date_label("daily", 1_756_224_000_000)
        );
        assert!(
            digest_summaries(&conn, "weekly")
                .iter()
                .all(|d| d.period == "weekly")
        );
        assert_eq!(digest_summaries(&conn, "weekly").len(), 1);

        rrss_core::ai_store::insert_digest(&conn, "daily", 1_756_224_000_000, "daily-b", "[]")
            .unwrap();
        let daily = digest_summaries(&conn, "daily");
        assert_eq!(daily.len(), 1);
        assert_eq!(digest(&conn, daily[0].id).unwrap().body, "daily-b");
    }

    #[test]
    fn digest_banner_strips_markdown_and_truncates() {
        let wall = format!("### {}\n\n{}", "标题党", "字".repeat(200));
        let banner = digest_banner_text(&wall);
        assert!(!banner.contains("###"));
        assert!(banner.chars().count() <= 97, "{banner}");
        assert!(banner.contains('…') || banner.chars().count() <= 96);
    }
}
