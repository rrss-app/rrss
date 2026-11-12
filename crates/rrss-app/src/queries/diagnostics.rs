use std::path::Path;

use rusqlite::Connection;

use rrss_core::ai_store;
use rrss_core::pi_store;
use rrss_core::rag::{self, vec_available, vector_stats};

use crate::views::*;

pub fn diagnostics(conn: &Connection, db_path: &Path) -> DiagnosticsPage {
    let mut page = DiagnosticsPage {
        scheduler: crate::scheduler::Scheduler::diagnostics_for(db_path),
        vec: String::new(),
        stats: String::new(),
        db: String::new(),
        ai: String::new(),
        log: crate::log::current().and_then(crate::log::Handle::status_view),
    };
    let vec = vec_available(conn);
    page.vec = if vec.available {
        format!("sqlite-vec {}", vec.version.unwrap_or_default())
    } else {
        format!(
            "sqlite-vec 不可用：{}",
            vec.reason.unwrap_or_else(|| "未知".into())
        )
    };
    if let Ok(stats) = vector_stats(conn) {
        let dims = stats
            .dim_histogram
            .iter()
            .map(|(d, n)| format!("{d}d×{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let models = stats
            .model_histogram
            .iter()
            .map(|(m, n)| format!("{m}:{n}"))
            .collect::<Vec<_>>()
            .join(", ");
        let errs = stats
            .recent_errors
            .iter()
            .map(|e| format!("#{} a{}: {}", e.chunk_id, e.attempts, e.last_error))
            .collect::<Vec<_>>()
            .join("\n");
        page.stats = format!(
            "向量 {}/{} 已嵌入，待嵌入 chunk {} / 文章 {}。兴趣 {}/{}，记忆已嵌入 {}。维度 [{}] 模型 [{}]\n{}",
            stats.chunks_embedded,
            stats.chunks_total,
            stats.chunks_pending,
            stats.articles_pending,
            stats.interests_embedded,
            stats.interests_total,
            stats.memories_embedded,
            dims,
            models,
            errs
        );
    }
    let fts = rag::fts_row_count(conn).unwrap_or(0);
    let size = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap_or(0);
    let wal: String = conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap_or_else(|_| "unknown".into());
    page.db = format!(
        "库 {} · {} 字节 · user_version {version} · {wal} · FTS {fts} 行",
        db_path.display(),
        size
    );
    let day = ai_store::local_day();
    let usage = ai_store::usage_on(conn, &day).unwrap_or_default();
    let budget = ai_store::daily_budget_tokens(conn).unwrap_or(0);
    let cache = rag::cache_row_count(conn).unwrap_or(0);
    let pi = pi_store::pi_usage_on(conn, &day).unwrap_or_default();
    page.ai = format!(
        "作业今日 {} tok / 预算 {budget} · 缓存 {cache} 条 · 对话 {}+{} tok / ${:.4}",
        usage.tokens, pi.input_tokens, pi.output_tokens, pi.cost
    );
    if let Some(scheduler) = &page.scheduler {
        let feeds = &scheduler.feeds;
        page.stats.push_str(&format!(
            "\n调度：抓取等待 {}（主机等待 {}），下载名额 {}/{}，解析 {}/{}，正文缓冲 {} 字节 / 高水位 {} 字节；模型在途 {}，交互等待 {}，后台等待 {}。",
            feeds.queued, feeds.parked, feeds.http_active, feeds.http_limit,
            feeds.parsing, feeds.parsing_limit, feeds.buffered_bytes, feeds.buffer_high_water,
            scheduler.models.active, scheduler.models.waiting_interactive, scheduler.models.waiting_background,
        ));
    }
    page
}
