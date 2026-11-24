use super::*;
use metrics::{Direction, Metrics, put, ratio};

#[derive(Debug, Clone, Serialize)]
pub struct DiffRow {
    pub metric: String,
    pub baseline: Option<f64>,
    pub current: Option<f64>,
    pub delta: Option<f64>,
    pub change_percent: Option<f64>,
    pub unit: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Comparison {
    pub baseline_revision: String,
    pub baseline_tier: String,
    pub warnings: Vec<String>,
    pub rows: Vec<DiffRow>,
}

/// Older online result files did not contain a metrics map. Recover the checks
/// and actual call usage from their serialized cases without needing their model.
fn read_metrics(doc: &Value) -> Result<Metrics> {
    if let Some(metrics) = doc.get("metrics") {
        let metrics: Metrics =
            serde_json::from_value(metrics.clone()).context("invalid baseline metrics")?;
        anyhow::ensure!(
            metrics.values().all(|m| m.value.is_finite()),
            "non-finite baseline metric"
        );
        return Ok(metrics);
    }
    let cases = doc["cases"]
        .as_array()
        .context("baseline has neither metrics nor online cases")?;
    let mut metrics = Metrics::new();
    let mut groups: BTreeMap<String, (usize, usize, f64, f64, f64, f64)> = BTreeMap::new();
    for case in cases {
        let section = case["section"]
            .as_str()
            .context("baseline case missing section")?;
        let key = case["key"].as_str().context("baseline case missing key")?;
        let outputs = case["outputs"]
            .as_object()
            .context("baseline case missing outputs")?;
        for (variant, output) in outputs {
            let prefix = format!("{variant}.{section}.{key}");
            let checks = output["checks"]
                .as_array()
                .context("baseline output missing checks")?;
            let passed = checks.iter().filter(|check| check["ok"] == true).count();
            put(
                &mut metrics,
                format!("{prefix}.check_pass_rate"),
                ratio(passed, checks.len()),
                "ratio",
                Direction::Higher,
            );
            let calls = output["calls"]
                .as_array()
                .context("baseline output missing calls")?;
            let prompt: f64 = calls
                .iter()
                .map(|c| c["prompt_tokens"].as_f64().unwrap_or(0.0))
                .sum();
            let completion: f64 = calls
                .iter()
                .map(|c| c["completion_tokens"].as_f64().unwrap_or(0.0))
                .sum();
            let ms: f64 = calls.iter().map(|c| c["ms"].as_f64().unwrap_or(0.0)).sum();
            let requests: f64 = calls
                .iter()
                .map(|c| c["requests"].as_f64().unwrap_or(1.0))
                .sum();
            for (name, value, unit) in [
                ("prompt_tokens", prompt, "tokens"),
                ("completion_tokens", completion, "tokens"),
                ("latency_ms", ms, "ms"),
                ("requests", requests, "requests"),
            ] {
                put(
                    &mut metrics,
                    format!("{prefix}.{name}"),
                    value,
                    unit,
                    Direction::Lower,
                );
            }
            for group in [format!("{variant}.{section}"), variant.clone()] {
                let group = groups.entry(group).or_default();
                group.0 += passed;
                group.1 += checks.len();
                group.2 += prompt;
                group.3 += completion;
                group.4 += ms;
                group.5 += requests;
            }
        }
    }
    for (prefix, (passed, count, prompt, completion, ms, requests)) in groups {
        put(
            &mut metrics,
            format!("{prefix}.check_pass_rate"),
            ratio(passed, count),
            "ratio",
            Direction::Higher,
        );
        for (name, value, unit) in [
            ("prompt_tokens", prompt, "tokens"),
            ("completion_tokens", completion, "tokens"),
            ("latency_ms", ms, "ms"),
            ("requests", requests, "requests"),
        ] {
            put(
                &mut metrics,
                format!("{prefix}.{name}"),
                value,
                unit,
                Direction::Lower,
            );
        }
    }
    Ok(metrics)
}

pub fn load(path: &Path) -> Result<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).context("read comparison baseline")?)
        .context("parse comparison baseline")
}

fn mode(doc: &Value) -> &str {
    doc["mode"]
        .as_str()
        .unwrap_or(if doc.get("cases").is_some() {
            "online"
        } else {
            "offline"
        })
}

