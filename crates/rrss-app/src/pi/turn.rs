//! 一轮对话：发送编排、流式快照、错误分类、确认请求与结算落库。
//! 只产出领域事件 [`ChatEvent`]；宿主实现 [`ChatSink`] 把它们映射成 `chat:*`。

use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tracing::{Instrument, instrument::WithSubscriber};

use rrss_ai::pi::{PiEvent, is_dialog_method, ui_cancel, ui_confirm};
use rrss_core::threads::{self, MessageRole};
use tokio::sync::mpsc;

use super::supervisor::{Supervisor, SupervisorConfig};
use super::{ChatError, ChatErrorKind};
use crate::ui_error::chat::ChatUiError;
use crate::views::UiError;
use crate::views::{
    ChatConfirmEvent, ChatContextEvent, ChatErrorEvent, ChatStatusEvent, ChatStepView,
    ChatStreamEvent, ContextUsageView, SendMessageArgs,
};
use rrss_core::logsafe::scrub_secrets;
use rrss_core::{Error, error::PiFailure};

/// 流式快照最小间隔。
const DELTA_THROTTLE: Duration = Duration::from_millis(30);

#[derive(Debug, Clone)]
pub enum ChatEvent {
    /// 线程归属先于任何增量到达。
    Start(i64),
    Delta(ChatStreamEvent),
    Status(ChatStatusEvent),
    Confirm(ChatConfirmEvent),
    Context(ChatContextEvent),
    Error(ChatErrorEvent),
    /// 本轮结束（成功、出错或中止都会发）。
    Done(i64),
}

pub trait ChatSink: Send + Sync + 'static {
    fn emit(&self, event: ChatEvent);
}

/// 一轮对话的文字状态。工具循环里可以有多条助手消息，结算时按顺序拼接。
#[derive(Debug, Clone, Default)]
pub struct TurnState {
    pub thread_id: i64,
    buffer: String,
    last_usage: Option<rrss_ai::pi::PiUsage>,
    context_window: Option<i64>,
    committed: String,
    settled: bool,
    model_error: Option<UiError>,
    aborted: bool,
}

/// 结算结果。
#[derive(Debug, Clone)]
pub struct Settled {
    pub snapshot: ChatStreamEvent,
    pub model_error: Option<UiError>,
    pub aborted: bool,
    pub used: Option<i64>,
    pub window: Option<i64>,
}

impl TurnState {
    pub fn begin(&mut self, thread_id: i64) {
        *self = Self {
            thread_id,
            ..Self::default()
        };
    }

    pub fn active(&self) -> bool {
        self.thread_id > 0 && !self.settled
    }

    pub fn push_delta(&mut self, delta: &str) {
        if self.active() {
            self.buffer.push_str(delta);
        }
    }

    /// user / toolResult 的 message_end 不是本轮结束。助手消息出错时记下原因，中止时保留已收到的部分。
    pub fn end_message(
        &mut self,
        role: &str,
        text: &str,
        stop_reason: Option<&str>,
        error: Option<&str>,
    ) {
        if !self.active() || role != "assistant" {
            return;
        }
        match stop_reason {
            Some("error") => {
                self.model_error = Some(
                    error
                        .map(|reason| UiError::from(scrub_secrets(reason)))
                        .unwrap_or_else(|| Error::Pi(PiFailure::ModelErrorMissing).into()),
                );
            }
            Some("aborted") => self.aborted = true,
            _ => {}
        }
        let text = if text.is_empty() {
            self.buffer.as_str()
        } else {
            text
        };
        if !text.is_empty() {
            if !self.committed.is_empty() {
                self.committed.push_str("\n\n");
            }
            self.committed.push_str(text);
        }
        self.buffer.clear();
    }

    /// 自动重试最终失败。
    pub fn retry_failed(&mut self, reason: &str) {
        if self.active() {
            self.model_error = Some(UiError::from(scrub_secrets(reason)));
        }
    }

    pub fn snapshot(&self) -> Option<ChatStreamEvent> {
        if !self.active() {
            return None;
        }
        let text = match (self.committed.is_empty(), self.buffer.is_empty()) {
            (false, false) => format!("{}\n\n{}", self.committed, self.buffer),
            (false, true) => self.committed.clone(),
            _ => self.buffer.clone(),
        };
        Some(ChatStreamEvent {
            thread_id: self.thread_id,
            document: crate::queries::markdown_document_json(&text),
            text,
        })
    }

    /// 只在 agent_settled / 进程退出时结算，一次提示只结算一次。
    pub fn finish(&mut self) -> Option<Settled> {
        let snapshot = self.snapshot()?;
        self.settled = true;
        Some(Settled {
            snapshot,
            model_error: self.model_error.clone(),
            aborted: self.aborted,
            used: self.last_usage.map(|u| u.context_tokens()),
            window: self.context_window,
        })
    }
}

