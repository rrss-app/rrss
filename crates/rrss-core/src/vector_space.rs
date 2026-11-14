//! 阶段 23「向量空间（V1）」：指纹、空间切换与固定探针；active 是读取唯一入口。
//! rebuild_from 区分 stale 的重建和生效空间。
use crate::error::{models::ModelChoiceProblem, settings::SettingProblem};
use crate::{ai_store::EmbedEndpoint, error::Result, rag::rebuild_inputs};
use rusqlite::{Connection, OptionalExtension, params};

pub const ALLOW_MODEL_CHANGE: &str = "allow_embedding_model_change";
pub const DIMS_SETTING: &str = "embedding_dims";
pub const CHECK_INTERVAL_MS: i64 = 6 * 60 * 60 * 1000;
pub const PROBES: [&str; 8] = [
    "晨间阅读：研究人员发布了一种更节能的计算方法。",
    "开源软件的新版本改进了搜索和数据备份。",
    "这篇报道讨论公共交通、城市生活与环境变化。",
    "我想比较不同观点，并保留可核对的来源。",
    "A small spacecraft measures dust beyond the orbit of Mars.",
    "Database transactions preserve consistent state during concurrent updates.",
    "The recipe combines fresh vegetables with rice and a mild sauce.",
    "Compare the evidence before drawing a conclusion about the experiment.",
];
pub type RebuildInput = (String, i64, String);

pub fn dims_setting(conn: &Connection) -> Result<String> {
    Ok(crate::db::get_setting(conn, DIMS_SETTING)?.unwrap_or_else(|| "auto".into()))
}
pub fn resolve_dims(value: &str, source_dims: usize) -> Result<usize> {
    let target = match value.trim() {
        "auto" => 1024,
        "native" => source_dims,
        n => n
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| crate::Error::InvalidSetting(SettingProblem::VectorDimsInvalid))?,
    };
    Ok(target.min(source_dims))
}
/// 只在截断时归一化；使用 f64 累加，避免有限 f32 分量的平方溢出。
pub fn fit(mut vector: Vec<f32>, target: usize) -> Vec<f32> {
    if target > 0 && vector.len() > target {
        vector.truncate(target);
        let norm = vector
            .iter()
            .map(|x| f64::from(*x).powi(2))
            .sum::<f64>()
            .sqrt();
        if norm > 0.0 {
            for x in &mut vector {
                *x = (f64::from(*x) / norm) as f32;
            }
        }
    }
    vector
}

