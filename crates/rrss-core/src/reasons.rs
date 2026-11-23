//! Locale-independent reasons, shared by storage and the IPC adapter.

// One declaration supplies storage serde, IPC serde/TS and the exported codes.
// Core has no dependency on ts-rs; the app invokes this with that extra derive.
#[macro_export]
macro_rules! reason_types {
    ([$($derive:path),*]; $($(#[$attr:meta])* $variant:ident $({ $($field:ident: $ty:ty),* $(,)? })? => $code:literal),* $(,)?) => {
        #[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize $(, $derive)*)]
        #[serde(tag = "code", content = "params", rename_all = "snake_case")]
        pub enum ReasonMessage {
            $($(#[$attr])* #[serde(rename = $code)] $variant $({ $($field: $ty),* })?),*
        }
        impl ReasonMessage {
            pub const CODES: &'static [&'static str] = &[$($code),*];
            pub fn code(&self) -> &'static str {
                match self { $(Self::$variant $({ $($field: _,)* })? => $code),* }
            }
        }
    };
    ($($derive:path),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize $(, $derive)*)]
        #[serde(rename_all = "snake_case")]
        pub enum Habit { Read, Skip }

        // Old rule/interest rows stored content, without a preformatted sentence.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize $(, $derive)*)]
        #[serde(rename_all = "snake_case")]
        pub enum ReasonPrefix {
            Author, Tag, TitleKw, TitleRe, UrlPhrase, UrlRe, BodyPhrase, BodyRe, Interest,
            RankTitleKwRead, RankTitleKwSkip, RankAuthorRead, RankAuthorSkip,
            RankTagRead, RankTagSkip, RankDomainRead, RankDomainSkip, RankAffinity,
        }

        $crate::reason_types!([$($derive),*];
            LegacyText { text: String, prefix: Option<ReasonPrefix> } => "legacy_text",
            RankTitleKw { habit: Habit, pattern: String } => "rank_title_kw",
            RankAuthor { habit: Habit, pattern: String } => "rank_author",
            RankTag { habit: Habit, pattern: String } => "rank_tag",
            RankDomain { habit: Habit, pattern: String } => "rank_domain",
            RankSource { habit: Habit } => "rank_source",
            RankCoverage { count: u64 } => "rank_coverage",
            RankFreshness {} => "rank_freshness",
            RankAffinity { pattern: String } => "rank_affinity",
            RuleAuthor { pattern: String } => "rule_author",
            RuleTag { pattern: String } => "rule_tag",
            RuleTitleKw { pattern: String } => "rule_title_kw",
            RuleTitleRe { pattern: String } => "rule_title_re",
            RuleUrlPhrase { pattern: String } => "rule_url_phrase",
            RuleUrlRe { pattern: String } => "rule_url_re",
            RuleBodyPhrase { pattern: String } => "rule_body_phrase",
            RuleBodyRe { pattern: String } => "rule_body_re",
            RuleConditionAuthor { pattern: String } => "rule_condition_author",
            RuleConditionTag { pattern: String } => "rule_condition_tag",
            RuleConditionTitleKw { pattern: String } => "rule_condition_title_kw",
            RuleConditionTitleRe { pattern: String } => "rule_condition_title_re",
            RuleConditionUrlPhrase { pattern: String } => "rule_condition_url_phrase",
            RuleConditionUrlRe { pattern: String } => "rule_condition_url_re",
            RuleConditionBodyPhrase { pattern: String } => "rule_condition_body_phrase",
            RuleConditionBodyRe { pattern: String } => "rule_condition_body_re",
            InterestMatch { pattern: String } => "interest_match",
            Unknown {} => "unknown",
        );
    };
}

reason_types!();

