//! IPC failures: domain codes, safe parameters and diagnostic text.
use crate::views::{UiError, UiErrorKind};
use rrss_core::{Error, logsafe};

// Only declared free text is scrubbed; enum identifiers and numbers are data.
macro_rules! scrub_error_param {
    ($field:expr, String, $secrets:expr) => {
        *$field = crate::ui_error::params::remote_reason(&rrss_core::logsafe::scrub_secrets_with(
            $field,
            $secrets.iter().copied(),
        ));
    };
    ($field:expr, Option<String>, $secrets:expr) => {
        if let Some(text) = $field {
            scrub_error_param!(text, String, $secrets);
        }
    };
    ($field:expr, $ty:ident $(<$inner:ident>)?, $secrets:expr) => {
        let _ = ($field, $secrets);
    };
}

// The same declaration supplies serde, TS, code lookup and the exported code list.
// Add variants in the owning domain, rather than maintaining another registry.
macro_rules! error_codes {
    ($name:ident { $($variant:ident $({ $($(#[$attr:meta])* $field:ident: $ty:ident $(<$inner:ident>)?),* $(,)? })? => $code:literal),* $(,)? }) => {
        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ts_rs::TS)]
        #[serde(tag = "code", content = "params", rename_all = "snake_case")]
        pub enum $name {
            $(#[serde(rename = $code)] $variant $({ $($(#[$attr])* $field: $ty $(<$inner>)?),* })?),*
        }
        impl $name {
            #[allow(dead_code)]
            pub(crate) fn scrub_parameters(&mut self, secrets: &[&str]) {
                let _ = secrets;
                match self {
                    $(Self::$variant $({ $($field,)* })? => {
                        $($(scrub_error_param!($field, $ty $(<$inner>)?, secrets);)*)?
                    }),*
                }
            }
            pub const CODES: &'static [&'static str] = &[$($code),*];
            pub fn code(&self) -> &'static str {
                match self { $(Self::$variant $({ $($field: _,)* })? => $code),* }
            }
        }
    };
}

pub mod backup;
pub mod chat;
#[cfg(test)]
mod codes;
pub mod common;
pub mod feeds;
pub mod folders;
pub mod host;
pub mod jobs;
pub mod mcp;
pub mod models;
pub mod network;
pub(crate) mod params;
pub mod pi;
pub mod reader;
pub mod rules;
pub mod settings;
#[cfg(test)]
pub(crate) mod test_display;
#[cfg(test)]
mod tests;
pub mod web;

impl UiError {
    pub fn new(kind: UiErrorKind, detail: impl AsRef<str>) -> Self {
        Self {
            kind,
            detail: logsafe::scrub_secrets(detail.as_ref()),
            diagnostic: None,
            cause: None,
        }
    }

    /// Text written by an older version (stored feed errors). New
    /// failures always carry a code; this is the only remaining legacy source.
    pub(crate) fn stored_legacy(text: &str) -> Self {
        Self::new(
            UiErrorKind::Common(common::CommonError::LegacyText {}),
            text,
        )
    }

    pub fn code(&self) -> &'static str {
        match &self.kind {
            UiErrorKind::Common(kind) => kind.code(),
            UiErrorKind::Feeds(kind) => kind.code(),
            UiErrorKind::Folders(kind) => kind.code(),
            UiErrorKind::Rules(kind) => kind.code(),
            UiErrorKind::Models(kind) => kind.code(),
            UiErrorKind::Chat(kind) => kind.code(),
            UiErrorKind::Reader(kind) => kind.code(),
            UiErrorKind::Web(kind) => kind.code(),
            UiErrorKind::Host(kind) => kind.code(),
            UiErrorKind::Network(kind) => kind.code(),
            UiErrorKind::Settings(kind) => kind.code(),
            UiErrorKind::Backup(kind) => kind.code(),
            UiErrorKind::Pi(kind) => kind.code(),
            UiErrorKind::Mcp(kind) => kind.code(),
            UiErrorKind::Jobs(kind) => kind.code(),
        }
    }

    // Typed failures retain their safe projection. Stored legacy text may
    // contain a model dump, so it gets only a fixed diagnostic.
    pub fn log_detail(&self) -> String {
        if let Some(diagnostic) = &self.diagnostic {
            diagnostic.clone()
        } else if self.code() == "legacy_text" {
            "legacy stored error".into()
        } else {
            logsafe::text(&self.detail, 500)
        }
    }

    pub(crate) fn from_core(error: &Error) -> Self {
        let kind = common::classify(error)
            .or_else(|| feeds::classify(error))
            .or_else(|| folders::classify(error))
            .or_else(|| rules::classify(error))
            .or_else(|| models::classify(error))
            .or_else(|| settings::classify(error))
            .or_else(|| backup::classify(error))
            .or_else(|| chat::classify(error))
            .or_else(|| reader::classify(error))
            .or_else(|| network::classify(error))
            .or_else(|| pi::classify(error))
            .or_else(|| mcp::classify(error))
            .or_else(|| jobs::classify(error))
            .or_else(|| web::classify(error))
            .or_else(|| host::classify(error));
        let diagnostic = logsafe::error(error);
        let mut ui = match kind {
            Some(kind) => Self::new(kind, &diagnostic),
            None => Self::new(
                UiErrorKind::Common(common::CommonError::Internal),
                &diagnostic,
            ),
        };
        ui.cause = mcp::cause(error);
        ui.diagnostic = Some(diagnostic);
        ui
    }

    // Use at the command sites that previously displayed anyhow's full chain.
    pub fn from_anyhow_pretty(error: anyhow::Error) -> Self {
        let detail = format!("{error:#}");
        Self::from_anyhow(&error, detail)
    }

    pub(crate) fn from_anyhow_ref(error: &anyhow::Error) -> Self {
        Self::from_anyhow(error, error.to_string())
    }

    fn from_anyhow(error: &anyhow::Error, detail: String) -> Self {
        if let Some(ui) = error
            .downcast_ref::<Self>()
            .or_else(|| error.chain().find_map(|cause| cause.downcast_ref::<Self>()))
        {
            return ui.clone();
        }
        if let Some(rpc) = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<rrss_ai::pi::rpc::RpcError>())
        {
            return chat::from_rpc(rpc);
        }
        let core = error.downcast_ref::<Error>().or_else(|| {
            error
                .chain()
                .find_map(|cause| cause.downcast_ref::<Error>())
        });
        if let Some(core) = core
            && matches!(core, Error::Pi(_) | Error::Mcp(_) | Error::Jobs(_))
        {
            return Self::from_core(core);
        }
        if core.is_none()
            && let Some(ai) = error.downcast_ref::<rrss_ai::Error>().or_else(|| {
                error
                    .chain()
                    .find_map(|cause| cause.downcast_ref::<rrss_ai::Error>())
            })
        {
            return jobs::from_ai(ai);
        }
        // Contexts are English diagnostics; the wrapped core failure owns the code.
        let mut ui = match core {
            Some(core) => Self::from_core(core),
            None => Self::new(UiErrorKind::Common(common::CommonError::Internal), detail),
        };
        ui.diagnostic = Some(crate::log::error(error.as_ref()));
        ui
    }
}

impl From<Error> for UiError {
    fn from(error: Error) -> Self {
        Self::from_core(&error)
    }
}

impl From<anyhow::Error> for UiError {
    fn from(error: anyhow::Error) -> Self {
        Self::from_anyhow_ref(&error)
    }
}

impl From<rusqlite::Error> for UiError {
    fn from(error: rusqlite::Error) -> Self {
        Self::from(Error::Sqlite(error))
    }
}

impl std::fmt::Display for UiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}
impl std::error::Error for UiError {}
