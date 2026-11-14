//! Pi 进程监督：按需启动、会话切换、一次一轮、空闲回收、有序退出。
//! 状态：未运行 → 启动中 → 就绪 ⇄ 生成中；进程随时可能退出，下次发送时重新启动。
//! 每个进程有递增的代次，事件带代次转发，旧进程迟到的事件由调用方丢弃。
//! 启动 = 准备托管目录 + 拉起进程 + 握手；发送前核对 Profile 指纹，变了就在空闲时重启。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use rrss_ai::pi::rpc::RpcError;
use rrss_ai::pi::{
    PiEvent, cmd_abort, cmd_get_available_models, cmd_get_available_thinking_levels,
    cmd_get_session_stats, cmd_get_state, cmd_new_session, cmd_prompt, cmd_set_model,
    cmd_set_thinking_level, cmd_switch_session, prompt_was_handled,
};
use serde_json::Value;
use tokio::sync::{Mutex, mpsc};

use super::compat::VersionGate;
use super::launch::{self, PiProcess, Prepared, RPC_TIMEOUT};
use super::profile::ModelSource;
use super::{ChatError, ChatErrorKind};
use crate::ui_error::chat::{ChatHandshakeStep, ChatUiError};
use crate::views::ModelChoice;
use rrss_core::{Error, error::PiFailure};

/// 关闭 stdin 后等待进程自行退出的时长，超过就强制结束。
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
/// `abort` 要等会话空闲才回应。
const ABORT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    pub db_path: PathBuf,
    pub data_dir: PathBuf,
    /// `rrss tool` 可执行文件，扩展通过它读本地库。
    pub tool_path: PathBuf,
    /// 空闲回收的检查间隔。
    pub idle_tick: Duration,
}

struct Running {
    process: Arc<PiProcess>,
    generation: u64,
    session_file: Option<String>,
    /// 启动时 Profile 的指纹。
    fingerprint: String,
    /// 握手得到的可用思考档位。
    thinking_levels: Vec<String>,
    /// 握手时 Pi 报告的 `provider/id`；自有配置没有默认模型时为空，此时不能发送。
    model: Option<String>,
    /// 当前会话正在用的 `provider/id`（打开会话或切换后记下）。
    session_model: Option<String>,
    /// 当前会话已设置的思考强度（按模型设置，阶段 19-7）；换会话或换模型后清空，下次发送前重设。
    session_thinking: Option<String>,
    /// 版本在兼容线以外或读不出时的版本号（读不出记为「未知」）。
    unverified: Option<String>,
}

/// 发送前核对得到的当前配置概要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileState {
    /// 这一轮要用的 `provider/id`：对话中选定的模型，没选时是对话默认；空表示接受 Pi 的默认模型。
    pub model: String,
    pub source: ModelSource,
    /// 这个模型的思考强度；自有配置下为空，不改 Pi 的设置。
    pub thinking: Option<String>,
    /// 本次核对因配置变化重启了进程。
    pub restarted: bool,
}

/// 打开会话的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedSession {
    pub file: String,
    /// 线程原来的会话无法恢复，已改用新会话。
    pub restarted: bool,
}

pub struct Supervisor {
    cfg: SupervisorConfig,
    running: Mutex<Option<Running>>,
    generation: AtomicU64,
    busy: AtomicBool,
    last_active: StdMutex<Instant>,
    events: mpsc::UnboundedSender<(u64, PiEvent)>,
    /// 最近一次发送所在对话选定的模型；进程按它启动，换对话时由 [`Self::sync_profile`] 更新。
    target: StdMutex<Option<ModelChoice>>,
}

