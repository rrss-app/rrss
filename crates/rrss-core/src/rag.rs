//! 向量存储、嵌入队列、混合检索。不改列表隐藏。日志 target：`rrss_core::rag`。

use rusqlite::{Connection, OptionalExtension, params};

use crate::ai_store::truncate_chars;
use crate::articles::hide_threshold;
use crate::chunk::{self, ChunkDraft};
use crate::error::Result;
use crate::search::{SearchHit, search_articles_for_retrieval};

const REQUIRED_TIER: &str = "CASE WHEN a.read = 1 OR a.starred = 1 THEN 2 ELSE 1 END";
const TEXT_INPUTS: &str = "SELECT 'interest' kind,id row_id,label text,embedding,space_id
    FROM interests WHERE enabled=1
    UNION ALL SELECT 'memory',id,content,embedding,space_id FROM memories
    WHERE COALESCE(source,'')!='pending' AND strength>0";

// 待嵌入、进度、暂存及切换共用同一份输入；历史已有向量的正文也参加重建。
pub(crate) fn rebuild_inputs() -> String {
    format!(
        "SELECT 'chunk' kind,c.id row_id,c.text,c.embedding,c.space_id FROM chunks c
        JOIN articles a ON a.id=c.article_id WHERE a.expires_at IS NULL AND a.pruned_at IS NULL
        AND (c.embedding IS NOT NULL OR (COALESCE(c.attempts,0)<3
            AND (c.ordinal=0 OR a.read=1 OR a.starred=1)))
        UNION ALL {TEXT_INPUTS}"
    )
}

const RRF_K: f64 = 60.0;
const RAG_SNIPPET_CHARS: usize = 150;
const RAG_CONTEXT_CHARS: usize = 1200;
const DIGEST_NEAR_DUP: f32 = 0.15;

#[derive(Debug, Clone)]
pub struct DueEmbedArticle {
    pub id: i64,
    pub title: String,
    pub summary: Option<String>,
    pub blocks_json: Option<String>,
    pub content_html: Option<String>,
    /// 本轮所需同步层；空间重建时包含历史上已同步的层。
    pub tier: i64,
}

impl DueEmbedArticle {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get(0)?,
            title: row.get(1)?,
            summary: row.get(2)?,
            blocks_json: row.get(3)?,
            content_html: row.get(4)?,
            tier: row.get(5)?,
        })
    }

    pub fn drafts(&self, tier: i64) -> Vec<ChunkDraft> {
        chunk::drafts_for_article(
            &self.title,
            self.summary.as_deref(),
            self.blocks_json.as_deref().filter(|_| tier >= 2),
            self.content_html.as_deref().filter(|_| tier >= 2),
        )
    }
}

#[derive(Debug, Clone)]
pub struct PendingChunk {
    pub id: i64,
    pub text: String,
}

/// 已读 / 收藏 / 关注优先；未读中性排最后，避免新订阅堵塞队列。
/// 缺向量且已失败满 3 次的分块不再让文章占住队首。
fn due_article_condition() -> String {
    format!(
        "a.expires_at IS NULL AND a.score > ?1 AND a.pruned_at IS NULL
           AND (
             a.embed_tier < {REQUIRED_TIER}
             OR EXISTS (
               SELECT 1 FROM chunks c
               WHERE c.article_id = a.id
                 AND (a.read = 1 OR a.starred = 1 OR c.ordinal = 0)
                 AND (c.embedding IS NULL OR c.space_id IS NULL OR c.space_id IS NOT ?2)
                 AND COALESCE(c.attempts, 0) < 3
             )
           ) "
    )
}

