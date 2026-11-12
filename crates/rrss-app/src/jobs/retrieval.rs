use std::path::Path;

use rrss_core::{format_rag_context, hybrid_search};

use super::{RAG_HITS, spawn_db};
use crate::query_embedding::QueryEmbedding;

/// 对话发送前：FTS + 可选向量混合，标题/摘要进 prompt，不带全文。
pub async fn retrieve_for_chat(
    db_path: &Path,
    network: &crate::network::NetworkClients,
    query: &str,
) -> String {
    let path = db_path.to_path_buf();
    let embedding = spawn_db(path.clone(), QueryEmbedding::load)
        .await
        .ok()
        .flatten();
    let q = query.to_string();
    let vec = if let Some(embedding) = embedding.as_ref() {
        let result = async {
            let client = network
                .client(rrss_core::network::NetworkScope::Api)
                .await?;
            embedding
                .embed_until(
                    &client,
                    &q,
                    tokio::time::Instant::now() + rrss_ai::deadline::ITEM_TIMEOUT,
                )
                .await
        }
        .await;
        match result {
            Ok(vector) => Some(vector),
            Err(err) => {
                tracing::warn!(error = %crate::log::error(err.as_ref()), "query embed failed; FTS only");
                None
            }
        }
    } else {
        None
    };
    spawn_db(path, move |conn| {
        let vec = match (embedding, vec) {
            (Some(embedding), Some(vector)) => embedding.fit(conn, vector)?,
            _ => None,
        };
        let hits = hybrid_search(conn, &q, vec.as_deref(), RAG_HITS)?;
        Ok(format_rag_context(&hits))
    })
    .await
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::{ai_store, db, interests, memories, rag};

    #[tokio::test]
    async fn chat_fits_query_vectors_to_the_effective_space() {
        use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"data":[{"index":0,"embedding":[3.0,4.0,10.0,20.0]}]}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("query-dims.db");
        let conn = db::open_at(&path).unwrap();
        db::set_setting(&conn, "pi_auto_download", "0").unwrap();
        let provider = crate::test_net::fake_provider(&conn, "fake", server.uri());
        ai_store::set_task_binding(&conn, "embed", Some(provider), Some("fixture")).unwrap();
        let ep = ai_store::resolve_embed(&conn).unwrap().unwrap();
        db::set_setting(&conn, "embedding_dims", "2").unwrap();
        rrss_core::vector_space::ensure_for_embedding(&conn, &ep, 4)
            .unwrap()
            .unwrap();
        // 设置可以指向正在重建的目标；检索始终以 active 的维度为准。
        db::set_setting(&conn, "embedding_dims", "native").unwrap();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://fixture.invalid/feed',0);
            INSERT INTO articles(id,feed_id,title,created_at) VALUES(1,1,'Vector-only fixture',0);
            INSERT INTO chunks(id,article_id,ordinal,text) VALUES(1,1,0,'unrelated content');",
        )
        .unwrap();
        rag::set_chunk_embedding(&conn, 1, "fixture", &[0.6, 0.8]).unwrap();
        let context = retrieve_for_chat(
            &path,
            &crate::network::NetworkClients::new(&path),
            "unmatched-query",
        )
        .await;
        assert!(context.contains("Vector-only fixture"), "{context}");
    }

    #[tokio::test]
    async fn no_embedding_connection_keeps_fts_and_skips_vector_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fts.db");
        {
            let conn = db::open_at(&path).unwrap();
            db::set_setting(&conn, "pi_auto_download", "0").unwrap();
            conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://fixture.invalid/feed',0);
                INSERT INTO articles(id,feed_id,title,summary,created_at) VALUES(1,1,'Rust retrieval','Rust fixture',0);
                INSERT INTO chunks(id,article_id,ordinal,text) VALUES(1,1,0,'Rust fixture');").unwrap();
            let iid = interests::insert_interest(&conn, "Rust", "manual").unwrap();
            let mid = memories::insert_memory(
                &conn,
                memories::MemoryKind::Semantic,
                "Rust fact",
                "test",
                1.0,
            )
            .unwrap();
            rag::set_chunk_embedding(&conn, 1, "unset", &[1.0, 0.0]).unwrap();
            rag::set_interest_embedding(&conn, iid, &[1.0, 0.0]).unwrap();
            rrss_core::rag::set_memory_embedding(&conn, mid, &[1.0, 0.0]).unwrap();
            assert!(rrss_core::vector_space::active(&conn).unwrap().is_none());
            assert_eq!(
                rag::hybrid_search(&conn, "Rust", Some(&[1.0, 0.0]), 10)
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(
                memories::search_memory_hybrid(&conn, "Rust", Some(&[1.0, 0.0]), 10)
                    .unwrap()
                    .len(),
                1
            );
        }
        let network = crate::network::NetworkClients::new(&path);
        let context = retrieve_for_chat(&path, &network, "Rust").await;
        assert!(context.contains("Rust retrieval"));
        super::super::embeddings::run_embed_queue(&path, &reqwest::Client::new())
            .await
            .unwrap();
        let conn = db::open_at(&path).unwrap();
        for table in ["chunks", "interests", "memories"] {
            assert_eq!(
                conn.query_row(&format!("SELECT COUNT(embedding) FROM {table}"), [], |r| {
                    r.get::<_, i64>(0)
                })
                .unwrap(),
                0
            );
        }
        assert!(rrss_core::vector_space::active(&conn).unwrap().is_none());
    }
}
