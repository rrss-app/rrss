//! OpenAI 兼容 `chat/completions`。无工具、无会话文件。日志 target：`rrss_ai::complete`。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub mod clean;
pub mod probe;
pub mod schemas;
pub mod types;

pub use clean::*;
pub use schemas::*;
use types::*;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no AI provider configured")]
    NoProvider,
    #[error("{0}")]
    Admission(&'static str),
    #[error("{0}")]
    Configuration(&'static str),
    #[error("自动模型预算已用尽")]
    BudgetExhausted,
    #[error("模型服务限流，冷却后自动重试")]
    ServiceCooldown,
    #[error("模型端点繁忙，请稍后重试")]
    Capacity,
    #[error("HTTP {status}: {message}")]
    Http { status: u16, message: String },
    #[error("empty completion")]
    EmptyContent,
    #[error("invalid completion content: {source}")]
    Content {
        #[source]
        source: Box<Error>,
        usage: Option<(u32, u32)>,
    },
    #[error("empty embedding")]
    EmptyEmbedding,
    #[error("invalid JSON: {0}")]
    InvalidJson(String),
    // 不保留 reqwest source：Debug / anyhow 错误链也不能带出代理地址或认证。
    #[error("{message}")]
    Request {
        timeout: bool,
        message: &'static str,
    },
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        let timeout = error.is_timeout();
        let message = if timeout {
            "模型请求超时"
        } else if error.is_connect() {
            "模型网络连接失败，请检查服务和代理设置"
        } else if error.is_decode() {
            "模型响应解码失败"
        } else {
            "模型网络请求失败"
        };
        Self::Request { timeout, message }
    }
}

impl Error {
    /// 日志不复述模型 / HTTP 的任意正文；界面错误与重试判定仍保留原信息。
    pub fn log_message(&self) -> String {
        match self {
            Self::Http { status, .. } => format!("http status {status}"),
            Self::InvalidJson(_) => "invalid model json".into(),
            Self::Content { source, .. } => source.log_message(),
            _ => rrss_core::logsafe::text(&self.to_string(), 200),
        }
    }
}

pub(crate) fn check_proxy_auth(status: reqwest::StatusCode) -> Result<()> {
    if status == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED {
        return Err(Error::Http {
            status: 407,
            message: "代理认证失败，请检查代理凭据".into(),
        });
    }
    Ok(())
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Protocol {
    #[default]
    Chat,
    Responses,
    Auto,
}

impl Protocol {
    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "responses" => Self::Responses,
            "auto" => Self::Auto,
            _ => Self::Chat,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Responses => "responses",
            Self::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompleteRequest {
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub system: String,
    pub user: String,
    /// JSON Schema 对象（`type: object` …）。`None` 时仍要求模型输出 JSON。
    pub json_schema: Option<Value>,
    pub protocol: Protocol,
}

#[derive(Debug, Clone)]
pub struct CompleteResponse {
    pub content: String,
    pub parsed: Value,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// 是否有可记录的真实计数；回退时只累积提供了完整计数的响应。
    pub usage_known: bool,
    pub from_cache: bool,
    /// `auto` 探测成功后写回 provider 用。
    pub detected_protocol: Option<String>,
}

#[derive(Clone, Copy)]
enum ResponseFormat {
    JsonSchema,
    JsonObject,
    PromptOnly,
}

impl ResponseFormat {
    fn as_str(self) -> &'static str {
        match self {
            Self::JsonSchema => "json_schema",
            Self::JsonObject => "json_object",
            Self::PromptOnly => "prompt_only",
        }
    }
}

pub fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?)
}

/// 缓存命中则不发网。
pub async fn complete_cached(
    client: &reqwest::Client,
    req: &CompleteRequest,
    cached: Option<&str>,
) -> Result<CompleteResponse> {
    complete_cached_with_usage(client, req, cached, &|_, _| {}).await
}

