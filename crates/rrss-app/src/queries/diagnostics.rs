use std::path::Path;

use rusqlite::Connection;

use rrss_core::ai_store;
use rrss_core::pi_store;
use rrss_core::rag::{self, vec_available, vector_stats};

use crate::views::*;

/// 诊断页的数与名；句子由前端按界面语言拼（展示文案不进后端）。
pub fn diagnostics(conn: &Connection, db_path: &Path) -> DiagnosticsPage {
    let vec = vec_available(conn);
    let vectors = vector_stats(conn).ok().map(|stats| VectorDiagnostics {
        chunks_embedded: stats.chunks_embedded,
        chunks_total: stats.chunks_total,
        chunks_pending: stats.chunks_pending,
        articles_pending: stats.articles_pending,
        interests_embedded: stats.interests_embedded,
        interests_total: stats.interests_total,
        memories_embedded: stats.memories_embedded,
        dims: stats
            .dim_histogram
            .iter()
            .map(|(dims, count)| DiagnosticCount {
                label: dims.to_string(),
                count: *count,
            })
            .collect(),
        models: stats
            .model_histogram
            .into_iter()
            .map(|(model, count)| DiagnosticCount {
                label: model,
                count,
            })
            .collect(),
        recent_errors: stats
            .recent_errors
            .into_iter()
            .map(|e| DiagnosticChunkError {
                chunk_id: e.chunk_id,
                attempts: e.attempts,
                error: e.last_error,
            })
            .collect(),
    });
    let day = ai_store::local_day();
    let usage = ai_store::usage_on(conn, &day).unwrap_or_default();
    let pi = pi_store::pi_usage_on(conn, &day).unwrap_or_default();
    DiagnosticsPage {
        scheduler: crate::scheduler::Scheduler::diagnostics_for(db_path),
        db: DbDiagnostics {
            path: db_path.display().to_string(),
            size_bytes: std::fs::metadata(db_path)
                .map(|m| i64::try_from(m.len()).unwrap_or(i64::MAX))
                .unwrap_or(0),
            user_version: conn
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap_or(0),
            journal_mode: conn
                .pragma_query_value(None, "journal_mode", |row| row.get(0))
                .unwrap_or_else(|_| "unknown".into()),
            fts_rows: rag::fts_row_count(conn).unwrap_or(0),
        },
        vec: VecDiagnostics {
            available: vec.available,
            version: vec.version,
            reason: vec.reason,
        },
        vectors,
        ai: AiUsageDiagnostics {
            job_tokens: usage.tokens,
            budget_tokens: ai_store::daily_budget_tokens(conn).unwrap_or(0),
            cache_rows: rag::cache_row_count(conn).unwrap_or(0),
            chat_input_tokens: pi.input_tokens,
            chat_output_tokens: pi.output_tokens,
            chat_cost: pi.cost,
        },
        log: crate::log::current().and_then(crate::log::Handle::status_view),
    }
}
