//! OpenAI 兼容 `embeddings`。不经 Pi。日志 target：`rrss_ai::embed`。

use serde::Deserialize;
use serde_json::{Value, json};

use crate::complete::{Error, Result};

#[derive(Debug, Clone)]
pub struct EmbedRequest {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub input: String,
}

#[derive(Debug, Clone)]
pub struct EmbedResponse {
    pub vector: Vec<f32>,
    pub prompt_tokens: u32,
}

/// 一次请求嵌入多段文本。`inputs` 只有一条时按字符串发送，不支持数组的服务也能用。
#[derive(Debug, Clone)]
pub struct EmbedBatchRequest {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub inputs: Vec<String>,
}

/// `vectors` 与 `inputs` 一一对应，顺序相同。
#[derive(Debug, Clone)]
pub struct EmbedBatchResponse {
    pub vectors: Vec<Vec<f32>>,
    pub prompt_tokens: u32,
}

pub async fn embed(client: &reqwest::Client, req: &EmbedRequest) -> Result<EmbedResponse> {
    let out = embed_batch(
        client,
        &EmbedBatchRequest {
            base_url: req.base_url.clone(),
            api_key: req.api_key.clone(),
            model: req.model.clone(),
            inputs: vec![req.input.clone()],
        },
    )
    .await?;
    let vector = out
        .vectors
        .into_iter()
        .next()
        .ok_or(Error::EmptyEmbedding)?;
    Ok(EmbedResponse {
        vector,
        prompt_tokens: out.prompt_tokens,
    })
}

pub async fn embed_batch(
    client: &reqwest::Client,
    req: &EmbedBatchRequest,
) -> Result<EmbedBatchResponse> {
    embed_batch_until(
        client,
        req,
        tokio::time::Instant::now() + crate::deadline::ITEM_TIMEOUT,
    )
    .await
}

pub async fn embed_batch_until(
    client: &reqwest::Client,
    req: &EmbedBatchRequest,
    deadline: tokio::time::Instant,
) -> Result<EmbedBatchResponse> {
    embed_batch_admitted_until(client, req, deadline, None).await
}

pub async fn embed_batch_admitted_until(
    client: &reqwest::Client,
    req: &EmbedBatchRequest,
    deadline: tokio::time::Instant,
    admission: Option<&dyn crate::admission::Admission>,
) -> Result<EmbedBatchResponse> {
    crate::deadline::until(deadline, async {
        let _permit = match admission {
            Some(admission) => Some(admission.acquire().await?),
            None => None,
        };
        crate::deadline::until(
            tokio::time::Instant::now() + crate::deadline::REQUEST_TIMEOUT,
            embed_batch_request(client, req, admission),
        )
        .await
    })
    .await
}

async fn embed_batch_request(
    client: &reqwest::Client,
    req: &EmbedBatchRequest,
    admission: Option<&dyn crate::admission::Admission>,
) -> Result<EmbedBatchResponse> {
    if req.base_url.trim().is_empty()
        || req.model.trim().is_empty()
        || req.inputs.is_empty()
        || req.inputs.iter().any(|s| s.trim().is_empty())
    {
        return Err(Error::NoProvider);
    }
    let url = embeddings_url(&req.base_url);
    let count = req.inputs.len();
    tracing::debug!(model = %req.model, host = %rrss_core::logsafe::host_str(&url), count, "sending embedding request");
    let input = match req.inputs.as_slice() {
        [one] => json!(one),
        many => json!(many),
    };
    let mut http = client.post(&url).json(&json!({
        "model": req.model,
        "input": input,
    }));
    if let Some(key) = req
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        http = http.bearer_auth(key);
    }
    let resp = http.send().await.map_err(|err| {
        let safe = Error::from(err);
        tracing::warn!(model = %req.model, error = %safe, "embedding network request failed");
        safe
    })?;
    let status = resp.status();
    if let Some(admission) = admission {
        admission
            .response(
                status.as_u16(),
                resp.headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned),
            )
            .await?;
    }
    crate::complete::check_proxy_auth(status)?;
    let body = resp.text().await?;
    if !status.is_success() {
        tracing::warn!(model = %req.model, status = status.as_u16(), count, error_kind = "http", response_bytes = body.len(), "embedding http response error");
        return Err(crate::complete::http_error(
            status,
            &body,
            req.api_key.as_deref(),
        ));
    }
    take_embeddings(&body, count)
}

