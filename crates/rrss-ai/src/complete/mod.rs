//! OpenAI 兼容 `chat/completions`。无工具、无会话文件。日志 target：`rrss_ai::complete`。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

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
    #[error("model admission failed: {0:?}")]
    Admission(rrss_core::error::JobFailure),
    #[error("{0}")]
    Configuration(&'static str),
    #[error("automatic model budget exhausted")]
    BudgetExhausted,
    #[error("model service cooling down")]
    ServiceCooldown,
    #[error("model endpoint busy")]
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
    #[error("model request failed: {kind:?}")]
    Request { kind: RequestKind },
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        let timeout = error.is_timeout();
        let kind = if timeout {
            RequestKind::Timeout
        } else if error.is_connect() {
            RequestKind::Connect
        } else if error.is_decode() {
            RequestKind::Decode
        } else {
            RequestKind::Other
        };
        Self::Request { kind }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    Timeout,
    Connect,
    Decode,
    Other,
}
impl RequestKind {
    pub fn code(self) -> &'static str {
        match self {
            Self::Timeout => "model_request_timeout",
            Self::Connect => "model_request_connect",
            Self::Decode => "model_request_decode",
            Self::Other => "model_request_failed",
        }
    }
}

impl Error {
    /// 服务端以 400 / 413 / 422 报输入或总长度超出上下文窗口（llama.cpp 的 `n_ctx`、OpenAI / vLLM 的
    /// `maximum context length` 等）。调用方据此缩小输入重试，而不是当作配置错误停止。
    pub fn is_context_too_long(&self) -> bool {
        matches!(self, Self::Http { status, message } if context_too_long(*status, message))
    }

    pub fn request_kind(&self) -> Option<RequestKind> {
        let Self::Request { kind } = self else {
            return None;
        };
        Some(*kind)
    }

    /// 日志不复述模型 / HTTP 的任意正文；界面错误与重试判定仍保留原信息。
    pub fn log_message(&self) -> String {
        match self {
            Self::Http { status, .. } => format!("http status {status}"),
            Self::InvalidJson(_) => "invalid model json".into(),
            Self::Content { source, .. } => source.log_message(),
            Self::Admission(_) => "model admission failed".into(),
            Self::Request { .. } => self
                .request_kind()
                .map_or("model_request_failed", RequestKind::code)
                .into(),
            _ => rrss_core::logsafe::text(&self.to_string(), 200),
        }
    }
}

