use super::{UiErrorKind, params};
use rrss_core::{Error, error::PiFailure};

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum ChatHandshakeStep {
    State,
    ThinkingLevels,
    SetThinking,
}

error_codes!(ChatUiError {
    RpcRejectedMissing => "pi_rpc_rejected_missing",
    RemoteReason { reason: String } => "chat_remote_reason",
    ModelSwitchRejectedMissing { expected: String } => "chat_model_switch_rejected_missing",
    ModelSwitchRejected { expected: String, reason: String } => "chat_model_switch_rejected",
    HandshakeRejectedMissing { step: ChatHandshakeStep } => "chat_handshake_rejected_missing",
    HandshakeRejected { step: ChatHandshakeStep, reason: String } => "chat_handshake_rejected",
    VersionUnverified { version: String, line: String } => "chat_version_unverified",
    SessionRestarted => "chat_session_restarted",
    CompactionFailed { reason: String } => "chat_compaction_failed",
    Compacting => "chat_compacting",
    Retrying { attempt: i64, max_attempts: i64 } => "chat_retrying",
    RetryFailed => "chat_retry_failed",

    OwnModelUnset => "pi_own_model_unset",
    SessionCancelled => "pi_session_cancelled",
    SessionPathMissing => "pi_session_path_missing",
    InputHandled => "pi_input_handled",
    NotRunning => "pi_not_running",
    Timeout => "pi_timeout",
    ProcessExit => "pi_process_exit",
    InputClosed => "pi_input_closed",
    PipeMissing => "pi_pipe_missing",
    ThreadGenerating => "chat_thread_generating",
    SendBusy => "chat_send_busy",
    ReplyPending => "chat_reply_pending",
    RestartBusy => "pi_restart_busy",
    RestartPending => "pi_restart_pending",
    ModelsBusy => "pi_models_busy",
    ModelsPending => "pi_models_pending",
    ModelErrorMissing => "pi_model_error_missing",
    ReplyEmpty => "pi_reply_empty",
    ExitEmpty => "pi_exit_empty",
    ExitPartial => "pi_exit_partial",
    EngineStartFailed => "chat_engine_start_failed",
    ConfigWriteFailed => "chat_config_write_failed",
    SessionSaveFailed => "chat_session_save_failed",
    ReplySaveFailed => "chat_reply_save_failed",
    HandshakeTimeout { step: ChatHandshakeStep } => "chat_handshake_timeout",
    HandshakeClosed { step: ChatHandshakeStep } => "chat_handshake_closed",
    ModelMismatch { expected: String, actual: Option<String> } => "chat_model_mismatch",
    ThreadNotFound => "thread_not_found",
    MemoryNotFound => "memory_not_found",
    InterestNotFound => "interest_not_found",
    McpServerNotFound => "mcp_server_not_found",
    PiToolNotFound { name: String } => "pi_tool_not_found",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    Some(UiErrorKind::Chat(match error {
        Error::Pi(reason) => match reason {
            PiFailure::OwnModelUnset => ChatUiError::OwnModelUnset,
            PiFailure::SessionCancelled => ChatUiError::SessionCancelled,
            PiFailure::SessionPathMissing => ChatUiError::SessionPathMissing,
            PiFailure::InputHandled => ChatUiError::InputHandled,
            PiFailure::NotRunning => ChatUiError::NotRunning,
            PiFailure::Timeout => ChatUiError::Timeout,
            PiFailure::ProcessExit => ChatUiError::ProcessExit,
            PiFailure::InputClosed => ChatUiError::InputClosed,
            PiFailure::PipeMissing => ChatUiError::PipeMissing,
            PiFailure::ThreadGenerating => ChatUiError::ThreadGenerating,
            PiFailure::SendBusy => ChatUiError::SendBusy,
            PiFailure::ReplyPending => ChatUiError::ReplyPending,
            PiFailure::RestartBusy => ChatUiError::RestartBusy,
            PiFailure::RestartPending => ChatUiError::RestartPending,
            PiFailure::ModelsBusy => ChatUiError::ModelsBusy,
            PiFailure::ModelsPending => ChatUiError::ModelsPending,
            PiFailure::ModelErrorMissing => ChatUiError::ModelErrorMissing,
            PiFailure::ReplyEmpty => ChatUiError::ReplyEmpty,
            PiFailure::ExitEmpty => ChatUiError::ExitEmpty,
            PiFailure::ExitPartial => ChatUiError::ExitPartial,
            _ => return None,
        },
        Error::ThreadNotFound => ChatUiError::ThreadNotFound,
        Error::MemoryNotFound => ChatUiError::MemoryNotFound,
        Error::InterestNotFound => ChatUiError::InterestNotFound,
        Error::McpServerNotFound => ChatUiError::McpServerNotFound,
        Error::PiToolNotFound(name) => ChatUiError::PiToolNotFound {
            name: params::name(name),
        },
        _ => return None,
    }))
}

impl ChatUiError {
    pub(crate) fn rpc_rejected(reason: Option<&str>) -> super::UiError {
        match reason {
            Some(reason) => Self::remote_reason(reason),
            None => Self::RpcRejectedMissing.failure("engine command rejected without a reason"),
        }
    }
    pub(crate) fn remote_reason(reason: &str) -> super::UiError {
        Self::RemoteReason {
            reason: params::remote_reason(reason),
        }
        .failure("engine or model request rejected")
    }
    pub(crate) fn model_switch_rejected(expected: &str, reason: Option<&str>) -> super::UiError {
        let Some(reason) = reason else {
            return Self::ModelSwitchRejectedMissing {
                expected: params::name(expected),
            }
            .failure("engine model switch rejected without a reason");
        };
        Self::ModelSwitchRejected {
            expected: params::name(expected),
            reason: params::remote_reason(reason),
        }
        .failure("engine model switch rejected")
    }
    pub(crate) fn handshake_rejected(
        step: ChatHandshakeStep,
        reason: Option<&str>,
    ) -> super::UiError {
        let Some(reason) = reason else {
            return Self::HandshakeRejectedMissing { step }
                .failure("engine handshake rejected without a reason");
        };
        Self::HandshakeRejected {
            step,
            reason: params::remote_reason(reason),
        }
        .failure("engine handshake rejected")
    }
    pub(crate) fn compaction_failed(reason: &str) -> super::UiError {
        Self::CompactionFailed {
            reason: params::remote_reason(reason),
        }
        .failure("engine compaction failed")
    }
    pub(crate) fn version_notice(version: &str, line: &str) -> super::UiError {
        Self::VersionUnverified {
            version: params::name(version),
            line: params::name(line),
        }
        .failure("engine version is unverified")
    }

    pub fn failure(self, detail: impl AsRef<str>) -> super::UiError {
        let config_write = matches!(self, Self::ConfigWriteFailed);
        let mut error = super::UiError::new(UiErrorKind::Chat(self), detail);
        if config_write {
            error.diagnostic = Some("could not write pi workspace configuration".into());
        }
        error
    }
    pub fn caused_by(self, cause: super::UiError) -> super::UiError {
        let mut error = self.failure(&cause.detail);
        error.cause = Some(Box::new(cause));
        error
    }
    pub fn handshake(step: ChatHandshakeStep, closed: bool) -> super::UiError {
        if closed {
            Self::HandshakeClosed { step }
        } else {
            Self::HandshakeTimeout { step }
        }
        .failure("pi startup check failed")
    }
    pub fn model_mismatch(expected: &str, actual: Option<&str>) -> super::UiError {
        Self::ModelMismatch {
            expected: params::name(expected),
            actual: actual.map(params::name),
        }
        .failure("pi did not load the configured model")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chat_codes_keep_operational_failures_structured() {
        for (reason, code) in [
            (PiFailure::ThreadGenerating, "chat_thread_generating"),
            (PiFailure::SendBusy, "chat_send_busy"),
            (PiFailure::ReplyPending, "chat_reply_pending"),
            (PiFailure::ReplyEmpty, "pi_reply_empty"),
            (PiFailure::SessionPathMissing, "pi_session_path_missing"),
        ] {
            assert_eq!(super::super::UiError::from(Error::Pi(reason)).code(), code);
        }
        let wire = serde_json::to_value(ChatUiError::model_mismatch("configured", None)).unwrap();
        assert_eq!(wire["code"], "chat_model_mismatch");
        assert!(wire["params"]["actual"].is_null());
        for (kind, code) in [
            (ChatUiError::EngineStartFailed, "chat_engine_start_failed"),
            (ChatUiError::ConfigWriteFailed, "chat_config_write_failed"),
            (ChatUiError::SessionSaveFailed, "chat_session_save_failed"),
            (ChatUiError::ReplySaveFailed, "chat_reply_save_failed"),
        ] {
            assert_eq!(kind.failure("fixture IO error").code(), code);
        }
        assert_eq!(
            ChatUiError::handshake(ChatHandshakeStep::State, false).code(),
            "chat_handshake_timeout"
        );
        assert_eq!(
            ChatUiError::handshake(ChatHandshakeStep::State, true).code(),
            "chat_handshake_closed"
        );
    }
}

#[cfg(test)]
#[test]
fn persistence_failures_retain_the_typed_cause() {
    let error =
        ChatUiError::SessionSaveFailed.caused_by(super::UiError::from(Error::SchemaTooOld {
            found: 1,
            min: 38,
        }));
    let wire = serde_json::to_value(error).unwrap();
    assert_eq!(wire["code"], "chat_session_save_failed");
    assert_eq!(wire["cause"]["code"], "schema_too_old");
    assert_eq!(wire["cause"]["params"]["found"], 1);
}

pub(super) fn from_rpc(error: &rrss_ai::pi::rpc::RpcError) -> super::UiError {
    match error {
        rrss_ai::pi::rpc::RpcError::Timeout => {
            ChatUiError::Timeout.failure("Pi command response timed out")
        }
        rrss_ai::pi::rpc::RpcError::Closed => ChatUiError::ProcessExit.failure("Pi process exited"),
        rrss_ai::pi::rpc::RpcError::Rejected(reason) => {
            ChatUiError::rpc_rejected(reason.as_deref())
        }
    }
}

#[cfg(test)]
#[test]
fn rpc_rejections_keep_absence_separate_from_remote_text() {
    let absent = from_rpc(&rrss_ai::pi::rpc::RpcError::Rejected(None));
    assert_eq!(absent.code(), "pi_rpc_rejected_missing");
    let marker = "pi_rpc_rejected_missing";
    let remote = from_rpc(&rrss_ai::pi::rpc::RpcError::Rejected(Some(marker.into())));
    assert_eq!(remote.code(), "chat_remote_reason");
    assert_eq!(
        serde_json::to_value(remote).unwrap()["params"]["reason"],
        marker
    );
    assert_eq!(
        ChatUiError::model_switch_rejected("fixture", None).code(),
        "chat_model_switch_rejected_missing"
    );
    assert_eq!(
        ChatUiError::handshake_rejected(ChatHandshakeStep::State, None).code(),
        "chat_handshake_rejected_missing"
    );
}