/// 批量请求被拒后是否值得拆小重试：条数 / 体积超限（400 / 413 / 422），
/// 或服务只认单条、返回条数对不上。认证、限流、5xx 与网络错误拆小也不会好。
pub fn retry_smaller(err: &Error) -> bool {
    match err {
        Error::Http { status, .. } => matches!(status, 400 | 413 | 422),
        Error::InvalidJson(_) | Error::EmptyEmbedding => true,
        _ => false,
    }
}

pub fn embeddings_url(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/embeddings")
    } else if base.ends_with("/embeddings") {
        base.to_string()
    } else {
        format!("{base}/v1/embeddings")
    }
}

fn take_embeddings(body: &str, expected: usize) -> Result<EmbedBatchResponse> {
    let parsed: EmbedEnvelope = serde_json::from_str(body)
        .map_err(|err| Error::InvalidJson(format!("embedding envelope: {err}: {body}")))?;
    if parsed.data.is_empty() {
        return Err(Error::EmptyEmbedding);
    }
    if parsed.data.len() != expected {
        return Err(Error::InvalidJson(format!(
            "expected {expected} embeddings, got {}",
            parsed.data.len()
        )));
    }
    let order = response_order(&parsed.data)?;
    let mut vectors = Vec::with_capacity(expected);
    for i in order {
        let vector = parse_vector(&parsed.data[i].embedding)?;
        if vector.is_empty() {
            return Err(Error::EmptyEmbedding);
        }
        vectors.push(vector);
    }
    let prompt_tokens = parsed
        .usage
        .as_ref()
        .map(|u| u.prompt_tokens.or(u.total_tokens).unwrap_or(0))
        .unwrap_or(0);
    tracing::debug!(
        count = vectors.len(),
        dim = vectors[0].len(),
        prompt_tokens,
        "embed ok"
    );
    Ok(EmbedBatchResponse {
        vectors,
        prompt_tokens,
    })
}

/// 返回按输入顺序排列的 `data` 下标。`index` 全缺时按数组顺序；
/// 给了就必须恰好覆盖 `0..n`，否则不敢猜对应关系。
fn response_order(data: &[EmbedData]) -> Result<Vec<usize>> {
    if data.iter().all(|d| d.index.is_none()) {
        return Ok((0..data.len()).collect());
    }
    let mut order = vec![usize::MAX; data.len()];
    for (pos, d) in data.iter().enumerate() {
        let slot = d
            .index
            .and_then(|i| usize::try_from(i).ok())
            .filter(|&i| i < data.len() && order[i] == usize::MAX)
            .ok_or_else(|| Error::InvalidJson("embedding indexes do not match inputs".into()))?;
        order[slot] = pos;
    }
    Ok(order)
}

fn parse_vector(value: &Value) -> Result<Vec<f32>> {
    let Some(arr) = value.as_array() else {
        return Err(Error::InvalidJson(
            "embedding is not a float array (base64 not supported)".into(),
        ));
    };
    arr.iter()
        .map(|v| {
            v.as_f64()
                .map(|n| n as f32)
                .ok_or_else(|| Error::InvalidJson("embedding value is not a number".into()))
        })
        .collect()
}

#[derive(Debug, Deserialize)]
struct EmbedEnvelope {
    #[serde(default)]
    data: Vec<EmbedData>,
    #[serde(default)]
    usage: Option<EmbedUsage>,
}

#[derive(Debug, Deserialize)]
struct EmbedData {
    #[serde(default)]
    index: Option<u32>,
    embedding: Value,
}