/// 每次真实响应即报告已知用量，后续回退失败 / 取消不会吞掉先前响应的计费记录。
pub async fn complete_cached_with_usage(
    client: &reqwest::Client,
    req: &CompleteRequest,
    cached: Option<&str>,
    on_usage: &(dyn Fn(u32, u32) + Send + Sync),
) -> Result<CompleteResponse> {
    complete_cached_until(
        client,
        req,
        cached,
        on_usage,
        tokio::time::Instant::now() + crate::deadline::ITEM_TIMEOUT,
    )
    .await
}

/// Protocol fallback and JSON repair consume the same absolute deadline.
pub async fn complete_cached_until(
    client: &reqwest::Client,
    req: &CompleteRequest,
    cached: Option<&str>,
    on_usage: &(dyn Fn(u32, u32) + Send + Sync),
    deadline: tokio::time::Instant,
) -> Result<CompleteResponse> {
    complete_admitted_until(client, req, cached, on_usage, deadline, None).await
}

pub async fn complete_admitted_until(
    client: &reqwest::Client,
    req: &CompleteRequest,
    cached: Option<&str>,
    on_usage: &(dyn Fn(u32, u32) + Send + Sync),
    deadline: tokio::time::Instant,
    admission: Option<&dyn crate::admission::Admission>,
) -> Result<CompleteResponse> {
    if let Some(raw) = cached {
        return Ok(CompleteResponse {
            content: raw.into(),
            parsed: parse_json_content(raw)?,
            prompt_tokens: 0,
            completion_tokens: 0,
            usage_known: false,
            from_cache: true,
            detected_protocol: None,
        });
    }
    crate::deadline::until(
        deadline,
        complete_observed(client, req, on_usage, admission),
    )
    .await
}

pub async fn complete(client: &reqwest::Client, req: &CompleteRequest) -> Result<CompleteResponse> {
    complete_cached_with_usage(client, req, None, &|_, _| {}).await
}

async fn complete_observed(
    client: &reqwest::Client,
    req: &CompleteRequest,
    on_usage: &(dyn Fn(u32, u32) + Send + Sync),
    admission: Option<&dyn crate::admission::Admission>,
) -> Result<CompleteResponse> {
    if req.base_url.trim().is_empty() || req.model.trim().is_empty() {
        return Err(Error::NoProvider);
    }
    match req.protocol {
        Protocol::Chat => complete_via(client, req, Protocol::Chat, on_usage, admission).await,
        Protocol::Responses => {
            complete_via(client, req, Protocol::Responses, on_usage, admission).await
        }
        Protocol::Auto => {
            match complete_via(client, req, Protocol::Chat, on_usage, admission).await {
                Ok(mut out) => {
                    out.detected_protocol = Some("chat".into());
                    Ok(out)
                }
                Err(err) if is_unknown_endpoint(&err) => {
                    tracing::debug!(
                        "chat/completions returned 404/405; auto fallback to responses"
                    );
                    let mut out =
                        complete_via(client, req, Protocol::Responses, on_usage, admission).await?;
                    out.detected_protocol = Some("responses".into());
                    Ok(out)
                }
                Err(err) => Err(err),
            }
        }
    }
}

fn is_unknown_endpoint(err: &Error) -> bool {
    matches!(
        err,
        Error::Http {
            status: 404 | 405,
            ..
        }
    )
}

