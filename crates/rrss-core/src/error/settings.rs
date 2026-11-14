//! Settings validation reasons and legacy display compatibility.
#[derive(Debug, Clone, thiserror::Error)]
pub enum SettingProblem {
    #[error("setting is unknown")]
    Unknown { id: String },
    #[error("setting requires a boolean")]
    BooleanRequired { id: String },
    #[error("setting requires an integer")]
    IntegerRequired { value: String },
    #[error("setting is outside its allowed range")]
    OutOfRange { min: i64, max: i64 },
    #[error("spot check rate is unsupported")]
    SpotCheckRateInvalid,
    #[error("setting text is too long")]
    TextTooLong { max: usize },
    #[error("setting value is not an allowed option")]
    OptionInvalid { value: String, options: Vec<String> },
    #[error("pinned scopes are invalid")]
    PinnedScopesInvalid,
    #[error("focus column scopes are invalid")]
    FocusColumnsInvalid,
    #[error("vector dimensions are invalid")]
    VectorDimsInvalid,
    #[error("vector data cannot be decoded for local dimension reduction")]
    VectorDataInvalid,
    #[error("vector space must be restored or rebuilt before changing dimensions")]
    VectorSpaceStale,
    #[error("translation configuration revision is exhausted")]
    TranslationRevisionExhausted,
    #[error("ranking switches require an independent transaction")]
    RankTransactionRequired,
    #[error("ranking switch is unknown")]
    RankSwitchUnknown,
    // Translation and MCP failures can still carry their original display text.
    #[error("{0}")]
    LegacyText(String),
}
impl From<&str> for SettingProblem {
    fn from(value: &str) -> Self {
        Self::LegacyText(value.into())
    }
}
impl From<String> for SettingProblem {
    fn from(value: String) -> Self {
        Self::LegacyText(value)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PiOverrideProblem {
    #[error("assistant note is too long")]
    NoteTooLong { max: usize },
    #[error("Pi override key is unknown")]
    KeyUnknown { key: String },
    #[error("Pi override requires a JSON object")]
    ObjectRequired,
    #[error("Pi model must use provider/id syntax")]
    ModelInvalid { model: String },
    // JSON parser diagnostics may echo input; preserve the old UI until a safe
    // parser-reason contract can reproduce the full diagnostic exactly.
    #[error("{0}")]
    LegacyText(String),
}
impl From<String> for PiOverrideProblem {
    fn from(value: String) -> Self {
        Self::LegacyText(value)
    }
}
