//! Pi RPC JSONL 的命令构造与事件解码。只按 `\n` 切行；未知事件与未知字段一律容忍。
//! 事件名与字段以兼容线（`pi/compat.json`）的 `docs/json.md`、`docs/rpc-extension-ui.md` 为准；0.87 → 1.5 未变。

use serde_json::{Value, json};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid JSONL: {0}")]
    InvalidJson(String),
    #[error("pi process: {0}")]
    Process(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// 严格 JSONL：只在 LF 处分帧，可选去掉行尾 CR。
#[derive(Debug, Default)]
pub struct JsonlDecoder {
    buf: Vec<u8>,
}

impl JsonlDecoder {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Value> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(i) = self.buf.iter().position(|&b| b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=i).collect();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if line.is_empty() {
                continue;
            }
            match serde_json::from_slice::<Value>(&line) {
                Ok(v) => out.push(v),
                Err(err) => tracing::warn!(
                    error_kind = "invalid_json",
                    line = err.line(),
                    column = err.column(),
                    "skip malformed jsonl line"
                ),
            }
        }
        out
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PiUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
}

impl PiUsage {
    pub fn context_tokens(self) -> i64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_write_tokens)
    }
}

fn usage_of(v: &Value) -> PiUsage {
    let token = |a: &str, b: &str| {
        v.get(a)
            .or_else(|| v.get(b))
            .and_then(Value::as_i64)
            .unwrap_or(0)
            .max(0)
    };
    PiUsage {
        input_tokens: token("input", "input_tokens"),
        output_tokens: token("output", "output_tokens"),
        cache_read_tokens: token("cacheRead", "cache_read_tokens"),
        cache_write_tokens: token("cacheWrite", "cache_write_tokens"),
    }
}

