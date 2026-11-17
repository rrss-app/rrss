//! Calculate interest results on a read snapshot and publish only a valid plan.
use super::*;

#[derive(Clone, Copy)]
pub(super) enum Change {
    Enabled(i64, bool),
    Weight(i64, i64),
    Delete(i64),
}
type InterestInput = (i64, String, i64, bool, Option<Vec<u8>>, Option<i64>);

struct Inputs {
    enabled: bool,
    threshold: i64,
    space: Option<i64>,
    interests: Vec<InterestInput>,
    selected: Vec<SelectedArticle>,
}

impl Inputs {
    fn load(conn: &Connection) -> Result<Self> {
        let mut inputs = Self::configuration(conn)?;
        if inputs.enabled {
            inputs.selected = load_selected_article_vectors(conn, inputs.threshold)?;
        }
        Ok(inputs)
    }
    fn configuration(conn: &Connection) -> Result<Self> {
        let enabled = boost_enabled(conn)?;
        let threshold = crate::articles::hide_threshold(conn)?;
        let space = crate::vector_space::active(conn)?.map(|s| s.id);
        let interests = conn
            .prepare(
                "SELECT id,label,weight,enabled,embedding,space_id FROM interests ORDER BY id",
            )?
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Self {
            enabled,
            threshold,
            space,
            interests,
            selected: Vec::new(),
        })
    }
    fn prototypes(&self, change: Option<Change>) -> Result<Vec<Prototype>> {
        let changed_id = change.map(|c| match c {
            Change::Enabled(id, _) | Change::Weight(id, _) | Change::Delete(id) => id,
        });
        if changed_id.is_some_and(|id| !self.interests.iter().any(|i| i.0 == id)) {
            return Err(Error::InterestNotFound);
        }
        if !self.enabled {
            return Ok(Vec::new());
        }
        Ok(self
            .interests
            .iter()
            .filter_map(|(id, label, weight, enabled, blob, space)| {
                let (mut weight, mut enabled) = (*weight, *enabled);
                match change {
                    Some(Change::Enabled(i, value)) if i == *id => enabled = value,
                    Some(Change::Weight(i, value)) if i == *id => weight = value,
                    Some(Change::Delete(i)) if i == *id => return None,
                    _ => {}
                }
                if !enabled || self.space.is_none() || *space != self.space {
                    return None;
                }
                Some(Prototype {
                    id: *id,
                    label: label.clone(),
                    weight,
                    vector: le_to_f32s(blob.as_deref()?)?,
                })
            })
            .collect())
    }
}

struct Row {
    id: i64,
    boost: i64,
    json: Option<String>,
    hits: Vec<(i64, i64)>,
}
struct Plan {
    source: i64,
    signature: String,
    rows: Vec<Row>,
    inputs: Inputs,
}
impl Plan {
    fn prepare(conn: &Connection, change: Option<Change>) -> Result<Self> {
        let inputs = Inputs::load(conn)?;
        let source = db::input_revision(conn, "interest")?;
        let prototypes = inputs.prototypes(change)?;
        let signature = if prototypes.is_empty() {
            String::new()
        } else {
            boost_signature(&prototypes, inputs.threshold, &inputs.selected)
        };
        let rows = if prototypes.is_empty() {
            Vec::new()
        } else {
            inputs
                .selected
                .iter()
                .map(|article| {
                    let (boost, reasons) =
                        calculate_boost(article.vector.as_deref().unwrap_or(&[]), &prototypes);
                    Row {
                        id: article.article_id,
                        boost,
                        json: reasons_json(&reasons),
                        hits: reasons.iter().map(|r| (r.interest_id, r.weight)).collect(),
                    }
                })
                .collect()
        };
        Ok(Self {
            source,
            signature,
            rows,
            inputs,
        })
    }

