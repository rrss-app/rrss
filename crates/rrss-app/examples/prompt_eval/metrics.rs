use super::*;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Higher,
    Lower,
    TowardOne,
    Observe,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metric {
    pub value: f64,
    pub unit: String,
    pub direction: Direction,
}

pub type Metrics = BTreeMap<String, Metric>;

pub fn put(
    metrics: &mut Metrics,
    key: impl Into<String>,
    value: impl Into<f64>,
    unit: &str,
    direction: Direction,
) {
    metrics.insert(
        key.into(),
        Metric {
            value: value.into(),
            unit: unit.into(),
            direction,
        },
    );
}

pub fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

pub fn chars(text: &str) -> usize {
    text.chars().count()
}

/// Fixed proxy, not a tokenizer: CJK (including kana/hangul) = 1 token,
/// ASCII/Latin = 0.25, other characters = 0.5; round the total up once.
pub fn estimate_tokens(text: &str) -> usize {
    let quarters: usize = text
        .chars()
        .map(|c| {
            if is_cjk(c) {
                4
            } else if c.is_ascii() || c <= '\u{024f}' {
                1
            } else {
                2
            }
        })
        .sum();
    quarters.div_ceil(4)
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x9fff | 0x20000..=0x323af | 0x3000..=0x30ff | 0xac00..=0xd7af | 0xff00..=0xffef)
}

pub fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// A block separator is a blank line. A heading followed by an omission marker
/// is not counted as the last block; the serialized segments are measured as sent.
pub fn orphan_headings(segments: &[String]) -> usize {
    segments
        .iter()
        .filter(|segment| {
            segment
                .split("\n\n")
                .map(str::trim)
                .filter(|b| !b.is_empty())
                .last()
                .is_some_and(|block| block.starts_with("### ") && !block.contains('\n'))
        })
        .count()
}

pub fn call_metrics(calls: &[Call]) -> Metrics {
    let mut metrics = Metrics::new();
    put(
        &mut metrics,
        "requests",
        calls.iter().map(|c| c.requests).sum::<usize>() as f64,
        "requests",
        Direction::Lower,
    );
    put(
        &mut metrics,
        "prompt_tokens",
        calls
            .iter()
            .map(|c| u64::from(c.prompt_tokens))
            .sum::<u64>() as f64,
        "tokens",
        Direction::Lower,
    );
    put(
        &mut metrics,
        "completion_tokens",
        calls
            .iter()
            .map(|c| u64::from(c.completion_tokens))
            .sum::<u64>() as f64,
        "tokens",
        Direction::Lower,
    );
    put(
        &mut metrics,
        "latency_ms",
        calls.iter().map(|c| c.ms).sum::<u128>() as f64,
        "ms",
        Direction::Lower,
    );
    metrics
}

pub fn online_metrics(
    cases: &[Case],
    totals: &BTreeMap<&'static str, (u64, u64, usize)>,
) -> Metrics {
    let mut metrics = Metrics::new();
    #[derive(Default)]
    struct Group {
        passed: usize,
        checks: usize,
        calls: Vec<Call>,
        latency_ms: f64,
        facts_hit: f64,
        facts_total: f64,
    }
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for case in cases {
        for (variant, output) in &case.outputs {
            let prefix = format!("{variant}.{}.{}", case.section, case.key);
            for (key, metric) in call_metrics(&output.calls)
                .into_iter()
                .chain(output.metrics.clone())
            {
                metrics.insert(format!("{prefix}.{key}"), metric);
            }
            let passed = output.checks.iter().filter(|c| c.ok).count();
            put(
                &mut metrics,
                format!("{prefix}.check_pass_rate"),
                ratio(passed, output.checks.len()),
                "ratio",
                Direction::Higher,
            );
            for key in [format!("{variant}.{}", case.section), variant.to_string()] {
                let group = groups.entry(key).or_default();
                group.passed += passed;
                group.checks += output.checks.len();
                group.calls.extend(output.calls.clone());
                group.latency_ms += output
                    .metrics
                    .get("latency_ms")
                    .map(|m| m.value)
                    .unwrap_or_else(|| output.calls.iter().map(|c| c.ms).sum::<u128>() as f64);
                group.facts_hit += output.metrics.get("facts_hit").map_or(0.0, |m| m.value);
                group.facts_total += output.metrics.get("facts_total").map_or(0.0, |m| m.value);
            }
        }
    }
    for (prefix, group) in groups {
        put(
            &mut metrics,
            format!("{prefix}.check_pass_rate"),
            ratio(group.passed, group.checks),
            "ratio",
            Direction::Higher,
        );
        let calls = call_metrics(&group.calls);
        put(
            &mut metrics,
            format!("{prefix}.service_latency_ms"),
            calls["latency_ms"].value,
            "ms",
            Direction::Lower,
        );
        for (key, metric) in calls {
            metrics.insert(format!("{prefix}.{key}"), metric);
        }
        put(
            &mut metrics,
            format!("{prefix}.latency_ms"),
            group.latency_ms,
            "ms",
            Direction::Lower,
        );
        if group.facts_total > 0.0 {
            put(
                &mut metrics,
                format!("{prefix}.fact_recall"),
                group.facts_hit / group.facts_total,
                "ratio",
                Direction::Higher,
            );
        }
    }
    // Include prerequisite summaries that are not themselves selected as report rows.
    for (variant, (prompt, completion, requests)) in totals {
        let prefix = format!("{variant}.all_requests");
        for (key, value, unit) in [
            ("prompt_tokens", *prompt as f64, "tokens"),
            ("completion_tokens", *completion as f64, "tokens"),
            ("requests", *requests as f64, "requests"),
        ] {
            put(
                &mut metrics,
                format!("{prefix}.{key}"),
                value,
                unit,
                Direction::Lower,
            );
        }
    }
    metrics
}

