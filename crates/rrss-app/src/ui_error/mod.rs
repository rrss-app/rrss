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

pub mod chat;
#[cfg(test)]
mod codes;
pub mod common;
pub mod feeds;
pub mod folders;
pub mod models;
pub mod network;
mod params;
pub mod reader;
pub mod rules;
#[cfg(test)]
mod tests;

impl UiError {
    pub fn new(kind: UiErrorKind, detail: impl AsRef<str>) -> Self {
        Self {
            kind,
            detail: logsafe::scrub_secrets(detail.as_ref()),
            diagnostic: None,
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
            UiErrorKind::Network(kind) => kind.code(),
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

    fn from_core(error: &Error) -> Self {
        let kind = common::classify(error)
            .or_else(|| feeds::classify(error))
            .or_else(|| folders::classify(error))
            .or_else(|| rules::classify(error))
            .or_else(|| models::classify(error))
            .or_else(|| chat::classify(error))
            .or_else(|| reader::classify(error))
            .or_else(|| network::classify(error));
        let diagnostic = logsafe::error(error);
        let mut ui = match kind {
            Some(kind) => Self::new(kind, &diagnostic),
            None => {
                let source = matches!(error, Error::InvalidNetworkProxy(_))
                    .then_some(common::LegacyErrorSource::NetworkProxy);
                Self::new(
                    UiErrorKind::Common(common::CommonError::LegacyText { source }),
                    error.to_string(),
                )
            }
        };
        ui.diagnostic = Some(diagnostic);
        ui
    }

    // Use at the command sites that previously displayed anyhow's full chain.
    pub fn from_anyhow_pretty(error: anyhow::Error) -> Self {
        let detail = format!("{error:#}");
        Self::from_anyhow(error, detail)
    }

    fn from_anyhow(error: anyhow::Error, detail: String) -> Self {
        let core = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<Error>());
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
        let detail = error.to_string();
        Self::from_anyhow(error, detail)
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
            UiErrorKind::Common(common::CommonError::LegacyText { source: None }),
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
