//! Explicit, low-cost capability probes. A successful ordinary completion is not evidence here.

use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Protocol, completions_url, responses_url};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityProbe {
    pub tool_calling: Support,
    pub structured_output: Support,
    /// 回复里带思考内容（或计了推理 token）时为支持；否则未知，不据此判定不支持。
    #[serde(default = "unknown")]
    pub reasoning: Support,
    pub recommended_compat: Value,
}

fn unknown() -> Support {
    Support::Unknown
}

/// 成功回复里的推理证据：`reasoning_content` / `reasoning` 字段、`reasoning` 输出项或推理 token。
fn shows_reasoning(result: &ProbeReply) -> bool {
    let Ok((status, body, _)) = result else {
        return false;
    };
    if !status.is_success() {
        return false;
    }
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let text = |x: &Value| x.as_str().is_some_and(|s| !s.trim().is_empty());
    let message = &v["choices"][0]["message"];
    text(&message["reasoning_content"])
        || text(&message["reasoning"])
        || v["output"]
            .as_array()
            .is_some_and(|items| items.iter().any(|i| i["type"] == "reasoning"))
        || v["usage"]["completion_tokens_details"]["reasoning_tokens"]
            .as_i64()
            .is_some_and(|n| n > 0)
        || v["usage"]["output_tokens_details"]["reasoning_tokens"]
            .as_i64()
            .is_some_and(|n| n > 0)
}

pub async fn probe_capabilities(
    client: &Client,
    base_url: &str,
    api_key: Option<&str>,
    model: &str,
    protocol: Protocol,
) -> CapabilityProbe {
    let mut recommended_compat = json!({});
    let mut reasoning = false;
    let first = post_probe(
        client,
        base_url,
        api_key,
        protocol,
        model,
        ProbeKind::Structured(true),
    )
    .await;
    reasoning |= shows_reasoning(&first);
    let structured_output = match first {
        Ok((status, body, used)) if developer_rejected(status, &body) && used == Protocol::Chat => {
            let retry = post_probe(
                client,
                base_url,
                api_key,
                Protocol::Chat,
                model,
                ProbeKind::Structured(false),
            )
            .await;
            reasoning |= shows_reasoning(&retry);
            if let Ok((status, _, _)) = &retry {
                if status.is_success() {
                    recommended_compat["supportsDeveloperRole"] = json!(false);
                }
            }
            classify(retry, "response_format", |body, used| {
                output_text(body, used)
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                    .is_some_and(|value| value == json!({"probe":"ok"}))
            })
        }
        other => classify(other, "response_format", |body, used| {
            output_text(body, used)
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .is_some_and(|value| value == json!({"probe":"ok"}))
        }),
    };
    let tool = post_probe(client, base_url, api_key, protocol, model, ProbeKind::Tool).await;
    reasoning |= shows_reasoning(&tool);
    let tool_calling = classify(tool, "tools", |body, used| {
        serde_json::from_str::<Value>(body).ok().is_some_and(|v| {
            if used == Protocol::Responses {
                v["output"].as_array().is_some_and(|items| {
                    items
                        .iter()
                        .any(|item| item["type"] == "function_call" && item["name"] == "rrss_probe")
                })
            } else {
                v["choices"][0]["message"]["tool_calls"]
                    .as_array()
                    .is_some_and(|calls| {
                        calls
                            .iter()
                            .any(|call| call["function"]["name"] == "rrss_probe")
                    })
            }
        })
    });
    CapabilityProbe {
        tool_calling,
        structured_output,
        reasoning: if reasoning {
            Support::Supported
        } else {
            Support::Unknown
        },
        recommended_compat,
    }
}

type ProbeReply = Result<(StatusCode, String, Protocol), reqwest::Error>;

#[derive(Clone, Copy)]
enum ProbeKind {
    Structured(bool),
    Tool,
}

fn payload(model: &str, protocol: Protocol, kind: ProbeKind) -> Value {
    match kind {
        ProbeKind::Structured(developer) => structured_payload(model, protocol, developer),
        ProbeKind::Tool => tool_payload(model, protocol),
    }
}