#[derive(Debug, Clone)]
pub struct VectorSpace {
    pub id: i64,
    pub provider_id: Option<i64>,
    pub base_url: String,
    pub model: String,
    pub dims: i64,
    pub source_dims: i64,
    pub state: String,
    pub rebuild_from: Option<i64>,
    pub checked_at: Option<i64>,
}
impl VectorSpace {
    pub fn target_dims(&self, value: &str) -> Result<usize> {
        resolve_dims(value, self.effective_source_dims() as usize)
    }
    fn effective_source_dims(&self) -> i64 {
        if self.source_dims > 0 {
            self.source_dims
        } else {
            self.dims
        }
    }
    fn fingerprint(&self) -> EmbedEndpoint {
        EmbedEndpoint {
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            api_key: None,
        }
    }
}
fn find(conn: &Connection, rebuild: bool) -> Result<Option<VectorSpace>> {
    conn.query_row(
        "SELECT id,provider_id,base_url,model,dims,state,rebuild_from,checked_at,source_dims FROM vector_spaces
         WHERE state IN ('active','building','stale') AND (rebuild_from IS NOT NULL)=?1",
        [rebuild],
        |r| {
            Ok(VectorSpace {
                id: r.get(0)?,
                provider_id: r.get(1)?,
                base_url: r.get(2)?,
                model: r.get(3)?,
                dims: r.get(4)?,
                state: r.get(5)?,
                rebuild_from: r.get(6)?,
                checked_at: r.get(7)?,
                source_dims: r.get(8)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}
pub fn active(conn: &Connection) -> Result<Option<VectorSpace>> {
    find(conn, false)
}
pub fn building(conn: &Connection) -> Result<Option<VectorSpace>> {
    find(conn, true)
}
pub fn write_space(conn: &Connection) -> Result<Option<VectorSpace>> {
    Ok(building(conn)?
        .or(active(conn)?)
        .filter(|s| s.state != "stale"))
}
fn bound_provider(conn: &Connection, endpoint: &EmbedEndpoint) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT p.id FROM task_bindings b JOIN ai_providers p ON p.id=b.provider_id
         WHERE b.task='embed' AND trim(b.model)=?1 AND p.base_url=?2 AND trim(p.base_url)!=''",
        params![endpoint.model, endpoint.base_url],
        |r| r.get(0),
    )
    .optional()
    .map_err(Into::into)
}
pub fn endpoint(conn: &Connection, space: &VectorSpace) -> Result<Option<EmbedEndpoint>> {
    let Some(id) = space.provider_id else {
        return Ok(None);
    };
    match crate::ai_store::get_provider(conn, id) {
        Ok(p) => Ok(Some(EmbedEndpoint {
            api_key: p.api_key,
            ..space.fingerprint()
        })),
        Err(crate::Error::ProviderNotFound) => Ok(None),
        Err(e) => Err(e),
    }
}
pub fn binding_matches(conn: &Connection, ep: &EmbedEndpoint) -> Result<bool> {
    let provider = bound_provider(conn, ep)?;
    let space = building(conn)?.or(active(conn)?);
    let matches = provider.is_some()
        && space.is_none_or(|s| {
            s.state != "stale"
                && s.provider_id == provider
                && s.base_url == ep.base_url
                && s.model == ep.model
        });
    if !matches {
        tracing::warn!(
            error_kind = "vector_fingerprint_mismatch",
            "embedding fingerprint differs; stopping vector writes"
        );
    }
    Ok(matches)
}
pub fn ensure_for_embedding(
    conn: &Connection,
    ep: &EmbedEndpoint,
    source_dims: usize,
) -> Result<Option<VectorSpace>> {
    if source_dims == 0 || !binding_matches(conn, ep)? {
        return Ok(None);
    }
    if let Some(space) = write_space(conn)? {
        return Ok(
            (space.dims > 0 && source_dims == space.effective_source_dims() as usize)
                .then_some(space),
        );
    }
    let dims = resolve_dims(&dims_setting(conn)?, source_dims)?;
    conn.execute(
        "INSERT INTO vector_spaces(provider_id,base_url,model,dims,prefix_version,normalized,state,created_at,source_dims)
         VALUES(?1,?2,?3,?4,1,0,'active',?5,?6)",
        params![bound_provider(conn,ep)?,ep.base_url,ep.model,dims as i64,crate::feeds::now_ms(),source_dims as i64],
    )?;
    active(conn)
}
pub fn prepare_binding(
    conn: &Connection,
    provider: Option<i64>,
    model: Option<&str>,
) -> Result<()> {
    let Some(space) = building(conn)?.or(active(conn)?) else {
        return Ok(());
    };
    let base = provider
        .map(|id| {
            conn.query_row("SELECT base_url FROM ai_providers WHERE id=?1", [id], |r| {
                r.get::<_, String>(0)
            })
        })
        .transpose()?;
    let allow = crate::db::setting_enabled(conn, ALLOW_MODEL_CHANGE, false)?;
    let same = space.provider_id == provider
        && Some(space.model.as_str()) == model.map(str::trim)
        && Some(space.base_url.as_str()) == base.as_deref();
    if same && (space.state != "stale" || !allow || space.rebuild_from.is_some()) {
        return Ok(());
    }
    if space.rebuild_from.is_some() {
        return Err(crate::Error::VectorSpaceRebuildInProgress);
    }
    if !allow {
        return Err(crate::Error::VectorSpaceRebuildRequired { model: space.model });
    }
    let (Some(provider), Some(model)) = (provider, model.map(str::trim).filter(|s| !s.is_empty()))
    else {
        return Err(crate::Error::InvalidModelChoice(
            ModelChoiceProblem::RebuildModelMissing,
        ));
    };
    conn.execute(
        "INSERT INTO vector_spaces(provider_id,base_url,model,dims,prefix_version,normalized,state,created_at,rebuild_from)
         VALUES(?1,?2,?3,0,1,0,'building',?4,?5)",
        params![provider,base,model,crate::feeds::now_ms(),space.id],
    )?;
    Ok(())
}
pub fn restore_binding(conn: &Connection) -> Result<()> {
    if let Some(space) = active(conn)? {
        let id = space.provider_id.ok_or(crate::Error::ProviderNotFound)?;
        if conn.execute(
            "UPDATE ai_providers SET base_url=?2 WHERE id=?1",
            params![id, space.base_url],
        )? == 0
        {
            return Err(crate::Error::ProviderNotFound);
        }
        conn.execute(
            "UPDATE task_bindings SET provider_id=?1,model=?2 WHERE task='embed'",
            params![id, space.model],
        )?;
    }
    Ok(())
}
pub fn cancel_rebuild(conn: &Connection) -> Result<()> {
    crate::db::atomic_write(conn, |conn| {
        if let Some(space) = building(conn)? {
            if let Some(active) = active(conn)?
                && space.provider_id == active.provider_id
                && space.base_url == active.base_url
                && space.model == active.model
                && active.target_dims(&dims_setting(conn)?)? != active.dims as usize
            {
                let value = if active.dims == active.source_dims {
                    "native".into()
                } else {
                    active.dims.to_string()
                };
                crate::db::set_setting(conn, DIMS_SETTING, &value)?;
            }
            conn.execute("DELETE FROM vector_spaces WHERE id=?1", [space.id])?;
            restore_binding(conn)?;
        }
        Ok(())
    })
}
pub fn pending_rebuild(conn: &Connection, id: i64) -> Result<Vec<RebuildInput>> {
    pending_rebuild_batch(conn, id, usize::MAX, |_, _, _| true)
}

pub fn pending_rebuild_batch(
    conn: &Connection,
    id: i64,
    limit: usize,
    mut eligible: impl FnMut(&str, i64, &str) -> bool,
) -> Result<Vec<RebuildInput>> {
    let mut statement = conn.prepare(&format!(
        "SELECT i.kind,i.row_id,i.text FROM ({}) i
        LEFT JOIN vector_staging s ON s.space_id=?1 AND s.kind=i.kind AND s.row_id=i.row_id
        AND s.input_text=i.text WHERE s.row_id IS NULL",
        rebuild_inputs()
    ))?;
    let mut rows = statement.query([id])?;
    let mut out = Vec::new();
    while out.len() < limit {
        let Some(row) = rows.next()? else {
            break;
        };
        let kind: String = row.get(0)?;
        let id = row.get(1)?;
        let text: String = row.get(2)?;
        if eligible(&kind, id, &text) {
            out.push((kind, id, text));
        }
    }
    Ok(out)
}
pub fn rebuild_progress(conn: &Connection, id: i64) -> Result<(i64, i64)> {
    conn.query_row(
        &format!(
            "SELECT COUNT(s.row_id),COUNT(*) FROM ({}) i
        LEFT JOIN vector_staging s ON s.space_id=?1 AND s.kind=i.kind AND s.row_id=i.row_id
        AND s.input_text=i.text",
            rebuild_inputs()
        ),
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .map_err(Into::into)
}
/// 输入文本只在行级暂存；SQL 跳过重建批次中迟到的旧文本结果。
pub fn stage(
    conn: &Connection,
    space: &VectorSpace,
    kind: &str,
    row_id: i64,
    text: Option<&str>,
    vector: &[f32],
) -> Result<bool> {
    if space.rebuild_from.is_none() {
        return Ok(false);
    }
    let vector = fit(vector.to_vec(), space.dims as usize);
    conn.execute(
        &format!(
            "INSERT INTO vector_staging(space_id,kind,row_id,embedding,input_text)
             SELECT ?1,kind,row_id,?4,text FROM ({})
             WHERE kind=?2 AND row_id=?3 AND (?5 IS NULL OR text=?5)
             ON CONFLICT(space_id,kind,row_id) DO UPDATE
             SET embedding=excluded.embedding,input_text=excluded.input_text",
            rebuild_inputs()
        ),
        params![
            space.id,
            kind,
            row_id,
            crate::rag::f32s_to_le(&vector),
            text
        ],
    )?;
    Ok(true)
}
/// 调用方持有写事务；本地缩减逐行暂存，缺失的向量留给重建作业。
pub fn prepare_dims_rebuild(conn: &Connection, active: &VectorSpace, dims: usize) -> Result<i64> {
    let local = dims < active.dims as usize;
    conn.execute(
        "INSERT INTO vector_spaces(provider_id,base_url,model,dims,source_dims,prefix_version,normalized,state,created_at,rebuild_from)
         VALUES(?1,?2,?3,?4,?5,1,0,'building',?6,?7)",
        params![active.provider_id, active.base_url, active.model, if local { dims as i64 } else { 0 },
            if local { active.source_dims } else { 0 }, crate::feeds::now_ms(), active.id],
    )?;
    let space = building(conn)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    if local {
        let decode = |blob: Vec<u8>| {
            crate::rag::le_to_f32s(&blob)
                .ok_or_else(|| crate::Error::InvalidSetting(SettingProblem::VectorDataInvalid))
        };
        let mut probes =
            conn.prepare("SELECT ordinal,embedding FROM vector_space_probes WHERE space_id=?1")?;
        for row in probes.query_map([active.id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
        })? {
            let (ordinal, blob) = row?;
            conn.execute(
                "INSERT INTO vector_space_probes VALUES(?1,?2,?3)",
                params![
                    space.id,
                    ordinal,
                    crate::rag::f32s_to_le(&fit(decode(blob)?, dims))
                ],
            )?;
        }
        let mut inputs = conn.prepare(&format!(
            "SELECT kind,row_id,text,embedding FROM ({}) WHERE space_id=?1 AND embedding IS NOT NULL", rebuild_inputs()
        ))?;
        for row in inputs.query_map([active.id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Vec<u8>>(3)?,
            ))
        })? {
            let (kind, id, text, blob) = row?;
            stage(conn, &space, &kind, id, Some(&text), &decode(blob)?)?;
        }
    }
    Ok(space.id)
}
pub fn finish_rebuild(conn: &Connection, id: i64) -> Result<bool> {
    crate::db::atomic_write(conn, |conn| {
        let Some(space) =
            building(conn)?.filter(|s| s.id == id && s.state == "building" && s.dims > 0)
        else {
            return Ok(false);
        };
        if !binding_matches(conn, &space.fingerprint())? {
            return Ok(false);
        }
        let (written, total) = rebuild_progress(conn, id)?;
        // dims 由完整且通过校验的八段探针设置；stale 已在上面挡住。
        if written != total {
            return Ok(false);
        }
        for (kind, table) in [
            ("chunk", "chunks"),
            ("interest", "interests"),
            ("memory", "memories"),
        ] {
            conn.execute(
                &format!(
                    "UPDATE {table} AS t SET embedding=s.embedding,space_id=s.space_id
                FROM vector_staging s JOIN ({}) i ON i.kind=s.kind AND i.row_id=s.row_id AND i.text=s.input_text
                WHERE s.space_id=?1 AND s.kind=?2 AND t.id=s.row_id", rebuild_inputs()
                ),
                params![id, kind],
            )?;
        }
        conn.execute(
            "UPDATE chunks SET model=?2,attempts=0,last_error=NULL WHERE space_id=?1",
            params![id, space.model],
        )?;
        conn.execute("DELETE FROM vector_staging WHERE space_id=?1", [id])?;
        conn.execute(
            "UPDATE vector_spaces SET state='retired' WHERE id=?1",
            [space.rebuild_from],
        )?;
        conn.execute(
            "UPDATE vector_spaces SET state='active',rebuild_from=NULL WHERE id=?1",
            [id],
        )?;
        Ok(true)
    })
}
pub fn record_probes(conn: &Connection, id: i64, vectors: &[Vec<f32>]) -> Result<bool> {
    crate::db::atomic_write(conn, |conn| {
        let Some(space) = building(conn)?.or(active(conn)?).filter(|s| s.id == id) else {
            return Ok(false);
        };
        let old = conn
            .prepare(
                "SELECT embedding FROM vector_space_probes WHERE space_id=?1 ORDER BY ordinal",
            )?
            .query_map([id], |r| r.get::<_, Vec<u8>>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let source_dims = vectors.first().map_or(0, Vec::len);
        let target = if space.dims > 0 {
            space.dims as usize
        } else {
            resolve_dims(&dims_setting(conn)?, source_dims)?
        };
        let fitted: Vec<_> = vectors.iter().cloned().map(|v| fit(v, target)).collect();
        let dims = fitted.first().map_or(0, Vec::len);
        let good = bound_provider(conn, &space.fingerprint())? == space.provider_id
            && dims > 0
            && (space.dims == 0 || space.dims == dims as i64)
            && vectors.len() == PROBES.len()
            && vectors
                .iter()
                .all(|v| v.len() == source_dims && v.iter().all(|x| x.is_finite()))
            && fitted.iter().all(|v| {
                v.len() == dims && v.iter().all(|x| x.is_finite()) && v.iter().any(|x| *x != 0.0)
            })
            && (old.is_empty()
                || (old.len() == PROBES.len()
                    && old.iter().zip(&fitted).all(|(b, v)| {
                        crate::rag::le_to_f32s(b)
                            .is_some_and(|a| crate::rag::cosine_distance(&a, v) <= 0.001)
                    })));
        if good && old.is_empty() {
            for (ordinal, vector) in fitted.iter().enumerate() {
                conn.execute(
                    "INSERT INTO vector_space_probes VALUES(?1,?2,?3)",
                    params![id, ordinal as i64, crate::rag::f32s_to_le(vector)],
                )?;
            }
        }
        let reason =
            (!good).then_some("向量与建库时不一致，可能是连接、模型版本、量化或维度变了。");
        conn.execute(
            "UPDATE vector_spaces SET checked_at=?2,stale_reason=?3,
            state=CASE WHEN ?3 IS NOT NULL THEN 'stale' WHEN rebuild_from IS NULL THEN 'active' ELSE 'building' END,
            dims=CASE WHEN dims=0 AND ?3 IS NULL THEN ?4 ELSE dims END,
            source_dims=CASE WHEN source_dims=0 AND ?3 IS NULL THEN ?5 ELSE source_dims END WHERE id=?1",
            params![id, crate::feeds::now_ms(), reason, dims as i64, source_dims as i64],
        )?;
        Ok(good)
    })
}
#[cfg(test)]
pub(crate) fn seed_active(conn: &Connection, model: &str, dims: i64) {
    conn.execute(
        "INSERT INTO vector_spaces(base_url, model, dims, prefix_version, normalized, state, created_at)
         VALUES('', ?1, ?2, 1, 0, 'active', 0)", params![model, dims],
    ).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{interests, memories, rag};

    #[test]
    fn fit_truncates_the_prefix_and_l2_normalizes_it() {
        assert_eq!(fit(vec![3.0, 4.0, 12.0], 2), vec![0.6, 0.8]);
        assert_eq!(fit(vec![0.0, 0.0, 1.0], 2), vec![0.0, 0.0]);
        let large = fit(vec![f32::MAX; 3], 2);
        assert!((large[0] - 1.0 / 2.0_f32.sqrt()).abs() < 1e-7);
    }

    #[test]
    fn fit_leaves_short_equal_and_unbounded_vectors_unchanged() {
        let vector = vec![3.0, 4.0];
        for target in [0, 2, 4] {
            assert_eq!(fit(vector.clone(), target), vector);
        }
        assert!(fit(Vec::new(), 2).is_empty());
    }

    #[test]
    fn equal_768_dimensions_do_not_cross_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("spaces.db")).unwrap();
        seed_active(&conn, "model-a", 768);
        conn.execute_batch("INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at)
            VALUES(2,'','model-b',768,1,0,'retired',0);
            INSERT INTO feeds(id,url,created_at) VALUES(1,'https://fixture.invalid/feed',0);
            INSERT INTO articles(id,feed_id,title,created_at) VALUES(1,1,'a',0),(2,1,'b',0);").unwrap();
        let mut vector = vec![0.0; 768];
        vector[0] = 1.0;
        let iid = interests::insert_interest(&conn, "a prototype", "manual").unwrap();
        rag::set_interest_embedding(&conn, iid, &vector).unwrap();
        for id in [1, 2] {
            conn.execute(
                "INSERT INTO chunks(article_id,ordinal,text,embedding,model,space_id)
                VALUES(?1,0,'fixture',?2,?3,?1)",
                params![
                    id,
                    rag::f32s_to_le(&vector),
                    if id == 1 { "model-a" } else { "model-b" }
                ],
            )
            .unwrap();
            let mid = memories::insert_memory(
                &conn,
                memories::MemoryKind::Semantic,
                "fixture",
                "test",
                1.0,
            )
            .unwrap();
            rag::set_memory_embedding(&conn, mid, &vector).unwrap();
            conn.execute(
                "UPDATE memories SET space_id=?1 WHERE id=?2",
                params![id, mid],
            )
            .unwrap();
        }
        assert_eq!(
            rag::knn_articles(&conn, &vector, 10)
                .unwrap()
                .iter()
                .map(|h| h.id)
                .collect::<Vec<_>>(),
            [1]
        );
        assert!(rag::title_embedding(&conn, 2).unwrap().is_none());
        interests::recompute_interest_boost(&conn).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM article_interests WHERE article_id=2",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            memories::search_memory_hybrid(&conn, "no match", Some(&vector), 10)
                .unwrap()
                .iter()
                .map(|m| m.id)
                .collect::<Vec<_>>(),
            [1]
        );
        assert_eq!(
            rag::TextEmbedding::Memory
                .pending(&conn)
                .unwrap()
                .iter()
                .map(|m| m.0)
                .collect::<Vec<_>>(),
            [2]
        );
        conn.execute("UPDATE interests SET space_id=2", []).unwrap();
        assert!(!interests::interest_ready(&conn).unwrap());
        assert_eq!(
            rag::TextEmbedding::Interest.pending(&conn).unwrap().len(),
            1
        );
    }
}
