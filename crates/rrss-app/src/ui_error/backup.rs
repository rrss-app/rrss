use super::{UiErrorKind, params};
use rrss_core::{
    Error,
    error::{backup::BackupProblem, rules::RuleProblem},
};

error_codes!(BackupError {
    BackupPathEncoding => "backup_path_encoding",
    BackupSecretStripping => "backup_secret_stripping",
    BackupManifestMissing => "backup_manifest_missing",
    BackupManifestInvalid => "backup_manifest_invalid",
    BackupFormatUnsupported => "backup_format_unsupported",
    BackupDatabaseMissing => "backup_database_missing",
    BackupDatabaseUnreadable => "backup_database_unreadable",
    BackupDatabaseCorrupt => "backup_database_corrupt",
    BackupSchemaUnsupported { found: i32, supported: i32 } => "backup_schema_unsupported",
    BackupDatabaseEmpty => "backup_database_empty",
    BackupSchemaTooOld { found: i32, min: i32 } => "backup_schema_too_old",
    BackupStagingMissing => "backup_staging_missing",
    BackupZipInvalid => "backup_zip_invalid",
    BackupScheduleUnknown { schedule: String } => "backup_schedule_unknown",
    PreferencesInvalid => "preferences_invalid",
    PreferencesKindInvalid => "preferences_kind_invalid",
    PreferencesFormatUnsupported => "preferences_format_unsupported",
    BackupSelectionPathInvalid => "backup_selection_path_invalid",
    SchemaTooOld { found: i32, min: i32 } => "schema_too_old",
    CleanupUnknown { action: String } => "cleanup_unknown",
    RetentionOutOfRange { days: i64 } => "retention_out_of_range",
    RetentionInvalid { value: String } => "retention_invalid",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    Some(UiErrorKind::Backup(match error {
        Error::Backup(problem) => match problem {
            BackupProblem::PathEncoding => BackupError::BackupPathEncoding,
            BackupProblem::SecretStripping => BackupError::BackupSecretStripping,
            BackupProblem::ManifestMissing => BackupError::BackupManifestMissing,
            BackupProblem::ManifestInvalid => BackupError::BackupManifestInvalid,
            BackupProblem::FormatUnsupported => BackupError::BackupFormatUnsupported,
            BackupProblem::DatabaseMissing => BackupError::BackupDatabaseMissing,
            BackupProblem::DatabaseUnreadable => BackupError::BackupDatabaseUnreadable,
            BackupProblem::DatabaseCorrupt => BackupError::BackupDatabaseCorrupt,
            BackupProblem::SchemaUnsupported { found, supported } => {
                BackupError::BackupSchemaUnsupported {
                    found: *found,
                    supported: *supported,
                }
            }
            BackupProblem::DatabaseEmpty => BackupError::BackupDatabaseEmpty,
            BackupProblem::SchemaTooOld { found, min } => BackupError::BackupSchemaTooOld {
                found: *found,
                min: *min,
            },
            BackupProblem::StagingMissing => BackupError::BackupStagingMissing,
            BackupProblem::ZipInvalid => BackupError::BackupZipInvalid,
            BackupProblem::ScheduleUnknown { schedule } => BackupError::BackupScheduleUnknown {
                schedule: params::name(schedule),
            },
            BackupProblem::PreferencesInvalid => BackupError::PreferencesInvalid,
            BackupProblem::PreferencesKindInvalid => BackupError::PreferencesKindInvalid,
            BackupProblem::PreferencesFormatUnsupported => {
                BackupError::PreferencesFormatUnsupported
            }
            BackupProblem::Serialization => {
                return Some(UiErrorKind::Common(super::common::CommonError::Internal));
            }
            BackupProblem::SelectionPathInvalid => BackupError::BackupSelectionPathInvalid,
            BackupProblem::LegacyText(_) => return None,
        },
        Error::SchemaTooOld { found, min } => BackupError::SchemaTooOld {
            found: *found,
            min: *min,
        },
        Error::UnknownCleanup(action) => BackupError::CleanupUnknown {
            action: params::name(action),
        },
        Error::InvalidRule(RuleProblem::RetentionOutOfRange { days }) => {
            BackupError::RetentionOutOfRange { days: *days }
        }
        Error::InvalidRule(RuleProblem::RetentionInvalid { value }) => {
            BackupError::RetentionInvalid {
                value: params::name(value),
            }
        }
        _ => return None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_error::tests::assert_code_params;
    use serde_json::json;

    #[test]
    fn backup_and_retention_codes_and_parameters() {
        for (problem, code, params) in [
            (
                BackupProblem::PathEncoding,
                "backup_path_encoding",
                json!({}),
            ),
            (
                BackupProblem::SecretStripping,
                "backup_secret_stripping",
                json!({}),
            ),
            (
                BackupProblem::ManifestMissing,
                "backup_manifest_missing",
                json!({}),
            ),
            (
                BackupProblem::ManifestInvalid,
                "backup_manifest_invalid",
                json!({}),
            ),
            (
                BackupProblem::FormatUnsupported,
                "backup_format_unsupported",
                json!({}),
            ),
            (
                BackupProblem::DatabaseMissing,
                "backup_database_missing",
                json!({}),
            ),
            (
                BackupProblem::DatabaseUnreadable,
                "backup_database_unreadable",
                json!({}),
            ),
            (
                BackupProblem::DatabaseCorrupt,
                "backup_database_corrupt",
                json!({}),
            ),
            (
                BackupProblem::SchemaUnsupported {
                    found: 60,
                    supported: 42,
                },
                "backup_schema_unsupported",
                json!({"found":60,"supported":42}),
            ),
            (
                BackupProblem::DatabaseEmpty,
                "backup_database_empty",
                json!({}),
            ),
            (
                BackupProblem::SchemaTooOld { found: 1, min: 2 },
                "backup_schema_too_old",
                json!({"found":1,"min":2}),
            ),
            (
                BackupProblem::StagingMissing,
                "backup_staging_missing",
                json!({}),
            ),
            (BackupProblem::ZipInvalid, "backup_zip_invalid", json!({})),
            (
                BackupProblem::ScheduleUnknown {
                    schedule: "fixture".into(),
                },
                "backup_schedule_unknown",
                json!({"schedule":"fixture"}),
            ),
            (
                BackupProblem::PreferencesInvalid,
                "preferences_invalid",
                json!({}),
            ),
            (
                BackupProblem::PreferencesKindInvalid,
                "preferences_kind_invalid",
                json!({}),
            ),
            (
                BackupProblem::PreferencesFormatUnsupported,
                "preferences_format_unsupported",
                json!({}),
            ),
            (BackupProblem::Serialization, "internal", json!({})),
            (
                BackupProblem::SelectionPathInvalid,
                "backup_selection_path_invalid",
                json!({}),
            ),
        ] {
            assert_code_params(Error::Backup(problem), code, params);
        }
        assert_code_params(
            Error::SchemaTooOld { found: 1, min: 2 },
            "schema_too_old",
            json!({"found":1,"min":2}),
        );
        assert_code_params(
            Error::UnknownCleanup("fixture".into()),
            "cleanup_unknown",
            json!({"action":"fixture"}),
        );
        assert_code_params(
            Error::InvalidRule(RuleProblem::RetentionOutOfRange { days: -1 }),
            "retention_out_of_range",
            json!({"days":-1}),
        );
        assert_code_params(
            Error::InvalidRule(RuleProblem::RetentionInvalid {
                value: "fixture".into(),
            }),
            "retention_invalid",
            json!({"value":"fixture"}),
        );
    }
}
