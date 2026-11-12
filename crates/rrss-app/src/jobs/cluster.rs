use std::path::Path;

use anyhow::Result;

use super::spawn_db;

pub async fn run_cluster_jobs(db_path: &Path) -> Result<()> {
    let path = db_path.to_path_buf();
    spawn_db(path, |conn| {
        rrss_core::cluster::rebuild_if_needed(&conn)?;
        rrss_core::backfill_simhash(&conn, rrss_core::cluster::BACKFILL_BATCH)?;
        rrss_core::assign_unclustered(&conn, rrss_core::cluster::BACKFILL_BATCH)?;
        rrss_core::merge_close_clusters(&conn)?;
        Ok(())
    })
    .await
}

pub async fn run_interest_boost(db_path: &Path) -> Result<()> {
    let path = db_path.to_path_buf();
    spawn_db(path, |conn| rrss_core::recompute_interest_boost(&conn)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::{cluster, db, feeds::now_ms};
    use rusqlite::params;

    #[tokio::test]
    async fn algorithm_upgrade_rebuilds_window_atomically_and_preserves_references() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cluster.db");
        let conn = db::open_at(&path).unwrap();
        let now = now_ms();
        let old = now - 15 * 86_400_000;
        conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://news.test/rss',1);
            INSERT INTO clusters(id,label,member_count,representative_id,created_at)
                VALUES(10,'old mixed cluster',3,1,1),(20,'recent only',1,4,1);
            INSERT INTO threads(id,created_at,updated_at,anchor_kind,anchor_id) VALUES(1,1,1,'article',1);").unwrap();
        for (id, time, cid) in [(1, now, 10), (2, now, 10), (3, old, 10), (4, now, 20)] {
            conn.execute("INSERT INTO articles(id,feed_id,title,hash,canonical_url,simhash,cluster_id,published_at,created_at)
                VALUES(?1,1,?2,?2,?2,123,?3,?4,?4)",params![id,format!("article{id}"),cid,time]).unwrap();
        }
        let body = "- Saved story";
        let sources =
            r#"[{"sources":[{"article_id":1,"title":"Saved story","feed_title":"News"}]}]"#;
        let citations = r#"[{"article_id":2,"title":"Saved reference","excerpt":"Original text"}]"#;
        conn.execute(
            "INSERT INTO digests(id,period,period_start,body,items_json,created_at)
            VALUES(1,'daily',1,?1,?2,1)",
            params![body, sources],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages(thread_id,role,citations_json,created_at)
            VALUES(1,'user',?1,1)",
            [citations],
        )
        .unwrap();
        db::set_setting(&conn, cluster::ALGORITHM_VERSION_KEY, "previous").unwrap();
        // Mid-rebuild failure must preserve the old version and all old assignments.
        conn.execute_batch(
            "CREATE TRIGGER fail_rebuild BEFORE UPDATE OF simhash ON articles
            WHEN NEW.id=2 AND NEW.simhash IS NOT NULL BEGIN SELECT RAISE(ABORT,'injected'); END;",
        )
        .unwrap();
        assert!(run_cluster_jobs(&path).await.is_err());
        assert_eq!(
            db::get_setting(&conn, cluster::ALGORITHM_VERSION_KEY)
                .unwrap()
                .as_deref(),
            Some("previous")
        );
        assert_eq!(cluster::cluster_members(&conn, 10).unwrap(), vec![3, 1, 2]);
        conn.execute_batch("DROP TRIGGER fail_rebuild").unwrap();
        run_cluster_jobs(&path).await.unwrap();
        assert_eq!(
            db::get_setting(&conn, cluster::ALGORITHM_VERSION_KEY)
                .unwrap()
                .as_deref(),
            Some(cluster::ALGORITHM_VERSION)
        );
        let snapshot = || {
            conn.prepare("SELECT id,simhash,cluster_id FROM articles ORDER BY id")
                .unwrap()
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let rebuilt = snapshot();
        assert_eq!(rebuilt[2], (3, 123, 10));
        assert_eq!(rebuilt[0].1, 0);
        assert_ne!(rebuilt[0].2, rebuilt[1].2);
        assert!(rebuilt.iter().filter(|r| r.0 != 3).all(|r| r.2 > 20));
        assert_eq!(cluster::cluster_members(&conn, 10).unwrap(), vec![3]);
        assert!(cluster::cluster_members(&conn, 20).unwrap().is_empty());
        let digest = rrss_core::digest::digest_by_id(&conn, 1).unwrap().unwrap();
        assert_eq!(
            (digest.body.as_str(), digest.items_json.as_str()),
            (body, sources)
        );
        assert_eq!(
            rrss_core::digest::parse_digest_items_json(&digest.items_json)[0][0].article_id,
            1
        );
        let saved: String = conn
            .query_row("SELECT citations_json FROM messages", [], |r| r.get(0))
            .unwrap();
        assert_eq!(saved, citations);
        assert_eq!(
            rrss_core::cite::parse_citations_json(Some(&saved))[0].article_id,
            2
        );
        let anchor: i64 = conn
            .query_row("SELECT anchor_id FROM threads", [], |r| r.get(0))
            .unwrap();
        assert_eq!(anchor, 1);
        run_cluster_jobs(&path).await.unwrap();
        assert_eq!(snapshot(), rebuilt);
    }
}