async fn complete_via(
    client: &reqwest::Client,
    req: &CompleteRequest,
    protocol: Protocol,
    on_usage: &(dyn Fn(u32, u32) + Send + Sync),
    admission: Option<&dyn crate::admission::Admission>,
) -> Result<CompleteResponse> {
    for format in [
        ResponseFormat::JsonSchema,
        ResponseFormat::JsonObject,
        ResponseFormat::PromptOnly,
    ] {
        match post_attempt(client, req, protocol, format, false, admission).await {
            Ok(body) => match parse_observed(protocol, &body, on_usage) {
                Ok(out) => return Ok(out),
                Err(err)
                    if matches!(&err, Error::InvalidJson(_))
                        || matches!(&err, Error::Content { source, .. } if matches!(**source, Error::InvalidJson(_))) =>
                {
                    tracing::warn!(model = %req.model, protocol = protocol.as_str(), format = format.as_str(), response_bytes = body.len(), error_kind = "invalid_json", "invalid json received; retrying with prompt-only format");
                    let usage = match err {
                        Error::Content { usage, .. } => usage,
                        _ => None,
                    };
                    let retry = async {
                        let body =
                            post_attempt(client, req, protocol, ResponseFormat::PromptOnly, true, admission)
                                .await?;
                        let parsed = parse_observed(protocol, &body, on_usage);
                        if parsed.is_err() {
                            tracing::warn!(model = %req.model, protocol = protocol.as_str(), response_bytes = body.len(), error_kind = "invalid_json", "prompt-only retry still not valid json");
                        }
                        parsed
                    }
                    .await;
                    return include_usage(retry, usage);
                }
                Err(err) => return Err(err),
            },
            Err(Error::Http { status, message })
                if is_format_rejected(
                    reqwest::StatusCode::from_u16(status)
                        .unwrap_or(reqwest::StatusCode::BAD_REQUEST),
                    &message,
                ) =>
            {
                tracing::debug!(
                    model = %req.model,
                    protocol = protocol.as_str(),
                    status = status,
                    "response_format rejected by provider; falling back"
                );
                continue;
            }
            Err(err) => return Err(err),
        }
    }
    Err(Error::EmptyContent)
}

async fn post_attempt(
    client: &reqwest::Client,
    req: &CompleteRequest,
    protocol: Protocol,
    format: ResponseFormat,
    is_retry: bool,
    admission: Option<&dyn crate::admission::Admission>,
) -> Result<String> {
    let _permit = match admission {
        Some(admission) => Some(admission.acquire().await?),
        None => None,
    };
    let (status, text) = crate::deadline::until(
        tokio::time::Instant::now() + crate::deadline::REQUEST_TIMEOUT,
        async {
            Ok(match protocol {
                Protocol::Responses => {
                    post_responses(client, req, format, is_retry, admission).await?
                }
                _ => post_chat(client, req, format, is_retry, admission).await?,
            })
        },
    )
    .await?;
    if !status.is_success() {
        return Err(http_error(status, &text, req.api_key.as_deref()));
    }
    Ok(text)
}

async fn post_chat(
    client: &reqwest::Client,
    req: &CompleteRequest,
    format: ResponseFormat,
    is_retry: bool,
    admission: Option<&dyn crate::admission::Admission>,
) -> Result<(reqwest::StatusCode, String)> {
    let url = completions_url(&req.base_url);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    if let Some(key) = &req.api_key {
        let key_clean = key.trim();
        if !key_clean.is_empty() {
            let auth = format!("Bearer {key_clean}");
            if let Ok(val) = reqwest::header::HeaderValue::from_str(&auth) {
                headers.insert(reqwest::header::AUTHORIZATION, val);
            }
        }
    }

    let system = system_prompt(req, format);
    let mut payload = json!({
        "model": req.model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": req.user }
        ],
        "temperature": if is_retry { 0.0 } else { 0.2 }
    });
    if let Some(rf) = response_format_value(req, format) {
        payload["response_format"] = rf;
    }

    tracing::debug!(model = %req.model, "posting chat completion");
    let resp = client
        .post(&url)
        .headers(headers)
        .json(&payload)
        .send()
        .await?;
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
    check_proxy_auth(status)?;
    let text = resp.text().await?;
    if !status.is_success() {
        tracing::warn!(model = %req.model, status = status.as_u16(), error_kind = "http", "chat completion response error");
    }
    Ok((status, text))
}

