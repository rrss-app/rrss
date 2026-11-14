//! Rule and editable-memory validation reasons.
#[derive(Debug, thiserror::Error)]
pub enum RuleProblem {
    #[error("unknown polarity {value}")]
    PolarityUnknown { value: String },
    #[error("unknown dimension {value}")]
    DimensionUnknown { value: String },
    #[error("unknown scope {value}")]
    ScopeUnknown { value: String },
    #[error("unknown source {value}")]
    SourceUnknown { value: String },
    #[error("unknown message role {value}")]
    MessageRoleUnknown { value: String },
    #[error("empty pattern")]
    PatternMissing,
    #[error("invalid regex: {source}")]
    RegexInvalid { source: regex::Error },
    #[error("feed scope needs scope_id")]
    FeedScopeIdMissing,
    #[error("folder scope needs scope_id")]
    FolderScopeIdMissing,
    #[error("preset TOML is invalid")]
    PresetInvalid,
    #[error("invalid site domain")]
    SiteDomainInvalid,
    #[error("site rule serialization failed")]
    SiteSerialization,
    #[error("retention days out of range: {days}")]
    RetentionOutOfRange { days: i64 },
    #[error("retention value is not a positive integer")]
    RetentionInvalid { value: String },
    // Site-rule tests and digest generation still use their own error envelope.
    #[error("{0}")]
    LegacyText(String),
}
impl From<&str> for RuleProblem {
    fn from(value: &str) -> Self {
        Self::LegacyText(value.into())
    }
}
impl From<String> for RuleProblem {
    fn from(value: String) -> Self {
        Self::LegacyText(value)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MemoryProblem {
    #[error("memory kind is unknown")]
    KindUnknown { kind: String },
    #[error("memory content is empty")]
    ContentMissing,
    #[error("interest label is empty")]
    InterestLabelMissing,
    #[error("memory has already been confirmed")]
    AlreadyConfirmed,
}
