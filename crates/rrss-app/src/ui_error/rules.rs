use super::{UiErrorKind, params};
use rrss_core::{
    Error,
    error::rules::{MemoryProblem, RuleProblem},
};

error_codes!(RuleError {
    RuleNotFound => "rule_not_found",
    RulePresetNotFound { name: String } => "rule_preset_not_found",
    RulePolarityUnknown { value: String } => "rule_polarity_unknown",
    RuleDimensionUnknown { value: String } => "rule_dimension_unknown",
    RuleScopeUnknown { value: String } => "rule_scope_unknown",
    RuleSourceUnknown { value: String } => "rule_source_unknown",
    RuleMessageRoleUnknown { value: String } => "rule_message_role_unknown",
    RulePatternMissing => "rule_pattern_missing",
    RuleRegexInvalid => "rule_regex_invalid",
    RuleFeedScopeIdMissing => "rule_feed_scope_id_missing",
    RuleFolderScopeIdMissing => "rule_folder_scope_id_missing",
    RulePresetInvalid => "rule_preset_invalid",
    SiteRuleDomainInvalid => "site_rule_domain_invalid",
    MemoryKindUnknown { kind: String } => "memory_kind_unknown",
    MemoryContentMissing => "memory_content_missing",
    InterestLabelMissing => "interest_label_missing",
    MemoryAlreadyConfirmed => "memory_already_confirmed",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    Some(UiErrorKind::Rules(match error {
        Error::RuleNotFound => RuleError::RuleNotFound,
        Error::PresetNotFound(name) => RuleError::RulePresetNotFound {
            name: params::name(name),
        },
        Error::InvalidRule(problem) => match problem {
            RuleProblem::PolarityUnknown { value } => RuleError::RulePolarityUnknown {
                value: params::name(value),
            },
            RuleProblem::DimensionUnknown { value } => RuleError::RuleDimensionUnknown {
                value: params::name(value),
            },
            RuleProblem::ScopeUnknown { value } => RuleError::RuleScopeUnknown {
                value: params::name(value),
            },
            RuleProblem::SourceUnknown { value } => RuleError::RuleSourceUnknown {
                value: params::name(value),
            },
            RuleProblem::MessageRoleUnknown { value } => RuleError::RuleMessageRoleUnknown {
                value: params::name(value),
            },
            RuleProblem::PatternMissing => RuleError::RulePatternMissing,
            RuleProblem::RegexInvalid { .. } => RuleError::RuleRegexInvalid,
            RuleProblem::FeedScopeIdMissing => RuleError::RuleFeedScopeIdMissing,
            RuleProblem::FolderScopeIdMissing => RuleError::RuleFolderScopeIdMissing,
            RuleProblem::PresetInvalid => RuleError::RulePresetInvalid,
            RuleProblem::SiteDomainInvalid => RuleError::SiteRuleDomainInvalid,
            RuleProblem::SiteSerialization => {
                return Some(UiErrorKind::Common(super::common::CommonError::Internal));
            }
            _ => return None,
        },
        Error::InvalidMemory(problem) => match problem {
            MemoryProblem::KindUnknown { kind } => RuleError::MemoryKindUnknown {
                kind: params::name(kind),
            },
            MemoryProblem::ContentMissing => RuleError::MemoryContentMissing,
            MemoryProblem::InterestLabelMissing => RuleError::InterestLabelMissing,
            MemoryProblem::AlreadyConfirmed => RuleError::MemoryAlreadyConfirmed,
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
    fn rule_and_memory_validation_codes_and_parameters() {
        for (problem, code) in [
            (
                RuleProblem::PolarityUnknown {
                    value: "fixture".into(),
                },
                "rule_polarity_unknown",
            ),
            (
                RuleProblem::DimensionUnknown {
                    value: "fixture".into(),
                },
                "rule_dimension_unknown",
            ),
            (
                RuleProblem::ScopeUnknown {
                    value: "fixture".into(),
                },
                "rule_scope_unknown",
            ),
            (
                RuleProblem::SourceUnknown {
                    value: "fixture".into(),
                },
                "rule_source_unknown",
            ),
            (
                RuleProblem::MessageRoleUnknown {
                    value: "fixture".into(),
                },
                "rule_message_role_unknown",
            ),
        ] {
            assert_code_params(
                Error::InvalidRule(problem),
                code,
                json!({"value":"fixture"}),
            );
        }
        for (problem, code) in [
            (RuleProblem::PatternMissing, "rule_pattern_missing"),
            (
                RuleProblem::FeedScopeIdMissing,
                "rule_feed_scope_id_missing",
            ),
            (
                RuleProblem::FolderScopeIdMissing,
                "rule_folder_scope_id_missing",
            ),
            (RuleProblem::SiteDomainInvalid, "site_rule_domain_invalid"),
            (RuleProblem::SiteSerialization, "internal"),
            (RuleProblem::PresetInvalid, "rule_preset_invalid"),
        ] {
            assert_code_params(Error::InvalidRule(problem), code, json!({}));
        }
        assert_code_params(
            rrss_core::rules::validate_pattern(rrss_core::rules::Dimension::TitleRe, "[")
                .unwrap_err(),
            "rule_regex_invalid",
            json!({}),
        );
        for (problem, code, params) in [
            (
                MemoryProblem::KindUnknown {
                    kind: "fixture".into(),
                },
                "memory_kind_unknown",
                json!({"kind":"fixture"}),
            ),
            (
                MemoryProblem::ContentMissing,
                "memory_content_missing",
                json!({}),
            ),
            (
                MemoryProblem::InterestLabelMissing,
                "interest_label_missing",
                json!({}),
            ),
            (
                MemoryProblem::AlreadyConfirmed,
                "memory_already_confirmed",
                json!({}),
            ),
        ] {
            assert_code_params(Error::InvalidMemory(problem), code, params);
        }
    }
}
