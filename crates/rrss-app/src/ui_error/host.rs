use super::{UiError, UiErrorKind};
use rrss_core::{Error, error::HostFailure};

error_codes!(HostError {
    CommandForbidden => "host_command_forbidden",
    LogNotInitialized => "host_log_not_initialized",
    LogLevelUnavailable => "host_log_level_unavailable",
    LogUnavailable => "host_log_unavailable",
    TrayStateUnavailable => "host_tray_state_unavailable",
    LogExportNotInitialized => "host_log_export_not_initialized",
});

impl From<HostError> for UiError {
    fn from(error: HostError) -> Self {
        let detail = error.code();
        Self::new(UiErrorKind::Host(error), detail)
    }
}

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    let Error::Host(reason) = error else { return None };
    Some(UiErrorKind::Host(match reason {
        HostFailure::CommandForbidden => HostError::CommandForbidden,
        HostFailure::LogNotInitialized => HostError::LogNotInitialized,
        HostFailure::LogLevelUnavailable => HostError::LogLevelUnavailable,
        HostFailure::LogUnavailable => HostError::LogUnavailable,
    }))
}

#[cfg(test)]
#[test]
fn host_failures_keep_their_codes() {
    for (error, code) in [
        (HostError::LogNotInitialized, "host_log_not_initialized"),
        (HostError::LogLevelUnavailable, "host_log_level_unavailable"),
        (HostError::LogUnavailable, "host_log_unavailable"),
        (
            HostError::TrayStateUnavailable,
            "host_tray_state_unavailable",
        ),
        (
            HostError::LogExportNotInitialized,
            "host_log_export_not_initialized",
        ),
    ] {
        let ui = UiError::from(error);
        assert_eq!(ui.code(), code);
        assert!(ui.detail.is_ascii());
        assert_eq!(serde_json::to_value(ui).unwrap()["code"], code);
    }
}