pub fn revision(root: &Path) -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

pub fn fixture_hash(root: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for file in ["input-cases.json", "cases.json", "baseline-zh.json"] {
        hash.update(file.as_bytes());
        hash.update(std::fs::read(root.join("testdata/eval").join(file))?);
    }
    hash.update(include_bytes!("fixtures.rs"));
    Ok(format!("{:x}", hash.finalize()))
}

pub fn production_hash(root: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    for file in [
        "crates/rrss-core/src/summary_extract.rs",
        "crates/rrss-core/src/digest.rs",
        "crates/rrss-core/src/digest_sources.rs",
        "crates/rrss-core/src/articles.rs",
        "crates/rrss-core/src/cluster_summary.rs",
        "crates/rrss-core/src/rag.rs",
        "crates/rrss-core/src/ai_store.rs",
        "crates/rrss-app/src/jobs/digests.rs",
        "crates/rrss-app/src/jobs/summaries.rs",
    ] {
        hash.update(file.as_bytes());
        hash.update(std::fs::read(root.join(file))?);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_budgets_and_weighted_tokens_do_not_count_bytes() {
        assert_eq!(chars("中a文"), 3);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("中文"), 2);
        assert_eq!(estimate_tokens("中文abcd"), 3);
        assert_eq!(estimate_tokens("é".repeat(4).as_str()), 1);
        assert_eq!(ratio(0, 0), 0.0);
        assert_eq!(ratio(3, 4), 0.75);
    }

    #[test]
    fn headings_count_only_at_the_end_of_nonempty_blocks() {
        assert_eq!(
            orphan_headings(&[
                "text\n\n### End\n\n".into(),
                "### Start\n\nbody".into(),
                "text\n\n### Cut\n\n…".into()
            ]),
            1
        );
    }

    #[test]
    fn online_groups_use_path_latency_and_micro_average_facts() {
        let mut extra = Metrics::new();
        put(&mut extra, "latency_ms", 5.0, "ms", Direction::Lower);
        put(&mut extra, "facts_hit", 1.0, "facts", Direction::Higher);
        put(&mut extra, "facts_total", 2.0, "facts", Direction::Observe);
        let case = Case {
            section: "article_summaries",
            key: "fixture".into(),
            title: "fixture".into(),
            input: String::new(),
            outputs: BTreeMap::from([(
                "en-en",
                Output {
                    shown: String::new(),
                    checks: vec![check("fields", true, "")],
                    calls: vec![
                        Call {
                            ms: 10,
                            requests: 1,
                            ..Call::default()
                        },
                        Call {
                            ms: 10,
                            requests: 1,
                            ..Call::default()
                        },
                    ],
                    metrics: extra,
                },
            )]),
        };
        let metrics = online_metrics(&[case], &BTreeMap::new());
        assert_eq!(metrics["en-en.article_summaries.latency_ms"].value, 5.0);
        assert_eq!(
            metrics["en-en.article_summaries.service_latency_ms"].value,
            20.0
        );
        assert_eq!(metrics["en-en.article_summaries.fact_recall"].value, 0.5);
    }
}
