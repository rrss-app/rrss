use super::{UiErrorKind, params};
use rrss_core::Error;

error_codes!(RuleError {
    RuleNotFound => "rule_not_found",
    RulePresetNotFound { name: String } => "rule_preset_not_found",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    Some(UiErrorKind::Rules(match error {
        Error::RuleNotFound => RuleError::RuleNotFound,
        Error::PresetNotFound(name) => RuleError::RulePresetNotFound {
            name: params::name(name),
        },
        _ => return None,
    }))
}