async fn post_responses(
    client: &reqwest::Client,
    req: &CompleteRequest,
    format: ResponseFormat,
    is_retry: bool,
    admission: Option<&dyn crate::admission::Admission>,
) -> Result<(reqwest::StatusCode, String)> {
    let url = responses_url(&req.base_url);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::CONTENT_TYPE,
        reqwest::header::HeaderValue::from_static("application/json"),
    );
    if let Some(key) = &req.api_key {
        let key_clean = key.trim();
        if !key_clean.is_empty() {
            let auth = format!("Bearer {key_clean}");
            if let Ok(val) = reqwest::header::HeaderValue::from_str(&auth) {
                headers.insert(reqwest::header::AUTHORIZATION, val);
            }
        }
    }

    let system = system_prompt(req, format);
    // Responses API 的系统提示叫 `instructions`；写成 `system` 会被多数服务静默忽略，
    // 模型只看到正文、不知道要输出 JSON（表现为一段很像样的 Markdown 摘要）。
    let mut payload = json!({
        "model": req.model,
        "instructions": system,
        "input": req.user,
        "temperature": if is_retry { 0.0 } else { 0.2 }
    });
    if let Some(tf) = text_format_value(req, format) {
        payload["text"] = tf;
    }

    tracing::debug!(model = %req.model, "posting responses completion");
    let resp = client
        .post(&url)
        .headers(headers)
        .json(&payload)
        .send()
        .await?;
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
    check_proxy_auth(status)?;
    let text = resp.text().await?;
    if !status.is_success() {
        tracing::warn!(model = %req.model, status = status.as_u16(), error_kind = "http", "responses completion response error");
    }
    Ok((status, text))
}

fn system_prompt(req: &CompleteRequest, format: ResponseFormat) -> String {
    match format {
        ResponseFormat::JsonSchema => req.system.clone(),
        ResponseFormat::JsonObject => {
            format!(
                "{}\n\nStrict requirement: reply with ONLY valid JSON.",
                req.system
            )
        }
        ResponseFormat::PromptOnly => format!(
            "{}\n\nStrict requirement: reply with ONLY valid JSON wrapped in ```json ... ``` or as a raw JSON object/array. Do not add conversational text.",
            req.system
        ),
    }
}

fn is_format_rejected(status: reqwest::StatusCode, body: &str) -> bool {
    if !status.is_client_error() {
        return false;
    }
    let lower = body.to_lowercase();
    lower.contains("response_format")
        || lower.contains("json_schema")
        || lower.contains("unsupported parameter")
        || lower.contains("extra_forbidden")
        || lower.contains("schema validation")
        || lower.contains("text_format")
}

fn text_format_value(req: &CompleteRequest, format: ResponseFormat) -> Option<Value> {
    match format {
        ResponseFormat::JsonSchema => {
            let schema = req
                .json_schema
                .clone()
                .unwrap_or_else(generic_object_schema);
            Some(json!({
                "type": "json_schema",
                "name": "rrss_result",
                "strict": true,
                "schema": schema
            }))
        }
        ResponseFormat::JsonObject => Some(json!({"type": "json_object"})),
        ResponseFormat::PromptOnly => None,
    }
}

fn response_format_value(req: &CompleteRequest, format: ResponseFormat) -> Option<Value> {
    match format {
        ResponseFormat::JsonSchema => {
            let schema = req
                .json_schema
                .clone()
                .unwrap_or_else(generic_object_schema);
            Some(json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "rrss_result",
                    "strict": true,
                    "schema": schema
                }
            }))
        }
        ResponseFormat::JsonObject => Some(json!({"type": "json_object"})),
        ResponseFormat::PromptOnly => None,
    }
}

fn completions_url(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/chat/completions")
    } else if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{base}/v1/chat/completions")
    }
}

fn responses_url(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/responses")
    } else if base.ends_with("/responses") {
        base.to_string()
    } else {
        format!("{base}/v1/responses")
    }
}

fn parse_complete_body(protocol: Protocol, body: &str) -> Result<CompleteResponse> {
    match protocol {
        Protocol::Responses => take_responses_json(body),
        _ => take_json(body),
    }
}

fn parse_observed(
    protocol: Protocol,
    body: &str,
    on_usage: &(dyn Fn(u32, u32) + Send + Sync),
) -> Result<CompleteResponse> {
    let result = parse_complete_body(protocol, body);
    match &result {
        Ok(out) if out.usage_known => on_usage(out.prompt_tokens, out.completion_tokens),
        Err(Error::Content {
            usage: Some((prompt, completion)),
            ..
        }) => on_usage(*prompt, *completion),
        _ => {}
    }
    result
}

