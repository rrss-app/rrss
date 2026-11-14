//! IPC failures: domain codes, safe parameters and diagnostic text.
use crate::views::{UiError, UiErrorKind};
use rrss_core::{Error, logsafe};

// The same declaration supplies serde, TS, code lookup and the exported code list.
// Add variants in the owning domain, rather than maintaining another registry.
macro_rules! error_codes {
    ($name:ident { $($variant:ident $({ $($(#[$attr:meta])* $field:ident: $ty:ident $(<$inner:ty>)?),* $(,)? })? => $code:literal),* $(,)? }) => {
        #[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ts_rs::TS)]
        #[serde(tag = "code", content = "params", rename_all = "snake_case")]
        pub enum $name {
            $(#[serde(rename = $code)] $variant $({ $($(#[$attr])* $field: $ty $(<$inner>)?),* })?),*
        }
        impl $name {
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

    // Typed failures retain their safe projection. Arbitrary legacy strings may
    // contain a model dump, so they still get only a fixed diagnostic.
    pub fn log_detail(&self) -> String {
        if let Some(diagnostic) = &self.diagnostic {
            diagnostic.clone()
        } else if self.code() == "legacy_text" {
            "legacy command error".into()
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
                UiErrorKind::Common(common::CommonError::LegacyText {}),
                error.to_string(),
            ),
        };
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
            return jobs::from_ai(ai, &detail);
        }
        let mut ui = match core {
            // A context can be the user's actionable message. Code only when
            // unwrapping the core failure preserves the old display projection.
            Some(core) if detail == core.to_string() => Self::from_core(core),
            _ => Self::from(detail),
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

impl From<String> for UiError {
    fn from(detail: String) -> Self {
        Self::new(
            UiErrorKind::Common(common::CommonError::LegacyText {}),
            detail,
        )
    }
}

impl std::fmt::Display for UiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}
impl std::error::Error for UiError {}