impl Supervisor {
    pub fn new(cfg: SupervisorConfig, events: mpsc::UnboundedSender<(u64, PiEvent)>) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            running: Mutex::new(None),
            generation: AtomicU64::new(0),
            busy: AtomicBool::new(false),
            last_active: StdMutex::new(Instant::now()),
            events,
            target: StdMutex::new(None),
        })
    }

    pub fn config(&self) -> &SupervisorConfig {
        &self.cfg
    }

    /// 当前（最近启动的）进程代次；事件代次与它不同就是旧进程的。
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub fn touch(&self) {
        if let Ok(mut t) = self.last_active.lock() {
            *t = Instant::now();
        }
    }

    fn idle_for(&self) -> Duration {
        self.last_active
            .lock()
            .map(|t| t.elapsed())
            .unwrap_or_default()
    }

    /// 一次只允许一轮生成。
    pub fn try_begin(&self) -> bool {
        let ok = self
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
        if ok {
            self.touch();
        }
        ok
    }

    pub fn end(&self) {
        self.touch();
        self.busy.store(false, Ordering::SeqCst);
    }

    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    pub async fn is_running(&self) -> bool {
        self.running
            .lock()
            .await
            .as_ref()
            .is_some_and(|r| r.process.is_alive())
    }

    /// 进程在跑就复用，否则准备托管目录、启动新进程、握手，并分配新代次。
    async fn process(&self) -> Result<Arc<PiProcess>, ChatError> {
        let mut guard = self.running.lock().await;
        if let Some(running) = guard.as_ref()
            && running.process.is_alive()
        {
            return Ok(running.process.clone());
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, mut rx) = mpsc::unbounded_channel::<PiEvent>();
        let out = self.events.clone();
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if out.send((generation, event)).is_err() {
                    break;
                }
            }
        });
        let (process, prepared) = launch::spawn(
            &self.cfg.db_path,
            &self.cfg.data_dir,
            &self.cfg.tool_path,
            self.target(),
            tx,
        )
        .await?;
        let process = Arc::new(process);
        let (thinking_levels, model) = match handshake(&process, &prepared).await {
            Ok(found) => found,
            Err(err) => {
                process.shutdown(SHUTDOWN_GRACE).await;
                return Err(err);
            }
        };
        let models = process
            .request(cmd_get_available_models(), RPC_TIMEOUT)
            .await;
        if let Ok(models) = models {
            let db = self.cfg.db_path.clone();
            let saved = crate::execution::blocking(move || {
                let conn = rrss_core::db::open_at(&db)?;
                super::context::cache_windows(&conn, &models)
            })
            .await;
            if let Err(error) = saved.map_err(anyhow::Error::from).flatten() {
                tracing::warn!(error = %crate::log::error(error.as_ref()), "could not cache pi context windows");
            }
        }
        tracing::info!(
            generation,
            model = model.as_deref().unwrap_or("none"),
            "pi ready"
        );
        // 记下可用档位，引擎停下后设置页仍能按它显示思考强度。
        if !thinking_levels.is_empty() {
            let (db, levels) = (self.cfg.db_path.clone(), thinking_levels.clone());
            let saved = crate::execution::blocking(move || {
                rrss_core::db::open_at(&db).and_then(|conn| {
                    rrss_core::pi_store::set_cached_thinking_levels(&conn, &levels)
                })
            })
            .await;
            if let Err(error) = saved
                .map_err(anyhow::Error::from)
                .and_then(|result| result.map_err(anyhow::Error::from))
            {
                tracing::warn!(error = %crate::log::error(error.as_ref()), "could not cache pi thinking levels");
            }
        }
        let unverified = (prepared.gate == VersionGate::Unverified).then(|| {
            prepared
                .version
                .map(|v| v.to_string())
                .unwrap_or_else(|| "unknown".into())
        });
        *guard = Some(Running {
            process: process.clone(),
            generation,
            session_file: None,
            fingerprint: prepared.profile.fingerprint(),
            thinking_levels,
            session_thinking: prepared.profile.thinking.clone(),
            model,
            session_model: None,
            unverified,
        });
        self.touch();
        Ok(process)
    }

    fn target(&self) -> Option<ModelChoice> {
        self.target.lock().map(|t| t.clone()).unwrap_or(None)
    }

    /// 发送前核对期望配置（调用方已持有一轮占用，进程此刻空闲）。指纹变了就有序关闭，下一步按新配置启动。
    /// `target` 是这条对话选定的模型；换成另一个模型同样表现为指纹变化。
    /// 缺模型等配置错误在这里直接返回，不必先启动进程。找不到 Pi 时交给启动流程（可能触发托管下载）。
    pub async fn sync_profile(
        &self,
        target: Option<ModelChoice>,
    ) -> Result<Option<ProfileState>, ChatError> {
        if let Ok(mut current) = self.target.lock() {
            *current = target.clone();
        }
        let (db, data) = (self.cfg.db_path.clone(), self.cfg.data_dir.clone());
        let current = crate::execution::blocking(move || {
            let conn = rrss_core::db::open_at(&db)
                .map_err(|err| ChatError::internal(ChatErrorKind::Internal, err))?;
            launch::current_profile(&conn, &data, target.as_ref()).map(|(p, _)| {
                (
                    p.fingerprint(),
                    p.wanted_model.clone(),
                    p.model_source,
                    p.wanted_thinking.clone(),
                )
            })
        })
        .await
        .map_err(|err| ChatError::internal(ChatErrorKind::Internal, err))?;
        let (fingerprint, model, source, thinking) = match current {
            Ok(found) => found,
            Err(err) if err.kind == ChatErrorKind::NotInstalled => return Ok(None),
            Err(err) => return Err(err),
        };
        let stale = self
            .running
            .lock()
            .await
            .as_ref()
            .filter(|r| r.process.is_alive())
            .is_some_and(|r| r.fingerprint != fingerprint);
        if stale {
            tracing::info!("pi profile changed; restarting before send");
            self.stop().await;
        }
        Ok(Some(ProfileState {
            model: model.unwrap_or_default(),
            source,
            thinking,
            restarted: stale,
        }))
    }

    /// Pi 当前能用的模型（`provider/id`），给「使用我自己的 Pi 配置」选模型用。需要时会先启动进程。
    pub async fn available_models(&self) -> Result<Vec<String>, ChatError> {
        let process = self.process().await?;
        let data = process
            .request(cmd_get_available_models(), RPC_TIMEOUT)
            .await
            .map_err(rpc_error)?;
        Ok(data
            .get("models")
            .and_then(Value::as_array)
            .map(|models| models.iter().filter_map(model_ref_of).collect())
            .unwrap_or_default())
    }

    /// 只查询现有进程，绝不因统计拉起或重启引擎。
    pub async fn context_used(&self) -> Option<i64> {
        let process = self.current().await.ok()?;
        let stats = process
            .request(cmd_get_session_stats(), RPC_TIMEOUT)
            .await
            .ok()?;
        super::context::current_used(&stats)
    }

    pub async fn session_model(&self) -> Option<String> {
        self.running
            .lock()
            .await
            .as_ref()
            .and_then(|r| r.session_model.clone().or(r.model.clone()))
    }

    /// 当前进程是未验证版本时返回版本号。
    pub async fn unverified_version(&self) -> Option<String> {
        self.running
            .lock()
            .await
            .as_ref()
            .and_then(|r| r.unverified.clone())
    }

    /// 握手得到的可用思考档位；进程未运行时为空。
    pub async fn thinking_levels(&self) -> Vec<String> {
        self.running
            .lock()
            .await
            .as_ref()
            .map(|r| r.thinking_levels.clone())
            .unwrap_or_default()
    }

    /// 手动启动（设置页「启动」）；已在运行则不动。
    pub async fn start(&self) -> Result<(), ChatError> {
        self.process().await.map(|_| ())
    }

    /// 切到线程的会话；没有会话时新建。原会话无法恢复（文件缺失、被取消、版本不兼容）时新建会话并标记，不静默丢弃。
    /// `want` 是这一轮要用的 `provider/id`；会话当前不是它时在进程内切换（`set_model`），不重启。
    /// `thinking` 是这个模型的思考强度，和会话当前的不同时设置（不支持的档位降到最近可用档）。
    pub async fn open_session(
        &self,
        file: Option<&str>,
        want: Option<&str>,
        thinking: Option<&str>,
    ) -> Result<OpenedSession, ChatError> {
        let opened = self.open_session_model(file, want).await?;
        if let Some(thinking) = thinking {
            let process = self.process().await?;
            let state = process
                .request(cmd_get_state(), RPC_TIMEOUT)
                .await
                .map_err(rpc_error)?;
            let reasoning = state
                .get("model")
                .and_then(|m| m.get("reasoning"))
                .and_then(Value::as_bool);
            let thinking = if reasoning == Some(false) {
                "off"
            } else {
                thinking
            };
            self.apply_thinking(&process, thinking).await?;
        }
        Ok(opened)
    }

    async fn open_session_model(
        &self,
        file: Option<&str>,
        want: Option<&str>,
    ) -> Result<OpenedSession, ChatError> {
        let process = self.process().await?;
        let has_model = self
            .running
            .lock()
            .await
            .as_ref()
            .is_some_and(|r| r.model.is_some());
        if !has_model {
            return Err(ChatError::new(
                ChatErrorKind::Config,
                Error::Pi(PiFailure::OwnModelUnset),
            ));
        }
        let wanted = file.map(str::trim).filter(|f| !f.is_empty());
        let want = want.map(str::trim).filter(|w| !w.is_empty());
        if let Some(wanted) = wanted {
            let (current, model) = self
                .running
                .lock()
                .await
                .as_ref()
                .map(|r| (r.session_file.clone(), r.session_model.clone()))
                .unwrap_or_default();
            if current.as_deref() == Some(wanted) {
                if want.is_some() && model.as_deref() != want {
                    self.select_model(&process, want.unwrap_or_default())
                        .await?;
                }
                return Ok(OpenedSession {
                    file: wanted.to_string(),
                    restarted: false,
                });
            }
        }
        let mut restarted = false;
        let switched = match wanted {
            Some(path) => match process.request(cmd_switch_session(path), RPC_TIMEOUT).await {
                Ok(data) if !is_cancelled(&data) => true,
                Ok(_) => {
                    tracing::warn!(
                        error_kind = "cancelled",
                        "pi cancelled session switch; starting a new session"
                    );
                    false
                }
                Err(RpcError::Rejected(_reason)) => {
                    tracing::warn!(
                        code = "pi_session_rejected",
                        "pi could not resume session; starting a new session"
                    );
                    false
                }
                Err(err) => return Err(rpc_error(err)),
            },
            None => false,
        };
        if !switched {
            restarted = wanted.is_some();
            let data = process
                .request(cmd_new_session(), RPC_TIMEOUT)
                .await
                .map_err(rpc_error)?;
            if is_cancelled(&data) {
                return Err(ChatError::new(
                    ChatErrorKind::Config,
                    Error::Pi(PiFailure::SessionCancelled),
                ));
            }
        }
        let state = process
            .request(cmd_get_state(), RPC_TIMEOUT)
            .await
            .map_err(rpc_error)?;
        let file = state
            .get("sessionFile")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                ChatError::new(
                    ChatErrorKind::Config,
                    Error::Pi(PiFailure::SessionPathMissing),
                )
            })?;
        let current = state.get("model").and_then(model_ref_of);
        if let Some(want) = want
            && current.as_deref() != Some(want)
        {
            self.select_model(&process, want).await?;
        } else if let Some(running) = self.running.lock().await.as_mut() {
            running.session_model = current;
        }
        if let Some(running) = self.running.lock().await.as_mut() {
            // 换了会话：它记着自己的思考强度，下次按模型重新设置。
            if running.session_file.as_deref() != Some(file.as_str()) {
                running.session_thinking = None;
            }
            running.session_file = Some(file.clone());
        }
        Ok(OpenedSession { file, restarted })
    }

    /// 按模型设置思考强度；会话当前已是这个值时不动。
    async fn apply_thinking(&self, process: &PiProcess, wanted: &str) -> Result<(), ChatError> {
        let current = self
            .running
            .lock()
            .await
            .as_ref()
            .and_then(|r| r.session_thinking.clone());
        if current.as_deref() == Some(wanted) {
            return Ok(());
        }
        let data = process
            .request(cmd_get_available_thinking_levels(), RPC_TIMEOUT)
            .await
            .map_err(rpc_error)?;
        let levels: Vec<String> = data
            .get("levels")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let level = if levels.iter().any(|l| l == wanted) {
            Some(wanted)
        } else {
            closest_thinking_level(wanted, &levels)
        };
        if let Some(level) = level {
            process
                .request(cmd_set_thinking_level(level), RPC_TIMEOUT)
                .await
                .map_err(rpc_error)?;
        }
        if let Some(running) = self.running.lock().await.as_mut() {
            running.session_thinking = Some(wanted.to_string());
        }
        Ok(())
    }

    /// 进程内切换当前会话的模型；Pi 把切换记进会话，恢复这条会话时沿用。
    async fn select_model(&self, process: &PiProcess, want: &str) -> Result<(), ChatError> {
        let (provider, id) = want.split_once('/').unwrap_or(("", want));
        process
            .request(cmd_set_model(provider, id), RPC_TIMEOUT)
            .await
            .map_err(|err| match err {
                RpcError::Rejected(reason) => ChatError::new(
                    ChatErrorKind::Config,
                    ChatUiError::model_switch_rejected(want, reason.as_deref()),
                ),
                other => rpc_error(other),
            })?;
        tracing::info!(model = want, "pi model switched in process");
        if let Some(running) = self.running.lock().await.as_mut() {
            running.session_model = Some(want.to_string());
            running.session_thinking = None;
        }
        Ok(())
    }

    /// 发送提示词；`success: false`（预检拒绝）作为模型错误返回。
    pub async fn prompt(&self, message: &str) -> Result<(), ChatError> {
        let process = self.current().await?;
        self.touch();
        let result = process.request(cmd_prompt(message), RPC_TIMEOUT).await;
        if matches!(result, Err(RpcError::Timeout)) {
            // 无法确认 prompt 是否已开始执行。先使旧事件失效并停掉进程，
            // 再让 send 释放本轮，避免没有轮次 id 的迟到增量混入下一轮。
            self.generation.fetch_add(1, Ordering::SeqCst);
            self.stop().await;
        }
        let data = result.map_err(|err| match err {
            RpcError::Rejected(reason) => ChatError::new(
                ChatErrorKind::Model,
                ChatUiError::rpc_rejected(reason.as_deref()),
            ),
            other => rpc_error(other),
        })?;
        // 1.x 会报告输入被扩展接管：不会开始运行，也等不到 agent_settled，本轮按失败结束。
        // 斜杠命令已在 cmd_prompt 里挡掉，剩下的是自有配置里用户扩展的输入处理器。
        if prompt_was_handled(&data) {
            return Err(ChatError::new(
                ChatErrorKind::Model,
                Error::Pi(PiFailure::InputHandled),
            ));
        }
        Ok(())
    }

    /// 中止当前生成并等待会话空闲。没有进程时什么也不做。
    pub async fn abort(&self) -> Result<(), ChatError> {
        let Ok(process) = self.current().await else {
            return Ok(());
        };
        process
            .request(cmd_abort(), ABORT_TIMEOUT)
            .await
            .map(|_| ())
            .map_err(rpc_error)
    }

    /// 回应扩展的对话框请求。
    pub async fn answer_ui(&self, response: &Value) -> Result<(), ChatError> {
        let process = self.current().await?;
        process.send(response).await.map_err(|err| {
            ChatError::new(
                ChatErrorKind::ProcessExit,
                crate::views::UiError::from_anyhow_pretty(err),
            )
        })
    }

    /// 有序关闭当前进程。之后的发送会重新启动。
    pub async fn stop(&self) {
        let mut guard = self.running.lock().await;
        if let Some(running) = guard.take() {
            tracing::info!(generation = running.generation, "stopping pi");
            running.process.shutdown(SHUTDOWN_GRACE).await;
        }
    }

    async fn current(&self) -> Result<Arc<PiProcess>, ChatError> {
        self.running
            .lock()
            .await
            .as_ref()
            .filter(|r| r.process.is_alive())
            .map(|r| r.process.clone())
            .ok_or_else(|| {
                ChatError::new(ChatErrorKind::ProcessExit, Error::Pi(PiFailure::NotRunning))
            })
    }

    /// 空闲回收：就绪且空闲达到 `pi_idle_timeout_secs` 秒时有序关闭。0 表示不回收。
    pub fn spawn_idle_reaper(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let sup = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                let Some(this) = sup.upgrade() else { break };
                let tick = this.cfg.idle_tick;
                drop(this);
                tokio::time::sleep(tick).await;
                let Some(this) = sup.upgrade() else { break };
                if this.is_busy() || !this.is_running().await {
                    continue;
                }
                let db_path = this.cfg.db_path.clone();
                let limit = tokio::task::spawn_blocking(move || {
                    rrss_core::db::open_at(&db_path)
                        .and_then(|conn| rrss_core::pi_store::idle_timeout_secs(&conn))
                        .unwrap_or(rrss_core::pi_store::DEFAULT_IDLE_TIMEOUT_SECS)
                })
                .await
                .unwrap_or(rrss_core::pi_store::DEFAULT_IDLE_TIMEOUT_SECS);
                this.reap_if_idle(limit).await;
            }
        })
    }

    async fn reap_if_idle(&self, limit: i64) {
        if limit > 0
            && self.idle_for() >= Duration::from_secs(limit as u64)
            && self
                .busy
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            // 与 try_begin 共用原子占用；持有到关闭完成，中途不能开始发送。
            if self.idle_for() >= Duration::from_secs(limit as u64) {
                tracing::info!(idle_secs = limit, "pi idle; releasing");
                self.stop().await;
            }
            self.end();
        }
    }
}

