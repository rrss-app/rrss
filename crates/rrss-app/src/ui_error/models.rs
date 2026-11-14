use super::{UiErrorKind, params};
use rrss_core::Error;

error_codes!(ModelError {
    ProviderNotFound => "provider_not_found",
    AiBudgetExhausted => "ai_budget_exhausted",
    PromptTemplateNotFound { name: String } => "prompt_template_not_found",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    Some(UiErrorKind::Models(match error {
        Error::ProviderNotFound => ModelError::ProviderNotFound,
        Error::DailyBudgetExhausted => ModelError::AiBudgetExhausted,
        Error::TemplateNotFound(name) => ModelError::PromptTemplateNotFound {
            name: params::name(name),
        },
        _ => return None,
    }))
}
