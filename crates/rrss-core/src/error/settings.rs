//! Settings validation reasons; user-facing wording belongs to the UI.
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
}

#[derive(Debug, thiserror::Error)]
pub enum PiOverrideProblem {
    #[error("assistant note is too long")]
    NoteTooLong { max: usize },
    #[error("Pi override key is unknown")]
    KeyUnknown { key: String },
    #[error("Pi override requires a JSON object")]
    ObjectRequired,
    #[error("Pi override is not valid JSON")]
    JsonInvalid { line: usize, column: usize },
    #[error("Pi model must use provider/id syntax")]
    ModelInvalid { model: String },
}
