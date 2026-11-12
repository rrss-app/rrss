//! Live OpenAI-compat embedding probe. Skipped by `cargo test --workspace`.
//!
//! ```text
//! RRSS_EMBED_API_KEY=… \
//!   RRSS_EMBED_BASE_URL=https://api.siliconflow.cn/v1 \
//!   RRSS_EMBED_MODEL=BAAI/bge-m3 \
//!   cargo test -p rrss-ai --test embed_live -- --ignored --nocapture
//! ```

use rrss_ai::{EmbedRequest, embed, http_client};
use rrss_core::db;
use rrss_core::feeds::{SubscribeRequest, subscribe};
use rrss_core::parse::ParsedEntry;
use rrss_core::rag::{self, cosine_distance};

const LIBRARY: &str = "市政厅新图书馆今日开放，开放时间延长到晚上九点。";
const LIBRARY_Q: &str = "图书馆几点开门？";
const SOCCER: &str = "足球比赛终场前补时进球，比分改写。";

#[tokio::test]
#[ignore = "needs RRSS_EMBED_API_KEY against a live OpenAI-compat endpoint"]
async fn live_openai_compat_embed_and_knn() {
    let key = std::env::var("RRSS_EMBED_API_KEY").expect("RRSS_EMBED_API_KEY");
    let base = std::env::var("RRSS_EMBED_BASE_URL")
        .unwrap_or_else(|_| "https://api.siliconflow.cn/v1".into());
    let model = std::env::var("RRSS_EMBED_MODEL").unwrap_or_else(|_| "BAAI/bge-m3".into());
    let client = http_client().expect("http client");

    let library = embed_one(&client, &base, &key, &model, LIBRARY).await;
    let query = embed_one(&client, &base, &key, &model, LIBRARY_Q).await;
    let soccer = embed_one(&client, &base, &key, &model, SOCCER).await;

    assert_eq!(library.dim, query.dim);
    assert_eq!(library.dim, soccer.dim);
    assert!(library.dim >= 256, "unexpected dim {}", library.dim);

    let d_lib = cosine_distance(&library.vector, &query.vector);
    let d_soccer = cosine_distance(&soccer.vector, &query.vector);
    println!(
        "model={model} dim={} tokens={} library-query={d_lib:.4} soccer-query={d_soccer:.4}",
        library.dim, library.tokens
    );
    assert!(
        d_lib < d_soccer,
        "library query should be closer to library article than soccer: {d_lib} vs {d_soccer}"
    );

    let dir = std::env::temp_dir().join(format!("rrss-embed-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("rrss.db");
    let conn = db::open_at(&db_path).unwrap();
    let feed = subscribe(
        &conn,
        SubscribeRequest {
            url: "https://news.example.com/rss.xml".into(),
            folder_id: None,
            as_rsshub: false,
            rsshub_instance_id: None,
        },
    )
    .unwrap();
    rrss_core::ingest::upsert_entries(
        &conn,
        feed,
        &[
            entry("lib", "市政厅新图书馆开放", "<p>开放时间延长。</p>", 1),
            entry("match", "足球补时绝杀", "<p>终场进球。</p>", 2),
        ],
        1,
    )
    .unwrap();

    let lib_id: i64 = conn
        .query_row(
            "SELECT id FROM articles WHERE title = '市政厅新图书馆开放'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    rrss_core::articles::set_read(&conn, lib_id, true).unwrap();

    store_title_embedding(&conn, "市政厅新图书馆开放", &model, &library.vector);
    store_title_embedding(&conn, "足球补时绝杀", &model, &soccer.vector);

    let hits = rag::knn_articles(&conn, &query.vector, 5).unwrap();
    assert!(!hits.is_empty(), "knn returned no rows");
    assert_eq!(hits[0].title, "市政厅新图书馆开放");

    let hybrid = rag::hybrid_search(&conn, "图书馆", Some(&query.vector), 5).unwrap();
    assert_eq!(hybrid[0].title, "市政厅新图书馆开放");
    println!("knn top={} hybrid top={}", hits[0].title, hybrid[0].title);

    let mem_id = rrss_core::memories::insert_pending_fact(&conn, "关心本地图书馆开放时间").unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM interests", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 0);
    // 转换用例由 rrss-app 覆盖；本测试只准备向量写入所需的已确认记录。
    rrss_core::memories::confirm_memory(&conn, mem_id).unwrap();
    let interest_id =
        rrss_core::interests::insert_interest(&conn, "关心本地图书馆开放时间", "from_chat")
            .unwrap();
    assert!(!rag::interest_has_embedding(&conn, interest_id).unwrap());
    let interest_vec = embed_one(&client, &base, &key, &model, "关心本地图书馆开放时间").await;
    rag::set_interest_embedding(&conn, interest_id, &interest_vec.vector).unwrap();
    assert!(rag::interest_has_embedding(&conn, interest_id).unwrap());
    println!("interest prototype stored dim={}", interest_vec.dim);

    let _ = std::fs::remove_dir_all(&dir);
}

struct LiveVec {
    vector: Vec<f32>,
    dim: usize,
    tokens: u32,
}

async fn embed_one(
    client: &reqwest::Client,
    base: &str,
    key: &str,
    model: &str,
    input: &str,
) -> LiveVec {
    let out = embed(
        client,
        &EmbedRequest {
            base_url: base.into(),
            api_key: Some(key.into()),
            model: model.into(),
            input: input.into(),
        },
    )
    .await
    .unwrap_or_else(|err| panic!("embed {input:?}: {err}"));
    LiveVec {
        dim: out.vector.len(),
        tokens: out.prompt_tokens,
        vector: out.vector,
    }
}

fn store_title_embedding(conn: &rusqlite::Connection, title: &str, model: &str, vector: &[f32]) {
    let id: i64 = conn
        .query_row("SELECT id FROM articles WHERE title = ?1", [title], |r| {
            r.get(0)
        })
        .unwrap();
    let article = rag::DueEmbedArticle {
        id,
        title: title.into(),
        summary: Some(title.into()),
        blocks_json: None,
        content_html: None,
        tier: 1,
    };
    let pending = rag::sync_article_chunks(conn, &article, article.tier).unwrap();
    assert!(!pending.is_empty());
    for chunk in pending {
        rag::set_chunk_embedding(conn, chunk.id, model, vector).unwrap();
    }
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
