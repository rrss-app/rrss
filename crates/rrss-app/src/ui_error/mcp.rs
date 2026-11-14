use super::UiErrorKind;
use rrss_core::{Error, error::McpFailure};

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum McpProbeHint {
    Sse,
    Auth,
    Local,
    Remote,
}

error_codes!(McpUiError {
    ConfigurationInvalid => "mcp_configuration_invalid",
    VariableNameInvalid => "mcp_variable_name_invalid",
    VariableValueInvalid => "mcp_variable_value_invalid",
    VariableDuplicate => "mcp_variable_duplicate",
    NameInvalid => "mcp_name_invalid",
    TransportInvalid => "mcp_transport_invalid",
    AuthInvalid => "mcp_auth_invalid",
    OAuthTransport => "mcp_o_auth_transport",
    ToolWildcard => "mcp_tool_wildcard",
    OAuthHeader => "mcp_o_auth_header",
    CommandInvalid => "mcp_command_invalid",
    UrlInvalid => "mcp_url_invalid",
    UrlCredentials => "mcp_url_credentials",
    NameDuplicate => "mcp_name_duplicate",
    JsonInvalid => "mcp_json_invalid",
    ServersObjectMissing => "mcp_servers_object_missing",
    ServersEmpty => "mcp_servers_empty",
    ServerObjectRequired => "mcp_server_object_required",
    TextFieldInvalid => "mcp_text_field_invalid",
    ArrayFieldInvalid => "mcp_array_field_invalid",
    MapFieldInvalid => "mcp_map_field_invalid",
    EnableLoginRequired => "mcp_enable_login_required",
    Import { name: String } => "mcp_import_invalid",
    InitializeReason { reason: String } => "mcp_initialize_reason",
    ToolsReason { reason: String } => "mcp_tools_reason",
    LoggedIn => "mcp_logged_in",
    InitializeRejected => "mcp_initialize_rejected",
    ToolsRejected => "mcp_tools_rejected",
    MessageTooLarge => "mcp_message_too_large",
    CommandPathInvalid => "mcp_command_path_invalid",
    SpawnFailed => "mcp_spawn_failed",
    ProcessExit { elapsed_ms: u64, status: i32 } => "mcp_process_exit",
    ProcessKilled { elapsed_ms: u64 } => "mcp_process_killed",
    OutputClosed => "mcp_output_closed",
    HttpStatus { status: u16 } => "mcp_http_status",
    ConnectFailed => "mcp_connect_failed",
    ResponseTooLarge => "mcp_response_too_large",
    StreamClosed => "mcp_stream_closed",
    ResponseInvalid => "mcp_response_invalid",
    ProtocolMissing => "mcp_protocol_missing",
    ProtocolUnsupported => "mcp_protocol_unsupported",
    ToolsMissing => "mcp_tools_missing",
    ToolNameMissing => "mcp_tool_name_missing",
    CursorStalled => "mcp_cursor_stalled",
    PageLimit => "mcp_page_limit",
    SseUnsupported => "mcp_sse_unsupported",
    OwnConfigProbe => "mcp_own_config_probe",
    ProbeTimeout => "mcp_probe_timeout",
    PiMissing => "mcp_pi_missing",
    PiPathInvalid => "mcp_pi_path_invalid",
    LoginCancelled => "mcp_login_cancelled",
    LoginSpawnFailed => "mcp_login_spawn_failed",
    LoginProcessFailed => "mcp_login_process_failed",
    LoginTimeout => "mcp_login_timeout",
    LoginBusy => "mcp_login_busy",
    OwnConfigLogin => "mcp_own_config_login",
    LoginTransport => "mcp_login_transport",
    LoginFailed => "mcp_login_failed",
    LoginConfigChanged => "mcp_login_config_changed",
    LoginNotSaved => "mcp_login_not_saved",
    Deleting => "mcp_deleting",
    LoginRequired => "mcp_login_required",
    CredentialsUnavailable => "mcp_credentials_unavailable",
    LoginExpired => "mcp_login_expired",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    let Error::Mcp(reason) = error else {
        return None;
    };
    Some(UiErrorKind::Mcp(match reason {
        McpFailure::ConfigurationInvalid => McpUiError::ConfigurationInvalid,
        McpFailure::VariableNameInvalid => McpUiError::VariableNameInvalid,
        McpFailure::VariableValueInvalid => McpUiError::VariableValueInvalid,
        McpFailure::VariableDuplicate => McpUiError::VariableDuplicate,
        McpFailure::NameInvalid => McpUiError::NameInvalid,
        McpFailure::TransportInvalid => McpUiError::TransportInvalid,
        McpFailure::AuthInvalid => McpUiError::AuthInvalid,
        McpFailure::OAuthTransport => McpUiError::OAuthTransport,
        McpFailure::ToolWildcard => McpUiError::ToolWildcard,
        McpFailure::OAuthHeader => McpUiError::OAuthHeader,
        McpFailure::CommandInvalid => McpUiError::CommandInvalid,
        McpFailure::UrlInvalid => McpUiError::UrlInvalid,
        McpFailure::UrlCredentials => McpUiError::UrlCredentials,
        McpFailure::NameDuplicate => McpUiError::NameDuplicate,
        McpFailure::JsonInvalid => McpUiError::JsonInvalid,
        McpFailure::ServersObjectMissing => McpUiError::ServersObjectMissing,
        McpFailure::ServersEmpty => McpUiError::ServersEmpty,
        McpFailure::ServerObjectRequired => McpUiError::ServerObjectRequired,
        McpFailure::TextFieldInvalid => McpUiError::TextFieldInvalid,
        McpFailure::ArrayFieldInvalid => McpUiError::ArrayFieldInvalid,
        McpFailure::MapFieldInvalid => McpUiError::MapFieldInvalid,
        McpFailure::EnableLoginRequired => McpUiError::EnableLoginRequired,
        McpFailure::Import { name, .. } => McpUiError::Import {
            name: super::params::name(name),
        },
        McpFailure::MessageTooLarge => McpUiError::MessageTooLarge,
        McpFailure::CommandPathInvalid => McpUiError::CommandPathInvalid,
        McpFailure::SpawnFailed => McpUiError::SpawnFailed,
        McpFailure::ProcessExit { elapsed_ms, status } => McpUiError::ProcessExit {
            elapsed_ms: *elapsed_ms,
            status: *status,
        },
        McpFailure::ProcessKilled { elapsed_ms } => McpUiError::ProcessKilled {
            elapsed_ms: *elapsed_ms,
        },
        McpFailure::OutputClosed => McpUiError::OutputClosed,
        McpFailure::HttpStatus { status } => McpUiError::HttpStatus { status: *status },
        McpFailure::ConnectFailed => McpUiError::ConnectFailed,
        McpFailure::ResponseTooLarge => McpUiError::ResponseTooLarge,
        McpFailure::StreamClosed => McpUiError::StreamClosed,
        McpFailure::ResponseInvalid => McpUiError::ResponseInvalid,
        McpFailure::ProtocolMissing => McpUiError::ProtocolMissing,
        McpFailure::ProtocolUnsupported => McpUiError::ProtocolUnsupported,
        McpFailure::ToolsMissing => McpUiError::ToolsMissing,
        McpFailure::ToolNameMissing => McpUiError::ToolNameMissing,
        McpFailure::CursorStalled => McpUiError::CursorStalled,
        McpFailure::PageLimit => McpUiError::PageLimit,
        McpFailure::SseUnsupported => McpUiError::SseUnsupported,
        McpFailure::OwnConfigProbe => McpUiError::OwnConfigProbe,
        McpFailure::ProbeTimeout => McpUiError::ProbeTimeout,
        McpFailure::PiMissing => McpUiError::PiMissing,
        McpFailure::PiPathInvalid => McpUiError::PiPathInvalid,
        McpFailure::LoginCancelled => McpUiError::LoginCancelled,
        McpFailure::LoginSpawnFailed => McpUiError::LoginSpawnFailed,
        McpFailure::LoginProcessFailed => McpUiError::LoginProcessFailed,
        McpFailure::LoginTimeout => McpUiError::LoginTimeout,
        McpFailure::LoginBusy => McpUiError::LoginBusy,
        McpFailure::OwnConfigLogin => McpUiError::OwnConfigLogin,
        McpFailure::LoginTransport => McpUiError::LoginTransport,
        McpFailure::LoginFailed => McpUiError::LoginFailed,
        McpFailure::LoginConfigChanged => McpUiError::LoginConfigChanged,
        McpFailure::LoginNotSaved => McpUiError::LoginNotSaved,
        McpFailure::Deleting => McpUiError::Deleting,
        McpFailure::LoginRequired => McpUiError::LoginRequired,
        McpFailure::CredentialsUnavailable => McpUiError::CredentialsUnavailable,
        McpFailure::LoginExpired => McpUiError::LoginExpired,
    }))
}

impl McpUiError {
    pub(crate) fn rejected(reason: &str, initialize: bool) -> super::UiError {
        let reason = super::params::remote_reason(reason);
        let kind = if initialize {
            Self::InitializeReason { reason }
        } else {
            Self::ToolsReason { reason }
        };
        kind.failure("MCP request rejected")
    }
    pub fn failure(self, detail: impl AsRef<str>) -> super::UiError {
        super::UiError::new(UiErrorKind::Mcp(self), detail)
    }
    pub fn project(error: anyhow::Error) -> super::UiError {
        if let Some(ui) = error.downcast_ref::<super::UiError>() {
            return ui.clone();
        }
        if error.downcast_ref::<Error>().is_some()
            || error
                .chain()
                .any(|cause| cause.downcast_ref::<Error>().is_some())
        {
            return super::UiError::from(error);
        }
        // IO, header parsing and worker joins had only technical diagnostics.
        super::UiError::new(
            UiErrorKind::Common(super::common::CommonError::Internal),
            crate::log::error(error.as_ref()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mcp_failures_keep_their_codes_through_contexts() {
        let ui = McpUiError::rejected(
            "remote reason https://service.example/path?token=canary workspace=/tmp/private-path/safe-file",
            true,
        );
        let wire = serde_json::to_value(ui).unwrap();
        assert_eq!(wire["code"], "mcp_initialize_reason");
        assert!(
            !wire["params"]["reason"]
                .as_str()
                .unwrap()
                .contains("canary")
        );
        assert!(
            !wire["params"]["reason"]
                .as_str()
                .unwrap()
                .contains("private-path")
        );

        for (reason, code) in [
            (McpFailure::MessageTooLarge, "mcp_message_too_large"),
            (McpFailure::CommandPathInvalid, "mcp_command_path_invalid"),
            (McpFailure::SpawnFailed, "mcp_spawn_failed"),
            (McpFailure::OutputClosed, "mcp_output_closed"),
            (McpFailure::ConnectFailed, "mcp_connect_failed"),
            (McpFailure::ResponseTooLarge, "mcp_response_too_large"),
            (McpFailure::StreamClosed, "mcp_stream_closed"),
            (McpFailure::ResponseInvalid, "mcp_response_invalid"),
            (McpFailure::ProtocolMissing, "mcp_protocol_missing"),
            (McpFailure::ProtocolUnsupported, "mcp_protocol_unsupported"),
            (McpFailure::ToolsMissing, "mcp_tools_missing"),
            (McpFailure::ToolNameMissing, "mcp_tool_name_missing"),
            (McpFailure::CursorStalled, "mcp_cursor_stalled"),
            (McpFailure::PageLimit, "mcp_page_limit"),
            (McpFailure::SseUnsupported, "mcp_sse_unsupported"),
            (McpFailure::OwnConfigProbe, "mcp_own_config_probe"),
            (McpFailure::ProbeTimeout, "mcp_probe_timeout"),
            (McpFailure::PiMissing, "mcp_pi_missing"),
            (McpFailure::PiPathInvalid, "mcp_pi_path_invalid"),
            (McpFailure::LoginCancelled, "mcp_login_cancelled"),
            (McpFailure::LoginSpawnFailed, "mcp_login_spawn_failed"),
            (McpFailure::LoginProcessFailed, "mcp_login_process_failed"),
            (McpFailure::LoginTimeout, "mcp_login_timeout"),
            (McpFailure::LoginBusy, "mcp_login_busy"),
            (McpFailure::OwnConfigLogin, "mcp_own_config_login"),
            (McpFailure::LoginTransport, "mcp_login_transport"),
            (McpFailure::LoginFailed, "mcp_login_failed"),
            (McpFailure::LoginConfigChanged, "mcp_login_config_changed"),
            (McpFailure::LoginNotSaved, "mcp_login_not_saved"),
            (McpFailure::Deleting, "mcp_deleting"),
            (McpFailure::LoginRequired, "mcp_login_required"),
            (
                McpFailure::CredentialsUnavailable,
                "mcp_credentials_unavailable",
            ),
            (McpFailure::LoginExpired, "mcp_login_expired"),
        ] {
            let error = anyhow::anyhow!("private response canary").context(Error::Mcp(reason));
            let ui = super::super::UiError::from_anyhow_pretty(error);
            assert_eq!(ui.code(), code);
            assert!(!ui.detail.contains("canary"));
            assert!(!ui.log_detail().contains("canary"));
        }
        assert_eq!(
            McpUiError::InitializeRejected
                .failure("MCP initialize rejected")
                .code(),
            "mcp_initialize_rejected"
        );
        assert_eq!(
            McpUiError::ToolsRejected
                .failure("MCP tools request rejected")
                .code(),
            "mcp_tools_rejected"
        );
        let wire = serde_json::to_value(super::super::UiError::from(Error::Mcp(
            McpFailure::HttpStatus { status: 403 },
        )))
        .unwrap();
        assert_eq!(wire["code"], "mcp_http_status");
        assert_eq!(wire["params"]["status"], 403);
    }
}

pub(super) fn cause(error: &Error) -> Option<Box<super::UiError>> {
    if let Error::Mcp(McpFailure::Import { reason, .. }) = error {
        Some(Box::new(super::UiError::from(Error::Mcp(*reason.clone()))))
    } else {
        None
    }
}
