//! 阶段 23 · V1：空间状态、COUNT 进度和更换成本的原始量。
use crate::views::{VectorRebuildView, VectorSpaceStatusView, VectorSpaceView};
use rrss_core::{Result, db, vector_space as spaces};
use rusqlite::{Connection, OptionalExtension};
fn view(conn: &Connection, id: i64) -> Result<VectorSpaceView> {
    conn.query_row(
        "SELECT v.id,v.model,COALESCE(p.name,'连接已删除'),v.dims,v.state,
         (SELECT COUNT(*) FROM chunks WHERE space_id=v.id AND embedding IS NOT NULL),
         v.stale_reason,v.checked_at,v.source_dims FROM vector_spaces v
         LEFT JOIN ai_providers p ON p.id=v.provider_id WHERE v.id=?1",
        [id],
        |r| {
            Ok(VectorSpaceView {
                id: r.get(0)?,
                model: r.get(1)?,
                provider_name: r.get(2)?,
                dims: r.get(3)?,
                state: r.get(4)?,
                chunks: r.get(5)?,
                stale_reason: r.get(6)?,
                checked_at: r.get(7)?,
                source_dims: r.get(8)?,
            })
        },
    )
    .map_err(Into::into)
}
pub fn vector_space_status(conn: &Connection) -> Result<VectorSpaceStatusView> {
    let effective = spaces::active(conn)?
        .map(|s| view(conn, s.id))
        .transpose()?;
    let building = spaces::building(conn)?
        .map(|s| {
            let (written, total) = spaces::rebuild_progress(conn, s.id)?;
            Ok::<_, rrss_core::Error>(VectorRebuildView {
                space: view(conn, s.id)?,
                written,
                total,
            })
        })
        .transpose()?;
    let mut chunks = 0;
    let mut chars = spaces::PROBES
        .iter()
        .map(|s| s.chars().count() as i64)
        .sum::<i64>();
    // 查询不写数据；与重建共用文章输入，未生成 chunks 的正文也计入预估。
    for article in rrss_core::rag::rebuild_articles(conn)? {
        let drafts = article.drafts(2);
        chunks += drafts.len() as i64;
        chars += drafts
            .iter()
            .map(|d| d.text.chars().count() as i64)
            .sum::<i64>();
    }
    chars += rrss_core::rag::text_input_chars(conn)?;
    let base: Option<String> = conn
        .query_row(
            "SELECT p.base_url FROM task_bindings b
        JOIN ai_providers p ON p.id=b.provider_id WHERE b.task='embed'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    let local = base
        .and_then(|s| reqwest::Url::parse(&s).ok())
        .is_some_and(|u| matches!(u.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")));
    Ok(VectorSpaceStatusView {
        effective,
        building,
        allow_model_change: db::setting_enabled(conn, spaces::ALLOW_MODEL_CHANGE, false)?,
        dims_setting: spaces::dims_setting(conn)?,
        estimate_chunks: chunks,
        estimate_chars: chars,
        estimate_tokens: (chars * 3 + 3) / 4,
        local,
    })
}
