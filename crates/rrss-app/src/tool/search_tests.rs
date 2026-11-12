use super::*;
use rrss_core::network::{self, ProxyConfig, ProxyRule};
use rrss_core::vector_space;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Fixture {
    conn: db::PooledConnection,
    path: PathBuf,
    _dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("search.db");
        let conn = db::open_at(&path).unwrap();
        db::set_setting(&conn, "pi_auto_download", "0").unwrap();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,title,created_at) VALUES(1,'https://fixture.invalid/feed','Fixture',0);
             INSERT INTO articles(id,feed_id,title,summary,published_at,created_at) VALUES
                 (1,1,'Rust local text','Rust fixture',10,0),
                 (2,1,'Orbital mechanics','Unrelated content',20,0);
             INSERT INTO chunks(id,article_id,ordinal,text) VALUES(1,2,0,'Unrelated content');",
        )
        .unwrap();
        Self {
            conn,
            path,
            _dir: dir,
        }
    }

    fn with_embedding(self, url: String) -> Self {
        let provider = crate::test_net::fake_provider(&self.conn, "fixture", url);
        ai_store::set_task_binding(&self.conn, "embed", Some(provider), Some("fixture")).unwrap();
        let ep = ai_store::resolve_embed(&self.conn).unwrap().unwrap();
        db::set_setting(&self.conn, "embedding_dims", "2").unwrap();
        vector_space::ensure_for_embedding(&self.conn, &ep, 4)
            .unwrap()
            .unwrap();
        rag::set_chunk_embedding(&self.conn, 1, "fixture", &[0.6, 0.8]).unwrap();
        self
    }

    async fn invoke(&self, command: &str, query: &str, timeout: Duration) -> Value {
        let path = self.path.clone();
        let args = ["rrss", "tool", command, "--query", query, "--limit", "10"].map(str::to_owned);
        // 工具自己建 current_thread runtime，不能在测试 runtime 内嵌套 block_on。
        let output = tokio::task::spawn_blocking(move || {
            let conn = db::open_at(&path).unwrap();
            run_with_connection(&args, &conn, timeout)
        })
        .await
        .unwrap()
        .expect("embedding failure must remain Ok, which run maps to exit code 0");
        serde_json::from_str(&output).expect("tool output must contain JSON only")
    }

    fn text_articles(&self, query: &str) -> Value {
        let hits = search::search_articles_for_retrieval(&self.conn, query, 10).unwrap();
        json!(
            hits.iter()
                .map(|h| json!({
                    "id": h.id, "title": h.title, "feed_title": h.feed_title,
                    "published_at": h.published_at, "score": h.score, "snippet": h.snippet,
                }))
                .collect::<Vec<_>>()
        )
    }
}

fn vector_reply() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "data": [{"index": 0, "embedding": [3.0, 4.0, 10.0, 20.0]}],
        "usage": {"prompt_tokens": 777},
    }))
}