/// 参数摘要：检索词加引号，网址取站点与路径；读本地文章时只给 id，标题由调用方查库。
/// 外部工具（MCP）的参数不展示：不知道里面有什么。
pub fn tool_detail(name: &str, args: &serde_json::Value) -> (String, Option<i64>) {
    let text = |key: &str| {
        args.get(key)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let id = |key: &str| {
        args.get(key)
            .and_then(|v| v.as_i64().or_else(|| v.as_str()?.trim().parse().ok()))
            .filter(|id| *id > 0)
    };
    let quoted = |key: &str| {
        text(key)
            .map(|q| format!("“{}”", clip(q, 32)))
            .unwrap_or_default()
    };
    match name {
        "search_articles" | "search_memory" => (quoted("query"), None),
        "add_interest" => (quoted("label"), None),
        "get_article" => (String::new(), id("id")),
        "explain_score" | "mark_read" | "star_article" => (String::new(), id("article_id")),
        "fetch_url" | "open_original" => (text("url").map(url_summary).unwrap_or_default(), None),
        _ => (String::new(), None),
    }
}

fn clip(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// 「github.com/rust-lang/rust/issues/14…」：去掉协议、查询与片段，只留站点和路径。
fn url_summary(raw: &str) -> String {
    let Ok(url) = url::Url::parse(raw) else {
        return clip(raw, 40);
    };
    let host = url
        .host_str()
        .unwrap_or_default()
        .trim_start_matches("www.");
    let path = url.path().trim_end_matches('/');
    clip(&format!("{host}{path}"), 40)
}

pub struct ChatRuntime {
    sup: Arc<Supervisor>,
    turn: StdMutex<TurnState>,
    turn_span: StdMutex<Option<tracing::Span>>,
    sink: Arc<dyn ChatSink>,
    mcp_confirms: StdMutex<std::collections::HashMap<String, (u64, std::time::Instant)>>,
    mcp_steps: StdMutex<std::collections::HashMap<String, (String, String)>>,
    network: crate::network::NetworkClients,
    /// 结算后是否跑记忆候选与情节压缩（测试里关掉，免得访问网络）。
    post_turn_jobs: bool,
    /// 极快的 settled 可能先于 prompt 的响应；发送编排返回前不接下一轮。
    dispatch_lock: Arc<tokio::sync::Mutex<()>>,
}

impl ChatRuntime {
    /// 建好监督器，并启动事件泵与空闲回收。
    pub fn start(
        cfg: SupervisorConfig,
        network: crate::network::NetworkClients,
        sink: Arc<dyn ChatSink>,
        post_turn_jobs: bool,
    ) -> Arc<Self> {
        if let Err(err) = super::workspace::migrate_legacy_skills(&cfg.data_dir) {
            tracing::warn!(error = %format!("{err:#}"), "migrate legacy pi skills");
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let sup = Supervisor::new(cfg, tx);
        sup.spawn_idle_reaper();
        let runtime = Arc::new(Self {
            sup,
            turn: StdMutex::new(TurnState::default()),
            turn_span: StdMutex::default(),
            mcp_confirms: StdMutex::default(),
            mcp_steps: StdMutex::default(),
            sink,
            network,
            post_turn_jobs,
            dispatch_lock: Arc::new(tokio::sync::Mutex::new(())),
        });
        tokio::spawn(runtime.clone().pump(rx).with_current_subscriber());
        runtime
    }

    pub fn supervisor(&self) -> &Arc<Supervisor> {
        &self.sup
    }

    fn db_path(&self) -> &Path {
        &self.sup.config().db_path
    }

    fn thread_id(&self) -> i64 {
        self.turn.lock().map(|t| t.thread_id).unwrap_or(0)
    }

    fn turn_span(&self) -> tracing::Span {
        self.turn_span
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(tracing::Span::none)
    }

    /// 与发送准备串行；生成和结算仍占 busy，不能删除正在写消息的线程。
    pub async fn lock_thread_deletion(
        &self,
        thread_id: i64,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, ChatError> {
        let guard = self.dispatch_lock.clone().lock_owned().await;
        if self.sup.is_busy() && self.thread_id() == thread_id {
            return Err(ChatError::new(
                ChatErrorKind::Busy,
                Error::Pi(PiFailure::ThreadGenerating),
            ));
        }
        // 下次发送会 switch_session / new_session，无需清除当前 Pi 会话或重启进程。
        Ok(guard)
    }

    /// 发送一条消息，返回线程 id。回复通过 [`ChatSink`] 流式送达。
    /// 任何一步失败都释放占用；提示词被拒绝时撤销刚写入的用户消息。
    pub async fn send(self: &Arc<Self>, args: SendMessageArgs) -> Result<i64, ChatError> {
        let _dispatch = self
            .dispatch_lock
            .try_lock()
            .map_err(|_| ChatError::new(ChatErrorKind::Busy, Error::Pi(PiFailure::SendBusy)))?;
        if !self.sup.try_begin() {
            return Err(ChatError::new(
                ChatErrorKind::Busy,
                Error::Pi(PiFailure::ReplyPending),
            ));
        }
        let span = tracing::span!(parent: None, tracing::Level::WARN, "pi_turn", thread_id = tracing::field::Empty);
        if args.thread_id > 0 {
            span.record("thread_id", args.thread_id);
        }
        *self.turn_span.lock().unwrap() = Some(span.clone());
        let result = self.dispatch(args).instrument(span.clone()).await;
        if result.is_err() {
            let _entered = span.enter();
            self.turn_span.lock().unwrap().take();
            if let Some(done) = self.turn.lock().ok().and_then(|mut t| t.finish()) {
                self.sink.emit(ChatEvent::Done(done.snapshot.thread_id));
            }
            self.sup.end();
        }
        result
    }

    async fn dispatch(self: &Arc<Self>, args: SendMessageArgs) -> Result<i64, ChatError> {
        crate::chat::validate_thinking(args.thinking.as_deref())
            .map_err(|err| ChatError::new(ChatErrorKind::Config, err))?;
        let db = self.db_path().to_path_buf();
        // 先核对配置：缺模型直接报错，不建线程；配置变了就在这里重启。
        let state = self.sup.sync_profile(args.model.clone()).await?;
        // 进程没在跑：这一轮要先启动（可能还要下载）对话引擎，界面先显示这一步。
        let cold = !self.sup.is_running().await;
        if cold {
            self.engine_step(args.thread_id, true);
        }
        let (wanted, thinking) = state
            .map(|state| (Some(state.model), state.thinking))
            .unwrap_or_default();
        let prepared = blocking(db.clone(), {
            let args = args.clone();
            move |conn| crate::chat::prepare_send(conn, &args)
        })
        .await?;
        let thread_id = prepared.thread.id;
        if args.thread_id <= 0 {
            tracing::Span::current().record("thread_id", thread_id);
        }
        let session = self
            .sup
            .open_session(
                prepared.thread.pi_session_file.as_deref(),
                wanted.as_deref(),
                prepared
                    .thread
                    .chat_thinking
                    .as_deref()
                    .or(thinking.as_deref()),
            )
            .await?;
        if cold {
            self.engine_step(thread_id, false);
        }

        let model = self.sup.session_model().await;
        let window = blocking(db.clone(), move |conn| {
            let window = model
                .as_deref()
                .map(|m| super::context::window(conn, m))
                .transpose()?
                .flatten();
            threads::set_context_window(conn, thread_id, window)?;
            Ok(window)
        })
        .await?;
        let mut prompt = prepared.prompt;
        // 带着引用提问时问的是那几篇，拿「这是什么」这类原话去检索只会附上无关文章；
        // 需要相关文章时模型自己调 `search_articles`。
        let rag = if args.citations.is_empty() {
            crate::jobs::retrieve_for_chat(&db, &self.network, &args.text).await
        } else {
            String::new()
        };
        if !rag.is_empty() {
            prompt = format!("{prompt}\n\n{rag}");
        }
        let message_id = blocking(db.clone(), {
            let args = args.clone();
            move |conn| {
                crate::actions::insert_user_message(
                    conn,
                    thread_id,
                    args.text.trim(),
                    &args.citations,
                )
                .map_err(Into::into)
            }
        })
        .await?;
        if let Ok(mut turn) = self.turn.lock() {
            turn.begin(thread_id);
            turn.context_window = window;
        }
        // 先通知线程归属，再写提示词，防止极快的回复先于 Start 到达。
        self.sink.emit(ChatEvent::Start(thread_id));
        if let Some(version) = self.sup.unverified_version().await
            && self.first_version_notice(&version).await
        {
            self.sink.emit(ChatEvent::Status(ChatStatusEvent {
                thread_id,
                text: format!(
                    "对话引擎 {version} 不在已验证的 {} 版本线内，可能有兼容问题",
                    super::compat::compat().line_label()
                ),
                notice_version: version.clone(),
                step: None,
            }));
        }
        if session.restarted {
            self.sink.emit(ChatEvent::Status(ChatStatusEvent {
                thread_id,
                text: "之前的对话上下文无法恢复，已从新会话开始".into(),
                notice_version: String::new(),
                step: None,
            }));
        }
        if let Err(err) = self.sup.prompt(&prompt).await {
            let _ = blocking(db, move |conn| {
                threads::discard_unsent_message(conn, thread_id, message_id).map_err(Into::into)
            })
            .await;
            return Err(err);
        }
        let file = session.file;
        if let Err(err) = blocking(db, move |conn| {
            threads::set_pi_session_file(conn, thread_id, Some(&file)).map_err(Into::into)
        })
        .await
        {
            self.emit_error(
                thread_id,
                ChatErrorKind::Internal,
                ChatUiError::SessionSaveFailed.caused_by(err.message),
            );
        }
        Ok(thread_id)
    }

    /// 「未验证版本」每个版本号分别记忆；旧的裸版本字符串不算展示确认。
    async fn first_version_notice(&self, version: &str) -> bool {
        let version = version.to_string();
        blocking(self.db_path().to_path_buf(), move |conn| {
            let raw = rrss_core::db::get_setting(conn, super::profile::UNVERIFIED_NOTICE_KEY)?;
            let seen: Vec<String> = raw
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            Ok(!seen.contains(&version))
        })
        .await
        .unwrap_or(false)
    }

    /// 19-4 在对应版本提示实际展示后调用，不能用当前版本替代事件中的版本。
    pub async fn acknowledge_version_notice(&self, version: &str) -> Result<(), ChatError> {
        let version = version.to_string();
        blocking(self.db_path().to_path_buf(), move |conn| {
            let tx = conn.unchecked_transaction()?;
            let key = super::profile::UNVERIFIED_NOTICE_KEY;
            let raw = rrss_core::db::get_setting(&tx, key)?;
            let mut seen: Vec<String> = raw
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            if !seen.contains(&version) {
                seen.push(version);
            }
            rrss_core::db::set_setting(&tx, key, &serde_json::to_string(&seen)?)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    pub async fn abort(&self) -> Result<(), ChatError> {
        self.sup.abort().instrument(self.turn_span()).await
    }

    /// 设置页「重启引擎」：同样不打断进行中的回复；有序关闭后按当前配置启动（含托管下载与握手）。
    pub async fn restart_engine(&self) -> Result<(), ChatError> {
        self.while_idle(PiFailure::RestartBusy, PiFailure::RestartPending, async {
            self.sup.stop().await;
            self.sup.start().await
        })
        .await
    }

    /// 显式获取模型可启动 Pi，但不能打断正在进行的回复；切换配置后先同步指纹。
    pub async fn available_models(&self) -> Result<Vec<String>, ChatError> {
        self.while_idle(PiFailure::ModelsBusy, PiFailure::ModelsPending, async {
            self.sup.sync_profile(None).await?;
            self.sup.available_models().await
        })
        .await
    }

    /// 占住一轮再做进程级操作：发送中或回复未结束时返回忙，完成后释放占用。
    async fn while_idle<T>(
        &self,
        busy: PiFailure,
        pending: PiFailure,
        work: impl std::future::Future<Output = Result<T, ChatError>>,
    ) -> Result<T, ChatError> {
        let _dispatch = self
            .dispatch_lock
            .try_lock()
            .map_err(|_| ChatError::new(ChatErrorKind::Busy, Error::Pi(busy)))?;
        if !self.sup.try_begin() {
            return Err(ChatError::new(ChatErrorKind::Busy, Error::Pi(pending)));
        }
        struct Release<'a>(&'a Supervisor);
        impl Drop for Release<'_> {
            fn drop(&mut self) {
                self.0.end();
            }
        }
        let _release = Release(&self.sup);
        work.await
    }

    pub async fn stop_engine(&self) {
        self.sup.stop().await;
    }

    /// 回应 chat:confirm。
    pub async fn confirm(&self, id: &str, ok: bool, remember: bool) -> Result<(), ChatError> {
        async {
            let pending = self.mcp_confirms.lock().unwrap().remove(id);
            if let Some((generation, deadline)) = pending {
                if generation != self.sup.generation() || std::time::Instant::now() >= deadline {
                    return Ok(());
                }
                let value = if !ok {
                    "deny"
                } else if remember {
                    "remember"
                } else {
                    "allow"
                };
                return self
                    .sup
                    .answer_ui(
                        &serde_json::json!({"type":"extension_ui_response","id":id,"value":value}),
                    )
                    .await;
            }
            self.sup.answer_ui(&ui_confirm(id, ok)).await
        }
        .instrument(self.turn_span())
        .await
    }

    fn emit_error(&self, thread_id: i64, kind: ChatErrorKind, message: UiError) {
        self.sink.emit(ChatEvent::Error(ChatErrorEvent {
            thread_id,
            message,
            kind: kind.as_str().into(),
        }));
    }

    fn status(&self, text: String) {
        let thread_id = self.thread_id();
        if thread_id > 0 {
            self.sink.emit(ChatEvent::Status(ChatStatusEvent {
                thread_id,
                text,
                notice_version: String::new(),
                step: None,
            }));
        }
    }

    /// 等待过程里的一步；不进输入框上方的过程条。
    fn step(&self, thread_id: i64, step: ChatStepView) {
        self.sink.emit(ChatEvent::Status(ChatStatusEvent {
            thread_id,
            text: String::new(),
            notice_version: String::new(),
            step: Some(step),
        }));
    }

    fn engine_step(&self, thread_id: i64, start: bool) {
        self.step(
            thread_id,
            ChatStepView {
                server: String::new(),
                server_tool: String::new(),
                kind: "engine".into(),
                id: "engine".into(),
                start,
                tool: String::new(),
                detail: String::new(),
                article_id: None,
                error: false,
            },
        );
    }

    async fn tool_started(&self, call_id: String, name: String, args: serde_json::Value) {
        let thread_id = self.thread_id();
        if thread_id <= 0 {
            return;
        }
        // The guard emits the external step with the original server tool name.
        if name.starts_with("mcp__") {
            return;
        }
        let (mut detail, article_id) = tool_detail(&name, &args);
        if let Some(id) = article_id {
            let title = blocking(self.db_path().to_path_buf(), move |conn| {
                use rusqlite::OptionalExtension;
                Ok(conn
                    .query_row("SELECT title FROM articles WHERE id = ?1", [id], |row| {
                        row.get::<_, String>(0)
                    })
                    .optional()?)
            })
            .await;
            if let Ok(Some(title)) = title
                && !title.trim().is_empty()
            {
                detail = format!("《{}》", clip(title.trim(), 40));
            }
        }
        self.step(
            thread_id,
            ChatStepView {
                server: String::new(),
                server_tool: String::new(),
                kind: "tool".into(),
                id: call_id,
                start: true,
                tool: name,
                detail,
                article_id,
                error: false,
            },
        );
    }

    fn tool_ended(&self, call_id: String, name: String, error: bool) {
        let thread_id = self.thread_id();
        if thread_id <= 0 {
            return;
        }
        let (server, server_tool) = self
            .mcp_steps
            .lock()
            .unwrap()
            .remove(&call_id)
            .unwrap_or_default();
        self.step(
            thread_id,
            ChatStepView {
                server,
                server_tool,
                kind: "tool".into(),
                id: call_id,
                start: false,
                tool: name,
                detail: String::new(),
                article_id: None,
                error,
            },
        );
    }

    async fn pump(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<(u64, PiEvent)>) {
        let mut last = Instant::now()
            .checked_sub(DELTA_THROTTLE)
            .unwrap_or_else(Instant::now);
        while let Some((generation, event)) = rx.recv().await {
            if generation != self.sup.generation() {
                tracing::debug!(generation, "drop event from a previous pi process");
                continue;
            }
            let span = self.turn_span();
            async {
            self.sup.touch();
            match event {
                PiEvent::TextDelta(delta) => {
                    let snapshot = self.turn.lock().ok().and_then(|mut t| {
                        t.push_delta(&delta);
                        (last.elapsed() >= DELTA_THROTTLE)
                            .then(|| t.snapshot())
                            .flatten()
                    });
                    if let Some(snapshot) = snapshot {
                        last = Instant::now();
                        self.sink.emit(ChatEvent::Delta(snapshot));
                    }
                }
                PiEvent::MessageEnd {
                    role,
                    text,
                    stop_reason,
                    error_message,
                    usage,
                } => {
                    let snapshot = self.turn.lock().ok().and_then(|mut t| {
                        if role == "assistant" && t.active() {
                            t.last_usage = usage;
                        }
                        t.end_message(
                            &role,
                            &text,
                            stop_reason.as_deref(),
                            error_message.as_deref(),
                        );
                        (role == "assistant").then(|| t.snapshot()).flatten()
                    });
                    if let Some(snapshot) = snapshot {
                        self.sink.emit(ChatEvent::Delta(snapshot));
                    }
                }
                PiEvent::ToolStart {
                    call_id,
                    name,
                    args,
                } => self.tool_started(call_id, name, args).await,
                PiEvent::ToolEnd {
                    call_id,
                    name,
                    is_error,
                } => self.tool_ended(call_id, name, is_error),
                PiEvent::CompactionEnd {
                    aborted,
                    error_message,
                } => {
                    let failed = error_message.is_some();
                    if !aborted
                        && let Some(reason) = error_message
                        && let Ok(mut t) = self.turn.lock()
                    {
                        t.retry_failed(&format!("对话压缩失败：{reason}"));
                    }
                    if !aborted && !failed {
                        let active =
                            self.turn.lock().ok().and_then(|t| {
                                t.active().then_some((t.thread_id, t.context_window))
                            });
                        if let Some((id, window)) = active {
                            // 压缩后可能未知，不能拿压缩前 usage 冒充新用量。
                            if let Ok(mut t) = self.turn.lock() {
                                t.last_usage = None;
                            }
                            if let Some(used) = self.sup.context_used().await {
                                self.save_context(id, used, window).await;
                            }
                        }
                    }
                    self.status(String::new());
                }
                PiEvent::CompactionStart { .. } => self.status("正在压缩较早的对话…".into()),
                PiEvent::AutoRetryStart {
                    attempt,
                    max_attempts,
                    ..
                } => self.status(format!("模型出错，正在重试（{attempt}/{max_attempts}）…")),
                PiEvent::AutoRetryEnd {
                    success,
                    final_error,
                } => {
                    if !success && let Ok(mut t) = self.turn.lock() {
                        t.retry_failed(final_error.as_deref().unwrap_or("自动重试失败"));
                    }
                    self.status(String::new());
                }
                PiEvent::ExtensionUiRequest {
                    id,
                    method,
                    title,
                    message,
                    timeout_ms,
                } => {
                    self.on_ui_request(id, method, title, message, timeout_ms)
                        .await
                }
                PiEvent::ExtensionError { message } => {
                    tracing::warn!(error = %rrss_core::logsafe::text(&message, 200), "pi extension error")
                }
                PiEvent::AgentSettled => self.settle(false).await,
                PiEvent::Exited { .. } => self.settle(true).await,
                _ => {}
            }
            }.instrument(span).await;
        }
    }

    async fn on_ui_request(
        &self,
        id: String,
        method: String,
        title: String,
        message: String,
        timeout_ms: Option<u64>,
    ) {
        if method == "notify"
            && let Some(raw) = message.strip_prefix("RRSS_MCP_STEP:")
        {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
                let field = |key| {
                    value
                        .get(key)
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                let (id, name, server, tool) =
                    (field("id"), field("name"), field("server"), field("tool"));
                self.mcp_steps
                    .lock()
                    .unwrap()
                    .insert(id.clone(), (server.clone(), tool.clone()));
                self.step(
                    self.thread_id(),
                    ChatStepView {
                        kind: "tool".into(),
                        id,
                        start: true,
                        tool: name,
                        server,
                        server_tool: tool,
                        detail: String::new(),
                        article_id: None,
                        error: false,
                    },
                );
            }
            return;
        }
        if method == "select"
            && self.thread_id() > 0
            && let Some(raw) = title.strip_prefix("RRSS_MCP_CONFIRM:")
        {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
                let field = |key| {
                    value
                        .get(key)
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                let (server, tool, args) = (field("server"), field("tool"), field("args"));
                self.mcp_confirms.lock().unwrap().insert(
                    id.clone(),
                    (
                        self.sup.generation(),
                        std::time::Instant::now() + Duration::from_secs(300),
                    ),
                );
                self.sink.emit(ChatEvent::Confirm(ChatConfirmEvent {
                    thread_id: self.thread_id(),
                    id,
                    title: format!("使用 {server} 的 {tool}"),
                    message: args.clone(),
                    timeout_ms: Some(300_000),
                    server,
                    tool,
                    args,
                    can_remember: true,
                }));
                return;
            }
        }
        if !is_dialog_method(&method) {
            return;
        }
        let thread_id = self.thread_id();
        if method == "confirm" && thread_id > 0 {
            self.sink.emit(ChatEvent::Confirm(ChatConfirmEvent {
                thread_id,
                id,
                title,
                message,
                timeout_ms: timeout_ms.map(|t| t as i64),
                server: String::new(),
                tool: String::new(),
                args: String::new(),
                can_remember: false,
            }));
            return;
        }
        // 选择、输入、编辑器不支持；没有进行中的对话时也没人能回答确认。
        if let Err(err) = self.sup.answer_ui(&ui_cancel(&id)).await {
            tracing::warn!(error = %err, "cancel pi dialog");
        }
    }

    async fn save_context(&self, thread_id: i64, used: i64, window: Option<i64>) {
        let context = ContextUsageView {
            used,
            window,
            at: rrss_core::feeds::now_ms(),
        };
        let at = context.at;
        let saved = blocking(self.db_path().to_path_buf(), move |conn| {
            threads::set_context(conn, thread_id, used, window, at).map_err(Into::into)
        })
        .await;
        match saved {
            Ok(()) => self
                .sink
                .emit(ChatEvent::Context(ChatContextEvent { thread_id, context })),
            // 用量只是指示，存不下不打断对话。
            Err(err) => {
                tracing::warn!(thread_id = rrss_core::logsafe::unless_in_span("thread_id", thread_id), error = %crate::log::error(&err), "could not save context usage")
            }
        }
    }

    async fn settle(&self, exited: bool) {
        let Some(done) = self.turn.lock().ok().and_then(|mut t| t.finish()) else {
            return;
        };
        self.mcp_confirms.lock().unwrap().clear();
        self.mcp_steps.lock().unwrap().clear();
        let thread_id = done.snapshot.thread_id;
        let body = done.snapshot.text.clone();
        let mut error: Option<(ChatErrorKind, UiError)> = None;
        if !body.trim().is_empty() {
            let text = body.clone();
            let saved = blocking(self.db_path().to_path_buf(), move |conn| {
                threads::insert_message(conn, thread_id, MessageRole::Assistant, &text, None)
                    .map(|_| ())
                    .map_err(Into::into)
            })
            .await;
            if let Err(err) = saved {
                error = Some((
                    ChatErrorKind::Internal,
                    ChatUiError::ReplySaveFailed.caused_by(err.message),
                ));
            }
        }
        if exited {
            let message = if body.trim().is_empty() {
                PiFailure::ExitEmpty
            } else {
                PiFailure::ExitPartial
            };
            error = Some((ChatErrorKind::ProcessExit, Error::Pi(message).into()));
        } else if let Some(reason) = done.model_error.clone() {
            // 类别已说明是模型服务出错，消息只带原因，界面不重复前缀。
            error = Some((ChatErrorKind::Model, reason));
        } else if body.trim().is_empty() && !done.aborted && error.is_none() {
            error = Some((
                ChatErrorKind::Model,
                Error::Pi(PiFailure::ReplyEmpty).into(),
            ));
        }
        self.sink.emit(ChatEvent::Delta(done.snapshot));
        self.sink.emit(ChatEvent::Status(ChatStatusEvent {
            thread_id,
            text: String::new(),
            notice_version: String::new(),
            step: None,
        }));
        if let Some((kind, message)) = error {
            self.emit_error(thread_id, kind, message);
        }
        if !exited {
            if let Some(used) = self.sup.context_used().await.or(done.used) {
                self.save_context(thread_id, used, done.window).await;
            }
        }
        self.turn_span.lock().unwrap().take();
        self.sup.end();
        self.sink.emit(ChatEvent::Done(thread_id));
        if self.post_turn_jobs && !body.trim().is_empty() {
            let path = self.db_path().to_path_buf();
            let network = self.network.clone();
            tokio::spawn(
                async move {
                    use rrss_core::network::NetworkScope;
                    match network.client(NetworkScope::Api).await {
                        Ok(client) => {
                            let _ =
                                crate::jobs::extract_memory_candidates(&path, &client, body).await;
                        }
                        Err(error) => {
                            tracing::warn!(%error, "memory extraction network unavailable")
                        }
                    }
                    match network.client(NetworkScope::Api).await {
                        Ok(client) => {
                            let _ =
                                crate::jobs::compress_episode_if_needed(&path, &client, thread_id)
                                    .await;
                        }
                        Err(error) => {
                            tracing::warn!(%error, "episode compression network unavailable")
                        }
                    }
                }
                .instrument(crate::jobs::span("post_turn"))
                .with_current_subscriber(),
            );
        }
    }
}

/// 在阻塞线程里开库执行；错误统一归为内部错误。
async fn blocking<T, F>(db: std::path::PathBuf, f: F) -> Result<T, ChatError>
where
    T: Send + 'static,
    F: FnOnce(&rrss_core::db::PooledConnection) -> anyhow::Result<T> + Send + 'static,
{
    crate::execution::with_db(db, f)
        .await
        .map_err(|err| ChatError::new(ChatErrorKind::Internal, UiError::from_anyhow_pretty(err)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turn_joins_tool_loop_messages_and_settles_once() {
        let mut turn = TurnState::default();
        assert!(turn.finish().is_none());
        turn.begin(7);
        turn.end_message("user", "展开第二条", None, None);
        turn.push_delta("先查");
        turn.push_delta("原文。");
        assert_eq!(turn.snapshot().unwrap().text, "先查原文。");
        turn.end_message("assistant", "先查原文。", Some("toolUse"), None);
        turn.end_message("toolResult", "工具结果不显示", None, None);
        turn.push_delta("第二条");
        turn.end_message("assistant", "第二条的具体内容。", Some("stop"), None);
        let done = turn.finish().unwrap();
        assert_eq!(done.snapshot.thread_id, 7);
        assert_eq!(done.snapshot.text, "先查原文。\n\n第二条的具体内容。");
        assert!(done.model_error.is_none() && !done.aborted);
        assert!(turn.finish().is_none());
        turn.push_delta("迟到的消息");
        assert!(turn.snapshot().is_none());
        turn.begin(8);
        assert_eq!(turn.snapshot().unwrap().text, "");
    }

    #[test]
    fn model_errors_are_recorded_and_redacted_abort_keeps_partial_text() {
        let mut turn = TurnState::default();
        turn.begin(1);
        turn.end_message(
            "assistant",
            "",
            Some("error"),
            Some("401 bad key sk-secret123\nauthorization: bearer lower-canary"),
        );
        let done = turn.finish().unwrap();
        let error = done.model_error.unwrap().detail;
        assert!(error.starts_with("401 bad key") && error.contains('\n'));
        assert!(!error.contains("secret123") && !error.contains("lower-canary"));
        assert!(done.snapshot.text.is_empty());

        turn.begin(2);
        turn.push_delta("写到一半");
        turn.end_message(
            "assistant",
            "",
            Some("aborted"),
            Some("Request was aborted"),
        );
        let done = turn.finish().unwrap();
        assert!(done.aborted && done.model_error.is_none());
        assert_eq!(done.snapshot.text, "写到一半");

        turn.begin(3);
        turn.retry_failed("529 overloaded");
        assert_eq!(
            turn.finish()
                .unwrap()
                .model_error
                .as_ref()
                .map(|error| error.detail.as_str()),
            Some("529 overloaded")
        );
    }

    #[test]
    fn only_last_assistant_usage_is_used_and_missing_last_usage_clears_it() {
        let mut turn = TurnState::default();
        turn.begin(1);
        turn.last_usage = Some(rrss_ai::pi::PiUsage {
            input_tokens: 1000,
            ..Default::default()
        });
        turn.end_message("assistant", "中间", Some("toolUse"), None);
        turn.last_usage = Some(rrss_ai::pi::PiUsage {
            input_tokens: 100,
            cache_read_tokens: 30,
            cache_write_tokens: 40,
            output_tokens: 20,
        });
        turn.end_message("assistant", "最终", Some("stop"), None);
        assert_eq!(turn.finish().unwrap().used, Some(190));
        turn.begin(2);
        turn.last_usage = None;
        turn.end_message("assistant", "无 usage", Some("stop"), None);
        assert_eq!(turn.finish().unwrap().used, None);
    }

    #[test]
    fn steps_export_tool_ids_without_display_labels() {
        let step = ChatStepView {
            kind: "tool".into(),
            id: "call".into(),
            start: true,
            tool: "search_articles".into(),
            server: String::new(),
            server_tool: String::new(),
            detail: String::new(),
            article_id: None,
            error: false,
        };
        let json = serde_json::to_value(step).unwrap();
        assert_eq!(json["kind"], "tool");
        assert_eq!(json["tool"], "search_articles");
        assert!(json.get("label").is_none());
    }

    #[test]
    fn tool_args_become_short_details_without_exposing_external_tools() {
        use serde_json::json;
        assert_eq!(
            tool_detail("search_articles", &json!({ "query": " Rust 1.97 回归 " })),
            ("“Rust 1.97 回归”".into(), None)
        );
        assert_eq!(
            tool_detail("get_article", &json!({ "id": 42 })),
            (String::new(), Some(42))
        );
        assert_eq!(
            tool_detail("explain_score", &json!({ "article_id": "7" })),
            (String::new(), Some(7))
        );
        assert_eq!(
            tool_detail("get_article", &json!({ "id": 0 })),
            (String::new(), None)
        );
        assert_eq!(
            tool_detail(
                "fetch_url",
                &json!({ "url": "https://www.github.com/rust-lang/rust/issues/148210?x=1#top" })
            )
            .0,
            "github.com/rust-lang/rust/issues/148210"
        );
        assert_eq!(
            tool_detail(
                "fetch_url",
                &json!({ "url": "https://example.org/a/very/long/path/that/keeps/going/on" })
            )
            .0,
            "example.org/a/very/long/path/that/keeps/…"
        );
        assert_eq!(
            tool_detail("search_articles", &json!({ "query": "长".repeat(40) }))
                .0
                .chars()
                .count(),
            35
        );
        assert_eq!(
            tool_detail("mcp_secret_tool", &json!({ "token": "abc" })),
            (String::new(), None)
        );
        assert_eq!(
            tool_detail("search_articles", &serde_json::Value::Null),
            (String::new(), None)
        );
    }
}
