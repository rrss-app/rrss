use super::{UiErrorKind, params};
use rrss_core::Error;

error_codes!(CommonError {
    LegacyText {} => "legacy_text",
    Internal => "internal",
    DatabaseBusy => "database_busy",
    Database => "database",
    Io {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        file: Option<String>,
    } => "io",
    DataDirUnavailable => "data_dir_unavailable",
    SchemaUnsupported { found: i32, supported: i32 } => "schema_unsupported",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    let kind = match error {
        Error::DataDirUnavailable => CommonError::DataDirUnavailable,
        Error::UnsupportedSchema { found, supported } => CommonError::SchemaUnsupported {
            found: *found,
            supported: *supported,
        },
        Error::CreateDir { path, .. } => CommonError::Io {
            file: params::file_name(path),
        },
        Error::Io(_) => CommonError::Io { file: None },
        Error::Sqlite(rusqlite::Error::SqliteFailure(error, _))
            if matches!(
                error.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) =>
        {
            CommonError::DatabaseBusy
        }
        Error::Sqlite(_) => CommonError::Database,
        _ => return None,
    };
    Some(UiErrorKind::Common(kind))
}
