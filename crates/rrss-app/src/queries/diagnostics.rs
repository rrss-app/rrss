use std::path::Path;

use rusqlite::Connection;

use rrss_core::ai_store;
use rrss_core::pi_store;
use rrss_core::rag::{self, vec_available, vector_stats};

use crate::views::*;

/// The text fields are English diagnostics, like logs; they are not UI copy.
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
            "sqlite-vec unavailable: {}",
            vec.reason.unwrap_or_else(|| "unknown".into())
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
            "vectors {}/{} embedded, pending chunks {} / articles {}; interests {}/{}, memories embedded {}; dims [{}] models [{}]\n{}",
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
        "db {} · {} bytes · user_version {version} · {wal} · FTS {fts} rows",
        db_path.display(),
        size
    );
    let day = ai_store::local_day();
    let usage = ai_store::usage_on(conn, &day).unwrap_or_default();
    let budget = ai_store::daily_budget_tokens(conn).unwrap_or(0);
    let cache = rag::cache_row_count(conn).unwrap_or(0);
    let pi = pi_store::pi_usage_on(conn, &day).unwrap_or_default();
    page.ai = format!(
        "jobs today {} tok / budget {budget} · cache {cache} rows · chat {}+{} tok / ${:.4}",
        usage.tokens, pi.input_tokens, pi.output_tokens, pi.cost
    );
    if let Some(scheduler) = &page.scheduler {
        let feeds = &scheduler.feeds;
        page.stats.push_str(&format!(
            "\nscheduler: fetch queued {} (host parked {}), downloads {}/{}, parsing {}/{}, body buffer {} bytes / high water {} bytes; models active {}, interactive waiting {}, background waiting {}.",
            feeds.queued, feeds.parked, feeds.http_active, feeds.http_limit,
            feeds.parsing, feeds.parsing_limit, feeds.buffered_bytes, feeds.buffer_high_water,
            scheduler.models.active, scheduler.models.waiting_interactive, scheduler.models.waiting_background,
        ));
    }
    page
}