#[derive(Debug, Deserialize)]
struct EmbedUsage {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    total_tokens: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn embeddings_url_joins_v1() {
        assert_eq!(
            embeddings_url("http://127.0.0.1:11434/v1"),
            "http://127.0.0.1:11434/v1/embeddings"
        );
        assert_eq!(
            embeddings_url("http://127.0.0.1:11434/v1/"),
            "http://127.0.0.1:11434/v1/embeddings"
        );
        assert_eq!(
            embeddings_url("https://api.openai.com"),
            "https://api.openai.com/v1/embeddings"
        );
        assert_eq!(
            embeddings_url("http://gw/v1/embeddings"),
            "http://gw/v1/embeddings"
        );
    }

    #[tokio::test]
    async fn no_provider_errors_without_http() {
        let err = embed(
            &crate::http_client().unwrap(),
            &EmbedRequest {
                base_url: String::new(),
                api_key: None,
                model: "m".into(),
                input: "hi".into(),
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::NoProvider));
    }

    #[tokio::test]
    async fn posts_openai_compat_and_reads_floats() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"embedding": [0.1, 0.2, 0.3], "index": 0}],
                "usage": {"prompt_tokens": 4}
            })))
            .mount(&server)
            .await;
        let out = embed(
            &crate::http_client().unwrap(),
            &EmbedRequest {
                base_url: server.uri(),
                api_key: Some("sk-test".into()),
                model: "nomic-embed-text".into(),
                input: "hello".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(out.vector.len(), 3);
        assert!((out.vector[0] - 0.1).abs() < 1e-6);
        assert_eq!(out.prompt_tokens, 4);
    }

    fn batch_req(base_url: String, inputs: &[&str]) -> EmbedBatchRequest {
        EmbedBatchRequest {
            base_url,
            api_key: None,
            model: "m".into(),
            inputs: inputs.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[tokio::test]
    async fn batch_sends_array_and_orders_by_index() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .and(body_json(json!({"model": "m", "input": ["a", "b", "c"]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    {"embedding": [2.0], "index": 2},
                    {"embedding": [0.0], "index": 0},
                    {"embedding": [1.0], "index": 1}
                ],
                "usage": {"prompt_tokens": 9}
            })))
            .expect(1)
            .mount(&server)
            .await;
        let out = embed_batch(
            &crate::http_client().unwrap(),
            &batch_req(server.uri(), &["a", "b", "c"]),
        )
        .await
        .unwrap();
        assert_eq!(out.vectors, vec![vec![0.0], vec![1.0], vec![2.0]]);
        assert_eq!(out.prompt_tokens, 9);
    }

    #[tokio::test]
    async fn single_input_is_sent_as_string() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .and(body_json(json!({"model": "m", "input": "solo"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"embedding": [0.5]}]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let out = embed_batch(
            &crate::http_client().unwrap(),
            &batch_req(server.uri(), &["solo"]),
        )
        .await
        .unwrap();
        assert_eq!(out.vectors, vec![vec![0.5]]);
    }

    #[test]
    fn missing_indexes_keep_array_order() {
        let body = json!({"data": [{"embedding": [1.0]}, {"embedding": [2.0]}]}).to_string();
        let out = take_embeddings(&body, 2).unwrap();
        assert_eq!(out.vectors, vec![vec![1.0], vec![2.0]]);
    }

    #[test]
    fn count_or_index_mismatch_is_rejected_and_retryable() {
        let short = json!({"data": [{"embedding": [1.0], "index": 0}]}).to_string();
        let err = take_embeddings(&short, 2).unwrap_err();
        assert!(retry_smaller(&err), "{err}");

        let dup = json!({"data": [
            {"embedding": [1.0], "index": 0},
            {"embedding": [2.0], "index": 0}
        ]})
        .to_string();
        let err = take_embeddings(&dup, 2).unwrap_err();
        assert!(retry_smaller(&err), "{err}");
    }

    #[test]
    fn only_size_like_http_errors_retry_smaller() {
        let http = |status| Error::Http {
            status,
            message: String::new(),
        };
        assert!(retry_smaller(&http(400)));
        assert!(retry_smaller(&http(413)));
        assert!(!retry_smaller(&http(401)));
        assert!(!retry_smaller(&http(429)));
        assert!(!retry_smaller(&http(500)));
    }
}