/// Existence-only counterpart of the candidate query, without loading bodies.
pub fn has_due_embed_articles(conn: &Connection) -> Result<bool> {
    let space = crate::vector_space::active(conn)?.map(|s| s.id);
    conn.query_row(
        &format!(
            "SELECT EXISTS(SELECT 1 FROM articles a WHERE {})",
            due_article_condition()
        ),
        params![hide_threshold(conn)?, space],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

pub fn due_embed_articles(conn: &Connection, limit: i64) -> Result<Vec<DueEmbedArticle>> {
    due_embed_articles_excluding(conn, limit, &[])
}

pub fn due_embed_articles_excluding(
    conn: &Connection,
    limit: i64,
    excluded: &[i64],
) -> Result<Vec<DueEmbedArticle>> {
    let excluded = serde_json::to_string(excluded).expect("embedding exclusions");
    let space_id = crate::vector_space::active(conn)?.map(|s| s.id);
    let threshold = hide_threshold(conn)?;
    let condition = due_article_condition();
    let mut stmt = conn.prepare(&format!(
        "SELECT a.id, a.title, a.summary, a.blocks_json, a.content_html, {REQUIRED_TIER}
         FROM articles a
         WHERE {condition}
           AND a.id NOT IN (SELECT value FROM json_each(?4))
         ORDER BY CASE WHEN a.starred = 1 THEN 0 WHEN a.read = 1 THEN 1
                       WHEN a.score > 0 THEN 2 ELSE 3 END,
                  COALESCE(a.published_at, 0) DESC, a.id DESC
         LIMIT ?3",
    ))?;
    let rows = stmt.query_map(
        params![threshold, space_id, limit, excluded],
        DueEmbedArticle::from_row,
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// 按本次层同步分块，未变的段保留向量；层 1 不触碰正文段。返回本层待嵌入的行。
pub fn sync_article_chunks(
    conn: &Connection,
    article: &DueEmbedArticle,
    tier: i64,
) -> Result<Vec<PendingChunk>> {
    if crate::articles::is_temporary(conn, article.id)? {
        return Ok(Vec::new());
    }
    let tier = tier.clamp(1, 2);
    let drafts = article.drafts(tier);
    for draft in &drafts {
        upsert_chunk(conn, article.id, draft)?;
    }
    if tier >= 2 {
        conn.execute(
            "DELETE FROM chunks WHERE article_id = ?1 AND (ordinal < 0 OR ordinal > ?2 OR (ordinal = 0 AND ?3))",
            params![article.id, drafts.last().map_or(-1, |d| d.ordinal),
                drafts.first().is_none_or(|d| d.ordinal != 0)],
        )?;
    }
    conn.execute(
        "UPDATE articles SET embed_tier = MAX(embed_tier, ?1) WHERE id = ?2",
        params![tier, article.id],
    )?;
    pending_chunks(conn, article.id, tier)
}

/// 重建包含历史已同步的正文层；状态预估也使用这份文章集合。
pub fn rebuild_articles(conn: &Connection) -> Result<Vec<DueEmbedArticle>> {
    conn.prepare(&format!(
        "SELECT a.id,a.title,a.summary,a.blocks_json,a.content_html,
         MAX(a.embed_tier, {REQUIRED_TIER}) FROM articles a WHERE a.pruned_at IS NULL"
    ))?
    .query_map([], DueEmbedArticle::from_row)?
    .collect::<rusqlite::Result<_>>()
    .map_err(Into::into)
}

fn upsert_chunk(conn: &Connection, article_id: i64, draft: &ChunkDraft) -> Result<()> {
    conn.execute(
        "INSERT INTO chunks (article_id, ordinal, kind, text, embedding, model, source_hash, attempts, last_error)
         VALUES (?1, ?2, ?3, ?4, NULL, NULL, ?5, 0, NULL)
         ON CONFLICT (article_id, ordinal) DO UPDATE SET
           kind = excluded.kind,
           text = excluded.text,
           source_hash = excluded.source_hash,
           embedding = CASE WHEN chunks.source_hash = excluded.source_hash THEN chunks.embedding ELSE NULL END,
           space_id = CASE WHEN chunks.source_hash = excluded.source_hash THEN chunks.space_id ELSE NULL END,
           model = CASE WHEN chunks.source_hash = excluded.source_hash THEN chunks.model ELSE NULL END,
           attempts = CASE WHEN chunks.source_hash = excluded.source_hash THEN chunks.attempts ELSE 0 END,
           last_error = CASE WHEN chunks.source_hash = excluded.source_hash THEN chunks.last_error ELSE NULL END",
        params![
            article_id,
            draft.ordinal,
            draft.kind,
            draft.text,
            draft.source_hash
        ],
    )?;
    Ok(())
}

fn pending_chunks(conn: &Connection, article_id: i64, tier: i64) -> Result<Vec<PendingChunk>> {
    let mut stmt = conn.prepare(
        "SELECT id, text FROM chunks
         WHERE article_id = ?1
           AND (?3 >= 2 OR ordinal = 0)
           AND (embedding IS NULL OR space_id IS NULL OR space_id IS NOT ?2)
           AND COALESCE(attempts, 0) < 3
         ORDER BY ordinal",
    )?;
    let rows = stmt.query_map(
        params![
            article_id,
            crate::vector_space::active(conn)?.map(|s| s.id),
            tier
        ],
        |row| {
            Ok(PendingChunk {
                id: row.get(0)?,
                text: row.get(1)?,
            })
        },
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn set_chunk_embedding(
    conn: &Connection,
    chunk_id: i64,
    model: &str,
    vector: &[f32],
) -> Result<()> {
    let Some(space) = crate::vector_space::write_space(conn)? else {
        return Ok(());
    };
    if space.model != model {
        tracing::warn!(
            error_kind = "vector_space_mismatch",
            "chunk model differs from active vector space; stopping vector write"
        );
        return Ok(());
    }
    if crate::vector_space::stage(conn, &space, "chunk", chunk_id, None, vector)? {
        return Ok(());
    }
    let blob = f32s_to_le(vector);
    conn.execute(
        "UPDATE chunks SET embedding = ?1, model = ?2, attempts = 0, last_error = NULL, space_id = ?4 WHERE id = ?3",
        params![blob, model, chunk_id, space.id],
    )?;
    Ok(())
}

pub fn record_chunk_error(conn: &Connection, chunk_id: i64, error: &str) -> Result<()> {
    let msg = crate::ai_store::truncate_chars(error, 400);
    conn.execute(
        "UPDATE chunks SET attempts = COALESCE(attempts, 0) + 1, last_error = ?1 WHERE id = ?2",
        params![msg, chunk_id],
    )?;
    Ok(())
}

pub fn reset_failed_chunks(conn: &Connection) -> Result<u32> {
    let n = conn.execute(
        "UPDATE chunks SET attempts = 0, last_error = NULL
         WHERE embedding IS NULL AND COALESCE(attempts, 0) >= 3",
        [],
    )?;
    Ok(n as u32)
}

#[derive(Debug, Clone)]
pub struct VecStatus {
    pub available: bool,
    pub version: Option<String>,
    pub reason: Option<String>,
}

pub fn vec_available(conn: &Connection) -> VecStatus {
    match conn.query_row("SELECT vec_version()", [], |row| row.get::<_, String>(0)) {
        Ok(version) => VecStatus {
            available: true,
            version: Some(version),
            reason: None,
        },
        Err(err) => VecStatus {
            available: false,
            version: None,
            reason: Some(err.to_string()),
        },
    }
}

#[derive(Debug, Clone)]
pub struct ChunkError {
    pub chunk_id: i64,
    pub article_id: i64,
    pub last_error: String,
    pub attempts: i64,
}

#[derive(Debug, Clone)]
pub struct VectorStats {
    pub chunks_total: i64,
    pub chunks_embedded: i64,
    pub chunks_pending: i64,
    pub articles_pending: i64,
    pub dim_histogram: Vec<(i64, i64)>,
    pub model_histogram: Vec<(String, i64)>,
    pub interests_embedded: i64,
    pub interests_total: i64,
    pub memories_embedded: i64,
    pub recent_errors: Vec<ChunkError>,
}

/// 焦点顶栏用：单次 COUNT，避免每次列表刷新跑完整 `vector_stats`。
pub fn pending_embed_article_count(conn: &Connection) -> Result<i64> {
    conn.query_row(
        "SELECT COUNT(DISTINCT article_id) FROM chunks
         WHERE embedding IS NULL AND COALESCE(attempts, 0) < 3",
        [],
        |r| r.get(0),
    )
    .map_err(Into::into)
}

pub fn vector_stats(conn: &Connection) -> Result<VectorStats> {
    let count = |sql| conn.query_row(sql, [], |r| r.get::<_, i64>(0));
    let mut dim_stmt = conn.prepare(
        "SELECT length(embedding) / 4, COUNT(*) FROM chunks
         WHERE embedding IS NOT NULL AND length(embedding) > 0
         GROUP BY length(embedding) / 4
         ORDER BY 1",
    )?;
    let dim_histogram = dim_stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut model_stmt = conn.prepare(
        "SELECT COALESCE(model, ''), COUNT(*) FROM chunks
         WHERE embedding IS NOT NULL
         GROUP BY COALESCE(model, '')
         ORDER BY 2 DESC",
    )?;
    let model_histogram = model_stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut err_stmt = conn.prepare(
        "SELECT id, article_id, COALESCE(last_error, ''), COALESCE(attempts, 0)
         FROM chunks
         WHERE last_error IS NOT NULL AND last_error != ''
         ORDER BY attempts DESC, id DESC
         LIMIT 5",
    )?;
    let recent_errors = err_stmt
        .query_map([], |row| {
            Ok(ChunkError {
                chunk_id: row.get(0)?,
                article_id: row.get(1)?,
                last_error: row.get(2)?,
                attempts: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(VectorStats {
        chunks_total: count("SELECT COUNT(*) FROM chunks")?,
        chunks_embedded: count(
            "SELECT COUNT(*) FROM chunks WHERE embedding IS NOT NULL AND length(embedding) > 0",
        )?,
        chunks_pending: count(
            "SELECT COUNT(*) FROM chunks WHERE embedding IS NULL AND COALESCE(attempts, 0) < 3",
        )?,
        articles_pending: pending_embed_article_count(conn)?,
        dim_histogram,
        model_histogram,
        interests_embedded: count(
            "SELECT COUNT(*) FROM interests WHERE embedding IS NOT NULL AND length(embedding) > 0",
        )?,
        interests_total: count("SELECT COUNT(*) FROM interests")?,
        memories_embedded: count(
            "SELECT COUNT(*) FROM memories WHERE embedding IS NOT NULL AND length(embedding) > 0",
        )?,
        recent_errors,
    })
}

pub fn fts_row_count(conn: &Connection) -> Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM articles_fts", [], |r| r.get(0))
        .map_err(Into::into)
}

pub fn cache_row_count(conn: &Connection) -> Result<i64> {
    conn.query_row("SELECT COUNT(*) FROM ai_cache", [], |r| r.get(0))
        .map_err(Into::into)
}

pub fn set_interest_embedding(conn: &Connection, id: i64, vector: &[f32]) -> Result<()> {
    TextEmbedding::Interest.set(conn, id, vector)
}

pub fn set_memory_embedding(conn: &Connection, id: i64, vector: &[f32]) -> Result<()> {
    TextEmbedding::Memory.set(conn, id, vector)
}

pub fn interest_has_embedding(conn: &Connection, id: i64) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM interests WHERE id = ?1 AND embedding IS NOT NULL AND length(embedding) > 0 AND space_id = ?2",
        params![id, crate::vector_space::active(conn)?.map(|s| s.id)],
        |row| row.get(0),
    )?;
    Ok(n > 0)
}

#[derive(Debug, Clone, Copy)]
pub enum TextEmbedding {
    Interest,
    Memory,
}

impl TextEmbedding {
    fn names(self) -> (&'static str, &'static str) {
        match self {
            Self::Interest => ("interest", "interests"),
            Self::Memory => ("memory", "memories"),
        }
    }

    pub fn pending(self, conn: &Connection) -> Result<Vec<(i64, String)>> {
        self.pending_batch(conn, usize::MAX, |_, _| true)
    }

    pub fn pending_batch(
        self,
        conn: &Connection,
        limit: usize,
        mut eligible: impl FnMut(i64, &str) -> bool,
    ) -> Result<Vec<(i64, String)>> {
        let mut statement = conn.prepare(&format!(
            "SELECT row_id,text FROM ({TEXT_INPUTS}) WHERE kind=?1
             AND (embedding IS NULL OR length(embedding)=0 OR space_id IS NOT ?2) ORDER BY row_id"
        ))?;
        let mut rows = statement.query(params![
            self.names().0,
            crate::vector_space::active(conn)?.map(|s| s.id)
        ])?;
        let mut out = Vec::new();
        while out.len() < limit {
            let Some(row) = rows.next()? else {
                break;
            };
            let id = row.get(0)?;
            let text: String = row.get(1)?;
            if eligible(id, &text) {
                out.push((id, text));
            }
        }
        Ok(out)
    }

    pub fn set(self, conn: &Connection, id: i64, vector: &[f32]) -> Result<()> {
        let Some(space) = crate::vector_space::write_space(conn)? else {
            return Ok(());
        };
        let (kind, table) = self.names();
        if crate::vector_space::stage(conn, &space, kind, id, None, vector)? {
            return Ok(());
        }
        if conn.execute(
            &format!("UPDATE {table} SET embedding=?1,space_id=?3 WHERE id=?2"),
            params![f32s_to_le(vector), id, space.id],
        )? == 0
        {
            return Err(crate::Error::MemoryNotFound);
        }
        Ok(())
    }
}

pub fn text_input_chars(conn: &Connection) -> Result<i64> {
    conn.query_row(
        &format!("SELECT COALESCE(SUM(length(text)),0) FROM ({TEXT_INPUTS})"),
        [],
        |r| r.get(0),
    )
    .map_err(Into::into)
}

pub(crate) fn knn_memories(
    conn: &Connection,
    query: &[f32],
    limit: i64,
) -> Result<Vec<crate::memories::Memory>> {
    let Some(space) = crate::vector_space::active(conn)? else {
        return Ok(Vec::new());
    };
    let blob = f32s_to_le(query);
    let nbytes = blob.len() as i64;
    let mut stmt = conn.prepare(
        "SELECT id, kind, content, scope, pinned, strength, source, last_used_at, created_at,
                CASE WHEN embedding IS NOT NULL AND length(embedding) > 0 THEN 1 ELSE 0 END
         FROM memories
         WHERE embedding IS NOT NULL AND length(embedding) = ?1 AND space_id = ?4
           AND COALESCE(source, '') != 'pending' AND strength > 0
         ORDER BY vec_distance_cosine(embedding, ?2) ASC
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(
        params![nbytes, blob, limit, space.id],
        crate::memories::map_memory,
    )?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn knn_articles(conn: &Connection, query: &[f32], limit: i64) -> Result<Vec<SearchHit>> {
    if query.is_empty() || limit <= 0 {
        return Ok(Vec::new());
    }
    let Some(space) = crate::vector_space::active(conn)? else {
        return Ok(Vec::new());
    };
    let threshold = hide_threshold(conn)?;
    let blob = f32s_to_le(query);
    let nbytes = blob.len() as i64;
    let mut stmt = conn.prepare(
        "SELECT a.id, a.title, COALESCE(f.title, f.url), a.published_at, a.score,
                MIN(vec_distance_cosine(c.embedding, ?1)) AS dist,
                c.text, a.read, a.starred, a.feed_id
         FROM chunks c
         JOIN articles a ON a.id = c.article_id
         JOIN feeds f ON f.id = a.feed_id
         WHERE a.expires_at IS NULL AND c.embedding IS NOT NULL
           AND length(c.embedding) = ?2
           AND c.space_id = ?5
           AND a.score > ?3
         GROUP BY a.id
         ORDER BY dist ASC
         LIMIT ?4",
    )?;
    let rows = stmt.query_map(params![blob, nbytes, threshold, limit, space.id], |row| {
        let chunk_text: String = row.get(6)?;
        Ok(SearchHit {
            id: row.get(0)?,
            title: row.get(1)?,
            feed_id: row.get(9)?,
            feed_title: row.get(2)?,
            published_at: row.get(3)?,
            score: row.get(4)?,
            snippet: truncate_chars(&chunk_text, RAG_SNIPPET_CHARS),
            read: row.get(7)?,
            starred: row.get(8)?,
            title_match: false,
            body_match: false,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

#[derive(Debug, Clone)]
pub struct RelatedArticle {
    pub article_id: i64,
    pub feed_id: i64,
    pub feed_title: String,
    pub title: String,
    pub published_at: Option<i64>,
    pub summary: String,
    pub summary_json: Option<String>,
    pub read: bool,
    pub similarity: f32,
}

/// 只比较当前空间的标题摘要向量；先在 SQLite 取近邻，再按事件簇去重。
pub fn related_articles(
    conn: &Connection,
    article_id: i64,
    limit: i64,
) -> Result<Vec<RelatedArticle>> {
    let limit = limit.clamp(1, 10);
    let Some(space) = crate::vector_space::active(conn)? else {
        return Ok(Vec::new());
    };
    let query: Option<Vec<u8>> = conn
        .query_row(
            "SELECT embedding FROM chunks WHERE article_id=?1 AND space_id=?2
             AND kind='title_summary' AND embedding IS NOT NULL
             ORDER BY ordinal LIMIT 1",
            params![article_id, space.id],
            |row| row.get(0),
        )
        .optional()?;
    let Some(query) = query.filter(|blob| !blob.is_empty()) else {
        return Ok(Vec::new());
    };
    let threshold = crate::cluster::cached_similarity(conn, space.id)?;
    let mut stmt = conn.prepare(
        "SELECT a.id, a.feed_id, COALESCE(f.title,f.url), a.title, a.published_at,
                COALESCE(a.summary,''), a.summary_json, a.read, a.cluster_id,
                MIN(vec_distance_cosine(c.embedding,?1)) AS distance
         FROM chunks c JOIN articles a ON a.id=c.article_id
         JOIN feeds f ON f.id=a.feed_id
         WHERE c.space_id=?2 AND c.kind='title_summary' AND c.embedding IS NOT NULL
           AND length(c.embedding)=?3 AND a.id!=?4 AND a.expires_at IS NULL
           AND a.score>?5
           AND (a.cluster_id IS NULL OR a.cluster_id IS NOT
                (SELECT cluster_id FROM articles WHERE id=?4))
         GROUP BY a.id HAVING 1-distance>=?6
         ORDER BY distance ASC, a.id ASC LIMIT ?7",
    )?;
    let rows = stmt.query_map(
        params![
            query,
            space.id,
            query.len(),
            article_id,
            hide_threshold(conn)?,
            threshold,
            limit * 4
        ],
        |row| {
            Ok((
                RelatedArticle {
                    article_id: row.get(0)?,
                    feed_id: row.get(1)?,
                    feed_title: row.get(2)?,
                    title: row.get(3)?,
                    published_at: row.get(4)?,
                    summary: row.get(5)?,
                    summary_json: row.get(6)?,
                    read: row.get(7)?,
                    similarity: 1.0 - row.get::<_, f32>(9)?,
                },
                row.get::<_, Option<i64>>(8)?,
            ))
        },
    )?;
    let mut clusters = std::collections::HashSet::new();
    let mut result = Vec::new();
    for row in rows {
        let (article, cluster_id) = row?;
        if cluster_id.is_none_or(|id| clusters.insert(id)) {
            result.push(article);
            if result.len() == limit as usize {
                break;
            }
        }
    }
    Ok(result)
}

pub fn hybrid_search(
    conn: &Connection,
    query: &str,
    query_vec: Option<&[f32]>,
    limit: i64,
) -> Result<Vec<SearchHit>> {
    let fetch = (limit * 2).max(limit);
    let fts = search_articles_for_retrieval(conn, query, fetch)?;
    let knn = match query_vec {
        Some(v) if !v.is_empty() => knn_articles(conn, v, fetch).unwrap_or_else(|err| {
            tracing::warn!(error = %err, "vector knn failed; FTS only");
            Vec::new()
        }),
        _ => Vec::new(),
    };
    Ok(rrf_merge(&fts, &knn, limit, |h| h.id))
}

pub fn format_rag_context(hits: &[SearchHit]) -> String {
    if hits.is_empty() {
        return String::new();
    }
    let mut out =
        String::from("可能相关的本地文章（按问题自动检索，只有标题和摘要，不一定相关）：\n");
    for hit in hits {
        let line = format!(
            "- id={} title={} source={}\n  {}\n",
            hit.id, hit.title, hit.feed_title, hit.snippet
        );
        if out.len() + line.len() > RAG_CONTEXT_CHARS {
            break;
        }
        out.push_str(&line);
    }
    out.push_str("用得上再用；要看全文用 get_article。");
    out
}

#[derive(Debug, Clone)]
pub struct DigestCandidate {
    pub article_id: i64,
    pub title: String,
    pub summary: String,
    pub cluster_id: Option<i64>,
}

/// 有标题向量时丢掉近重复，仍只输出标题 + 一句话。同 cluster 的近重复保留。
pub fn digest_representatives(
    conn: &Connection,
    items: Vec<DigestCandidate>,
    keep: usize,
) -> Result<Vec<DigestCandidate>> {
    let mut kept: Vec<(DigestCandidate, Option<Vec<f32>>)> = Vec::new();
    for item in items {
        if kept.len() >= keep {
            break;
        }
        let vec = title_embedding(conn, item.article_id)?;
        let too_close = vec.as_ref().is_some_and(|v| {
            kept.iter().any(|(other_item, other)| {
                if item.cluster_id.is_some() && item.cluster_id == other_item.cluster_id {
                    return false;
                }
                other
                    .as_ref()
                    .is_some_and(|o| cosine_distance(v, o) < DIGEST_NEAR_DUP)
            })
        });
        if too_close {
            continue;
        }
        kept.push((item, vec));
    }
    Ok(kept.into_iter().map(|(item, _)| item).collect())
}

pub(crate) struct SelectedArticle {
    pub article_id: i64,
    pub blob: Vec<u8>,
    pub vector: Option<Vec<f32>>,
}

/// 标题向量优先，否则首个正文向量；损坏的标题向量也不退回正文。
const SELECTED_CHUNK_SQL: &str = "(
    SELECT c2.id FROM chunks c2
    WHERE c2.article_id = a.id AND c2.embedding IS NOT NULL AND c2.space_id = ?2
    ORDER BY CASE WHEN c2.kind = 'title_summary' THEN 0 ELSE 1 END, c2.ordinal
    LIMIT 1
)";

pub(crate) fn load_selected_article_vectors(
    conn: &Connection,
    threshold: i64,
) -> Result<Vec<SelectedArticle>> {
    let sql = format!(
        "SELECT a.id, c.embedding
         FROM articles a
         JOIN chunks c ON c.id = {SELECTED_CHUNK_SQL}
         WHERE a.expires_at IS NULL AND a.score > ?1
         ORDER BY a.id",
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        params![threshold, crate::vector_space::active(conn)?.map(|s| s.id)],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
    )?;
    let mut out = Vec::new();
    for row in rows {
        let (article_id, blob) = row?;
        let vector = le_to_f32s(&blob);
        out.push(SelectedArticle {
            article_id,
            blob,
            vector,
        });
    }
    Ok(out)
}

/// 文章的代表向量（标题优先，否则首个正文段）；兴趣加权与离线回放也用它。
pub fn title_embedding(conn: &Connection, article_id: i64) -> Result<Option<Vec<f32>>> {
    let blob: Option<Vec<u8>> = conn
        .prepare_cached(&format!(
            "SELECT c.embedding FROM articles a JOIN chunks c ON c.id = {SELECTED_CHUNK_SQL} WHERE a.id = ?1"
        ))?
        .query_row(
            params![article_id, crate::vector_space::active(conn)?.map(|s| s.id)],
            |r| r.get(0),
        )
        .optional()?;
    Ok(blob.as_deref().and_then(le_to_f32s))
}

/// 严格标题向量：按 ordinal 取首个合法向量，不使用正文回退。
pub(crate) fn normalized_title_vectors(
    conn: &Connection,
    wanted: Option<&[i64]>,
) -> Result<std::collections::BTreeMap<i64, Vec<f32>>> {
    use rusqlite::params_from_iter;
    use std::collections::BTreeMap;
    let mut vectors = BTreeMap::new();
    if let Some(space) = crate::vector_space::active(conn)? {
        let sql = "SELECT article_id,ordinal,embedding FROM chunks
                WHERE article_id IN (SELECT id FROM articles WHERE expires_at IS NULL) AND space_id=?1 AND kind='title_summary' AND embedding IS NOT NULL";
        // 每批最多 401 个参数，兼容 SQLite 的 999 参数下限；整表只查一次。
        let batches = wanted
            .map(|ids| ids.chunks(400).collect::<Vec<_>>())
            .unwrap_or_else(|| vec![&[]]);
        for batch in batches {
            let query = if wanted.is_some() {
                // 只拼接占位符；空间与文章 id 始终参数绑定。
                let placeholders = std::iter::repeat_n("?", batch.len())
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{sql} AND article_id IN ({placeholders})")
            } else {
                sql.to_owned()
            };
            let mut stmt = conn.prepare(&query)?;
            let bind = std::iter::once(space.id).chain(batch.iter().copied());
            for row in stmt.query_map(params_from_iter(bind), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    le_to_f32s(r.get_ref(2)?.as_blob()?),
                ))
            })? {
                let (id, ordinal, vector) = row?;
                if let Some(vector) = vector
                    .filter(|v| v.len() as i64 == space.dims)
                    .and_then(crate::rank::affinity::normalized_owned)
                    && vectors.get(&id).is_none_or(|(old, _)| ordinal < *old)
                {
                    vectors.insert(id, (ordinal, vector));
                }
            }
        }
    }
    Ok(vectors.into_iter().map(|(id, (_, v))| (id, v)).collect())
}

pub(crate) fn rrf_merge<T: Clone>(
    first: &[T],
    second: &[T],
    limit: i64,
    id: impl Fn(&T) -> i64,
) -> Vec<T> {
    use std::collections::HashMap;
    let mut scores: HashMap<i64, f64> = HashMap::new();
    let mut by_id = HashMap::new();
    for hits in [first, second] {
        for (rank, hit) in hits.iter().enumerate() {
            let id = id(hit);
            *scores.entry(id).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
            by_id.entry(id).or_insert_with(|| hit.clone());
        }
    }
    let mut ids: Vec<_> = scores.keys().copied().collect();
    ids.sort_by(|a, b| {
        scores[b]
            .partial_cmp(&scores[a])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.cmp(b))
    });
    ids.into_iter()
        .take(limit.max(0) as usize)
        .filter_map(|id| by_id.remove(&id))
        .collect()
}

pub fn f32s_to_le(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

pub fn le_to_f32s(bytes: &[u8]) -> Option<Vec<f32>> {
    if !bytes.len().is_multiple_of(4) || bytes.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4);
    for chunk in bytes.chunks_exact(4) {
        out.push(f32::from_le_bytes(chunk.try_into().ok()?));
    }
    Some(out)
}

pub fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 2.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 {
        2.0
    } else {
        1.0 - (dot / denom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{PooledConnection, open_at};
    use crate::feeds::{SubscribeRequest, subscribe};
    use crate::memories;
    use crate::parse::ParsedEntry;
    use crate::rules::{Dimension, NewRule, Polarity, insert_rule};
    use tempfile::tempdir;

    fn open_temp() -> (tempfile::TempDir, PooledConnection) {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        crate::vector_space::seed_active(&conn, "m", 3);
        (dir, conn)
    }

    fn seed_feed(conn: &Connection) -> i64 {
        subscribe(
            conn,
            SubscribeRequest {
                url: "https://news.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap()
    }

    fn entry(guid: &str, title: &str, html: &str, published: i64) -> ParsedEntry {
        ParsedEntry {
            guid: guid.into(),
            canonical_url: Some(format!("https://news.example.com/{guid}")),
            title: title.into(),
            author: None,
            published_at: Some(published),
            content_html: Some(html.into()),
            hash: guid.into(),
            tags: vec![],
        }
    }

    #[test]
    fn sqlite_vec_cosine_zero_for_same_vector() {
        let (_dir, conn) = open_temp();
        let blob = f32s_to_le(&[0.2, 0.4, 0.8]);
        let d: f64 = conn
            .query_row(
                "SELECT vec_distance_cosine(?1, ?2)",
                params![blob.clone(), blob],
                |row| row.get(0),
            )
            .unwrap();
        assert!(d.abs() < 1e-5, "distance {d}");
    }

    #[test]
    fn unread_does_not_block_starred_and_read() {
        let (_dir, conn) = open_temp();
        let feed = seed_feed(&conn);
        insert_rule(
            &conn,
            NewRule::manual_global(Polarity::Like, Dimension::TitleKw, "focus"),
        )
        .unwrap();
        crate::ingest::upsert_entries(
            &conn,
            feed,
            &[
                entry("u", "unread neutral", "<p>u</p>", 30),
                entry("s", "starred item", "<p>s</p>", 10),
                entry("r", "read item", "<p>r</p>", 20),
                entry("f", "focus piece", "<p>f</p>", 5),
            ],
            1,
        )
        .unwrap();
        let starred: i64 = conn
            .query_row(
                "SELECT id FROM articles WHERE title = 'starred item'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let read: i64 = conn
            .query_row(
                "SELECT id FROM articles WHERE title = 'read item'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        crate::articles::set_starred(&conn, starred, true).unwrap();
        crate::articles::set_read(&conn, read, true).unwrap();

        let due = due_embed_articles(&conn, 10).unwrap();
        let titles: Vec<_> = due.iter().map(|d| d.title.as_str()).collect();
        assert_eq!(due[0].title, "starred item");
        assert_eq!(due[0].tier, 2);
        assert_eq!(due[1].title, "read item");
        assert_eq!(due[1].tier, 2);
        assert!(titles.contains(&"focus piece"));
        let unread_pos = titles.iter().position(|t| *t == "unread neutral").unwrap();
        let focus_pos = titles.iter().position(|t| *t == "focus piece").unwrap();
        assert!(focus_pos < unread_pos);
    }

    #[test]
    fn exhausted_chunk_does_not_starve_an_article_without_chunks() {
        let (_dir, conn) = open_temp();
        let feed = seed_feed(&conn);
        crate::ingest::upsert_entries(
            &conn,
            feed,
            &[
                entry("fresh", "fresh article", "<p>n</p>", 1),
                entry("stuck", "stuck article", "<p>s</p>", 2),
            ],
            1,
        )
        .unwrap();
        let stuck: i64 = conn
            .query_row(
                "SELECT id FROM articles WHERE title = 'stuck article'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let fresh: i64 = conn
            .query_row(
                "SELECT id FROM articles WHERE title = 'fresh article'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        crate::articles::set_starred(&conn, stuck, true).unwrap();
        conn.execute(
            "INSERT INTO chunks (article_id, ordinal, kind, text, embedding, attempts)
             VALUES (?1, 0, 'body', 'stuck', NULL, 3)",
            [stuck],
        )
        .unwrap();
        conn.execute("UPDATE articles SET embed_tier = 2 WHERE id = ?1", [stuck])
            .unwrap();
        let due = due_embed_articles(&conn, 1).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].id, fresh);
    }

    fn tier_fixture(conn: &Connection, read: bool, title: &str, html: &str) -> DueEmbedArticle {
        let feed = seed_feed(conn);
        crate::ingest::upsert_entries(conn, feed, &[entry("tier", title, html, 1)], 1).unwrap();
        let id = conn
            .query_row("SELECT id FROM articles WHERE guid = 'tier'", [], |r| {
                r.get(0)
            })
            .unwrap();
        crate::articles::set_read(conn, id, read).unwrap();
        due_embed_articles(conn, 1).unwrap().remove(0)
    }

    fn embed_tier(conn: &Connection, id: i64) -> i64 {
        conn.query_row("SELECT embed_tier FROM articles WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap()
    }

    fn embed_chunks(conn: &Connection, chunks: &[PendingChunk]) {
        for chunk in chunks {
            set_chunk_embedding(conn, chunk.id, "m", &[1.0, 0.0, 0.0]).unwrap();
        }
    }

    #[test]
    fn empty_title_and_summary_do_not_fall_back_to_body() {
        let (_dir, conn) = open_temp();
        let article = tier_fixture(&conn, false, " \n ", "<p>Body stays unembedded</p>");
        assert_eq!(article.tier, 1);
        assert!(
            sync_article_chunks(&conn, &article, article.tier)
                .unwrap()
                .is_empty()
        );
        assert_eq!(embed_tier(&conn, article.id), 1);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(due_embed_articles(&conn, 1).unwrap().is_empty());
    }

    #[test]
    fn read_article_without_body_is_synchronized_once() {
        let (_dir, conn) = open_temp();
        let article = tier_fixture(&conn, true, "Title only", "");
        let pending = sync_article_chunks(&conn, &article, article.tier).unwrap();
        assert_eq!(pending.len(), 1);
        embed_chunks(&conn, &pending);
        assert_eq!(embed_tier(&conn, article.id), 2);
        assert!(due_embed_articles(&conn, 1).unwrap().is_empty());
    }

    #[test]
    fn content_writes_reset_tier_and_only_changed_chunks_need_embedding() {
        let (_dir, conn) = open_temp();
        let article = tier_fixture(&conn, true, "Title", "<p>Original body</p>");
        let original = sync_article_chunks(&conn, &article, article.tier).unwrap();
        assert_eq!(original.len(), 2);
        embed_chunks(&conn, &original);

        crate::articles::set_summary(&conn, article.id, "New summary").unwrap();
        assert_eq!(embed_tier(&conn, article.id), 0);
        let article = due_embed_articles(&conn, 1).unwrap().remove(0);
        let pending = sync_article_chunks(&conn, &article, article.tier).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, original[0].id);
        assert_eq!(pending[0].text, "Title\nNew summary");
        embed_chunks(&conn, &pending);

        let json =
            rrss_render::document_to_json(&rrss_render::markdown_to_document("Revised body"))
                .unwrap();
        crate::articles::set_fulltext(&conn, article.id, "Revised body", &json).unwrap();
        assert_eq!(embed_tier(&conn, article.id), 0);
        let article = due_embed_articles(&conn, 1).unwrap().remove(0);
        let pending = sync_article_chunks(&conn, &article, article.tier).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, original[1].id);
        assert_eq!(pending[0].text, "Revised body");
        embed_chunks(&conn, &pending);

        crate::articles::set_blocks_json(&conn, article.id, &json).unwrap();
        assert_eq!(embed_tier(&conn, article.id), 0);
        let article = due_embed_articles(&conn, 1).unwrap().remove(0);
        assert!(
            sync_article_chunks(&conn, &article, article.tier)
                .unwrap()
                .is_empty()
        );
        assert_eq!(embed_tier(&conn, article.id), 2);
        assert!(due_embed_articles(&conn, 1).unwrap().is_empty());
    }

    #[test]
    fn hybrid_rrf_prefers_agreement() {
        let fts = vec![hit(1, "fts-only"), hit(2, "both")];
        let knn = vec![hit(3, "knn-only"), hit(2, "both")];
        let merged = rrf_merge(&fts, &knn, 3, |h| h.id);
        assert_eq!(merged[0].id, 2);
        assert_eq!(merged.len(), 3);
    }

    #[test]
    fn unconfirmed_memory_does_not_write_interest_vector() {
        let (_dir, conn) = open_temp();
        let id = memories::insert_pending_fact(&conn, "I like Rust async").unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM interests", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
        // 转换事务由 rrss-app 验证；这里仅构造已确认的输入，验证向量存储。
        memories::confirm_memory(&conn, id).unwrap();
        let interest_id =
            crate::interests::insert_interest(&conn, "I like Rust async", "from_chat").unwrap();
        assert!(!interest_has_embedding(&conn, interest_id).unwrap());
        set_interest_embedding(&conn, interest_id, &[0.1, 0.2]).unwrap();
        assert!(interest_has_embedding(&conn, interest_id).unwrap());
    }

    #[test]
    fn digest_skips_near_duplicate_titles() {
        let (_dir, conn) = open_temp();
        let feed = seed_feed(&conn);
        crate::ingest::upsert_entries(
            &conn,
            feed,
            &[
                entry("a", "Library opens", "<p>a</p>", 1),
                entry("b", "Library opening day", "<p>b</p>", 2),
                entry("c", "Unrelated sports", "<p>c</p>", 3),
            ],
            1,
        )
        .unwrap();
        let ids: Vec<(i64, String)> = {
            let mut stmt = conn
                .prepare("SELECT id, title FROM articles ORDER BY id")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        let same = [1.0f32, 0.0, 0.0];
        let other = [0.0f32, 1.0, 0.0];
        for (id, title) in &ids {
            let draft = chunk::drafts_for_article(title, Some("one liner."), None, None);
            upsert_chunk(&conn, *id, &draft[0]).unwrap();
            let chunk_id: i64 = conn
                .query_row(
                    "SELECT id FROM chunks WHERE article_id = ?1 AND ordinal = 0",
                    [*id],
                    |r| r.get(0),
                )
                .unwrap();
            let vec = if title.contains("Library") {
                &same
            } else {
                &other
            };
            set_chunk_embedding(&conn, chunk_id, "m", vec).unwrap();
        }
        let items: Vec<_> = ids
            .iter()
            .map(|(id, title)| DigestCandidate {
                article_id: *id,
                title: title.clone(),
                summary: "one liner.".into(),
                cluster_id: None,
            })
            .collect();
        let kept = digest_representatives(&conn, items, 3).unwrap();
        let titles: Vec<_> = kept.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(titles.len(), 2);
        assert!(titles.iter().filter(|t| t.contains("Library")).count() == 1);
        assert!(titles.iter().any(|t| t.contains("sports")));
    }

    #[test]
    fn digest_keeps_near_duplicate_titles_in_same_cluster() {
        let (_dir, conn) = open_temp();
        let feed = seed_feed(&conn);
        crate::ingest::upsert_entries(
            &conn,
            feed,
            &[
                entry("a", "Library opens", "<p>a</p>", 1),
                entry("b", "Library opening day", "<p>b</p>", 2),
            ],
            1,
        )
        .unwrap();
        let ids: Vec<(i64, String)> = {
            let mut stmt = conn
                .prepare("SELECT id, title FROM articles ORDER BY id")
                .unwrap();
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        for (id, _) in &ids {
            conn.execute("UPDATE articles SET cluster_id = 99 WHERE id = ?1", [*id])
                .unwrap();
        }
        let same = [1.0f32, 0.0, 0.0];
        for (id, title) in &ids {
            let draft = chunk::drafts_for_article(title, Some("one liner."), None, None);
            upsert_chunk(&conn, *id, &draft[0]).unwrap();
            let chunk_id: i64 = conn
                .query_row(
                    "SELECT id FROM chunks WHERE article_id = ?1 AND ordinal = 0",
                    [*id],
                    |r| r.get(0),
                )
                .unwrap();
            set_chunk_embedding(&conn, chunk_id, "m", &same).unwrap();
        }
        let items: Vec<_> = ids
            .iter()
            .map(|(id, title)| DigestCandidate {
                article_id: *id,
                title: title.clone(),
                summary: "one liner.".into(),
                cluster_id: Some(99),
            })
            .collect();
        let kept = digest_representatives(&conn, items, 3).unwrap();
        assert_eq!(kept.len(), 2);
    }

    #[test]
    fn knn_finds_nearby_read_article() {
        let (_dir, conn) = open_temp();
        let feed = seed_feed(&conn);
        crate::ingest::upsert_entries(
            &conn,
            feed,
            &[
                entry("lib", "City library", "<p>books</p>", 1),
                entry("sport", "Match report", "<p>goal</p>", 2),
            ],
            1,
        )
        .unwrap();
        let lib: i64 = conn
            .query_row(
                "SELECT id FROM articles WHERE title = 'City library'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        crate::articles::set_read(&conn, lib, true).unwrap();
        for (title, vec) in [
            ("City library", [1.0f32, 0.0, 0.0]),
            ("Match report", [0.0f32, 1.0, 0.0]),
        ] {
            let id: i64 = conn
                .query_row("SELECT id FROM articles WHERE title = ?1", [title], |r| {
                    r.get(0)
                })
                .unwrap();
            let draft = chunk::drafts_for_article(title, Some(title), None, None);
            upsert_chunk(&conn, id, &draft[0]).unwrap();
            let chunk_id: i64 = conn
                .query_row(
                    "SELECT id FROM chunks WHERE article_id = ?1 AND ordinal = 0",
                    [id],
                    |r| r.get(0),
                )
                .unwrap();
            set_chunk_embedding(&conn, chunk_id, "m", &vec).unwrap();
        }
        let hits = knn_articles(&conn, &[0.95, 0.05, 0.0], 5).unwrap();
        assert_eq!(hits[0].title, "City library");
        let hybrid = hybrid_search(&conn, "library", Some(&[0.95, 0.05, 0.0]), 5).unwrap();
        assert_eq!(hybrid[0].title, "City library");
        let ctx = format_rag_context(&hybrid);
        assert!(ctx.contains("id="));
        assert!(!ctx.contains("<p>"));
    }

    mod related {
        use super::*;

        fn article(conn: &Connection, feed: i64, similarity: f32) -> i64 {
            conn.execute(
                "INSERT INTO articles(feed_id,title,created_at) VALUES(?1,'Related',1)",
                [feed],
            )
            .unwrap();
            let id = conn.last_insert_rowid();
            let vector = [similarity, (1.0 - similarity * similarity).sqrt(), 0.0];
            conn.execute(
                "INSERT INTO chunks(article_id,ordinal,kind,text,embedding,space_id)
                 VALUES(?1,0,'title_summary','Related',?2,?3)",
                params![
                    id,
                    f32s_to_le(&vector),
                    crate::vector_space::active(conn).unwrap().unwrap().id
                ],
            )
            .unwrap();
            id
        }

        fn ids(conn: &Connection, source: i64, limit: i64) -> Vec<i64> {
            related_articles(conn, source, limit)
                .unwrap()
                .into_iter()
                .map(|row| row.article_id)
                .collect()
        }

        #[test]
        fn no_active_space_returns_empty() {
            let (_dir, conn) = open_temp();
            let feed = seed_feed(&conn);
            let source = article(&conn, feed, 1.0);
            article(&conn, feed, 0.95);
            conn.execute("UPDATE vector_spaces SET state='retired'", [])
                .unwrap();
            assert!(ids(&conn, source, 10).is_empty());
        }

        #[test]
        fn source_requires_title_vector_in_current_space() {
            let (_dir, conn) = open_temp();
            let feed = seed_feed(&conn);
            let source = article(&conn, feed, 1.0);
            article(&conn, feed, 0.95);
            conn.execute(
                "UPDATE chunks SET kind='body' WHERE article_id=?1",
                [source],
            )
            .unwrap();
            assert!(ids(&conn, source, 10).is_empty());
            conn.execute("UPDATE vector_spaces SET state='retired'", [])
                .unwrap();
            crate::vector_space::seed_active(&conn, "new", 3);
            conn.execute("UPDATE chunks SET kind='title_summary'", [])
                .unwrap();
            article(&conn, feed, 0.95);
            assert!(ids(&conn, source, 10).is_empty());
        }

        #[test]
        fn excludes_source_same_cluster_hidden_expired_and_wrong_chunks() {
            let (_dir, conn) = open_temp();
            let feed = seed_feed(&conn);
            article(&conn, feed, 1.0);
            conn.execute("UPDATE vector_spaces SET state='retired'", [])
                .unwrap();
            crate::vector_space::seed_active(&conn, "new", 3);
            let source = article(&conn, feed, 1.0);
            let same_cluster = article(&conn, feed, 0.99);
            conn.execute(
                "UPDATE articles SET cluster_id=7 WHERE id IN (?1,?2)",
                params![source, same_cluster],
            )
            .unwrap();
            let hidden = article(&conn, feed, 0.98);
            crate::articles::set_hide_threshold(&conn, -20).unwrap();
            conn.execute("UPDATE articles SET score=-20 WHERE id=?1", [hidden])
                .unwrap();
            let expired = article(&conn, feed, 0.97);
            conn.execute(
                "UPDATE articles SET expires_at=9999999999999 WHERE id=?1",
                [expired],
            )
            .unwrap();
            let body_only = article(&conn, feed, 0.96);
            conn.execute(
                "UPDATE chunks SET kind='body' WHERE article_id=?1",
                [body_only],
            )
            .unwrap();
            let read = article(&conn, feed, 0.95);
            conn.execute("UPDATE articles SET read=1 WHERE id=?1", [read])
                .unwrap();
            let rows = related_articles(&conn, source, 10).unwrap();
            assert_eq!(
                rows.iter().map(|r| r.article_id).collect::<Vec<_>>(),
                [read]
            );
            assert!(rows[0].read);
        }

        #[test]
        fn keeps_closest_per_cluster_and_independent_unclustered_articles() {
            let (_dir, conn) = open_temp();
            let feed = seed_feed(&conn);
            let source = article(&conn, feed, 1.0);
            let first = article(&conn, feed, 0.99);
            let duplicate = article(&conn, feed, 0.98);
            let other_cluster = article(&conn, feed, 0.97);
            let unclustered = article(&conn, feed, 0.96);
            let other_unclustered = article(&conn, feed, 0.95);
            conn.execute(
                "UPDATE articles SET cluster_id=7 WHERE id IN (?1,?2)",
                params![first, duplicate],
            )
            .unwrap();
            conn.execute(
                "UPDATE articles SET cluster_id=8 WHERE id=?1",
                [other_cluster],
            )
            .unwrap();
            let rows = related_articles(&conn, source, 4).unwrap();
            assert_eq!(
                rows.iter().map(|r| r.article_id).collect::<Vec<_>>(),
                [first, other_cluster, unclustered, other_unclustered]
            );
            assert!(rows.windows(2).all(|r| r[0].similarity > r[1].similarity));
            assert!((rows[0].similarity - 0.99).abs() < 1e-6);
        }

        #[test]
        fn threshold_uses_only_valid_today_cache_without_writing() {
            let (_dir, conn) = open_temp();
            let feed = seed_feed(&conn);
            let source = article(&conn, feed, 1.0);
            let close = article(&conn, feed, 0.96);
            let related = article(&conn, feed, 0.86);
            article(&conn, feed, 0.84);
            let space = crate::vector_space::active(&conn).unwrap().unwrap().id;
            let day = crate::feeds::now_ms() / 86_400_000;
            let key = format!("{}:{space}", crate::cluster::CALIBRATION_KEY_PREFIX);
            crate::db::set_setting(
                &conn,
                &format!("{}:{}", crate::cluster::CALIBRATION_KEY_PREFIX, space + 1),
                &format!("[{day},5000,0.99]"),
            )
            .unwrap();
            for (cache, threshold, expected) in [
                (None, 0.85, vec![close, related]),
                (Some(format!("[{day},5000,0.9]")), 0.9, vec![close]),
                (
                    Some(format!("[{},5000,0.9]", day - 1)),
                    0.85,
                    vec![close, related],
                ),
                (
                    Some(format!("[{},5000,0.9]", day + 1)),
                    0.85,
                    vec![close, related],
                ),
                (
                    Some(format!("[{day},5000,0.8]")),
                    0.85,
                    vec![close, related],
                ),
                (
                    Some(format!("[{day},5000,1.1]")),
                    0.85,
                    vec![close, related],
                ),
                (Some("invalid".into()), 0.85, vec![close, related]),
                (
                    Some(format!("[{day},5000,1e100]")),
                    0.85,
                    vec![close, related],
                ),
            ] {
                conn.execute("DELETE FROM settings WHERE key=?1", [&key])
                    .unwrap();
                if let Some(raw) = cache {
                    crate::db::set_setting(&conn, &key, &raw).unwrap();
                }
                let changes = conn.total_changes();
                conn.pragma_update(None, "query_only", true).unwrap();
                assert_eq!(
                    crate::cluster::cached_similarity(&conn, space).unwrap(),
                    threshold
                );
                assert_eq!(ids(&conn, source, 10), expected);
                assert_eq!(conn.total_changes(), changes);
                conn.pragma_update(None, "query_only", false).unwrap();
            }
        }

        #[test]
        fn clamps_limit_to_one_through_ten() {
            let (_dir, conn) = open_temp();
            let feed = seed_feed(&conn);
            let source = article(&conn, feed, 1.0);
            for _ in 0..12 {
                article(&conn, feed, 0.95);
            }
            for (limit, count) in [(i64::MIN, 1), (0, 1), (3, 3), (10, 10), (i64::MAX, 10)] {
                assert_eq!(ids(&conn, source, limit).len(), count);
            }
        }

        #[test]
        #[ignore = "manual Debug timing; no latency assertion"]
        fn timing_5000_articles() {
            let (_dir, conn) = open_temp();
            let feed = seed_feed(&conn);
            conn.execute("UPDATE vector_spaces SET dims=1024", [])
                .unwrap();
            let space = crate::vector_space::active(&conn).unwrap().unwrap().id;
            let tx = conn.unchecked_transaction().unwrap();
            let mut above_threshold = 0;
            for id in 1..=5000 {
                let similarity = if id == 1 {
                    1.0
                } else {
                    0.60 + 0.39 * ((id - 2) % 100) as f32 / 99.0
                };
                above_threshold +=
                    usize::from(id != 1 && similarity >= crate::cluster::VECTOR_MIN_SIMILARITY);
                let mut vector = vec![((1.0 - similarity * similarity) / 1023.0).sqrt(); 1024];
                vector[0] = similarity;
                tx.execute("INSERT INTO articles(id,feed_id,title,created_at,cluster_id) VALUES(?1,?2,'Fixture',1,?3)", params![id, feed, (id + 1) / 3]).unwrap();
                tx.execute("INSERT INTO chunks(article_id,ordinal,kind,text,embedding,space_id) VALUES(?1,0,'title_summary','Fixture',?2,?3)", params![id, f32s_to_le(&vector), space]).unwrap();
            }
            tx.commit().unwrap();
            let start = std::time::Instant::now();
            let rows = related_articles(&conn, 1, 10).unwrap();
            let elapsed = start.elapsed();
            eprintln!(
                "related_articles: articles=5000 dims=1024 limit=10 threshold=0.85 above_threshold={above_threshold} returned={} elapsed_ms={:.3}",
                rows.len(),
                elapsed.as_secs_f64() * 1000.0
            );
        }
    }

    fn hit(id: i64, title: &str) -> SearchHit {
        SearchHit {
            id,
            title: title.into(),
            feed_id: 1,
            feed_title: "News".into(),
            published_at: None,
            score: 0,
            snippet: title.into(),
            read: false,
            starred: false,
            title_match: false,
            body_match: false,
        }
    }
}
