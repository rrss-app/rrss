//! 领域层：订阅、SQLite、规则、OPML。不含 UI。
//!
//! 日志 target 为模块路径（`rrss_core::db`、`rrss_core::ingest`、`rrss_core::rules`）。

pub mod ai_store;
pub mod articles;
pub mod backup;
pub mod chunk;
pub mod cite;
pub mod cluster;
pub mod cluster_summary;
pub mod db;
pub mod digest;
pub mod digest_sources;
pub mod error;
pub mod eval;
pub mod features;
pub mod feed_health;
pub mod feed_icon;
pub mod feed_scope;
pub mod feedback;
pub mod feeds;
pub mod hidden;
pub mod image_size;
pub mod ingest;
pub mod interests;
pub mod kind;
pub mod logsafe;
pub mod media;
pub mod memories;
pub mod model_pool;
pub mod network;
pub mod opml;
pub mod parse;
pub mod paths;
pub mod pi_store;
pub mod platform;
pub mod prefs;
pub mod presets;
pub mod rag;
pub mod rank;
pub mod retention;
pub mod rule_stats;
pub mod rules;
pub mod score;
pub mod search;
pub mod signals;
pub mod simhash;
pub mod storage;
pub mod summary_extract;
pub mod text;
pub mod threads;
pub mod timeline;
pub mod translations;
pub mod tree_order;
pub mod url_norm;
pub mod vector_space;

#[cfg(test)]
mod test_log;

pub use ai_store::{
    AI_DAILY_BUDGET_TOKENS_KEY, ALL_TASKS, AiProvider, DEVELOPER_TOOLS_KEY, EmbedEndpoint,
    FETCH_URL_ENABLED_KEY, PI_BINARY_KEY, ResolvedTask, TASK_ARTICLE_SUMMARY,
    TASK_ARTICLE_TRANSLATE, TASK_CHAT, TASK_CLUSTER_SUMMARY,
    TASK_DIGEST_DAILY, TASK_DIGEST_WEEKLY, TASK_EMBED, TASK_EPISODE_COMPRESS, TASK_FOCUS_SUMMARY,
    TASK_MEMORY_CANDIDATE,
};
pub use articles::{
    ArticleCard, ArticleRow, FOCUS_PAGE_SIZE, FocusFilter, get_article, list_focus,
    list_focus_page, list_focus_stream, set_focus_filter, set_hide_threshold, set_read,
    set_starred, set_summary,
};
pub use cite::{CitationSnap, expand_user_prompt, parse_citations_json, passage_around};
pub use cluster::{assign_article, assign_unclustered, backfill_simhash, merge_close_clusters};
pub use db::{
    CLUSTER_GROUP_ENABLED_KEY, FOCUS_FILTER_KEY, FOCUS_RERANK_ENABLED_KEY,
    FOCUS_SUMMARY_NEUTRAL_LIMIT_KEY, HIDE_NOISY_KEY, HIDE_THRESHOLD_KEY,
    INTEREST_BOOST_ENABLED_KEY, PI_IDLE_TIMEOUT_SECS_KEY, PooledConnection, SCHEMA_VERSION,
    THEME_PREFERENCE_KEY, get_setting, open, open_at, set_setting, set_theme_preference,
    theme_preference,
};
pub use digest::{
    DigestItem, DigestRow, DigestSource, align_digest_sources, apply_digest_picks,
    count_digest_list_items, digest_by_id, digest_candidates, digest_date_label,
    digest_generate_due, digest_generate_minutes, digest_pack_input, format_digest_catalog,
    format_digest_deep_input, format_digest_input, format_digest_items, insert_digest,
    latest_digest, list_digests, one_liner, parse_digest_item_ns, parse_digest_items_json,
    parse_digest_picks, parse_summary_details, parse_summary_details_for_digest, period_start_ms,
    sanitize_digest_blurb, sanitize_digest_body, serialize_digest_items, serialize_digest_sources,
    set_digest_generate_minutes,
};
pub use error::{Error, Result};
pub use features::{FeatureChip, extract_features};
pub use feedback::{FeedbackKind, RateResult, feedback_for_article, rate_article};
pub use feeds::{
    FeedRow, FolderRow, FolderView, SubscribeRequest, create_folder, delete_folder, display_title,
    list_folder_views, list_folders, rename_feed, rename_folder,
    set_folder, subscribe, unsubscribe, update_feed_url,
};
pub use ingest::{
    FetchRequest, FetchResponse, HttpFetcher, RefreshResult, RefreshStatus, apply_fetch, due_feeds,
    fetch_http, http_client, record_fetch_failure, refresh_feed,
};
pub use interests::{
    Interest, InterestHit, InterestReason, boost_enabled, cluster_group_enabled, delete_interest,
    hide_noisy_enabled, insert_interest, interest_ready, list_interest_hits, list_interests,
    parse_interest_reasons, ranking_note, recompute_interest_boost, rerank_enabled,
    set_boost_enabled, set_cluster_group_enabled, set_enabled as set_interest_enabled,
    set_hide_noisy_enabled, set_rerank_enabled, set_weight as set_interest_weight,
};
pub use kind::FeedKind;
pub use media::{
    MAX_CACHE_BYTES, MAX_DIRECT_BYTES, MAX_DIRECT_CACHE_BYTES, MAX_IMAGE_BYTES, MediaRow,
    allowed_url, cache_path, cleanup_article_media, cleanup_feed_media, direct_len_allowed,
    evict_direct_lru, evict_lru, get as get_media, lookup, parse_byte_range, read_file_span,
    remove_cached, sniff_image_ext, sniff_image_mime, store_direct, store_image, store_sidecar,
    touch,
};
pub use memories::{Memory, MemoryKind, inject_preview, list_memories, search_memory};
pub use opml::{ImportReport, export_opml, import_opml};
pub use parse::{ParsedFeed, parse_feed_bytes};
pub use paths::{data_dir, db_path};
pub use pi_store::{McpServer, PiTool, ToolManifest};
pub use presets::{PackStatus, PresetPack};
pub use rag::{format_rag_context, hybrid_search, vec_available, vector_stats};
pub use rules::{
    Dimension, NewRule, Polarity, Rule, RuleAction, RuleFilter, RuleSource, ScopeKind,
    confirm_rule, delete_rule, insert_rule, list_rules, set_rule_action, set_rule_enabled,
    update_rule, validate_pattern,
};
pub use score::{Reason, ScoreResult, parse_reasons, rescore_all, tags_for_article};
pub use search::{SearchHit, search_articles};
pub use threads::{Message, MessageRole, Thread, create_thread, insert_message, list_messages};
pub use translations::{
    ArticleTranslation, ENGINE_COMPLETE, ENGINE_DEEPL, ENGINE_DEEPLX, ENGINE_HTTP,
    TRANSLATE_BACKEND_KEY, TRANSLATE_HTTP_KEY_KEY, TRANSLATE_HTTP_URL_KEY,
    TRANSLATE_TARGET_LANG_KEY, backend_id as translate_backend_id,
    target_lang as translate_target_lang,
};

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_matches() {
        assert_eq!(env!("CARGO_PKG_NAME"), "rrss-core");
    }
}
