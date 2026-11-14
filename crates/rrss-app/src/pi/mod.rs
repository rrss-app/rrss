//! Pi 对话运行时（阶段 18）：`profile` 读期望配置，`adapter` 按版本渲染启动参数与文件，`workspace` 写托管目录，
//! `launch` 拉起进程，`supervisor` 管进程生命周期与握手，`turn` 结算一轮对话。
//! `rrss-desktop` 只把 [`turn::ChatEvent`] 映射成 `chat:*` 事件。

pub mod adapter;
pub mod compat;
pub mod context;
pub mod install;
pub mod launch;
pub mod profile;
pub mod supervisor;
pub mod turn;
pub mod workspace;

pub use supervisor::{ProfileState, Supervisor, SupervisorConfig};
pub use turn::{ChatEvent, ChatRuntime, ChatSink};

/// 对话错误的类别，随 `chat:error.kind` 发给前端。界面文案见阶段 19。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatErrorKind {
    /// 找不到对话引擎，或托管下载失败。
    NotInstalled,
    /// 对话引擎版本低于兼容下限（P3 起生效）。
    Version,
    /// 缺配置：未绑定对话模型、缺密钥、会话无法建立等。
    Config,
    /// 模型返回错误或自动重试最终失败；也包括本轮没有任何回复。
    Model,
    /// 对话引擎意外退出。
    ProcessExit,
    /// 上一轮尚未结束。
    Busy,
    /// 其余本地错误，例如写库失败。
    Internal,
}

impl ChatErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::Version => "version",
            Self::Config => "config",
            Self::Model => "model",
            Self::ProcessExit => "process_exit",
            Self::Busy => "busy",
            Self::Internal => "internal",
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct ChatError {
    pub kind: ChatErrorKind,
    pub message: crate::views::UiError,
}

impl ChatError {
    pub fn new(kind: ChatErrorKind, message: impl Into<crate::views::UiError>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn internal(kind: ChatErrorKind, error: impl Into<anyhow::Error>) -> Self {
        Self::new(kind, crate::ui_error::pi::PiUiError::project(error.into()))
    }

    /// 从 `anyhow` 错误里还原类别；没有标注的按 `fallback`。
    pub fn classify(err: &anyhow::Error, fallback: ChatErrorKind) -> Self {
        match err.downcast_ref::<ChatError>() {
            Some(e) => e.clone(),
            None => Self::new(fallback, crate::ui_error::pi::PiUiError::project_ref(err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_keeps_tagged_kind() {
        let tagged = anyhow::Error::new(ChatError::new(
            ChatErrorKind::NotInstalled,
            "fixture missing".to_string(),
        ));
        assert_eq!(
            ChatError::classify(&tagged, ChatErrorKind::Internal).kind,
            ChatErrorKind::NotInstalled
        );
        let plain = anyhow::anyhow!("disk full");
        assert_eq!(
            ChatError::classify(&plain, ChatErrorKind::Internal).kind,
            ChatErrorKind::Internal
        );
    }
}
