//! Backup validation reasons and compatibility with persisted restore notices.
#[derive(Debug, thiserror::Error)]
pub enum BackupProblem {
    #[error("backup path cannot be represented as UTF-8")]
    PathEncoding,
    #[error("MCP values cannot be stripped safely")]
    SecretStripping,
    #[error("backup manifest is missing")]
    ManifestMissing,
    #[error("backup manifest is unreadable")]
    ManifestInvalid,
    #[error("backup format is newer than supported")]
    FormatUnsupported,
    #[error("backup database is missing")]
    DatabaseMissing,
    #[error("backup database cannot be opened")]
    DatabaseUnreadable,
    #[error("backup database is corrupt")]
    DatabaseCorrupt,
    #[error("backup schema {found} is newer than supported {supported}")]
    SchemaUnsupported { found: i32, supported: i32 },
    #[error("backup database is empty")]
    DatabaseEmpty,
    #[error("backup schema {found} is older than minimum {min}")]
    SchemaTooOld { found: i32, min: i32 },
    #[error("staged backup is missing")]
    StagingMissing,
    #[error("archive is not a valid ZIP file")]
    ZipInvalid,
    #[error("backup schedule is unknown")]
    ScheduleUnknown { schedule: String },
    #[error("preferences file is invalid or damaged")]
    PreferencesInvalid,
    #[error("file is not an RRSS preferences export")]
    PreferencesKindInvalid,
    #[error("preferences format is newer than supported")]
    PreferencesFormatUnsupported,
    #[error("backup metadata serialization failed")]
    Serialization,
    #[error("selected file does not resolve to a local path")]
    SelectionPathInvalid,
    // Persisted restore notices still embed the original error text.
    #[error("{0}")]
    LegacyText(String),
}
impl From<&str> for BackupProblem {
    fn from(value: &str) -> Self {
        Self::LegacyText(value.into())
    }
}
impl From<String> for BackupProblem {
    fn from(value: String) -> Self {
        Self::LegacyText(value)
    }
}