// 格式重试也可能已计费；即使后一次失败，也把前次可确认的用量交给调用者。
fn include_usage(
    result: Result<CompleteResponse>,
    usage: Option<(u32, u32)>,
) -> Result<CompleteResponse> {
    let Some((prompt, completion)) = usage else {
        return result;
    };
    match result {
        Ok(mut out) => {
            out.prompt_tokens = out.prompt_tokens.saturating_add(prompt);
            out.completion_tokens = out.completion_tokens.saturating_add(completion);
            out.usage_known = true;
            Ok(out)
        }
        Err(Error::Content { source, usage }) => {
            let (p, c) = usage.unwrap_or_default();
            Err(Error::Content {
                source,
                usage: Some((p.saturating_add(prompt), c.saturating_add(completion))),
            })
        }
        Err(source) => Err(Error::Content {
            source: Box::new(source),
            usage: Some((prompt, completion)),
        }),
    }
}

fn take_json(body: &str) -> Result<CompleteResponse> {
    let parsed: ChatCompletion = serde_json::from_str(body)
        .map_err(|err| Error::InvalidJson(format!("completion envelope: {err}: {body}")))?;
    let usage = parsed.usage.unwrap_or_default();
    let known = usage.prompt_tokens.zip(usage.completion_tokens);
    let content = parsed
        .choices
        .first()
        .and_then(|c| c.message.content.clone())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Content {
            source: Box::new(Error::EmptyContent),
            usage: known,
        })?;
    let value = parse_json_content(&content).map_err(|source| Error::Content {
        source: Box::new(source),
        usage: known,
    })?;
    tracing::debug!(
        prompt_tokens = usage.prompt_tokens.unwrap_or_default(),
        completion_tokens = usage.completion_tokens.unwrap_or_default(),
        "complete ok"
    );
    Ok(CompleteResponse {
        content,
        parsed: value,
        prompt_tokens: known.map_or(0, |(p, _)| p),
        completion_tokens: known.map_or(0, |(_, c)| c),
        usage_known: known.is_some(),
        from_cache: false,
        detected_protocol: None,
    })
}

fn take_responses_json(body: &str) -> Result<CompleteResponse> {
    let parsed: ResponsesEnvelope = serde_json::from_str(body)
        .map_err(|err| Error::InvalidJson(format!("responses envelope: {err}: {body}")))?;
    let usage = parsed.usage.unwrap_or_default();
    let known = usage.input_tokens.zip(usage.output_tokens);
    let mut texts = Vec::new();
    for item in &parsed.output {
        if item.ty != "message" {
            continue;
        }
        for part in &item.content {
            if part.ty == "output_text" {
                if let Some(text) = &part.text {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        texts.push(trimmed.to_string());
                    }
                }
            }
        }
    }
    let content = texts.join("");
    if content.is_empty() {
        return Err(Error::Content {
            source: Box::new(Error::EmptyContent),
            usage: known,
        });
    }
    let value = parse_json_content(&content).map_err(|source| Error::Content {
        source: Box::new(source),
        usage: known,
    })?;
    tracing::debug!(
        prompt_tokens = usage.input_tokens.unwrap_or_default(),
        completion_tokens = usage.output_tokens.unwrap_or_default(),
        "responses ok"
    );
    Ok(CompleteResponse {
        content,
        parsed: value,
        prompt_tokens: known.map_or(0, |(p, _)| p),
        completion_tokens: known.map_or(0, |(_, c)| c),
        usage_known: known.is_some(),
        from_cache: false,
        detected_protocol: None,
    })
}

