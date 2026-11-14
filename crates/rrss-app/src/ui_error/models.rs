use super::{UiErrorKind, params};
use rrss_core::{
    Error,
    error::models::{CompatProblem, ModelChoiceProblem, ProviderProblem, TaskModelLabel},
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum TaskModelLabelKind {
    Id,
    Name,
}

error_codes!(ModelError {
    ProviderNotFound => "provider_not_found",
    AiBudgetExhausted => "ai_budget_exhausted",
    PromptTemplateNotFound { name: String } => "prompt_template_not_found",
    ProviderNameMissing => "provider_name_missing",
    ProviderUrlInvalid => "provider_url_invalid",
    ProviderUrlComponentsInvalid => "provider_url_components_invalid",
    ProviderProtocolUnsupported => "provider_protocol_unsupported",
    ProviderPresetUnknown => "provider_preset_unknown",
    ModelIdMissing => "model_id_missing",
    ChatModelUsesOwnConfig => "chat_model_uses_own_config",
    ModelNotAdded { model: String } => "model_not_added",
    ModelChatDisabled { model: String } => "model_chat_disabled",
    ModelReplacementMissing { model: String } => "model_replacement_missing",
    ModelNotFound { model: String } => "model_not_found",
    ModelChatDefaultRequired => "model_chat_default_required",
    ModelThinkingUnknown { level: String } => "model_thinking_unknown",
    ModelContextWindowNonPositive => "model_context_window_non_positive",
    ModelMaxOutputNonPositive => "model_max_output_non_positive",
    ModelSelectionIncomplete => "model_selection_incomplete",
    VectorRebuildModelMissing => "vector_rebuild_model_missing",
    ModelCompatObjectRequired => "model_compat_object_required",
    ProviderCompatObjectRequired => "provider_compat_object_required",
    ModelCompatSettingObjectRequired => "model_compat_setting_object_required",
    ProviderCompatPresetInvalid => "provider_compat_preset_invalid",
    VectorSpaceRebuildRequired { model: String } => "vector_space_rebuild_required",
    VectorSpaceRebuildInProgress => "vector_space_rebuild_in_progress",
    TaskModelUnset { task: String, label: TaskModelLabelKind } => "task_model_unset",
    AiTaskUnknown { task: String } => "ai_task_unknown",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    Some(UiErrorKind::Models(match error {
        Error::ProviderNotFound => ModelError::ProviderNotFound,
        Error::DailyBudgetExhausted => ModelError::AiBudgetExhausted,
        Error::TemplateNotFound(name) => ModelError::PromptTemplateNotFound {
            name: params::name(name),
        },
        Error::InvalidProvider(problem) => match problem {
            ProviderProblem::NameMissing => ModelError::ProviderNameMissing,
            ProviderProblem::UrlInvalid => ModelError::ProviderUrlInvalid,
            ProviderProblem::UrlComponentsInvalid => ModelError::ProviderUrlComponentsInvalid,
            ProviderProblem::ProtocolUnsupported => ModelError::ProviderProtocolUnsupported,
            ProviderProblem::PresetUnknown => ModelError::ProviderPresetUnknown,
            ProviderProblem::ModelIdMissing => ModelError::ModelIdMissing,
            ProviderProblem::ChatUsesOwnConfig => ModelError::ChatModelUsesOwnConfig,
            ProviderProblem::LegacyText(_) => return None,
        },
        Error::InvalidModelChoice(problem) => match problem {
            ModelChoiceProblem::NotAdded { model } => ModelError::ModelNotAdded {
                model: params::name(model),
            },
            ModelChoiceProblem::ChatDisabled { model } => ModelError::ModelChatDisabled {
                model: params::name(model),
            },
            ModelChoiceProblem::ReplacementMissing { model } => {
                ModelError::ModelReplacementMissing {
                    model: params::name(model),
                }
            }
            ModelChoiceProblem::NotFound { model } => ModelError::ModelNotFound {
                model: params::name(model),
            },
            ModelChoiceProblem::ChatDefaultRequired => ModelError::ModelChatDefaultRequired,
            ModelChoiceProblem::ThinkingUnknown { level } => ModelError::ModelThinkingUnknown {
                level: params::name(level),
            },
            ModelChoiceProblem::ContextWindowNonPositive => {
                ModelError::ModelContextWindowNonPositive
            }
            ModelChoiceProblem::MaxOutputNonPositive => ModelError::ModelMaxOutputNonPositive,
            ModelChoiceProblem::SelectionIncomplete => ModelError::ModelSelectionIncomplete,
            ModelChoiceProblem::RebuildModelMissing => ModelError::VectorRebuildModelMissing,
        },
        Error::InvalidCompat(problem) => match problem {
            CompatProblem::ObjectRequired => ModelError::ModelCompatObjectRequired,
            CompatProblem::ProviderObjectRequired => ModelError::ProviderCompatObjectRequired,
            CompatProblem::ModelObjectRequired => ModelError::ModelCompatSettingObjectRequired,
            CompatProblem::PresetInvalid => ModelError::ProviderCompatPresetInvalid,
        },
        Error::VectorSpaceRebuildRequired { model } => ModelError::VectorSpaceRebuildRequired {
            model: params::name(model),
        },
        Error::VectorSpaceRebuildInProgress => ModelError::VectorSpaceRebuildInProgress,
        Error::TaskModelUnset(TaskModelLabel::Id(task)) => ModelError::TaskModelUnset {
            task: params::name(task),
            label: TaskModelLabelKind::Id,
        },
        Error::TaskModelUnset(TaskModelLabel::Name(task)) => ModelError::TaskModelUnset {
            task: params::name(task),
            label: TaskModelLabelKind::Name,
        },
        Error::UnknownAiTask(task) => ModelError::AiTaskUnknown {
            task: params::name(task),
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
    fn model_validation_codes_and_parameters() {
        for (problem, code) in [
            (ProviderProblem::NameMissing, "provider_name_missing"),
            (ProviderProblem::UrlInvalid, "provider_url_invalid"),
            (
                ProviderProblem::UrlComponentsInvalid,
                "provider_url_components_invalid",
            ),
            (
                ProviderProblem::ProtocolUnsupported,
                "provider_protocol_unsupported",
            ),
            (ProviderProblem::PresetUnknown, "provider_preset_unknown"),
            (ProviderProblem::ModelIdMissing, "model_id_missing"),
            (
                ProviderProblem::ChatUsesOwnConfig,
                "chat_model_uses_own_config",
            ),
        ] {
            assert_code_params(Error::InvalidProvider(problem), code, json!({}));
        }
        for (problem, code) in [
            (
                CompatProblem::ObjectRequired,
                "model_compat_object_required",
            ),
            (
                CompatProblem::ProviderObjectRequired,
                "provider_compat_object_required",
            ),
            (
                CompatProblem::ModelObjectRequired,
                "model_compat_setting_object_required",
            ),
            (
                CompatProblem::PresetInvalid,
                "provider_compat_preset_invalid",
            ),
        ] {
            assert_code_params(Error::InvalidCompat(problem), code, json!({}));
        }
        for (problem, code, params) in [
            (
                ModelChoiceProblem::NotAdded {
                    model: "fixture".into(),
                },
                "model_not_added",
                json!({"model":"fixture"}),
            ),
            (
                ModelChoiceProblem::ChatDisabled {
                    model: "fixture".into(),
                },
                "model_chat_disabled",
                json!({"model":"fixture"}),
            ),
            (
                ModelChoiceProblem::ReplacementMissing {
                    model: "fixture".into(),
                },
                "model_replacement_missing",
                json!({"model":"fixture"}),
            ),
            (
                ModelChoiceProblem::NotFound {
                    model: "fixture".into(),
                },
                "model_not_found",
                json!({"model":"fixture"}),
            ),
            (
                ModelChoiceProblem::ThinkingUnknown {
                    level: "fixture".into(),
                },
                "model_thinking_unknown",
                json!({"level":"fixture"}),
            ),
            (
                ModelChoiceProblem::ChatDefaultRequired,
                "model_chat_default_required",
                json!({}),
            ),
            (
                ModelChoiceProblem::ContextWindowNonPositive,
                "model_context_window_non_positive",
                json!({}),
            ),
            (
                ModelChoiceProblem::MaxOutputNonPositive,
                "model_max_output_non_positive",
                json!({}),
            ),
            (
                ModelChoiceProblem::SelectionIncomplete,
                "model_selection_incomplete",
                json!({}),
            ),
            (
                ModelChoiceProblem::RebuildModelMissing,
                "vector_rebuild_model_missing",
                json!({}),
            ),
        ] {
            assert_code_params(Error::InvalidModelChoice(problem), code, params);
        }
        assert_code_params(
            Error::VectorSpaceRebuildRequired {
                model: "fixture".into(),
            },
            "vector_space_rebuild_required",
            json!({"model":"fixture"}),
        );
        assert_code_params(
            Error::VectorSpaceRebuildInProgress,
            "vector_space_rebuild_in_progress",
            json!({}),
        );
        assert_code_params(
            Error::UnknownAiTask("fixture".into()),
            "ai_task_unknown",
            json!({"task":"fixture"}),
        );
        for task in [
            "focus_summary",
            "article_summary",
            "article_segment",
            "digest_daily",
            "digest_weekly",
            "digest_pick_daily",
            "digest_pick_weekly",
            "cluster_summary",
            "memory_candidate",
            "episode_compress",
            "article_translate",
            "embed",
            "chat",
        ] {
            assert_code_params(
                Error::TaskModelUnset(TaskModelLabel::Id(task.into())),
                "task_model_unset",
                json!({"task":task,"label":"id"}),
            );
        }
        assert_code_params(
            Error::TaskModelUnset(TaskModelLabel::Name("fixture".into())),
            "task_model_unset",
            json!({"task":"fixture","label":"name"}),
        );
        assert_code_params(
            Error::TaskModelUnset(TaskModelLabel::Id("fixture".into())),
            "task_model_unset",
            json!({"task":"fixture","label":"id"}),
        );
    }
}