/// 对话需要的事件子集。其余事件落到 [`PiEvent::Other`]，调用方忽略即可。
#[derive(Debug, Clone, PartialEq)]
pub enum PiEvent {
    /// 命令响应；`id` 与请求一致。
    Response {
        id: Option<String>,
        command: String,
        success: bool,
        error: Option<String>,
        data: Value,
    },
    AgentStart,
    /// 一次底层运行结束；之后仍可能重试或压缩，结算看 [`PiEvent::AgentSettled`]。
    AgentEnd {
        will_retry: bool,
    },
    /// Pi 不会再自动继续：一轮对话到此结算。
    AgentSettled,
    TextDelta(String),
    Usage {
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: u32,
        cache_write_tokens: u32,
    },
    /// 一条消息完成。助手消息带 `stop_reason`（`stop` / `toolUse` / `error` / `aborted` …），出错时带 `error_message`。
    MessageEnd {
        role: String,
        text: String,
        stop_reason: Option<String>,
        error_message: Option<String>,
        usage: Option<PiUsage>,
    },
    /// 工具开始执行；`args` 是模型给的参数（缺省为 `Null`），只用来生成界面上的参数摘要。
    ToolStart {
        call_id: String,
        name: String,
        args: Value,
    },
    ToolEnd {
        call_id: String,
        name: String,
        is_error: bool,
    },
    CompactionStart {
        reason: String,
    },
    CompactionEnd {
        aborted: bool,
        error_message: Option<String>,
    },
    AutoRetryStart {
        attempt: u32,
        max_attempts: u32,
        error_message: String,
    },
    /// 自动重试结束；`success = false` 时 `final_error` 是最终失败原因。
    AutoRetryEnd {
        success: bool,
        final_error: Option<String>,
    },
    ExtensionError {
        message: String,
    },
    /// 扩展请求用户交互。对话框方法（`select` / `confirm` / `input` / `editor`）需要回应，其余只是通知。
    ExtensionUiRequest {
        id: String,
        method: String,
        title: String,
        message: String,
        timeout_ms: Option<u64>,
    },
    /// 宿主合成：已回收子进程；code 是真实退出码，无退出码时为 -1（信号另记日志）。
    Exited {
        code: i32,
    },
    Other(String),
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn u32_field(v: &Value, key: &str) -> u32 {
    v.get(key).and_then(Value::as_u64).unwrap_or(0) as u32
}

pub fn interpret(v: &Value) -> PiEvent {
    let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
    match ty {
        "response" => PiEvent::Response {
            id: v.get("id").and_then(Value::as_str).map(str::to_string),
            command: str_field(v, "command"),
            success: v.get("success").and_then(Value::as_bool).unwrap_or(false),
            error: v.get("error").and_then(Value::as_str).map(str::to_string),
            data: v.get("data").cloned().unwrap_or(Value::Null),
        },
        "agent_start" => PiEvent::AgentStart,
        "agent_end" => PiEvent::AgentEnd {
            will_retry: v.get("willRetry").and_then(Value::as_bool).unwrap_or(false),
        },
        "agent_settled" => PiEvent::AgentSettled,
        "message_update" => {
            let ev = &v["assistantMessageEvent"];
            if ev.get("type").and_then(Value::as_str) == Some("text_delta") {
                PiEvent::TextDelta(str_field(ev, "delta"))
            } else if let Some(usage) = v.get("usage") {
                PiEvent::Usage {
                    input_tokens: u32_field(usage, "input").max(u32_field(usage, "input_tokens")),
                    output_tokens: u32_field(usage, "output")
                        .max(u32_field(usage, "output_tokens")),
                    cache_read_tokens: u32_field(usage, "cacheRead"),
                    cache_write_tokens: u32_field(usage, "cacheWrite"),
                }
            } else {
                PiEvent::Other(ty.into())
            }
        }
        "message_end" => {
            let message = &v["message"];
            PiEvent::MessageEnd {
                role: str_field(message, "role"),
                text: message_text(message),
                stop_reason: opt_str(message, "stopReason"),
                error_message: opt_str(message, "errorMessage"),
                usage: message.get("usage").filter(|u| u.is_object()).map(usage_of),
            }
        }
        "tool_execution_start" => PiEvent::ToolStart {
            call_id: str_field(v, "toolCallId"),
            name: str_field(v, "toolName"),
            args: v.get("args").cloned().unwrap_or(Value::Null),
        },
        "tool_execution_end" => PiEvent::ToolEnd {
            call_id: str_field(v, "toolCallId"),
            name: str_field(v, "toolName"),
            is_error: v.get("isError").and_then(Value::as_bool).unwrap_or(false),
        },
        "compaction_start" => PiEvent::CompactionStart {
            reason: str_field(v, "reason"),
        },
        "compaction_end" => PiEvent::CompactionEnd {
            aborted: v.get("aborted").and_then(Value::as_bool).unwrap_or(false),
            error_message: opt_str(v, "errorMessage"),
        },
        "auto_retry_start" => PiEvent::AutoRetryStart {
            attempt: u32_field(v, "attempt"),
            max_attempts: u32_field(v, "maxAttempts"),
            error_message: str_field(v, "errorMessage"),
        },
        "auto_retry_end" => PiEvent::AutoRetryEnd {
            success: v.get("success").and_then(Value::as_bool).unwrap_or(false),
            final_error: opt_str(v, "finalError"),
        },
        "extension_error" => PiEvent::ExtensionError {
            message: opt_str(v, "error")
                .or_else(|| opt_str(v, "message"))
                .unwrap_or_default(),
        },
        "extension_ui_request" => PiEvent::ExtensionUiRequest {
            id: str_field(v, "id"),
            method: str_field(v, "method"),
            title: str_field(v, "title"),
            message: str_field(v, "message"),
            timeout_ms: v.get("timeout").and_then(Value::as_u64),
        },
        other => {
            tracing::debug!(event = other, "ignore pi event");
            PiEvent::Other(other.into())
        }
    }
}

pub fn message_text(message: &Value) -> String {
    match &message["content"] {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| {
                if p.get("type").and_then(Value::as_str) == Some("text") {
                    p.get("text").and_then(Value::as_str).map(str::to_string)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// 对话框方法：Pi 会阻塞等待 `extension_ui_response`。
pub fn is_dialog_method(method: &str) -> bool {
    matches!(method, "select" | "confirm" | "input" | "editor")
}

/// 用户输入按原文发给模型。Pi 把以 `/` 开头的提示当作扩展命令（内置的 `/llama`、`/mcp`，MCP adapter
/// 也注册命令；托管模式关掉了内置扩展，自有配置关不掉）：命令执行后不开始运行，也不会有 `agent_settled`，
/// 这一轮就停在等待里。RRSS 不提供斜杠命令，所以在开头补一个空格；Pi 只判断 `startsWith("/")`，不去空白。
pub fn cmd_prompt(message: &str) -> Value {
    let message = if message.starts_with('/') {
        format!(" {message}")
    } else {
        message.to_string()
    };
    json!({"type": "prompt", "message": message})
}

/// `prompt` 成功响应的 `data.disposition`（Pi 0.99 起）。`handled` 表示被扩展命令或输入处理器接管，
/// 不会开始运行，也不会有 `agent_settled`。
pub fn prompt_was_handled(data: &Value) -> bool {
    data.get("disposition").and_then(Value::as_str) == Some("handled")
}

pub fn cmd_abort() -> Value {
    json!({"type": "abort"})
}

pub fn cmd_new_session() -> Value {
    json!({"type": "new_session"})
}

pub fn cmd_switch_session(path: &str) -> Value {
    json!({"type": "switch_session", "sessionPath": path})
}

pub fn cmd_get_session_stats() -> Value {
    json!({"type": "get_session_stats"})
}

pub fn cmd_get_state() -> Value {
    json!({"type": "get_state"})
}

pub fn cmd_get_available_models() -> Value {
    json!({"type": "get_available_models"})
}

pub fn cmd_get_available_thinking_levels() -> Value {
    json!({"type": "get_available_thinking_levels"})
}

/// 切换当前会话的模型；不改 Pi 的默认模型设置，会话里记下切换，恢复会话时沿用。
pub fn cmd_set_model(provider: &str, model_id: &str) -> Value {
    json!({"type": "set_model", "provider": provider, "modelId": model_id})
}

pub fn cmd_set_thinking_level(level: &str) -> Value {
    json!({"type": "set_thinking_level", "level": level})
}

/// `confirm` 的回应。
pub fn ui_confirm(id: &str, confirmed: bool) -> Value {
    json!({"type": "extension_ui_response", "id": id, "confirmed": confirmed})
}

/// 任意对话框的取消回应：`confirm` 收到 `false`，其余收到 `undefined`。
pub fn ui_cancel(id: &str) -> Value {
    json!({"type": "extension_ui_response", "id": id, "cancelled": true})
}

pub fn encode_line(v: &Value) -> String {
    format!("{v}\n")
}

/// 已完成段落交给 Markdown 解析，尾巴保持纯文本。
pub fn split_committed_tail(full: &str) -> (String, String) {
    match full.rfind("\n\n") {
        Some(i) => (full[..=i + 1].to_string(), full[i + 2..].to_string()),
        None => (String::new(), full.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_never_starts_with_a_slash_command() {
        assert_eq!(cmd_prompt("/llama")["message"], " /llama");
        assert_eq!(cmd_prompt("/mcp status")["message"], " /mcp status");
        assert_eq!(cmd_prompt("路径 /tmp")["message"], "路径 /tmp");
        assert!(prompt_was_handled(&json!({"disposition": "handled"})));
        assert!(!prompt_was_handled(&json!({"disposition": "started"})));
        assert!(!prompt_was_handled(&Value::Null));
    }

    #[test]
    fn assistant_usage_includes_cache() {
        let event = interpret(
            &json!({"type":"message_end","message":{"role":"assistant","usage":{"input":100,"output":20,"cacheRead":30,"cacheWrite":40}}}),
        );
        assert!(matches!(event, PiEvent::MessageEnd {usage:Some(u),..} if u.context_tokens()==190));
        let update = interpret(
            &json!({"type":"message_update","usage":{"input":1,"output":2,"cacheRead":3,"cacheWrite":4}}),
        );
        assert!(matches!(
            update,
            PiEvent::Usage {
                input_tokens: 1,
                output_tokens: 2,
                cache_read_tokens: 3,
                cache_write_tokens: 4
            }
        ));
        assert!(matches!(
            interpret(&json!({"type":"message_end","message":{"role":"assistant"}})),
            PiEvent::MessageEnd { usage: None, .. }
        ));
    }

    #[test]
    fn splits_only_on_lf_not_unicode_separators() {
        let mut d = JsonlDecoder::default();
        let line1 = r#"{"type":"agent_start"}"#;
        let mixed = format!("{line1}\n{{\"type\":\"other\",\"s\":\"a\u{2028}b\"}}\n");
        let frames = d.push(mixed.as_bytes());
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0]["type"], "agent_start");
        assert!(frames[1]["s"].as_str().unwrap().contains('\u{2028}'));
        assert!(d.push(b"{\"type\":\"partial\"").is_empty());
        let rest = d.push(b"}\r\n");
        assert_eq!(rest.len(), 1);
    }

    #[test]
    fn split_paragraph_throttle() {
        let (c, t) = split_committed_tail("para one\n\nstill typing");
        assert_eq!(c, "para one\n\n");
        assert_eq!(t, "still typing");
        let (c2, t2) = split_committed_tail("no break yet");
        assert!(c2.is_empty());
        assert_eq!(t2, "no break yet");
    }

    #[test]
    fn dialog_methods_need_an_answer() {
        assert!(is_dialog_method("confirm") && is_dialog_method("select"));
        assert!(!is_dialog_method("notify") && !is_dialog_method("setStatus"));
        assert_eq!(ui_cancel("u")["cancelled"], true);
        assert_eq!(ui_confirm("u", false)["confirmed"], false);
    }
}
