//! Compatibility boundary for display reasons and private ranking metadata.

use super::{combine::Reason, topics};
use serde::{Deserialize, Serialize};

pub(crate) fn reasons(json: &str) -> Vec<Reason> {
    #[derive(Deserialize)]
    struct Display {
        reasons: Vec<Reason>,
    }
    if json.trim_start().starts_with('[') {
        serde_json::from_str(json).unwrap_or_default()
    } else {
        serde_json::from_str::<Display>(json)
            .map(|cache| cache.reasons)
            .unwrap_or_default()
    }
}

pub(crate) fn topics(json: &str) -> Option<topics::Evidence> {
    // Old arrays (also used for neutral new caches) need no JSON parsing here.
    if !json.trim_start().starts_with('{') {
        return None;
    }
    #[derive(Deserialize)]
    struct Attribution {
        topics: topics::Evidence,
    }
    let evidence = serde_json::from_str::<Attribution>(json).ok()?.topics;
    evidence.valid().then_some(evidence)
}

pub(crate) fn encode(
    reasons: &[Reason],
    topics: Option<topics::Evidence>,
) -> crate::Result<String> {
    #[derive(Serialize)]
    struct Stored<'a> {
        lane: topics::Lane,
        reasons: &'a [Reason],
        topics: topics::Evidence,
    }
    let result = if let Some(topics) = topics {
        serde_json::to_string(&Stored {
            lane: topics::Lane::from_evidence(&topics),
            reasons,
            topics,
        })
    } else {
        serde_json::to_string(reasons)
    };
    result.map_err(|e| crate::Error::Parse(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_and_new_display_reasons_survive_missing_or_bad_attribution() {
        let old = r#"[{"component":"freshness","code":"rank_freshness","params":{},"points":-2},
            {"component":"affinity","text":"old display only","points":2}]"#;
        let display = reasons(old);
        assert_eq!(display.len(), 2);
        assert!(topics(old).is_none());
        for metadata in [
            "null",
            "{}",
            r#"{"features":100,"affinity":0,"lexical":[],"semantic":null}"#,
        ] {
            let json = format!(r#"{{"reasons":{old},"topics":{metadata}}}"#);
            assert_eq!(reasons(&json), display);
            assert!(topics(&json).is_none());
        }
        let json = encode(
            &display,
            Some(topics::Evidence {
                raw_score: 1.0,
                features: 2.2,
                affinity: -1.2,
                lexical: vec![("title_kw:fermentation".into(), 2.2)],
                semantic: None,
            }),
        )
        .unwrap();
        assert_eq!(reasons(&json), display);
        let allocation = topics(&json)
            .unwrap()
            .allocation(super::super::combine::Switches::ALL, true);
        let mut covered = std::collections::HashMap::new();
        allocation.observe(&mut covered);
        assert_eq!(
            allocation.discount(&covered),
            0.5,
            "negative evidence reduces the entire budget"
        );
        assert!(reasons("broken").is_empty());
        assert!(topics("broken").is_none());
    }
}