fn classify(
    result: ProbeReply,
    rejected_field: &str,
    evidence: impl FnOnce(&str, Protocol) -> bool,
) -> Support {
    match result {
        Ok((status, body, used)) if status.is_success() => {
            if evidence(&body, used) {
                Support::Supported
            } else {
                Support::Unknown
            }
        }
        Ok((status, body, _))
            if matches!(status.as_u16(), 400 | 422)
                && (body.to_ascii_lowercase().contains(rejected_field)
                    || (rejected_field == "response_format"
                        && (body.to_ascii_lowercase().contains("json_schema")
                            || body.to_ascii_lowercase().contains("text.format")))
                    || (rejected_field == "tools"
                        && body.to_ascii_lowercase().contains("tool_choice"))) =>
        {
            Support::Unsupported
        }
        _ => Support::Unknown,
    }
}

fn developer_rejected(status: StatusCode, body: &str) -> bool {
    matches!(status.as_u16(), 400 | 422) && body.to_ascii_lowercase().contains("developer")
}

fn structured_payload(model: &str, protocol: Protocol, developer: bool) -> Value {
    let schema = json!({
        "type": "object",
        "properties": {"probe": {"type": "string", "enum": ["ok"]}},
        "required": ["probe"],
        "additionalProperties": false
    });
    if protocol == Protocol::Responses {
        json!({
            "model": model,
            "input": "Reply with exactly this JSON object: {\"probe\":\"ok\"}",
            "text": {"format": {"type":"json_schema", "name":"rrss_probe", "strict":true, "schema":schema}}
        })
    } else {
        json!({
            "model": model,
            "messages": [
                {"role": if developer {"developer"} else {"system"}, "content":"Reply with exactly the requested JSON object and no other text."},
                {"role":"user", "content":"Reply with exactly this JSON object: {\"probe\":\"ok\"}"}
            ],
            "response_format": {"type":"json_schema", "json_schema": {"name":"rrss_probe", "strict":true, "schema":schema}}
        })
    }
}

fn tool_payload(model: &str, protocol: Protocol) -> Value {
    let parameters = json!({"type":"object", "properties":{}, "additionalProperties":false});
    if protocol == Protocol::Responses {
        json!({
            "model":model,
            "input":"Call rrss_probe now. Do not answer in text.",
            "tools":[{"type":"function", "name":"rrss_probe", "description":"Capability check", "parameters":parameters}],
            "tool_choice":{"type":"function", "name":"rrss_probe"}
        })
    } else {
        json!({
            "model":model,
            "messages":[{"role":"user", "content":"Call rrss_probe now. Do not answer in text."}],
            "tools":[{"type":"function", "function":{"name":"rrss_probe", "description":"Capability check", "parameters":parameters}}],
            "tool_choice":{"type":"function", "function":{"name":"rrss_probe"}}
        })
    }
}

fn output_text(body: &str, protocol: Protocol) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    if protocol == Protocol::Responses {
        v["output"].as_array()?.iter().find_map(|item| {
            item["content"].as_array()?.iter().find_map(|part| {
                (part["type"] == "output_text")
                    .then(|| part["text"].as_str().map(str::to_string))
                    .flatten()
            })
        })
    } else {
        v["choices"][0]["message"]["content"]
            .as_str()
            .map(str::to_string)
    }
}

async fn post_probe(
    client: &Client,
    base_url: &str,
    api_key: Option<&str>,
    protocol: Protocol,
    model: &str,
    kind: ProbeKind,
) -> ProbeReply {
    let used = if protocol == Protocol::Auto {
        Protocol::Chat
    } else {
        protocol
    };
    let (status, body) = send(client, base_url, api_key, used, &payload(model, used, kind)).await?;
    if protocol == Protocol::Auto && matches!(status.as_u16(), 404 | 405) {
        let (status, body) = send(
            client,
            base_url,
            api_key,
            Protocol::Responses,
            &payload(model, Protocol::Responses, kind),
        )
        .await?;
        Ok((status, body, Protocol::Responses))
    } else {
        Ok((status, body, used))
    }
}

