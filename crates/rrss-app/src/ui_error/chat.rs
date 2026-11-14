use super::{UiErrorKind, params};
use rrss_core::Error;

error_codes!(ChatUiError {
    ThreadNotFound => "thread_not_found",
    MemoryNotFound => "memory_not_found",
    InterestNotFound => "interest_not_found",
    McpServerNotFound => "mcp_server_not_found",
    PiToolNotFound { name: String } => "pi_tool_not_found",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    Some(UiErrorKind::Chat(match error {
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