pub fn validate(baseline: &Value, offline: bool) -> Result<()> {
    let expected = if offline { "offline" } else { "online" };
    anyhow::ensure!(
        mode(baseline) == expected,
        "comparison baseline must be {expected}"
    );
    read_metrics(baseline)?;
    Ok(())
}

pub fn compare(baseline: &Value, current: &Value) -> Result<Comparison> {
    anyhow::ensure!(
        mode(baseline) == mode(current),
        "cannot compare online and offline metrics with each other"
    );
    let old = read_metrics(baseline)?;
    let new = read_metrics(current)?;
    let mut warnings = Vec::new();
    let baseline_tier = baseline["tier"].as_str().unwrap_or("medium").to_string();
    if baseline_tier != current["tier"].as_str().unwrap_or("medium") {
        warnings.push(
            "Input tiers differ; treat budget and usage changes as configuration comparisons."
                .into(),
        );
    }
    if baseline.get("fixture_hash").is_some() && baseline["fixture_hash"] != current["fixture_hash"]
    {
        warnings.push("Fixture hashes differ; facts and pool sizes may have changed.".into());
    }
    if baseline.get("fixture_hash").is_none() {
        warnings.push(
            "Legacy baseline has no fixture hash; prerequisite usage cannot be recovered.".into(),
        );
    }
    for key in ["sections", "variants"] {
        if baseline.get(key).is_some()
            && current.get(key).is_some()
            && baseline[key] != current[key]
        {
            warnings.push(format!(
                "{key} selections differ; aggregate checks and usage cover different scopes."
            ));
        }
    }
    let keys: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    let mut rows = Vec::new();
    for key in keys {
        let before = old.get(key);
        let after = new.get(key);
        if let (Some(a), Some(b)) = (before, after) {
            anyhow::ensure!(
                a.unit == b.unit && a.direction == b.direction,
                "metric definition changed for {key}"
            );
        }
        let unit = after.or(before).unwrap().unit.clone();
        let (delta, change_percent, status) = match (before, after) {
            (Some(a), Some(b)) => {
                let raw_delta = b.value - a.value;
                let delta = if raw_delta.abs() < 1e-9 {
                    0.0
                } else {
                    raw_delta
                };
                let pct = if delta == 0.0 {
                    Some(0.0)
                } else if a.value != 0.0 {
                    Some(delta / a.value.abs() * 100.0)
                } else if b.value == 0.0 {
                    Some(0.0)
                } else {
                    None
                };
                let quality_delta = match b.direction {
                    Direction::Higher => delta,
                    Direction::Lower => -delta,
                    Direction::TowardOne => (a.value - 1.0).abs() - (b.value - 1.0).abs(),
                    Direction::Observe => 0.0,
                };
                let status = if delta.abs() < 1e-9 {
                    "unchanged"
                } else if quality_delta < -1e-9 {
                    "REGRESSED"
                } else if quality_delta > 1e-9 {
                    "improved"
                } else {
                    "changed"
                };
                (Some(delta), pct, status)
            }
            (None, Some(_)) => (None, None, "new"),
            (Some(_), None) => (None, None, "missing"),
            _ => unreachable!(),
        };
        rows.push(DiffRow {
            metric: key.clone(),
            baseline: before.map(|m| m.value),
            current: after.map(|m| m.value),
            delta,
            change_percent,
            unit,
            status: status.into(),
        });
    }
    Ok(Comparison {
        baseline_revision: baseline["source_revision"]
            .as_str()
            .unwrap_or("unknown")
            .into(),
        baseline_tier,
        warnings,
        rows,
    })
}

