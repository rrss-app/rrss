//! Bounded, restartable chunk preparation. The database queue tracks source changes.
use crate::{Result, db, rag};
use rusqlite::{Connection, OptionalExtension};

const PAGE_ROWS: usize = 32;
const PAGE_BYTES: usize = 256 * 1024;

struct Input {
    article: rag::DueEmbedArticle,
    revision: Vec<u8>,
}

pub fn pending(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM chunk_preparation p JOIN articles a
        ON a.id=p.article_id WHERE a.pruned_at IS NULL AND a.expires_at IS NULL)",
        [],
        |r| r.get(0),
    )?)
}

/// One page per scheduler step; documents are rendered before taking any writer lock.
pub fn prepare_page(conn: &Connection, space_id: i64) -> Result<usize> {
    prepare_with(conn, space_id, || Ok(()))
}
fn prepare_with(
    conn: &Connection,
    space_id: i64,
    between: impl Fn() -> Result<()>,
) -> Result<usize> {
    let inputs = db::read_snapshot(conn, |conn| {
        let mut stmt = conn.prepare(
            "SELECT a.id,a.title,a.summary,a.blocks_json,a.content_html,
            MAX(a.embed_tier,CASE WHEN a.read=1 OR a.starred=1 THEN 2 ELSE 1 END),p.revision
            FROM chunk_preparation p JOIN articles a ON a.id=p.article_id
            WHERE a.pruned_at IS NULL AND a.expires_at IS NULL ORDER BY a.id LIMIT ?1",
        )?;
        let mut rows = stmt.query([PAGE_ROWS as i64])?;
        let mut inputs = Vec::new();
        let mut bytes = 0;
        while let Some(row) = rows.next()? {
            let article = rag::DueEmbedArticle::from_row(row)?;
            bytes += article.title.len()
                + article.summary.as_ref().map_or(0, String::len)
                + article.blocks_json.as_ref().map_or(0, String::len)
                + article.content_html.as_ref().map_or(0, String::len);
            inputs.push(Input {
                article,
                revision: row.get(6)?,
            });
            if bytes >= PAGE_BYTES {
                break;
            }
        }
        Ok(inputs)
    })?;
    between()?;
    let mut written = 0;
    for input in inputs {
        let drafts = input.article.drafts(input.article.tier);
        written += db::atomic_write(conn, |conn| {
            if crate::vector_space::building(conn)?
                .is_none_or(|s| s.id != space_id || s.state != "building")
            {
                return Ok(0);
            }
            let revision: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT revision FROM chunk_preparation WHERE article_id=?1",
                    [input.article.id],
                    |r| r.get(0),
                )
                .optional()?;
            if revision != Some(input.revision) {
                return Ok(0);
            }
            rag::apply_chunk_drafts(conn, input.article.id, input.article.tier, &drafts)?;
            // embed_tier may have advanced our own revision; no other writer can
            // change the input between the check and this acknowledgement.
            conn.execute(
                "DELETE FROM chunk_preparation WHERE article_id=?1",
                [input.article.id],
            )?;
            Ok(1)
        })?;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preparation_is_bounded_resumable_and_rejects_changed_or_cancelled_inputs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prepare.db");
        let conn = db::open_at(&path).unwrap();
        conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://prepare.example/feed',0);
            INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at) VALUES(1,'','fixture',3,1,0,'active',0),(2,'','fixture',3,1,0,'building',0);
            UPDATE vector_spaces SET rebuild_from=1 WHERE id=2;
            WITH RECURSIVE n(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM n WHERE id<70)
            INSERT INTO articles(id,feed_id,title,created_at) SELECT id,1,'original '||id,0 FROM n;").unwrap();
        let other = db::open_at(&path).unwrap();
        assert_eq!(
            prepare_with(&conn, 2, || {
                other.busy_timeout(std::time::Duration::ZERO)?;
                other.execute("UPDATE articles SET title='changed' WHERE id=1", [])?;
                Ok(())
            })
            .unwrap(),
            PAGE_ROWS - 1
        );
        assert!(pending(&conn).unwrap());
        drop(conn);
        let conn = db::open_at(&path).unwrap();
        while pending(&conn).unwrap() {
            assert!(prepare_page(&conn, 2).unwrap() > 0);
        }
        assert_eq!(prepare_page(&conn, 2).unwrap(), 0);
        assert_eq!(
            conn.query_row("SELECT text FROM chunks WHERE article_id=1", [], |r| r
                .get::<_, String>(
                0
            ))
            .unwrap(),
            "changed"
        );
        other
            .execute(
                "UPDATE articles SET starred=1,content_html='<p>new body</p>' WHERE id=1",
                [],
            )
            .unwrap();
        assert_eq!(
            prepare_with(&conn, 2, || {
                other.execute("DELETE FROM vector_spaces WHERE id=2", [])?;
                Ok(())
            })
            .unwrap(),
            0
        );
        assert!(pending(&conn).unwrap());
    }
}
