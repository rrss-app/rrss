//! Reason IPC types use the same declaration as the stored core messages.

use crate::ui_error::params;
use rrss_core::rules::Dimension;

rrss_core::reason_types!(ts_rs::TS);

impl From<rrss_core::rank::reasons::ReasonMessage> for ReasonMessage {
    fn from(message: rrss_core::rank::reasons::ReasonMessage) -> Self {
        // The shared declaration guarantees identical wire shapes. Only string
        // data crosses the privacy boundary; enums/counts retain their types.
        let mut wire = serde_json::to_value(message).unwrap_or_default();
        if let Some(fields) = wire
            .get_mut("params")
            .and_then(serde_json::Value::as_object_mut)
        {
            for name in ["pattern", "text"] {
                if let Some(value) = fields.get(name).and_then(serde_json::Value::as_str) {
                    let safe = if name == "text" {
                        params::reason_legacy_text(value)
                    } else {
                        params::reason_text(value)
                    };
                    fields.insert(name.into(), safe.into());
                }
            }
        }
        serde_json::from_value(wire).unwrap_or(Self::Unknown {})
    }
}

impl ReasonMessage {
    pub(crate) fn rule_condition(dimension: Dimension, pattern: &str) -> Self {
        let pattern = params::reason_text(pattern);
        match dimension {
            Dimension::Author => Self::RuleConditionAuthor { pattern },
            Dimension::Tag => Self::RuleConditionTag { pattern },
            Dimension::TitleKw => Self::RuleConditionTitleKw { pattern },
            Dimension::TitleRe => Self::RuleConditionTitleRe { pattern },
            Dimension::UrlPhrase => Self::RuleConditionUrlPhrase { pattern },
            Dimension::UrlRe => Self::RuleConditionUrlRe { pattern },
            Dimension::BodyPhrase => Self::RuleConditionBodyPhrase { pattern },
            Dimension::BodyRe => Self::RuleConditionBodyRe { pattern },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_parameters_keep_whitespace_but_never_urls_or_credentials() {
        use rrss_core::rank::reasons as core;
        let message = ReasonMessage::from(core::ReasonMessage::RankAuthor {
            habit: core::Habit::Read,
            pattern:
                "https://reader:canary-pass@source.example/private?q=canary-query sk-canary-key"
                    .into(),
        });
        let wire = serde_json::to_value(message).unwrap();
        assert_eq!(wire["code"], "rank_author");
        assert_eq!(wire["params"]["habit"], "read");
        let pattern = wire["params"]["pattern"].as_str().unwrap();
        assert!(pattern.contains("source.example"));
        for private in ["://", "private", "canary", "reader:"] {
            assert!(!pattern.contains(private));
        }
        assert!(wire.get("detail").is_none());
        let spaced = ReasonMessage::from(core::ReasonMessage::InterestMatch {
            pattern: "Rust  async\nnotes".into(),
        });
        assert_eq!(
            serde_json::to_value(spaced).unwrap()["params"],
            serde_json::json!({"pattern":"Rust  async\nnotes"})
        );
    }
}