/// 启动握手：核对模型，补全思考档位并把不支持的档位降到最接近的可用档。任一步失败都是配置错误。
/// 返回可用档位与 Pi 报告的模型。
async fn handshake(
    process: &PiProcess,
    prepared: &Prepared,
) -> Result<(Vec<String>, Option<String>), ChatError> {
    let failed = |step: ChatHandshakeStep, err: RpcError| {
        let message = match err {
            RpcError::Timeout => ChatUiError::handshake(step, false),
            RpcError::Closed => ChatUiError::handshake(step, true),
            RpcError::Rejected(reason) => ChatUiError::handshake_rejected(step, reason.as_deref()),
        };
        ChatError::new(ChatErrorKind::Config, message)
    };
    let state = process
        .request(cmd_get_state(), RPC_TIMEOUT)
        .await
        .map_err(|err| failed(ChatHandshakeStep::State, err))?;
    let actual = state_model(&state);
    match (&prepared.spec.expected_model, &actual) {
        (Some(expected), actual) if actual.as_deref() != Some(expected.as_str()) => {
            return Err(ChatError::new(
                ChatErrorKind::Config,
                ChatUiError::model_mismatch(expected, actual.as_deref()),
            ));
        }
        // 自有配置没选模型也没有默认模型：进程照常可用（可以列出模型），发送时再报错。
        _ => {}
    }
    let data = process
        .request(cmd_get_available_thinking_levels(), RPC_TIMEOUT)
        .await
        .map_err(|err| failed(ChatHandshakeStep::ThinkingLevels, err))?;
    let levels: Vec<String> = data
        .get("levels")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let Some(wanted) = prepared.profile.thinking.as_deref() else {
        return Ok((levels, actual));
    };
    if !levels.is_empty()
        && !levels.iter().any(|l| l == wanted)
        && let Some(closest) = closest_thinking_level(wanted, &levels)
    {
        tracing::warn!(
            wanted,
            closest,
            error_kind = "unsupported_thinking_level",
            "thinking level unsupported by model; downgrading"
        );
        process
            .request(cmd_set_thinking_level(closest), RPC_TIMEOUT)
            .await
            .map_err(|err| failed(ChatHandshakeStep::SetThinking, err))?;
    }
    Ok((levels, actual))
}