    // A foreground edit can converge while a small embedding batch finishes.
    // Reuse every unchanged result; never turn a full rescore into a writer job.
    fn rebase(&mut self, conn: &Connection, change: Option<Change>) -> Result<bool> {
        const MAX_CHANGED_ARTICLES: usize = 32;
        const MAX_VECTOR_COMPONENTS: usize = 128 * 1024;
        const MAX_SCANNED_ARTICLES: usize = 2048;
        const MAX_SCANNED_BYTES: usize = 8 * 1024 * 1024;
        let inputs = Inputs::configuration(conn)?;
        if inputs.enabled != self.inputs.enabled
            || inputs.threshold != self.inputs.threshold
            || inputs.space != self.inputs.space
            || inputs.interests != self.inputs.interests
        {
            return Ok(false);
        }
        let prototypes = inputs.prototypes(change)?;
        if prototypes.is_empty() {
            self.signature.clear();
            self.rows.clear();
            return Ok(true);
        }
        let old: std::collections::BTreeMap<_, _> = self
            .inputs
            .selected
            .iter()
            .map(|article| (article.article_id, &article.blob))
            .collect();
        // Compare raw blobs without decoding the entire corpus again. The
        // large vector comparisons remain in the original read-snapshot plan.
        let sql = format!(
            "SELECT a.id,c.embedding FROM articles a JOIN chunks c ON c.id={}
            WHERE a.expires_at IS NULL AND a.score>?1 ORDER BY a.id",
            crate::rag::SELECTED_CHUNK_SQL
        );
        let mut stmt = conn.prepare(&sql)?;
        let mut selected = stmt.query(params![inputs.threshold, inputs.space])?;
        let mut ids = Vec::new();
        let mut changed = Vec::new();
        let mut components = 0usize;
        let mut scanned_bytes = 0usize;
        if inputs.enabled {
            while let Some(row) = selected.next()? {
                let id: i64 = row.get(0)?;
                let blob = row.get_ref(1)?.as_blob().map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Blob,
                        Box::new(error),
                    )
                })?;
                ids.push(id);
                scanned_bytes = scanned_bytes.saturating_add(blob.len());
                if ids.len() > MAX_SCANNED_ARTICLES || scanned_bytes > MAX_SCANNED_BYTES {
                    return Ok(false);
                }
                if old.get(&id).map(|b| b.as_slice()) == Some(blob) {
                    continue;
                }
                components =
                    components.saturating_add((blob.len() / 4).saturating_mul(prototypes.len()));
                if changed.len() >= MAX_CHANGED_ARTICLES || components > MAX_VECTOR_COMPONENTS {
                    return Ok(false);
                }
                changed.push((id, le_to_f32s(blob)));
            }
        }
        let mut rows: std::collections::BTreeMap<_, _> = std::mem::take(&mut self.rows)
            .into_iter()
            .map(|row| (row.id, row))
            .collect();
        for (id, vector) in changed {
            let (boost, reasons) = calculate_boost(vector.as_deref().unwrap_or(&[]), &prototypes);
            rows.insert(
                id,
                Row {
                    id,
                    boost,
                    json: reasons_json(&reasons),
                    hits: reasons.iter().map(|r| (r.interest_id, r.weight)).collect(),
                },
            );
        }
        self.rows = ids.into_iter().filter_map(|id| rows.remove(&id)).collect();
        // Publish exact rows now; refresh the signature on the next background
        // check rather than hashing every vector under the writer lock.
        self.signature = "recheck".into();
        Ok(true)
    }
    fn publish(&self, conn: &Connection, change: Option<Change>) -> Result<u32> {
        match change {
            Some(Change::Enabled(id, value)) => {
                conn.execute(
                    "UPDATE interests SET enabled=?2 WHERE id=?1",
                    params![id, value],
                )?;
            }
            Some(Change::Weight(id, value)) => {
                conn.execute(
                    "UPDATE interests SET weight=?2 WHERE id=?1",
                    params![id, value],
                )?;
            }
            Some(Change::Delete(id)) => {
                conn.execute("DELETE FROM interests WHERE id=?1", [id])?;
            }
            None => {}
        }
        if self.signature.is_empty() {
            return clear_boosts(conn);
        }
        let ids = serde_json::to_string(&self.rows.iter().map(|r| r.id).collect::<Vec<_>>())
            .expect("article ids");
        conn.execute("DELETE FROM article_interests WHERE article_id NOT IN (SELECT value FROM json_each(?1))", [&ids])?;
        conn.execute(
            "UPDATE articles SET interest_boost=0,interest_reasons_json=NULL
            WHERE (interest_boost!=0 OR interest_reasons_json IS NOT NULL)
            AND id NOT IN (SELECT value FROM json_each(?1))",
            [&ids],
        )?;
        let mut update = conn.prepare_cached(
            "UPDATE articles SET interest_boost=?2,interest_reasons_json=?3
            WHERE id=?1 AND (interest_boost IS NOT ?2 OR interest_reasons_json IS NOT ?3)",
        )?;
        let mut remove = conn.prepare_cached(
            "DELETE FROM article_interests WHERE article_id=?1
            AND interest_id NOT IN (SELECT value FROM json_each(?2))",
        )?;
        let mut hit = conn.prepare_cached(
            "INSERT INTO article_interests(article_id,interest_id,weight) VALUES(?1,?2,?3)
            ON CONFLICT(article_id,interest_id) DO UPDATE SET weight=excluded.weight
            WHERE weight IS NOT excluded.weight",
        )?;
        for row in &self.rows {
            update.execute(params![row.id, row.boost, row.json])?;
            let interests =
                serde_json::to_string(&row.hits.iter().map(|h| h.0).collect::<Vec<_>>())
                    .expect("interest ids");
            remove.execute(params![row.id, interests])?;
            for (id, weight) in &row.hits {
                hit.execute(params![row.id, id, weight])?;
            }
        }
        db::set_setting(conn, INTEREST_BOOST_SIG_KEY, &self.signature)?;
        Ok(self.rows.iter().filter(|r| r.boost > 0).count() as u32)
    }
}

pub(super) fn run(conn: &Connection, change: Option<Change>) -> Result<u32> {
    run_with(conn, change, || Ok(()))
}
pub(super) fn run_with(
    conn: &Connection,
    change: Option<Change>,
    between: impl Fn() -> Result<()>,
) -> Result<u32> {
    for _ in 0..3 {
        let plan = db::read_snapshot(conn, |conn| {
            if change.is_none() && boost_up_to_date(conn)? {
                return Ok(None);
            }
            Plan::prepare(conn, change).map(Some)
        })?;
        let Some(mut plan) = plan else {
            return Ok(0);
        };
        between()?;
        let result = db::atomic_write(conn, |conn| {
            // Only relevant inputs invalidate the calculation; media, read flags,
            // unrelated settings and logging do not force expensive recomputation.
            if db::input_revision(conn, "interest")? != plan.source {
                if change.is_none() || !plan.rebase(conn, change)? {
                    return Ok(None);
                }
            }
            plan.publish(conn, change).map(Some)
        })?;
        if let Some(result) = result {
            return Ok(result);
        }
    }
    // Never fall back to doing the expensive calculation under the writer lock.
    Err(
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY), None)
            .into(),
    )
}