// Used before storing parameters and by the app's centralized parameter adapter.
// Content fields (matched snippets, model-facing labels) remain separate.
pub fn safe_text(value: &str) -> String {
    let clean = crate::logsafe::scrub_secrets(value);
    if clean.split_whitespace().any(|part| part.contains("://")) {
        clean
            .split_whitespace()
            .map(|part| {
                if part.contains("://") {
                    crate::logsafe::host_str(part)
                } else {
                    part.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        clean
    }
}

#[derive(serde::Serialize)]
pub(crate) struct StoredReason<'a> {
    pub component: &'static str,
    #[serde(flatten)]
    pub message: &'a ReasonMessage,
    pub points: i64,
}

impl ReasonPrefix {
    pub(crate) fn for_feature(dimension: crate::rules::Dimension, habit: Habit) -> Self {
        use crate::rules::Dimension;
        match (dimension, habit) {
            (Dimension::TitleKw, Habit::Read) => Self::RankTitleKwRead,
            (Dimension::TitleKw, Habit::Skip) => Self::RankTitleKwSkip,
            (Dimension::Author, Habit::Read) => Self::RankAuthorRead,
            (Dimension::Author, Habit::Skip) => Self::RankAuthorSkip,
            (Dimension::Tag, Habit::Read) => Self::RankTagRead,
            (Dimension::Tag, Habit::Skip) => Self::RankTagSkip,
            (_, Habit::Read) => Self::RankDomainRead,
            (_, Habit::Skip) => Self::RankDomainSkip,
        }
    }

    pub fn for_dimension(dimension: crate::rules::Dimension) -> Self {
        use crate::rules::Dimension;
        match dimension {
            Dimension::Author => Self::Author,
            Dimension::Tag => Self::Tag,
            Dimension::TitleKw => Self::TitleKw,
            Dimension::TitleRe => Self::TitleRe,
            Dimension::UrlPhrase => Self::UrlPhrase,
            Dimension::UrlRe => Self::UrlRe,
            Dimension::BodyPhrase => Self::BodyPhrase,
            Dimension::BodyRe => Self::BodyRe,
        }
    }
}

impl ReasonMessage {
    pub fn rule(dimension: crate::rules::Dimension, pattern: String) -> Self {
        use crate::rules::Dimension;
        let pattern = safe_text(&pattern);
        match dimension {
            Dimension::Author => Self::RuleAuthor { pattern },
            Dimension::Tag => Self::RuleTag { pattern },
            Dimension::TitleKw => Self::RuleTitleKw { pattern },
            Dimension::TitleRe => Self::RuleTitleRe { pattern },
            Dimension::UrlPhrase => Self::RuleUrlPhrase { pattern },
            Dimension::UrlRe => Self::RuleUrlRe { pattern },
            Dimension::BodyPhrase => Self::RuleBodyPhrase { pattern },
            Dimension::BodyRe => Self::RuleBodyRe { pattern },
        }
    }

    pub(crate) fn from_stored(
        value: &serde_json::Value,
        content: &str,
        prefix: Option<ReasonPrefix>,
    ) -> Self {
        if value.get("code").is_some() {
            serde_json::from_value(value.clone()).unwrap_or(Self::Unknown {})
        } else {
            let text = value.get("text").and_then(serde_json::Value::as_str);
            Self::LegacyText {
                text: text.unwrap_or(content).to_string(),
                prefix: if text.is_some() { None } else { prefix },
            }
        }
    }

    // Content-only fields remain available to the model-facing tool projection.
    pub(crate) fn content(&self) -> Option<&str> {
        match self {
            Self::LegacyText { text, .. } => Some(text),
            Self::RankTitleKw { pattern, .. }
            | Self::RankAuthor { pattern, .. }
            | Self::RankTag { pattern, .. }
            | Self::RankDomain { pattern, .. }
            | Self::RankAffinity { pattern }
            | Self::RuleAuthor { pattern }
            | Self::RuleTag { pattern }
            | Self::RuleTitleKw { pattern }
            | Self::RuleTitleRe { pattern }
            | Self::RuleUrlPhrase { pattern }
            | Self::RuleUrlRe { pattern }
            | Self::RuleBodyPhrase { pattern }
            | Self::RuleBodyRe { pattern }
            | Self::RuleConditionAuthor { pattern }
            | Self::RuleConditionTag { pattern }
            | Self::RuleConditionTitleKw { pattern }
            | Self::RuleConditionTitleRe { pattern }
            | Self::RuleConditionUrlPhrase { pattern }
            | Self::RuleConditionUrlRe { pattern }
            | Self::RuleConditionBodyPhrase { pattern }
            | Self::RuleConditionBodyRe { pattern }
            | Self::InterestMatch { pattern } => Some(pattern),
            _ => None,
        }
    }
}