/// `get_state.model` 的 `provider/id`。
fn state_model(state: &Value) -> Option<String> {
    model_ref_of(state.get("model")?)
}

/// Pi 的 Model 对象 → `provider/id`。
fn model_ref_of(model: &Value) -> Option<String> {
    let provider = model.get("provider").and_then(Value::as_str)?;
    let id = model.get("id").and_then(Value::as_str)?;
    Some(format!("{provider}/{id}"))
}

/// 不高于 `wanted` 的最高可用档；没有就取最低的可用档。
pub fn closest_thinking_level<'a>(wanted: &str, available: &'a [String]) -> Option<&'a str> {
    // 档位由低到高，与模型页共用一份。
    let rank = |level: &str| {
        rrss_core::model_pool::THINKING_LEVELS
            .iter()
            .position(|l| *l == level)
    };
    let want = rank(wanted).unwrap_or(0);
    let ranked = || {
        available
            .iter()
            .filter_map(|l| rank(l).map(|r| (r, l.as_str())))
    };
    ranked()
        .filter(|(r, _)| *r <= want)
        .max_by_key(|(r, _)| *r)
        .or_else(|| ranked().min_by_key(|(r, _)| *r))
        .map(|(_, l)| l)
}

fn is_cancelled(data: &Value) -> bool {
    data.get("cancelled").and_then(Value::as_bool) == Some(true)
}