pub(crate) fn http_error(
    status: reqwest::StatusCode,
    body: &str,
    known_secret: Option<&str>,
) -> Error {
    let parsed = serde_json::from_str::<ApiErrorBody>(body)
        .ok()
        .and_then(|b| b.error.message)
        .filter(|s| !s.is_empty());
    // 先解析完整消息，再清洗、限长；JSON 转义和截断不能把密钥变成无法识别的片段。
    let clean = rrss_core::logsafe::scrub_secrets_with(
        parsed.as_deref().unwrap_or(body),
        known_secret.map(str::trim),
    );
    let message = if parsed.is_some() {
        clean
    } else {
        clean.chars().take(400).collect()
    };
    Error::Http {
        status: status.as_u16(),
        message,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestChatResult {
    pub ok: bool,
    pub reply: String,
    pub latency_ms: u64,
    pub error: Option<String>,
}

pub async fn test_chat(
    client: &reqwest::Client,
    base_url: &str,
    api_key: Option<&str>,
    model: &str,
    protocol: Protocol,
) -> TestChatResult {
    let start = std::time::Instant::now();
    let req = CompleteRequest {
        base_url: base_url.to_string(),
        api_key: api_key.map(str::to_string),
        model: model.to_string(),
        system: "You are a test helper. Reply with ONLY valid JSON: {\"reply\":\"pong\"}".into(),
        user: "ping".into(),
        json_schema: Some(json!({
            "type": "object",
            "properties": { "reply": { "type": "string" } },
            "required": ["reply"]
        })),
        protocol,
    };
    match complete(client, &req).await {
        Ok(res) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            let reply = res
                .parsed
                .get("reply")
                .and_then(Value::as_str)
                .unwrap_or(&res.content)
                .to_string();
            TestChatResult {
                ok: true,
                reply,
                latency_ms,
                error: None,
            }
        }
        Err(err) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            TestChatResult {
                ok: false,
                reply: String::new(),
                latency_ms,
                error: Some(err.to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn strip_thinking_tags() {
        assert_eq!(strip_thinking("<think>foo bar</think>hello"), "hello");
        assert_eq!(
            strip_thinking("<think>foo\nbar\n</think>\n\nhello world\n"),
            "hello world"
        );
        assert_eq!(strip_thinking("no think"), "no think");
        assert_eq!(strip_thinking("<think>only think"), "");
    }

    #[tokio::test]
    async fn no_provider_errors_without_http() {
        let client = http_client().unwrap();
        let req = CompleteRequest {
            base_url: String::new(),
            api_key: None,
            model: "test".into(),
            system: "s".into(),
            user: "u".into(),
            json_schema: None,
            protocol: Protocol::Chat,
        };
        let err = complete(&client, &req).await.unwrap_err();
        assert!(matches!(err, Error::NoProvider));
    }

    #[tokio::test]
    async fn cache_hit_does_not_call_server() {
        let client = http_client().unwrap();
        let req = CompleteRequest {
            base_url: "http://127.0.0.1:9".into(),
            api_key: None,
            model: "m".into(),
            system: "s".into(),
            user: "u".into(),
            json_schema: None,
            protocol: Protocol::Chat,
        };
        let out = complete_cached(&client, &req, Some("{\"summary\":\"cached\"}"))
            .await
            .unwrap();
        assert!(out.from_cache);
        assert_eq!(out.parsed.get("summary").unwrap(), "cached");
    }

    #[tokio::test]
    async fn json_schema_fallback_to_json_object() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": { "message": "unsupported parameter response_format" }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{
                    "message": { "content": "{\"summary\":\"ok\"}" }
                }],
                "usage": { "prompt_tokens": 10, "completion_tokens": 5 }
            })))
            .mount(&server)
            .await;

        let client = http_client().unwrap();
        let req = CompleteRequest {
            base_url: server.uri(),
            api_key: None,
            model: "m".into(),
            system: "s".into(),
            user: "u".into(),
            json_schema: Some(summary_json_schema()),
            protocol: Protocol::Chat,
        };
        let out = complete(&client, &req).await.unwrap();
        assert_eq!(out.parsed.get("summary").unwrap(), "ok");
        assert_eq!(out.prompt_tokens, 10);
    }

    #[tokio::test]
    async fn invalid_json_retries_once() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{
                    "message": { "content": "sorry I cannot answer as JSON" }
                }],
                "usage": { "prompt_tokens": 5, "completion_tokens": 5 }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{
                    "message": { "content": "{\"summary\":\"retried\"}" }
                }],
                "usage": { "prompt_tokens": 6, "completion_tokens": 6 }
            })))
            .mount(&server)
            .await;

        let client = http_client().unwrap();
        let req = CompleteRequest {
            base_url: server.uri(),
            api_key: None,
            model: "m".into(),
            system: "s".into(),
            user: "u".into(),
            json_schema: None,
            protocol: Protocol::Chat,
        };
        let out = complete(&client, &req).await.unwrap();
        assert_eq!(out.parsed.get("summary").unwrap(), "retried");
        assert!(out.usage_known);
        assert_eq!((out.prompt_tokens, out.completion_tokens), (11, 11));
    }

    #[test]
    fn unknown_or_incomplete_usage_is_not_a_known_zero() {
        for usage in [
            json!(null),
            json!({}),
            json!({"prompt_tokens":10,"input_tokens":10}),
        ] {
            let chat = take_json(
                &json!({"choices":[{"message":{"content":"{}"}}],"usage":usage}).to_string(),
            )
            .unwrap();
            let responses = take_responses_json(&json!({"output":[{"type":"message","content":[{"type":"output_text","text":"{}"}]}],"usage":usage}).to_string()).unwrap();
            for out in [chat, responses] {
                assert!(!out.usage_known);
                assert_eq!((out.prompt_tokens, out.completion_tokens), (0, 0));
            }
        }
        let zero = take_json(&json!({"choices":[{"message":{"content":"{}"}}],"usage":{"prompt_tokens":0,"completion_tokens":0}}).to_string()).unwrap();
        assert!(
            zero.usage_known,
            "an actual zero response is distinct from absent usage"
        );
    }

    #[tokio::test]
    async fn responses_protocol_reads_output_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "output": [
                    {
                        "type": "message",
                        "content": [
                            { "type": "output_text", "text": "{\"body\":\"resp-ok\"}" }
                        ]
                    }
                ],
                "usage": { "input_tokens": 12, "output_tokens": 8 }
            })))
            .mount(&server)
            .await;

        let client = http_client().unwrap();
        let req = CompleteRequest {
            base_url: server.uri(),
            api_key: None,
            model: "claude-3-5".into(),
            system: "s".into(),
            user: "u".into(),
            json_schema: None,
            protocol: Protocol::Responses,
        };
        let out = complete(&client, &req).await.unwrap();
        assert_eq!(out.parsed.get("body").unwrap(), "resp-ok");
        assert_eq!(out.prompt_tokens, 12);
        assert_eq!(out.completion_tokens, 8);
    }

    #[tokio::test]
    async fn responses_send_the_system_prompt_as_instructions() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .and(body_partial_json(json!({ "instructions": "只输出 JSON", "input": "正文" })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "output": [{ "type": "message", "content": [{ "type": "output_text", "text": "{\"body\":\"ok\"}" }] }]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let req = CompleteRequest {
            base_url: server.uri(),
            api_key: None,
            model: "m".into(),
            system: "只输出 JSON".into(),
            user: "正文".into(),
            json_schema: None,
            protocol: Protocol::Responses,
        };
        let out = complete(&http_client().unwrap(), &req).await.unwrap();
        assert_eq!(out.parsed.get("body").unwrap(), "ok");
    }

    #[tokio::test]
    async fn auto_falls_back_from_missing_chat_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "output": [
                    {
                        "type": "message",
                        "content": [
                            { "type": "output_text", "text": "{\"body\":\"fell back\"}" }
                        ]
                    }
                ]
            })))
            .mount(&server)
            .await;

        let client = http_client().unwrap();
        let req = CompleteRequest {
            base_url: server.uri(),
            api_key: None,
            model: "m".into(),
            system: "s".into(),
            user: "u".into(),
            json_schema: None,
            protocol: Protocol::Auto,
        };
        let out = complete(&client, &req).await.unwrap();
        assert_eq!(out.parsed.get("body").unwrap(), "fell back");
        assert_eq!(out.detected_protocol.as_deref(), Some("responses"));
    }
}

#[cfg(test)]
mod deadline_tests;