impl Comparison {
    pub fn markdown(&self) -> String {
        let regressed = self
            .rows
            .iter()
            .filter(|row| row.status == "REGRESSED")
            .count();
        let changed = self
            .rows
            .iter()
            .filter(|row| row.status != "unchanged")
            .count();
        let mut out = format!(
            "## Comparison\n\n{} metrics; {changed} changed or unmatched; **{regressed} regressed**. Baseline tier: {}; revision: {}.\n\n",
            self.rows.len(),
            self.baseline_tier,
            self.baseline_revision
        );
        for warning in &self.warnings {
            out.push_str(&format!("{warning}\n\n"));
        }
        out.push_str("| Metric | Baseline | Current | Delta | Change | Status |\n| --- | ---: | ---: | ---: | ---: | --- |\n");
        let number = |value: Option<f64>| {
            value
                .map(|v| format!("{v:.4}"))
                .unwrap_or_else(|| "—".into())
        };
        for row in &self.rows {
            let percent = row
                .change_percent
                .map(|v| format!("{v:+.2}%"))
                .unwrap_or_else(|| "— (zero baseline or unmatched)".into());
            out.push_str(&format!(
                "| {} ({}) | {} | {} | {} | {} | {} |\n",
                row.metric.replace('|', "\\|"),
                row.unit,
                number(row.baseline),
                number(row.current),
                number(row.delta),
                percent,
                row.status
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics::Metric;

    fn doc(values: &[(&str, f64, Direction)]) -> Value {
        let metrics: Metrics = values
            .iter()
            .map(|(key, value, direction)| {
                (
                    key.to_string(),
                    Metric {
                        value: *value,
                        unit: "ratio".into(),
                        direction: *direction,
                    },
                )
            })
            .collect();
        json!({"mode":"offline","tier":"medium","fixture_hash":"same","metrics":metrics})
    }

    #[test]
    fn identifies_regressions_for_quality_cost_and_over_budget_utilization() {
        let a = doc(&[
            ("recall", 0.8, Direction::Higher),
            ("tokens", 100.0, Direction::Lower),
            ("utilization", 0.95, Direction::TowardOne),
        ]);
        let b = doc(&[
            ("recall", 0.7, Direction::Higher),
            ("tokens", 120.0, Direction::Lower),
            ("utilization", 1.2, Direction::TowardOne),
        ]);
        let diff = compare(&a, &b).unwrap();
        assert!(diff.rows.iter().all(|r| r.status == "REGRESSED"));
        assert_eq!(
            diff.rows
                .iter()
                .find(|r| r.metric == "tokens")
                .unwrap()
                .change_percent,
            Some(20.0)
        );
        assert!(diff.markdown().contains("REGRESSED"));
    }

    #[test]
    fn self_comparison_and_zero_baselines_are_finite_and_missing_is_explicit() {
        let a = doc(&[
            ("zero", 0.0, Direction::Lower),
            ("keep", 1.0, Direction::Higher),
        ]);
        assert!(
            compare(&a, &a)
                .unwrap()
                .rows
                .iter()
                .all(|r| r.status == "unchanged")
        );
        let b = doc(&[
            ("zero", 1.0, Direction::Lower),
            ("new", 2.0, Direction::Observe),
        ]);
        let diff = compare(&a, &b).unwrap();
        assert_eq!(
            diff.rows
                .iter()
                .find(|r| r.metric == "zero")
                .unwrap()
                .change_percent,
            None
        );
        assert_eq!(
            diff.rows
                .iter()
                .find(|r| r.metric == "keep")
                .unwrap()
                .status,
            "missing"
        );
        assert_eq!(
            diff.rows.iter().find(|r| r.metric == "new").unwrap().status,
            "new"
        );
    }

    #[test]
    fn json_roundoff_is_zero_in_machine_readable_self_diffs() {
        let a = doc(&[("recall", 0.1, Direction::Higher)]);
        let b = doc(&[("recall", 0.1 + f64::EPSILON, Direction::Higher)]);
        let diff = compare(&a, &b).unwrap();
        assert_eq!(diff.rows[0].status, "unchanged");
        assert_eq!(diff.rows[0].delta, Some(0.0));
        assert_eq!(diff.rows[0].change_percent, Some(0.0));
    }

    #[test]
    fn legacy_online_files_recover_checks_tokens_and_latency() {
        let legacy = json!({"cases":[{"section":"digests","key":"issue","outputs":{"en-zh":{
            "checks":[{"ok":true},{"ok":false}], "calls":[{"prompt_tokens":100,"completion_tokens":25,"ms":50}]
        }}}]});
        let metrics = read_metrics(&legacy).unwrap();
        assert_eq!(metrics["en-zh.digests.issue.check_pass_rate"].value, 0.5);
        assert_eq!(metrics["en-zh.digests.issue.requests"].value, 1.0);
        assert_eq!(metrics["en-zh.digests.latency_ms"].value, 50.0);
        let current = json!({"mode":"online","tier":"medium","metrics":metrics});
        assert!(
            compare(&legacy, &current)
                .unwrap()
                .rows
                .iter()
                .all(|r| r.status == "unchanged")
        );
        assert!(compare(&legacy, &doc(&[])).is_err());
    }
}