fn rpc_error(err: RpcError) -> ChatError {
    match err {
        RpcError::Rejected(reason) => ChatError::new(
            ChatErrorKind::Config,
            ChatUiError::rpc_rejected(reason.as_deref()),
        ),
        RpcError::Timeout => {
            ChatError::new(ChatErrorKind::ProcessExit, Error::Pi(PiFailure::Timeout))
        }
        RpcError::Closed => ChatError::new(
            ChatErrorKind::ProcessExit,
            Error::Pi(PiFailure::ProcessExit),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn thinking_levels_downgrade_to_the_closest_available() {
        let levels = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let reasoning = levels(&["off", "minimal", "low", "medium"]);
        assert_eq!(closest_thinking_level("high", &reasoning), Some("medium"));
        assert_eq!(closest_thinking_level("low", &reasoning), Some("low"));
        assert_eq!(
            closest_thinking_level("medium", &levels(&["off"])),
            Some("off")
        );
        // 只有更高的档可用（不能关掉思考的模型）：取最低的那档。
        assert_eq!(
            closest_thinking_level("off", &levels(&["high", "max"])),
            Some("high")
        );
        assert_eq!(closest_thinking_level("low", &[]), None);
    }

    #[tokio::test]
    async fn reaper_and_sender_hold_the_same_gate_until_shutdown_finishes() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let sup = Supervisor::new(
            SupervisorConfig {
                db_path: PathBuf::new(),
                data_dir: PathBuf::new(),
                tool_path: PathBuf::new(),
                idle_tick: Duration::from_secs(1),
            },
            tx,
        );
        *sup.last_active.lock().unwrap() = Instant::now() - Duration::from_secs(10);
        let running = sup.running.lock().await;
        let reaper = tokio::spawn({
            let sup = sup.clone();
            async move { sup.reap_if_idle(1).await }
        });
        tokio::task::yield_now().await;
        assert!(
            sup.is_busy(),
            "reaper owns gate before waiting for running lock"
        );
        assert!(!sup.try_begin(), "send cannot race shutdown");
        drop(running);
        reaper.await.unwrap();
        assert!(sup.try_begin());
        *sup.last_active.lock().unwrap() = Instant::now() - Duration::from_secs(10);
        sup.reap_if_idle(1).await;
        assert!(sup.is_busy(), "reaper cannot release a sender's gate");
        sup.end();
    }

    #[test]
    fn state_model_reads_provider_and_id() {
        assert_eq!(
            state_model(&json!({"model": {"provider": "rrss", "id": "qwen2.5:7b", "api": "x"}})),
            Some("rrss/qwen2.5:7b".into())
        );
        assert_eq!(state_model(&json!({"model": null})), None);
        assert_eq!(state_model(&json!({})), None);
    }
}
