use super::{UiErrorKind, params};
use rrss_core::{
    Error,
    error::settings::{PiOverrideProblem, SettingProblem},
};

error_codes!(SettingError {
    SettingUnknown { id: String } => "setting_unknown",
    SettingBooleanRequired { id: String } => "setting_boolean_required",
    SettingIntegerRequired { value: String } => "setting_integer_required",
    SettingOutOfRange { min: i64, max: i64 } => "setting_out_of_range",
    SettingSpotCheckRateInvalid => "setting_spot_check_rate_invalid",
    SettingTextTooLong { max: usize } => "setting_text_too_long",
    SettingOptionInvalid { value: String, options: String } => "setting_option_invalid",
    SettingPinnedScopesInvalid => "setting_pinned_scopes_invalid",
    SettingFocusColumnsInvalid => "setting_focus_columns_invalid",
    SettingVectorDimsInvalid => "setting_vector_dims_invalid",
    SettingVectorDataInvalid => "setting_vector_data_invalid",
    SettingVectorSpaceStale => "setting_vector_space_stale",
    SettingTranslationRevisionExhausted => "setting_translation_revision_exhausted",
    SettingRankTransactionRequired => "setting_rank_transaction_required",
    SettingRankSwitchUnknown => "setting_rank_switch_unknown",
    PiOverrideNoteTooLong { max: usize } => "pi_override_note_too_long",
    PiOverrideKeyUnknown { key: String } => "pi_override_key_unknown",
    PiOverrideObjectRequired => "pi_override_object_required",
    PiOverrideModelInvalid { model: String } => "pi_override_model_invalid",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    Some(UiErrorKind::Settings(match error {
        Error::InvalidSetting(problem) => match problem {
            SettingProblem::Unknown { id } => SettingError::SettingUnknown {
                id: params::name(id),
            },
            SettingProblem::BooleanRequired { id } => SettingError::SettingBooleanRequired {
                id: params::name(id),
            },
            SettingProblem::IntegerRequired { value } => SettingError::SettingIntegerRequired {
                value: params::name(value),
            },
            SettingProblem::OutOfRange { min, max } => SettingError::SettingOutOfRange {
                min: *min,
                max: *max,
            },
            SettingProblem::SpotCheckRateInvalid => SettingError::SettingSpotCheckRateInvalid,
            SettingProblem::TextTooLong { max } => SettingError::SettingTextTooLong { max: *max },
            SettingProblem::OptionInvalid { value, options } => {
                SettingError::SettingOptionInvalid {
                    value: params::name(value),
                    options: params::options(options),
                }
            }
            SettingProblem::PinnedScopesInvalid => SettingError::SettingPinnedScopesInvalid,
            SettingProblem::FocusColumnsInvalid => SettingError::SettingFocusColumnsInvalid,
            SettingProblem::VectorDimsInvalid => SettingError::SettingVectorDimsInvalid,
            SettingProblem::VectorDataInvalid => SettingError::SettingVectorDataInvalid,
            SettingProblem::VectorSpaceStale => SettingError::SettingVectorSpaceStale,
            SettingProblem::TranslationRevisionExhausted => {
                SettingError::SettingTranslationRevisionExhausted
            }
            SettingProblem::RankTransactionRequired => SettingError::SettingRankTransactionRequired,
            SettingProblem::RankSwitchUnknown => SettingError::SettingRankSwitchUnknown,
            SettingProblem::LegacyText(_) => return None,
        },
        Error::InvalidPiOverride(problem) => match problem {
            PiOverrideProblem::NoteTooLong { max } => {
                SettingError::PiOverrideNoteTooLong { max: *max }
            }
            PiOverrideProblem::KeyUnknown { key } => SettingError::PiOverrideKeyUnknown {
                key: params::name(key),
            },
            PiOverrideProblem::ObjectRequired => SettingError::PiOverrideObjectRequired,
            PiOverrideProblem::ModelInvalid { model } => SettingError::PiOverrideModelInvalid {
                model: params::name(model),
            },
            PiOverrideProblem::LegacyText(_) => return None,
        },
        _ => return None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_error::tests::assert_code_params;
    use serde_json::json;

    #[test]
    fn setting_validation_codes_and_parameters() {
        for (problem, code, params) in [
            (
                SettingProblem::Unknown {
                    id: "fixture".into(),
                },
                "setting_unknown",
                json!({"id":"fixture"}),
            ),
            (
                SettingProblem::BooleanRequired {
                    id: "fixture".into(),
                },
                "setting_boolean_required",
                json!({"id":"fixture"}),
            ),
            (
                SettingProblem::IntegerRequired {
                    value: "fixture".into(),
                },
                "setting_integer_required",
                json!({"value":"fixture"}),
            ),
            (
                SettingProblem::OutOfRange { min: -10, max: 10 },
                "setting_out_of_range",
                json!({"min":-10,"max":10}),
            ),
            (
                SettingProblem::SpotCheckRateInvalid,
                "setting_spot_check_rate_invalid",
                json!({}),
            ),
            (
                SettingProblem::TextTooLong { max: 40 },
                "setting_text_too_long",
                json!({"max":40}),
            ),
            (
                SettingProblem::OptionInvalid {
                    value: "fixture".into(),
                    options: vec!["off".into(), "on".into()],
                },
                "setting_option_invalid",
                json!({"value":"fixture","options":"off / on"}),
            ),
            (
                SettingProblem::PinnedScopesInvalid,
                "setting_pinned_scopes_invalid",
                json!({}),
            ),
            (
                SettingProblem::FocusColumnsInvalid,
                "setting_focus_columns_invalid",
                json!({}),
            ),
            (
                SettingProblem::VectorDimsInvalid,
                "setting_vector_dims_invalid",
                json!({}),
            ),
            (
                SettingProblem::VectorDataInvalid,
                "setting_vector_data_invalid",
                json!({}),
            ),
            (
                SettingProblem::VectorSpaceStale,
                "setting_vector_space_stale",
                json!({}),
            ),
            (
                SettingProblem::TranslationRevisionExhausted,
                "setting_translation_revision_exhausted",
                json!({}),
            ),
            (
                SettingProblem::RankTransactionRequired,
                "setting_rank_transaction_required",
                json!({}),
            ),
            (
                SettingProblem::RankSwitchUnknown,
                "setting_rank_switch_unknown",
                json!({}),
            ),
        ] {
            assert_code_params(Error::InvalidSetting(problem), code, params);
        }
        for (problem, code, params) in [
            (
                PiOverrideProblem::NoteTooLong { max: 2000 },
                "pi_override_note_too_long",
                json!({"max":2000}),
            ),
            (
                PiOverrideProblem::KeyUnknown {
                    key: "fixture".into(),
                },
                "pi_override_key_unknown",
                json!({"key":"fixture"}),
            ),
            (
                PiOverrideProblem::ObjectRequired,
                "pi_override_object_required",
                json!({}),
            ),
            (
                PiOverrideProblem::ModelInvalid {
                    model: "fixture".into(),
                },
                "pi_override_model_invalid",
                json!({"model":"fixture"}),
            ),
        ] {
            assert_code_params(Error::InvalidPiOverride(problem), code, params);
        }
    }

    #[test]
    fn json_override_parser_failure_preserves_legacy_display() {
        let dir = tempfile::tempdir().unwrap();
        let conn = rrss_core::db::open_at(&dir.path().join("fixture.db")).unwrap();
        let error = rrss_core::pi_store::set_pi_override(
            &conn,
            rrss_core::pi_store::PI_SETTINGS_OVERRIDE_KEY,
            "{",
        )
        .unwrap_err();
        let original = error.to_string();
        let ui = crate::UiError::from(error);
        assert_eq!(ui.code(), "legacy_text");
        assert_eq!(ui.detail, original);
        assert_eq!(ui.log_detail(), "invalid Pi override");
    }
}