#[tokio::test]
async fn articles_find_vector_only_hits_through_api_proxy_without_budget_or_usage() {
    let api = MockServer::start().await;
    let unused = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .and(body_json(
            json!({"model": "fixture", "input": "unmatched-query"}),
        ))
        .respond_with(vector_reply())
        .expect(1)
        .mount(&api)
        .await;
    // 非回环目标，确保请求确实经 Api 代理，其他用途各自独立。
    let fixture = Fixture::new().with_embedding("http://embedding.example".into());
    network::save(
        &fixture.conn,
        &ProxyConfig {
            global: ProxyRule::custom(unused.uri()),
            content: ProxyRule::custom(unused.uri()),
            api: ProxyRule::custom(api.uri()),
            pi: ProxyRule::custom(unused.uri()),
            ..Default::default()
        },
    )
    .unwrap();
    let day = ai_store::local_day();
    db::set_setting(&fixture.conn, "ai_embed_daily_budget_tokens", "1").unwrap();
    ai_store::add_embed_usage(&fixture.conn, &day, 1).unwrap();
    assert_eq!(fixture.text_articles("unmatched-query"), json!([]));
    let rows = fixture
        .invoke("search_articles", "unmatched-query", QUERY_EMBED_TIMEOUT)
        .await;
    assert_eq!(
        rows,
        json!([{
            "id": 2, "title": "Orbital mechanics", "feed_title": "Fixture",
            "published_at": 20, "score": 0, "snippet": "Unrelated content",
        }])
    );
    let usage = ai_store::usage_on(&fixture.conn, &day).unwrap();
    assert_eq!(usage.embed_tokens, 1);
    assert_eq!(usage.tokens, 0);
    assert!(unused.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn missing_endpoint_preserves_text_results_with_existing_vectors() {
    let fixture = Fixture::new().with_embedding("http://unused.invalid".into());
    fixture
        .conn
        .execute(
            "UPDATE task_bindings SET provider_id=NULL, model=NULL WHERE task='embed'",
            [],
        )
        .unwrap();
    assert!(vector_space::active(&fixture.conn).unwrap().is_some());
    assert_eq!(
        fixture
            .invoke("search_articles", "Rust", QUERY_EMBED_TIMEOUT)
            .await,
        fixture.text_articles("Rust")
    );
}

async fn assert_embedding_failure_falls_back(reply: ResponseTemplate, timeout: Duration) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(reply)
        .expect(1)
        .mount(&server)
        .await;
    let fixture = Fixture::new().with_embedding(server.uri());
    // 成功嵌入会额外命中 id=2；这里只允许原有全文结果。
    assert_eq!(
        fixture.invoke("search_articles", "Rust", timeout).await,
        fixture.text_articles("Rust")
    );
}

#[tokio::test]
async fn service_error_keeps_successful_text_output() {
    assert_embedding_failure_falls_back(ResponseTemplate::new(503), QUERY_EMBED_TIMEOUT).await;
}

#[tokio::test]
async fn query_deadline_keeps_successful_text_output() {
    assert_embedding_failure_falls_back(
        vector_reply().set_delay(Duration::from_secs(3)),
        Duration::from_millis(200),
    )
    .await;
}

#[tokio::test]
async fn empty_embedding_keeps_successful_text_output() {
    assert_embedding_failure_falls_back(
        ResponseTemplate::new(200).set_body_json(json!({"data": [{"index": 0, "embedding": []}]})),
        QUERY_EMBED_TIMEOUT,
    )
    .await;
}

#[tokio::test]
async fn rebuilding_keeps_query_embedding_on_the_active_space_endpoint() {
    let server = MockServer::start().await;
    let other = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .and(body_json(
            json!({"model": "fixture", "input": "unmatched-query"}),
        ))
        .respond_with(vector_reply())
        .expect(1)
        .mount(&server)
        .await;
    let fixture = Fixture::new().with_embedding(server.uri());
    let provider = crate::test_net::fake_provider(&fixture.conn, "replacement", other.uri());
    db::set_setting(&fixture.conn, vector_space::ALLOW_MODEL_CHANGE, "1").unwrap();
    ai_store::set_task_binding(
        &fixture.conn,
        "embed",
        Some(provider),
        Some("replacement-model"),
    )
    .unwrap();
    assert!(vector_space::building(&fixture.conn).unwrap().is_some());
    assert_eq!(fixture.text_articles("unmatched-query"), json!([]));
    let rows = fixture
        .invoke("search_articles", "unmatched-query", QUERY_EMBED_TIMEOUT)
        .await;
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["id"], 2);
    assert!(other.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn memory_finds_vector_only_hits() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(vector_reply())
        .expect(1)
        .mount(&server)
        .await;
    let fixture = Fixture::new().with_embedding(server.uri());
    let id = memories::insert_memory(
        &fixture.conn,
        memories::MemoryKind::Semantic,
        "Remember orbital periods",
        "test",
        1.0,
    )
    .unwrap();
    rag::set_memory_embedding(&fixture.conn, id, &[0.6, 0.8]).unwrap();
    assert!(
        memories::search_memory(&fixture.conn, "unmatched-query", 20)
            .unwrap()
            .is_empty()
    );
    let rows = fixture
        .invoke("search_memory", "unmatched-query", QUERY_EMBED_TIMEOUT)
        .await;
    assert_eq!(
        rows,
        json!([{
            "id": id, "kind": "semantic", "content": "Remember orbital periods",
            "pinned": false, "pending": false,
        }])
    );
}
