//! One owner waits/reaps the child. Pipe tasks drain before publishing Exited,
//! so final text and the diagnostic tail cannot arrive after turn settlement.
use super::{ChatError, ChatErrorKind, log_stderr};
use anyhow::Result;
use rrss_core::{Error, error::PiFailure};
use rrss_ai::pi::{self, JsonlDecoder, PiEvent, RpcCorrelator, encode_line, rpc::RpcError};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex as StdMutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin},
    sync::{Mutex, mpsc, watch},
};
use tracing::instrument::WithSubscriber;

const STDERR_LINES: usize = 20;
const STDERR_LINE_BYTES: usize = 8192;
const PIPE_GRACE: Duration = Duration::from_secs(1);

pub struct PiProcess {
    stdin: Mutex<Option<ChildStdin>>,
    rpc: Arc<RpcCorrelator>,
    exited: Arc<AtomicBool>,
    shutdown: watch::Sender<Option<Duration>>,
    finished: watch::Receiver<bool>,
}

impl PiProcess {
    pub async fn send(&self, value: &serde_json::Value) -> Result<()> {
        let mut stdin = self.stdin.lock().await;
        let stdin = stdin.as_mut().ok_or(Error::Pi(PiFailure::InputClosed))?;
        stdin.write_all(encode_line(value).as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    pub async fn request(
        &self,
        command: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, RpcError> {
        let (command, reply) = self.rpc.register(command);
        if let Err(error) = self.send(&command).await {
            self.rpc.cancel(&reply.id);
            tracing::warn!(error = %crate::log::error(error.as_ref()), "write pi command failed");
            return Err(RpcError::Closed);
        }
        self.rpc.wait(reply, timeout).await
    }

    pub fn is_alive(&self) -> bool {
        !self.exited.load(Ordering::SeqCst)
    }

    pub async fn shutdown(&self, grace: Duration) {
        // Set intent before closing stdin; a quick orderly exit must not look
        // unexpected to the process owner.
        self.shutdown.send_if_modified(|requested| {
            if requested.is_some() {
                false
            } else {
                *requested = Some(grace);
                true
            }
        });
        self.stdin.lock().await.take();
        let _ = self.finished.clone().wait_for(|done| *done).await;
    }

    pub(super) fn watch(
        mut child: Child,
        secrets: Vec<String>,
        events: mpsc::UnboundedSender<PiEvent>,
    ) -> Result<Self, ChatError> {
        let process_id = child.id();
        let (Some(stdin), Some(stdout), Some(stderr)) =
            (child.stdin.take(), child.stdout.take(), child.stderr.take())
        else {
            return Err(ChatError::new(
                ChatErrorKind::Internal,
                Error::Pi(PiFailure::PipeMissing),
            ));
        };
        let rpc = Arc::new(RpcCorrelator::default());
        let exited = Arc::new(AtomicBool::new(false));
        let (shutdown, mut requested) = watch::channel(None);
        let (done, finished) = watch::channel(false);
        let process = Self {
            stdin: Mutex::new(Some(stdin)),
            rpc: rpc.clone(),
            exited: exited.clone(),
            shutdown,
            finished,
        };
        let tail = Arc::new(StdMutex::new(VecDeque::with_capacity(STDERR_LINES)));
        let stderr_tail = tail.clone();
        let mut stderr_task = tokio::spawn(
            read_stderr(stderr, secrets.clone(), stderr_tail).with_current_subscriber(),
        );
        let mut stdout_task = tokio::spawn(
            read_stdout(stdout, rpc.clone(), events.clone(), secrets).with_current_subscriber(),
        );
        tokio::spawn(async move {
            let mut stdout_done = false;
            let (status, intentional) = tokio::select! {
                biased;
                result = child.wait() => (result, requested.borrow().is_some() || requested.has_changed().is_err()),
                _ = requested.changed() => {
                    let grace = (*requested.borrow()).unwrap_or(Duration::ZERO);
                    (wait_or_kill(&mut child, grace, true).await, true)
                },
                result = &mut stdout_task => {
                    stdout_done = true;
                    if let Err(error) = result {
                        tracing::error!(error = %crate::log::error(&error), "pi stdout reader failed");
                    }
                    exited.store(true, Ordering::SeqCst);
                    rpc.close_all();
                    // EOF can race the real exit; give it time to report its
                    // status. A still-running process with no RPC output is
                    // unusable and must not survive into the next generation.
                    let status = wait_or_kill(&mut child, PIPE_GRACE, false).await;
                    (status, requested.borrow().is_some() || requested.has_changed().is_err())
                },
            };
            exited.store(true, Ordering::SeqCst);
            // Inherited pipes must not keep shutdown or chat settlement waiting
            // indefinitely. Abort readers only after a bounded drain period.
            let mut stderr_done = false;
            let drain = async {
                if !stdout_done { let _ = (&mut stdout_task).await; stdout_done = true; }
                let _ = (&mut stderr_task).await;
                stderr_done = true;
            };
            if tokio::time::timeout(PIPE_GRACE, drain).await.is_err() {
                if !stdout_done { stdout_task.abort(); let _ = stdout_task.await; }
                if !stderr_done { stderr_task.abort(); let _ = stderr_task.await; }
            }
            rpc.close_all();
            let code = status.as_ref().ok().and_then(|status| status.code()).unwrap_or(-1);
            match status {
                Ok(status) if !intentional => {
                    let exit_status = rrss_core::platform::exit_description(status);
                    let tail = tail.lock().unwrap();
                    tracing::warn!(process_id, error_kind = "process_exit", %exit_status, stderr_lines = tail.len(), "pi exited unexpectedly");
                    // One bounded event per retained line avoids formatter
                    // truncation swallowing the last diagnostic lines.
                    for (index, line) in tail.iter().enumerate() {
                        tracing::warn!(process_id, error_kind = "process_exit", index, %line, "pi exit stderr");
                    }
                },
                Ok(status) => tracing::debug!(process_id, exit_status = %rrss_core::platform::exit_description(status), "pi stopped"),
                Err(error) => tracing::warn!(process_id, error_kind = "wait", error = %crate::log::error(&error), "pi exit status unavailable"),
            }
            let _ = events.send(PiEvent::Exited { code });
            done.send_replace(true);
        }.with_current_subscriber());
        Ok(process)
    }
}

async fn wait_or_kill(
    child: &mut Child,
    grace: Duration,
    intentional: bool,
) -> std::io::Result<std::process::ExitStatus> {
    if let Ok(result) = tokio::time::timeout(grace, child.wait()).await {
        return result;
    }
    if intentional && !grace.is_zero() {
        tracing::warn!(
            error_kind = "shutdown_timeout",
            "pi did not exit after stdin closed; killing"
        );
    } else if !intentional {
        tracing::warn!(
            error_kind = "stdout_closed",
            "pi stdout closed while process was still running; killing"
        );
    }
    child.start_kill()?;
    child.wait().await
}

async fn read_stdout(
    mut stdout: impl AsyncRead + Unpin,
    rpc: Arc<RpcCorrelator>,
    events: mpsc::UnboundedSender<PiEvent>,
    secrets: Vec<String>,
) {
    let mut decoder = JsonlDecoder::default();
    let mut buf = [0u8; 8192];
    loop {
        match stdout.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                for value in decoder.push(&buf[..n]) {
                    let mut event = pi::interpret(&value);
                    // 在进入 RPC 等待方或会话状态前清洗错误字段；正文与工具参数不作文本替换。
                    let error = match &mut event {
                        PiEvent::Response { error, .. }
                        | PiEvent::MessageEnd {
                            error_message: error,
                            ..
                        }
                        | PiEvent::CompactionEnd {
                            error_message: error,
                            ..
                        }
                        | PiEvent::AutoRetryEnd {
                            final_error: error, ..
                        } => error.as_mut(),
                        PiEvent::AutoRetryStart { error_message, .. }
                        | PiEvent::ExtensionError {
                            message: error_message,
                        } => Some(error_message),
                        _ => None,
                    };
                    if let Some(error) = error {
                        *error = rrss_core::logsafe::scrub_secrets_with(
                            error,
                            secrets.iter().map(String::as_str),
                        );
                    }
                    if !rpc.resolve(&event) {
                        let _ = events.send(event);
                    }
                }
            }
            Err(error) => {
                tracing::error!(error = %crate::log::error(&error), "pi stdout read failed");
                break;
            }
        }
    }
}

async fn read_stderr(
    mut stderr: impl AsyncRead + Unpin,
    secrets: Vec<String>,
    tail: Arc<StdMutex<VecDeque<String>>>,
) {
    let mut buffer = [0u8; 4096];
    let mut line = Vec::new();
    let mut oversized = false;
    let retain = |line: &[u8], oversized: bool| {
        // Discard the entire oversized line: cutting before redaction can leave
        // a fragment of a known secret at the truncation boundary.
        let safe = if oversized {
            log_stderr("<oversized stderr line omitted>", &[])
        } else {
            log_stderr(&String::from_utf8_lossy(line), &secrets)
        };
        let mut tail = tail.lock().unwrap();
        if tail.len() == STDERR_LINES {
            tail.pop_front();
        }
        tail.push_back(safe);
    };
    loop {
        match stderr.read(&mut buffer).await {
            Ok(0) => break,
            Ok(n) => {
                for byte in &buffer[..n] {
                    if *byte == b'\n' {
                        retain(&line, oversized);
                        line.clear();
                        oversized = false;
                    } else if line.len() < STDERR_LINE_BYTES {
                        line.push(*byte);
                    } else {
                        oversized = true;
                    }
                }
            }
            Err(error) => {
                tracing::debug!(error = %crate::log::error(&error), "pi stderr read failed");
                break;
            }
        }
    }
    if !line.is_empty() || oversized {
        retain(&line, oversized);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Stdio;

    fn spawn(script: &str, secrets: Vec<String>) -> (PiProcess, mpsc::UnboundedReceiver<PiEvent>) {
        let child = tokio::process::Command::new("python3")
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let (send, receive) = mpsc::unbounded_channel();
        (PiProcess::watch(child, secrets, send).unwrap(), receive)
    }

    async fn exit(events: &mut mpsc::UnboundedReceiver<PiEvent>) -> (i32, Vec<PiEvent>) {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut previous = Vec::new();
            loop {
                let event = events.recv().await.unwrap();
                if let PiEvent::Exited { code } = event {
                    return (code, previous);
                }
                previous.push(event);
            }
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn stdout_cleans_known_credentials_before_rpc_and_chat_receive_errors() {
        use serde_json::json;
        let secret = "xy";
        let error = format!("HTTP 401 {secret}\nretry later");
        let rpc = Arc::new(RpcCorrelator::default());
        let (_, reply) = rpc.register(json!({"type": "prompt"}));
        let raw = [
            json!({"type":"response", "id":reply.id, "success":false, "error":error}),
            json!({"type":"message_end", "message":{
                "role":"assistant", "content":"partial answer", "stopReason":"error", "errorMessage":error
            }}),
            json!({"type":"auto_retry_end", "success":false, "finalError":error}),
        ].iter().map(encode_line).collect::<String>();
        let (send, mut receive) = mpsc::unbounded_channel();
        read_stdout(raw.as_bytes(), rpc.clone(), send, vec![secret.into()]).await;
        let error = rpc.wait(reply, Duration::from_secs(1)).await.unwrap_err();
        let RpcError::Rejected(message) = error else {
            panic!("wrong RPC error")
        };
        assert!(message.starts_with("HTTP 401") && message.contains("\nretry later"));
        assert!(!message.contains(secret));
        let PiEvent::MessageEnd {
            text,
            error_message: Some(message),
            ..
        } = receive.recv().await.unwrap()
        else {
            panic!("missing chat error")
        };
        assert_eq!(text, "partial answer");
        assert!(message.contains("401") && !message.contains(secret));
        let PiEvent::AutoRetryEnd {
            success: false,
            final_error: Some(message),
        } = receive.recv().await.unwrap()
        else {
            panic!("missing retry error")
        };
        assert!(message.contains("401") && !message.contains(secret));
    }

    #[tokio::test]
    async fn unexpected_exit_keeps_real_status_final_events_and_safe_last_twenty_lines() {
        let dir = tempfile::tempdir().unwrap();
        let (subscriber, handle) = crate::log::subscriber(
            crate::log::Mode::Desktop {
                log_dir: dir.path().into(),
            },
            "",
        );
        async {
            let (process, mut events) = spawn(r#"
import sys
for i in range(30):
    sys.stderr.write(f'fixture-{i:02d} custom-canary-key https://user:proxy-canary@example.test/a?token=query-canary\n')
sys.stderr.flush()
print('{"type":"fixture-final"}', flush=True)
sys.exit(3)
"#, vec!["custom-canary-key".into(), "proxy-canary".into()]);
            let (code, previous) = exit(&mut events).await;
            assert_eq!(code, 3);
            assert!(previous.iter().any(|event| matches!(event, PiEvent::Other(kind) if kind == "fixture-final")));
            assert!(!process.is_alive());
            process.shutdown(Duration::from_millis(10)).await;
        }.with_subscriber(subscriber).await;
        handle.end();
        let log = std::fs::read_to_string(dir.path().join("rrss.log")).unwrap();
        assert_eq!(log.matches("pi exited unexpectedly").count(), 1, "{log}");
        assert!(log.contains("exit_status=code 3"), "{log}");
        assert_eq!(log.matches("pi exit stderr").count(), 20, "{log}");
        assert!(
            !log.contains("fixture-09") && log.contains("fixture-10") && log.contains("fixture-29"),
            "{log}"
        );
        for secret in ["custom-canary-key", "proxy-canary", "query-canary", "user:"] {
            assert!(!log.contains(secret), "{log}");
        }
    }

    #[tokio::test]
    async fn orderly_shutdown_and_forced_stop_are_not_crashes() {
        let dir = tempfile::tempdir().unwrap();
        let (subscriber, handle) = crate::log::subscriber(
            crate::log::Mode::Desktop {
                log_dir: dir.path().into(),
            },
            "",
        );
        async {
            let (process, mut events) = spawn("import sys; sys.stdin.read(); sys.exit(0)", vec![]);
            process.shutdown(Duration::from_secs(1)).await;
            assert_eq!(exit(&mut events).await.0, 0);
            let (process, mut events) = spawn("import time; time.sleep(60)", vec![]);
            process.shutdown(Duration::from_millis(20)).await;
            assert_eq!(exit(&mut events).await.0, -1);
            assert!(!process.is_alive());
        }
        .with_subscriber(subscriber)
        .await;
        handle.end();
        let log = std::fs::read_to_string(dir.path().join("rrss.log")).unwrap();
        assert!(!log.contains("pi exited unexpectedly"), "{log}");
        assert_eq!(log.matches("shutdown_timeout").count(), 1, "{log}");
    }

    #[tokio::test]
    async fn dropped_handle_and_closed_stdout_do_not_leave_a_running_child() {
        let (process, mut events) = spawn("import time; time.sleep(60)", vec![]);
        drop(process);
        assert_eq!(exit(&mut events).await.0, -1);
        let (process, mut events) = spawn("import os,time; os.close(1); time.sleep(60)", vec![]);
        assert_eq!(exit(&mut events).await.0, -1);
        assert!(!process.is_alive());
    }

    #[tokio::test]
    async fn stderr_bounds_skip_whole_oversized_lines_and_keep_final_fragment() {
        let tail = Arc::new(StdMutex::new(VecDeque::new()));
        let input = format!(
            "{}\nfinal custom-canary-key",
            "custom-canary-key".repeat(STDERR_LINE_BYTES)
        );
        read_stderr(
            input.as_bytes(),
            vec!["custom-canary-key".into()],
            tail.clone(),
        )
        .await;
        let lines = tail.lock().unwrap();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "<oversized stderr line omitted>");
        assert_eq!(lines[1], "final ***");
    }
}