pub(crate) fn check_proxy_auth(status: reqwest::StatusCode) -> Result<()> {
    if status == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED {
        return Err(Error::Http {
            status: 407,
            message: "proxy authentication failed; check proxy credentials".into(),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

    fn fallbacks(self) -> &'static [Self] {
        match self {
            Self::JsonSchema => &[Self::JsonSchema, Self::JsonObject, Self::PromptOnly],
            Self::JsonObject => &[Self::JsonObject, Self::PromptOnly],
            Self::PromptOnly => &[Self::PromptOnly],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EndpointCaps {
    format: ResponseFormat,
    supports_temperature: bool,
}

impl Default for EndpointCaps {
    fn default() -> Self {
        Self {
            format: ResponseFormat::JsonSchema,
            supports_temperature: true,
        }
    }
}

type CapsKey = (String, String, &'static str);
const MAX_CAPS_ENTRIES: usize = 256;

fn caps_cache() -> &'static Mutex<HashMap<CapsKey, EndpointCaps>> {
    static CACHE: OnceLock<Mutex<HashMap<CapsKey, EndpointCaps>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn caps_key(req: &CompleteRequest, protocol: Protocol) -> CapsKey {
    (
        req.base_url.trim().trim_end_matches('/').to_string(),
        req.model.trim().to_string(),
        protocol.as_str(),
    )
}

fn load_caps(req: &CompleteRequest, protocol: Protocol) -> EndpointCaps {
    let key = caps_key(req, protocol);
    caps_cache()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
        .copied()
        .unwrap_or_default()
}

fn store_caps(req: &CompleteRequest, protocol: Protocol, caps: EndpointCaps) {
    let key = caps_key(req, protocol);
    let mut cache = caps_cache().lock().unwrap_or_else(|e| e.into_inner());
    if cache.len() >= MAX_CAPS_ENTRIES && !cache.contains_key(&key) {
        cache.clear();
    }
    cache.insert(key, caps);
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
    complete_admitted_with_repair_until(client, req, cached, on_usage, deadline, admission, true)
        .await
}

pub async fn complete_admitted_with_repair_until(
    client: &reqwest::Client,
    req: &CompleteRequest,
    cached: Option<&str>,
    on_usage: &(dyn Fn(u32, u32) + Send + Sync),
    deadline: tokio::time::Instant,
    admission: Option<&dyn crate::admission::Admission>,
    repair_json: bool,
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
        complete_observed(client, req, on_usage, admission, repair_json),
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
    repair_json: bool,
) -> Result<CompleteResponse> {
    if req.base_url.trim().is_empty() || req.model.trim().is_empty() {
        return Err(Error::NoProvider);
    }
    match req.protocol {
        Protocol::Chat | Protocol::Responses => {
            complete_via(client, req, req.protocol, on_usage, admission, repair_json).await
        }
        Protocol::Auto => {
            match complete_via(
                client,
                req,
                Protocol::Chat,
                on_usage,
                admission,
                repair_json,
            )
            .await
            {
                Ok(mut out) => {
                    out.detected_protocol = Some("chat".into());
                    Ok(out)
                }
                Err(err) if is_unknown_endpoint(&err) => {
                    tracing::debug!(
                        "chat/completions returned 404/405; auto fallback to responses"
                    );
                    let mut out = complete_via(
                        client,
                        req,
                        Protocol::Responses,
                        on_usage,
                        admission,
                        repair_json,
                    )
                    .await?;
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
    repair_json: bool,
) -> Result<CompleteResponse> {
    let mut caps = load_caps(req, protocol);
    let formats = caps.format.fallbacks();
    for &format in formats {
        loop {
            match post_attempt(
                client,
                req,
                protocol,
                format,
                false,
                caps.supports_temperature,
                admission,
            )
            .await
            {
                Ok(body) => {
                    if format != caps.format {
                        caps.format = format;
                        store_caps(req, protocol, caps);
                    }
                    match parse_observed(protocol, &body, on_usage) {
                        Ok(out) => return Ok(out),
                        Err(err)
                            if repair_json
                                && (matches!(&err, Error::InvalidJson(_))
                                    || matches!(&err, Error::Content { source, .. } if matches!(**source, Error::InvalidJson(_)))) =>
                        {
                            tracing::warn!(model = %req.model, protocol = protocol.as_str(), format = format.as_str(), response_bytes = body.len(), error_kind = "invalid_json", "invalid json received; retrying with prompt-only format");
                            let usage = match err {
                                Error::Content { usage, .. } => usage,
                                _ => None,
                            };
                            let retry = async {
                                let body = post_attempt(
                                    client,
                                    req,
                                    protocol,
                                    ResponseFormat::PromptOnly,
                                    true,
                                    caps.supports_temperature,
                                    admission,
                                )
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
                    }
                }
                // 上下文超限换格式或去掉温度都不会变短，直接交给调用方缩小输入。
                Err(Error::Http { status, message })
                    if caps.supports_temperature
                        && !context_too_long(status, &message)
                        && is_temperature_rejected(
                            reqwest::StatusCode::from_u16(status)
                                .unwrap_or(reqwest::StatusCode::BAD_REQUEST),
                            &message,
                        ) =>
                {
                    tracing::debug!(
                        model = %req.model,
                        protocol = protocol.as_str(),
                        status = status,
                        "temperature rejected by provider; retrying without temperature"
                    );
                    caps.supports_temperature = false;
                    store_caps(req, protocol, caps);
                    continue;
                }
                Err(Error::Http { status, message })
                    if !context_too_long(status, &message)
                        && is_format_rejected(
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
                    break;
                }
                Err(err) => return Err(err),
            }
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
    send_temperature: bool,
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
                    post_responses(client, req, format, is_retry, send_temperature, admission)
                        .await?
                }
                _ => {
                    post_chat(client, req, format, is_retry, send_temperature, admission).await?
                }
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
    send_temperature: bool,
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
        ]
    });
    if send_temperature {
        payload["temperature"] = json!(if is_retry { 0.0 } else { 0.2 });
    }
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
    send_temperature: bool,
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
        "input": req.user
    });
    if send_temperature {
        payload["temperature"] = json!(if is_retry { 0.0 } else { 0.2 });
    }
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

/// 上下文超限报文的小写关键词；只在 400 / 413 / 422 上认。
const CONTEXT_TOO_LONG: &[&str] = &[
    "exceed_context_size",
    "exceeds the available context size",
    "n_ctx",
    "context_length_exceeded",
    "maximum context length",
    "context window",
    "prompt is too long",
    "input is too long",
    "too many tokens",
];

fn context_too_long(status: u16, message: &str) -> bool {
    if !matches!(status, 400 | 413 | 422) {
        return false;
    }
    let lower = message.to_lowercase();
    CONTEXT_TOO_LONG.iter().any(|marker| lower.contains(marker))
}

fn is_temperature_rejected(status: reqwest::StatusCode, body: &str) -> bool {
    if !status.is_client_error() {
        return false;
    }
    let lower = body.to_lowercase();
    lower.contains("temperature")
        && (lower.contains("unsupported")
            || lower.contains("not support")
            || lower.contains("only the default")
            || lower.contains("not allowed")
            || lower.contains("extra_forbidden")
            || lower.contains("invalid"))
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
    let body_json = serde_json::from_str::<ApiErrorBody>(body).ok();
    // 只取已知的上下文超限码补在消息后：有的服务只在 `type` / `code` 里写明，消息本身很笼统。
    let context_code = body_json.as_ref().and_then(|b| b.error.context_code());
    let parsed = body_json
        .and_then(|b| b.error.message)
        .filter(|s| !s.is_empty());
    // 先解析完整消息，再清洗、限长；JSON 转义和截断不能把密钥变成无法识别的片段。
    let clean = rrss_core::logsafe::scrub_secrets_with(
        parsed.as_deref().unwrap_or(body),
        known_secret.map(str::trim),
    );
    let mut message = if parsed.is_some() {
        clean
    } else {
        clean.chars().take(400).collect()
    };
    if let Some(code) = context_code
        && !message.to_lowercase().contains(code)
    {
        message = format!("{message} ({code})");
    }
    Error::Http {
        status: status.as_u16(),
        message,
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

    #[tokio::test]
    async fn response_format_fallback_is_cached_across_requests() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(|request: &wiremock::Request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                if body["response_format"]["type"] == "json_schema" {
                    ResponseTemplate::new(400).set_body_json(json!({
                        "error": { "message": "json_schema is not supported" }
                    }))
                } else {
                    ResponseTemplate::new(200).set_body_json(json!({
                        "choices": [{ "message": { "content": "{\"summary\":\"ok\"}" } }],
                        "usage": { "prompt_tokens": 5, "completion_tokens": 3 }
                    }))
                }
            })
            .mount(&server)
            .await;

        let client = http_client().unwrap();
        let req = CompleteRequest {
            base_url: server.uri(),
            api_key: None,
            model: "cached-fmt-model".into(),
            system: "s".into(),
            user: "u".into(),
            json_schema: Some(summary_json_schema()),
            protocol: Protocol::Chat,
        };
        complete(&client, &req).await.unwrap();
        complete(&client, &req).await.unwrap();
        // 首次试探 2 次（json_schema 400 + json_object 200），第二次直接用缓存的 json_object（仅 1 次）。
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn unsupported_temperature_retries_without_temperature_and_caches() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(|request: &wiremock::Request| {
                let body: Value = serde_json::from_slice(&request.body).unwrap();
                if body.get("temperature").is_some() {
                    ResponseTemplate::new(400).set_body_json(json!({
                        "error": { "message": "Unsupported parameter: temperature is only the default (1)" }
                    }))
                } else {
                    ResponseTemplate::new(200).set_body_json(json!({
                        "choices": [{ "message": { "content": "{\"summary\":\"no-temp\"}" } }],
                        "usage": { "prompt_tokens": 4, "completion_tokens": 2 }
                    }))
                }
            })
            .mount(&server)
            .await;

        let client = http_client().unwrap();
        let req = CompleteRequest {
            base_url: server.uri(),
            api_key: None,
            model: "reasoning-model".into(),
            system: "s".into(),
            user: "u".into(),
            json_schema: Some(summary_json_schema()),
            protocol: Protocol::Chat,
        };
        let first = complete(&client, &req).await.unwrap();
        assert_eq!(first.parsed["summary"], "no-temp");
        complete(&client, &req).await.unwrap();
        // 首次试探 2 次，第二次直接省略 temperature（共 3 次请求）。
        assert_eq!(server.received_requests().await.unwrap().len(), 3);
    }

    fn http(status: u16, message: &str) -> Error {
        Error::Http {
            status,
            message: message.into(),
        }
    }

    #[test]
    fn context_overflow_is_recognised_only_on_request_size_statuses() {
        let llama = http_error(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":{"code":400,"message":"the request exceeds the available context size, try increasing it","type":"exceed_context_size_error","n_prompt_tokens":2300,"n_ctx":2048}}"#,
            None,
        );
        assert!(llama.is_context_too_long());
        // 只在 type / code 里写明的也认，消息本身很笼统。
        let coded = http_error(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"Request rejected","type":"invalid_request_error","code":"context_length_exceeded"}}"#,
            None,
        );
        assert!(coded.is_context_too_long(), "{coded}");
        for (status, message) in [
            (
                400,
                "This model's maximum context length is 8192 tokens. However, your messages resulted in 9100 tokens.",
            ),
            (
                400,
                "This model's maximum context length is 2048 tokens. However, you requested 3000 tokens (2000 in the messages, 1000 in the completion).",
            ),
            (413, "prompt is too long: 210000 tokens > 200000 maximum"),
            (422, "Input is too long for the context window"),
            (400, "too many tokens: requested 5000, n_ctx 4096"),
        ] {
            assert!(
                http(status, message).is_context_too_long(),
                "{status} {message}"
            );
        }
        for (status, message) in [
            (401, "maximum context length exceeded"),
            (403, "context_length_exceeded"),
            (404, "model not found"),
            (429, "too many tokens per minute"),
            (500, "exceeds the available context size"),
            (400, "invalid model"),
            (400, "unsupported parameter response_format"),
            (400, "invalid_api_key"),
        ] {
            assert!(
                !http(status, message).is_context_too_long(),
                "{status} {message}"
            );
        }
        assert!(!Error::EmptyContent.is_context_too_long());
    }

    #[tokio::test]
    async fn context_overflow_is_not_retried_with_another_format_or_temperature() {
        let server = MockServer::start().await;
        // 报文里碰巧带格式 / 温度字样也不能触发降级重试。
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": {
                    "message": "maximum context length exceeded; temperature and response_format are not supported by this request",
                    "type": "exceed_context_size_error"
                }
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
        let err = complete(&client, &req).await.unwrap_err();
        assert!(err.is_context_too_long(), "{err}");
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}

#[cfg(test)]
mod deadline_tests;
