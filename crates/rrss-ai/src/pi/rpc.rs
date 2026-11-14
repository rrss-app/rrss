//! 命令与响应按 `id` 关联。只管登记、匹配和超时，不持有进程或管道。

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::oneshot;

use super::protocol::PiEvent;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RpcError {
    /// Pi 返回 `success: false`。
    #[error("Pi command rejected: {0:?}")]
    Rejected(Option<String>),
    #[error("Pi command response timed out")]
    Timeout,
    /// 进程退出或关联表被清空，响应不会再来。
    #[error("Pi process exited")]
    Closed,
}

type Reply = std::result::Result<Value, RpcError>;

/// 一条已发出、等待响应的命令。
pub struct PendingReply {
    pub id: String,
    rx: oneshot::Receiver<Reply>,
}

#[derive(Default)]
pub struct RpcCorrelator {
    pending: Mutex<HashMap<String, oneshot::Sender<Reply>>>,
    next: AtomicU64,
}

impl RpcCorrelator {
    /// 给命令分配 `rrss-<n>` id（已有 id 则沿用），登记后返回待发送的命令和等待句柄。
    pub fn register(&self, mut command: Value) -> (Value, PendingReply) {
        let id = command
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("rrss-{}", self.next.fetch_add(1, Ordering::Relaxed) + 1));
        command["id"] = Value::String(id.clone());
        let (tx, rx) = oneshot::channel();
        if let Ok(mut pending) = self.pending.lock() {
            pending.insert(id.clone(), tx);
        }
        (command, PendingReply { id, rx })
    }

    /// 是本表登记过的响应就交给等待方并返回 `true`；其余事件原样交给调用方处理。
    pub fn resolve(&self, event: &PiEvent) -> bool {
        let PiEvent::Response {
            id: Some(id),
            success,
            error,
            data,
            ..
        } = event
        else {
            return false;
        };
        let Some(sender) = self.pending.lock().ok().and_then(|mut p| p.remove(id)) else {
            return false;
        };
        let _ = sender.send(if *success {
            Ok(data.clone())
        } else {
            Err(RpcError::Rejected(error.clone()))
        });
        true
    }

    /// 等待响应；超时后撤销登记，迟到的响应不会再匹配。
    pub async fn wait(&self, reply: PendingReply, timeout: Duration) -> Reply {
        let PendingReply { id, rx } = reply;
        let result = match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err(RpcError::Closed),
            Err(_) => Err(RpcError::Timeout),
        };
        self.cancel(&id);
        result
    }

    /// 发送失败时撤销登记。
    pub fn cancel(&self, id: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(id);
        }
    }

    /// 进程退出：所有等待方立即收到 [`RpcError::Closed`]。
    pub fn close_all(&self) {
        if let Ok(mut pending) = self.pending.lock() {
            for (_, sender) in pending.drain() {
                let _ = sender.send(Err(RpcError::Closed));
            }
        }
    }

    #[cfg(test)]
    pub fn pending_len(&self) -> usize {
        self.pending.lock().map(|p| p.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn response(id: &str, success: bool) -> PiEvent {
        PiEvent::Response {
            id: Some(id.into()),
            command: "prompt".into(),
            success,
            error: (!success).then(|| "model unavailable".into()),
            data: json!({"ok": success}),
        }
    }

    #[tokio::test]
    async fn matches_by_id_and_reports_rejection() {
        let rpc = RpcCorrelator::default();
        let (cmd_a, a) = rpc.register(json!({"type": "prompt"}));
        let (cmd_b, b) = rpc.register(json!({"type": "abort"}));
        assert_ne!(cmd_a["id"], cmd_b["id"]);
        assert!(!rpc.resolve(&response("unknown", true)));
        assert!(!rpc.resolve(&PiEvent::AgentSettled));
        assert!(rpc.resolve(&response(cmd_b["id"].as_str().unwrap(), true)));
        assert!(rpc.resolve(&response(cmd_a["id"].as_str().unwrap(), false)));
        let t = Duration::from_secs(1);
        assert_eq!(
            rpc.wait(a, t).await,
            Err(RpcError::Rejected(Some("model unavailable".into())))
        );
        assert_eq!(rpc.wait(b, t).await.unwrap()["ok"], true);
        let (command, missing) = rpc.register(json!({"type": "prompt"}));
        let mut rejected = response(command["id"].as_str().unwrap(), false);
        if let PiEvent::Response { error, .. } = &mut rejected {
            *error = None;
        }
        rpc.resolve(&rejected);
        assert_eq!(rpc.wait(missing, t).await, Err(RpcError::Rejected(None)));
        assert_eq!(rpc.pending_len(), 0);
    }

    #[tokio::test]
    async fn timeout_forgets_the_request_and_exit_wakes_waiters() {
        let rpc = RpcCorrelator::default();
        let (cmd, slow) = rpc.register(json!({"type": "get_state"}));
        assert_eq!(
            rpc.wait(slow, Duration::from_millis(10)).await,
            Err(RpcError::Timeout)
        );
        assert!(
            !rpc.resolve(&response(cmd["id"].as_str().unwrap(), true)),
            "late reply is dropped"
        );

        let (_, waiting) = rpc.register(json!({"type": "prompt"}));
        rpc.close_all();
        assert_eq!(
            rpc.wait(waiting, Duration::from_secs(1)).await,
            Err(RpcError::Closed)
        );
    }
}
