use super::UiErrorKind;
use rrss_core::{Error, error::JobFailure};

error_codes!(JobsUiError {
    RuntimeStopped => "job_runtime_stopped",
    ModelBusy => "job_model_busy",
    ModelStopped => "job_model_stopped",
    ModelStoppedRetry => "job_model_stopped_retry",
    WaitLimit => "job_wait_limit",
    InputChanged => "job_input_changed",
    DigestStopped => "job_digest_stopped",
    DigestPeriodInvalid => "job_digest_period_invalid",
    DigestWaitLimit => "job_digest_wait_limit",
    DigestBusy => "job_digest_busy",
    EndpointInvalid => "job_endpoint_invalid",
    LimitRecordingInterrupted => "job_limit_recording_interrupted",
    ModelRuntimeStopped => "job_model_runtime_stopped",
    BudgetCheckFailed => "job_budget_check_failed",
    SpaceCheckFailed => "job_space_check_failed",
    SpaceChanged => "job_space_changed",
    SpaceUnreadable => "job_space_unreadable",
    SpaceVerifyFailed => "job_space_verify_failed",
    SpaceProbeFailed => "job_space_probe_failed",
    DigestNoArticles => "job_digest_no_articles",
    DigestEmpty => "job_digest_empty",
    DigestChanged => "job_digest_changed",
    ClusterTooSmall => "job_cluster_too_small",
    ClusterEmpty => "job_cluster_empty",
    SummaryChanged => "job_summary_changed",
    SummaryDiscarded => "job_summary_discarded",
    ConfigChanged => "job_config_changed",
    EmbeddingStopped => "job_embedding_stopped",
    EmbeddingChanged => "job_embedding_changed",
    ModelHttp { status: u16 } => "job_model_http",
    ModelBudget => "job_model_budget",
    ModelCooldown => "job_model_cooldown",
    EndpointBusy => "job_endpoint_busy",
    RequestTimeout => "job_request_timeout",
    RequestConnect => "job_request_connect",
    RequestDecode => "job_request_decode",
    RequestFailed => "job_request_failed",
    DigestMissing => "digest_selection_missing",
    DigestUpdated => "digest_selection_updated",
    DigestSelectSources => "digest_selection_required",
    DigestDuplicate => "digest_selection_duplicate",
    DigestBlockInvalid => "digest_block_invalid",
    DigestBlockNoSources => "digest_block_no_sources",
    DigestItemChanged => "digest_item_changed",
    DigestSourcesMissing => "digest_sources_missing",
    DigestContentChanged => "digest_content_changed",
    DigestSourceRemoved => "digest_source_removed",
    DigestSourceUnavailable => "digest_source_unavailable",
    DigestDraftNotObject => "digest_draft_not_object",
    DigestDraftSectionsInvalid => "digest_draft_sections_invalid",
    DigestDraftEmpty => "digest_draft_empty",
    DigestDraftSourcesMismatch => "digest_draft_sources_mismatch",
    DigestDraftBodyMissing => "digest_draft_body_missing",
    DigestDraftSerialize => "digest_draft_serialize",
    ModelProxyAuth { status: u16 } => "job_model_proxy_auth",
    ModelInvalidJson => "job_model_invalid_json",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    let Error::Jobs(reason) = error else {
        return None;
    };
    Some(UiErrorKind::Jobs(match reason {
        JobFailure::RuntimeStopped => JobsUiError::RuntimeStopped,
        JobFailure::ModelBusy => JobsUiError::ModelBusy,
        JobFailure::ModelStopped => JobsUiError::ModelStopped,
        JobFailure::ModelStoppedRetry => JobsUiError::ModelStoppedRetry,
        JobFailure::WaitLimit => JobsUiError::WaitLimit,
        JobFailure::InputChanged => JobsUiError::InputChanged,
        JobFailure::DigestStopped => JobsUiError::DigestStopped,
        JobFailure::DigestPeriodInvalid => JobsUiError::DigestPeriodInvalid,
        JobFailure::DigestWaitLimit => JobsUiError::DigestWaitLimit,
        JobFailure::DigestBusy => JobsUiError::DigestBusy,
        JobFailure::EndpointInvalid => JobsUiError::EndpointInvalid,
        JobFailure::LimitRecordingInterrupted => JobsUiError::LimitRecordingInterrupted,
        JobFailure::ModelRuntimeStopped => JobsUiError::ModelRuntimeStopped,
        JobFailure::BudgetCheckFailed => JobsUiError::BudgetCheckFailed,
        JobFailure::SpaceCheckFailed => JobsUiError::SpaceCheckFailed,
        JobFailure::SpaceChanged => JobsUiError::SpaceChanged,
        JobFailure::SpaceUnreadable => JobsUiError::SpaceUnreadable,
        JobFailure::SpaceVerifyFailed => JobsUiError::SpaceVerifyFailed,
        JobFailure::SpaceProbeFailed => JobsUiError::SpaceProbeFailed,
        JobFailure::DigestNoArticles => JobsUiError::DigestNoArticles,
        JobFailure::DigestEmpty => JobsUiError::DigestEmpty,
        JobFailure::DigestChanged => JobsUiError::DigestChanged,
        JobFailure::ClusterTooSmall => JobsUiError::ClusterTooSmall,
        JobFailure::ClusterEmpty => JobsUiError::ClusterEmpty,
        JobFailure::SummaryChanged => JobsUiError::SummaryChanged,
        JobFailure::SummaryDiscarded => JobsUiError::SummaryDiscarded,
        JobFailure::ConfigChanged => JobsUiError::ConfigChanged,
        JobFailure::EmbeddingStopped => JobsUiError::EmbeddingStopped,
        JobFailure::EmbeddingChanged => JobsUiError::EmbeddingChanged,
        JobFailure::ModelHttp { status } => JobsUiError::ModelHttp { status: *status },
        JobFailure::ModelBudget => JobsUiError::ModelBudget,
        JobFailure::ModelCooldown => JobsUiError::ModelCooldown,
        JobFailure::EndpointBusy => JobsUiError::EndpointBusy,
        JobFailure::RequestTimeout => JobsUiError::RequestTimeout,
        JobFailure::RequestConnect => JobsUiError::RequestConnect,
        JobFailure::RequestDecode => JobsUiError::RequestDecode,
        JobFailure::RequestFailed => JobsUiError::RequestFailed,
        JobFailure::DigestMissing => JobsUiError::DigestMissing,
        JobFailure::DigestUpdated => JobsUiError::DigestUpdated,
        JobFailure::DigestSelectSources => JobsUiError::DigestSelectSources,
        JobFailure::DigestDuplicate => JobsUiError::DigestDuplicate,
        JobFailure::DigestBlockInvalid => JobsUiError::DigestBlockInvalid,
        JobFailure::DigestBlockNoSources => JobsUiError::DigestBlockNoSources,
        JobFailure::DigestItemChanged => JobsUiError::DigestItemChanged,
        JobFailure::DigestSourcesMissing => JobsUiError::DigestSourcesMissing,
        JobFailure::DigestContentChanged => JobsUiError::DigestContentChanged,
        JobFailure::DigestSourceRemoved => JobsUiError::DigestSourceRemoved,
        JobFailure::DigestSourceUnavailable => JobsUiError::DigestSourceUnavailable,
        JobFailure::DigestDraftNotObject => JobsUiError::DigestDraftNotObject,
        JobFailure::DigestDraftSectionsInvalid => JobsUiError::DigestDraftSectionsInvalid,
        JobFailure::DigestDraftEmpty => JobsUiError::DigestDraftEmpty,
        JobFailure::DigestDraftSourcesMismatch => JobsUiError::DigestDraftSourcesMismatch,
        JobFailure::DigestDraftBodyMissing => JobsUiError::DigestDraftBodyMissing,
        JobFailure::DigestDraftSerialize => JobsUiError::DigestDraftSerialize,
    }))
}

impl JobsUiError {
    pub fn failure(self, detail: impl AsRef<str>) -> super::UiError {
        super::UiError::new(UiErrorKind::Jobs(self), detail)
    }
}

// The protocol still represents admission and non-timeout request causes as opaque
// static messages. Preserve those actionable texts until their owning producer is typed.
pub(crate) fn from_ai(error: &rrss_ai::Error, legacy: &str) -> super::UiError {
    use rrss_ai::Error as Ai;
    let kind = match error {
        Ai::Content { source, .. } => return from_ai(source, legacy),
        Ai::Http { status: 407, .. } => JobsUiError::ModelProxyAuth { status: 407 },
        Ai::Http { status, .. } => JobsUiError::ModelHttp { status: *status },
        Ai::BudgetExhausted => JobsUiError::ModelBudget,
        Ai::ServiceCooldown => JobsUiError::ModelCooldown,
        Ai::Capacity => JobsUiError::EndpointBusy,
        Ai::Configuration(_) => JobsUiError::EndpointInvalid,
        Ai::InvalidJson(_) => JobsUiError::ModelInvalidJson,
        Ai::Request { timeout: true, .. } => JobsUiError::RequestTimeout,
        Ai::Request { .. } | Ai::Admission(_) => {
            let mut ui = super::UiError::from(legacy.to_string());
            ui.diagnostic = Some(
                match error {
                    Ai::Admission(_) => "model admission failed",
                    _ => "model network request failed",
                }
                .into(),
            );
            return ui;
        }
        Ai::NoProvider => return super::UiError::from(Error::ProviderNotFound),
        Ai::EmptyContent | Ai::EmptyEmbedding => {
            return super::UiError::new(
                UiErrorKind::Common(super::common::CommonError::Internal),
                "empty model output",
            );
        }
    };
    let detail = format!("model failure: {}", kind.code());
    kind.failure(detail)
}

#[cfg(test)]
pub(crate) fn display_anyhow(error: &anyhow::Error) -> String {
    let ui = if let Some(ui) = error.downcast_ref::<super::UiError>() {
        ui.clone()
    } else if let Some(core) = error.downcast_ref::<Error>() {
        super::UiError::from_core(core)
    } else if let Some(ai) = error.downcast_ref::<rrss_ai::Error>() {
        from_ai(ai, &error.to_string())
    } else {
        super::UiError::from(error.to_string())
    };
    super::test_display::display(&ui)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_job_reasons_survive_anyhow_contexts() {
        for (reason, code) in [
            (JobFailure::RuntimeStopped, "job_runtime_stopped"),
            (JobFailure::ModelBusy, "job_model_busy"),
            (JobFailure::ModelStopped, "job_model_stopped"),
            (JobFailure::ModelStoppedRetry, "job_model_stopped_retry"),
            (JobFailure::WaitLimit, "job_wait_limit"),
            (JobFailure::InputChanged, "job_input_changed"),
            (JobFailure::DigestStopped, "job_digest_stopped"),
            (JobFailure::DigestPeriodInvalid, "job_digest_period_invalid"),
            (JobFailure::DigestWaitLimit, "job_digest_wait_limit"),
            (JobFailure::DigestBusy, "job_digest_busy"),
            (JobFailure::EndpointInvalid, "job_endpoint_invalid"),
            (
                JobFailure::LimitRecordingInterrupted,
                "job_limit_recording_interrupted",
            ),
            (JobFailure::ModelRuntimeStopped, "job_model_runtime_stopped"),
            (JobFailure::BudgetCheckFailed, "job_budget_check_failed"),
            (JobFailure::SpaceCheckFailed, "job_space_check_failed"),
            (JobFailure::SpaceChanged, "job_space_changed"),
            (JobFailure::SpaceUnreadable, "job_space_unreadable"),
            (JobFailure::SpaceVerifyFailed, "job_space_verify_failed"),
            (JobFailure::SpaceProbeFailed, "job_space_probe_failed"),
            (JobFailure::DigestNoArticles, "job_digest_no_articles"),
            (JobFailure::DigestEmpty, "job_digest_empty"),
            (JobFailure::DigestChanged, "job_digest_changed"),
            (JobFailure::ClusterTooSmall, "job_cluster_too_small"),
            (JobFailure::ClusterEmpty, "job_cluster_empty"),
            (JobFailure::SummaryChanged, "job_summary_changed"),
            (JobFailure::SummaryDiscarded, "job_summary_discarded"),
            (JobFailure::ConfigChanged, "job_config_changed"),
            (JobFailure::EmbeddingStopped, "job_embedding_stopped"),
            (JobFailure::EmbeddingChanged, "job_embedding_changed"),
            (JobFailure::ModelBudget, "job_model_budget"),
            (JobFailure::ModelCooldown, "job_model_cooldown"),
            (JobFailure::EndpointBusy, "job_endpoint_busy"),
            (JobFailure::RequestTimeout, "job_request_timeout"),
            (JobFailure::RequestConnect, "job_request_connect"),
            (JobFailure::RequestDecode, "job_request_decode"),
            (JobFailure::RequestFailed, "job_request_failed"),
            (JobFailure::DigestMissing, "digest_selection_missing"),
            (JobFailure::DigestUpdated, "digest_selection_updated"),
            (JobFailure::DigestSelectSources, "digest_selection_required"),
            (JobFailure::DigestDuplicate, "digest_selection_duplicate"),
            (JobFailure::DigestBlockInvalid, "digest_block_invalid"),
            (JobFailure::DigestBlockNoSources, "digest_block_no_sources"),
            (JobFailure::DigestItemChanged, "digest_item_changed"),
            (JobFailure::DigestSourcesMissing, "digest_sources_missing"),
            (JobFailure::DigestContentChanged, "digest_content_changed"),
            (JobFailure::DigestSourceRemoved, "digest_source_removed"),
            (
                JobFailure::DigestSourceUnavailable,
                "digest_source_unavailable",
            ),
            (JobFailure::DigestDraftNotObject, "digest_draft_not_object"),
            (
                JobFailure::DigestDraftSectionsInvalid,
                "digest_draft_sections_invalid",
            ),
            (JobFailure::DigestDraftEmpty, "digest_draft_empty"),
            (
                JobFailure::DigestDraftSourcesMismatch,
                "digest_draft_sources_mismatch",
            ),
            (
                JobFailure::DigestDraftBodyMissing,
                "digest_draft_body_missing",
            ),
            (JobFailure::DigestDraftSerialize, "digest_draft_serialize"),
        ] {
            let error = anyhow::anyhow!("response body canary").context(Error::Jobs(reason));
            let ui = super::super::UiError::from_anyhow_pretty(error);
            assert_eq!(ui.code(), code);
            assert!(!serde_json::to_string(&ui).unwrap().contains("canary"));
        }
    }
    #[test]
    fn model_failures_expose_status_without_response_or_output() {
        for (error, code) in [
            (
                rrss_ai::Error::Http {
                    status: 503,
                    message: "private response canary".into(),
                },
                "job_model_http",
            ),
            (
                rrss_ai::Error::Http {
                    status: 407,
                    message: "private response canary".into(),
                },
                "job_model_proxy_auth",
            ),
            (
                rrss_ai::Error::InvalidJson("private model canary".into()),
                "job_model_invalid_json",
            ),
            (rrss_ai::Error::BudgetExhausted, "job_model_budget"),
            (rrss_ai::Error::ServiceCooldown, "job_model_cooldown"),
            (rrss_ai::Error::Capacity, "job_endpoint_busy"),
            (
                rrss_ai::Error::Configuration("private endpoint canary"),
                "job_endpoint_invalid",
            ),
            (
                rrss_ai::Error::Request {
                    timeout: true,
                    message: "private request canary",
                },
                "job_request_timeout",
            ),
        ] {
            let ui = super::super::UiError::from(anyhow::Error::new(error));
            assert_eq!(ui.code(), code);
            assert!(!serde_json::to_string(&ui).unwrap().contains("canary"));
            assert!(!ui.log_detail().contains("canary"));
        }
        let ui = super::super::UiError::from(anyhow::Error::new(rrss_ai::Error::Http {
            status: 503,
            message: "canary".into(),
        }));
        assert_eq!(serde_json::to_value(ui).unwrap()["params"]["status"], 503);
        let wrapped = rrss_ai::Error::Content {
            source: Box::new(rrss_ai::Error::Http {
                status: 502,
                message: "private response canary".into(),
            }),
            usage: Some((1, 2)),
        };
        let ui = super::super::UiError::from_anyhow_pretty(anyhow::Error::new(wrapped));
        assert_eq!(ui.code(), "job_model_http");
        assert!(!serde_json::to_string(&ui).unwrap().contains("canary"));
        assert!(!ui.log_detail().contains("canary"));
    }
    #[test]
    fn opaque_admission_and_request_reasons_retain_their_original_display() {
        for error in [
            rrss_ai::Error::Admission("fixture admission"),
            rrss_ai::Error::Request {
                timeout: false,
                message: "fixture request",
            },
        ] {
            let detail = error.to_string();
            let ui = super::super::UiError::from(anyhow::Error::new(error));
            assert_eq!(ui.code(), "legacy_text");
            assert_eq!(ui.detail, detail);
            assert!(!ui.log_detail().contains("fixture"));
        }
    }
}
