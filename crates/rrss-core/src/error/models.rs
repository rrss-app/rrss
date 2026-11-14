//! Model validation reasons; user-facing wording belongs to the UI.
#[derive(Debug, thiserror::Error)]
pub enum ProviderProblem {
    #[error("provider name is empty")]
    NameMissing,
    #[error("provider URL is invalid")]
    UrlInvalid,
    #[error("provider URL contains unsupported components")]
    UrlComponentsInvalid,
    #[error("provider protocol is unsupported")]
    ProtocolUnsupported,
    #[error("provider preset is unknown")]
    PresetUnknown,
    #[error("model identifier is empty")]
    ModelIdMissing,
    #[error("chat model is controlled by the user's Pi configuration")]
    ChatUsesOwnConfig,
    // Compatibility for callers that still supply sentence-style validation.
    #[error("{0}")]
    LegacyText(String),
}
impl From<&str> for ProviderProblem {
    fn from(value: &str) -> Self {
        Self::LegacyText(value.into())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ModelChoiceProblem {
    #[error("model has not been added to the pool")]
    NotAdded { model: String },
    #[error("model is not enabled for chat")]
    ChatDisabled { model: String },
    #[error("replacement model is absent from the pool")]
    ReplacementMissing { model: String },
    #[error("model is absent from the pool")]
    NotFound { model: String },
    #[error("chat default must be replaced before disabling chat")]
    ChatDefaultRequired,
    #[error("thinking level is unknown")]
    ThinkingUnknown { level: String },
    #[error("context window must be positive")]
    ContextWindowNonPositive,
    #[error("maximum output must be positive")]
    MaxOutputNonPositive,
    #[error("select a model or clear the task")]
    SelectionIncomplete,
    #[error("vector rebuild requires an embedding model")]
    RebuildModelMissing,
}

#[derive(Debug, thiserror::Error)]
pub enum CompatProblem {
    #[error("compatibility value must be a JSON object")]
    ObjectRequired,
    #[error("provider compatibility configuration must be a JSON object")]
    ProviderObjectRequired,
    #[error("model compatibility setting must be a JSON object")]
    ModelObjectRequired,
    #[error("provider preset compatibility configuration is invalid")]
    PresetInvalid,
}

#[derive(Debug, thiserror::Error)]
pub enum TaskModelLabel {
    #[error("task {0}")]
    Id(String),
    #[error("task named {0}")]
    Name(String),
}
impl From<&str> for TaskModelLabel {
    fn from(value: &str) -> Self {
        Self::Name(value.into())
    }
}
impl From<String> for TaskModelLabel {
    fn from(value: String) -> Self {
        Self::Name(value)
    }
}