async fn send(
    client: &Client,
    base_url: &str,
    api_key: Option<&str>,
    protocol: Protocol,
    payload: &Value,
) -> Result<(StatusCode, String), reqwest::Error> {
    let url = match protocol {
        Protocol::Responses => responses_url(base_url),
        _ => completions_url(base_url),
    };
    let mut request = client.post(url).json(payload);
    if let Some(key) = api_key.map(str::trim).filter(|key| !key.is_empty()) {
        request = request.bearer_auth(key);
    }
    let response = request.send().await?;
    let status = response.status();
    Ok((status, response.text().await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn chat_probe_requires_protocol_evidence_and_keeps_unknown_distinct() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_partial_json(
                json!({"response_format":{"type":"json_schema"}}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices":[{"message":{"content":"{\"probe\":\"ok\"}"}}]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_partial_json(
                json!({"tool_choice":{"type":"function"}}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices":[{"message":{"content":"I cannot call tools"}}]
            })))
            .mount(&server)
            .await;
        let out =
            probe_capabilities(&Client::new(), &server.uri(), None, "m", Protocol::Chat).await;
        assert_eq!(out.structured_output, Support::Supported);
        assert_eq!(out.tool_calling, Support::Unknown);
        assert_eq!(
            out.reasoning,
            Support::Unknown,
            "no thinking in the replies"
        );
        assert_eq!(out.recommended_compat, json!({}));
    }

    #[tokio::test]
    async fn thinking_in_any_reply_marks_reasoning_supported() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices":[{"message":{"content":"{\"probe\":\"ok\"}","reasoning_content":"Let me think."}}]
            })))
            .mount(&server)
            .await;
        let out =
            probe_capabilities(&Client::new(), &server.uri(), None, "m", Protocol::Chat).await;
        assert_eq!(out.reasoning, Support::Supported);
        // 旧缓存里没有 reasoning 字段时按未知读。
        let old: CapabilityProbe = serde_json::from_value(json!({
            "tool_calling":"supported","structured_output":"supported","recommended_compat":{}
        }))
        .unwrap();
        assert_eq!(old.reasoning, Support::Unknown);
    }

    #[tokio::test]
    async fn explicit_rejections_and_developer_retry_yield_conservative_recommendation() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_partial_json(
                json!({"messages":[{"role":"developer"}]}),
            ))
            .respond_with(ResponseTemplate::new(400).set_body_string("developer role unsupported"))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_partial_json(json!({"messages":[{"role":"system"}]})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "choices":[{"message":{"content":"{\"probe\":\"ok\"}"}}]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_partial_json(
                json!({"tool_choice":{"type":"function"}}),
            ))
            .respond_with(ResponseTemplate::new(400).set_body_string("tools unsupported"))
            .mount(&server)
            .await;
        let out =
            probe_capabilities(&Client::new(), &server.uri(), None, "m", Protocol::Chat).await;
        assert_eq!(out.structured_output, Support::Supported);
        assert_eq!(out.tool_calling, Support::Unsupported);
        assert_eq!(
            out.recommended_compat,
            json!({"supportsDeveloperRole":false})
        );
    }

    #[tokio::test]
    async fn auto_fallback_uses_responses_payload_for_both_probes() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .and(body_partial_json(json!({"text":{"format":{"type":"json_schema"}}})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "output":[{"type":"message", "content":[{"type":"output_text", "text":"{\"probe\":\"ok\"}"}]}]
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/responses"))
            .and(body_partial_json(
                json!({"tool_choice":{"type":"function"}}),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "output":[{"type":"function_call", "name":"rrss_probe"}]
            })))
            .mount(&server)
            .await;
        let out =
            probe_capabilities(&Client::new(), &server.uri(), None, "m", Protocol::Auto).await;
        assert_eq!(out.structured_output, Support::Supported);
        assert_eq!(out.tool_calling, Support::Supported);
    }
}
