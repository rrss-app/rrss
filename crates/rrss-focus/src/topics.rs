//! Private, rebuildable attribution for marginal topic utility in Focus.
//! Never infer identity or strength from rounded, truncated display reasons.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::combine::Switches;

pub(crate) const MIN_SUPPORT: f64 = 0.5;
pub(crate) const MAX_KEYS: usize = 16;

// A compact SQL projection avoids decoding all display reasons and lexical
// keys for every admitted member on every page. Full attribution is loaded
// only after shortlisting. Projection order is shared with the SQL expression.
pub(crate) const LANE_SQL: &str = "json_extract(r.reasons_json, '$.lane')";

#[derive(Serialize, Deserialize)]
pub(crate) struct Lane(
    f64,
    f64,
    f64,
    f64,
    Option<String>,
    Option<String>,
    Option<i64>,
);

#[derive(Hash, PartialEq, Eq)]
pub(crate) enum LaneKey<'a> {
    Lexical(&'a str),
    Semantic(&'a str, i64),
}

impl Lane {
    pub fn from_evidence(e: &Evidence) -> Self {
        Self(
            e.raw_score,
            e.features,
            e.affinity,
            e.lexical.iter().map(|(_, w)| w).sum(),
            e.lexical.first().map(|(key, _)| key.clone()),
            e.semantic.as_ref().map(|(epoch, _)| epoch.clone()),
            e.semantic.as_ref().map(|(_, id)| *id),
        )
    }
    pub fn decode(json: &str) -> Option<Self> {
        let lane: Self = serde_json::from_str(json).ok()?;
        (lane.0.is_finite()
            && lane.1.is_finite()
            && lane.2.is_finite()
            && lane.3.is_finite()
            && lane.3 >= 0.0
            && lane.4.as_deref().is_none_or(valid_lexical)
            && match (&lane.5, lane.6) {
                (Some(epoch), Some(id)) => valid_semantic(epoch, id),
                (None, None) => true,
                _ => false,
            })
        .then_some(lane)
    }

    pub fn epoch(&self) -> Option<&str> {
        self.5.as_deref()
    }

    pub fn key(&self, on: Switches, semantic: bool) -> Option<LaneKey<'_>> {
        let features = if on.features { self.1 } else { 0.0 };
        let affinity = if on.affinity { self.2 } else { 0.0 };
        let lexical = if features >= MIN_SUPPORT && self.4.is_some() {
            features.min(self.3)
        } else {
            0.0
        };
        let vector = if semantic && affinity >= MIN_SUPPORT && self.5.is_some() {
            affinity
        } else {
            0.0
        };
        let points = (lexical + vector).min((features + affinity).max(0.0));
        if points < MIN_SUPPORT || self.0.round() - (self.0 - points).round() <= 0.0 {
            return None;
        }
        if vector > 0.0 {
            Some(LaneKey::Semantic(self.5.as_deref()?, self.6?))
        } else {
            Some(LaneKey::Lexical(self.4.as_deref()?))
        }
    }
}

fn valid_lexical(key: &str) -> bool {
    (key.starts_with("title_kw:") || key.starts_with("tag:")) && key.len() <= 1024
}

fn valid_semantic(epoch: &str, id: i64) -> bool {
    epoch.len() == 64 && epoch.bytes().all(|b| b.is_ascii_hexdigit()) && id > 0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Evidence {
    pub raw_score: f64,
    pub features: f64,
    pub affinity: f64,
    // Stable full feature keys, with positive contribution before normalization.
    pub lexical: Vec<(String, f64)>,
    // The representative belongs to this exact learned vector partition.
    pub semantic: Option<(String, i64)>,
}

impl Evidence {
    pub fn valid(&self) -> bool {
        self.raw_score.is_finite()
            && self.features.is_finite()
            && self.affinity.is_finite()
            && self.lexical.len() <= MAX_KEYS
            && self
                .lexical
                .iter()
                .all(|(k, w)| valid_lexical(k) && w.is_finite() && *w > 0.0)
            && self
                .semantic
                .as_ref()
                .is_none_or(|(epoch, id)| valid_semantic(epoch, *id))
    }

    pub fn epoch(&self) -> Option<&str> {
        self.semantic.as_ref().map(|(epoch, _)| epoch.as_str())
    }

    pub fn allocation(&self, on: Switches, semantic: bool) -> Allocation {
        let features = if on.features { self.features } else { 0.0 };
        let affinity = if on.affinity { self.affinity } else { 0.0 };
        // Negative evidence in either enabled component reduces the entire
        // attributable budget, even when its positive identity is unavailable.
        let net = (features + affinity).max(0.0);
        let mut keys = Vec::new();
        let sum = self.lexical.iter().map(|(_, w)| w).sum::<f64>();
        if features >= MIN_SUPPORT && sum > 0.0 {
            keys.extend(
                self.lexical
                    .iter()
                    .map(|(k, w)| (k.clone(), features.min(sum) * w / sum)),
            );
        }
        if semantic && affinity >= MIN_SUPPORT {
            if let Some((epoch, id)) = &self.semantic {
                keys.push((format!("vector:{epoch}:{id}"), affinity));
            }
        }
        let positive = keys.iter().map(|(_, w)| w).sum::<f64>();
        let scale = if positive > 0.0 {
            (net / positive).min(1.0)
        } else {
            0.0
        };
        for (_, w) in &mut keys {
            *w *= scale;
        }
        if positive * scale < MIN_SUPPORT {
            keys.clear();
        } else {
            // Finite difference of the existing integer rank, removing exactly
            // these attributable points. This also works after a mixed vector
            // partition disables only semantic attribution. Rounding residuals
            // cannot permanently starve a weaker topic deep in the list.
            let projected =
                (self.raw_score.round() - (self.raw_score - positive * scale).round()).max(0.0);
            if projected == 0.0 {
                keys.clear();
            } else {
                for (_, w) in &mut keys {
                    *w *= projected / (positive * scale);
                }
            }
        }
        Allocation(keys)
    }
}

#[derive(Default)]
pub(crate) struct Allocation(pub Vec<(String, f64)>);

impl Allocation {
    pub fn eligible(&self) -> bool {
        !self.0.is_empty()
    }

    pub fn discount(&self, covered: &HashMap<String, usize>) -> f64 {
        self.0
            .iter()
            .map(|(key, points)| {
                let n = covered.get(key).copied().unwrap_or(0) as f64;
                points * n / (1.0 + n)
            })
            .sum()
    }

    pub fn total(&self) -> f64 {
        self.0.iter().map(|(_, w)| w).sum()
    }

    pub fn observe(&self, covered: &mut HashMap<String, usize>) {
        for (key, _) in &self.0 {
            *covered.entry(key.clone()).or_default() += 1;
        }
    }
}
