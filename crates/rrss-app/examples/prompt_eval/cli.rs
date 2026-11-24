use super::*;

pub const ONLINE_SECTIONS: &[&str] = &[
    "summaries",
    "digests",
    "clusters",
    "memory",
    "citations",
    "weekly_digests",
    "article_summaries",
    "digests_cold",
];
pub const OFFLINE_SECTIONS: &[&str] = &[
    "summary_extract",
    "digests",
    "auto_summary",
    "cluster_summary",
];

pub const HELP: &str = "RRSS prompt evaluation

cargo run -p rrss-app --example prompt_eval -- [OPTIONS]

  --offline                 Deterministic inputs only; zero model requests
  --tier low|medium|high     Input tier (default: medium)
  --compare <baseline.json> Compare offline-results.json or online results.json
  --only <sections>         Comma-separated sections (default: all in this mode)
  --variants <variants>     baseline,en-zh,en-en (online; default: all)
  --concurrency <n>         Concurrent model requests (default: 4)
  --out <directory>         Default: test-results/prompt-eval
  --help                    Show this help

Online sections: summaries,digests,clusters,memory,citations,
                 weekly_digests,article_summaries,digests_cold
Offline sections: summary_extract,digests,auto_summary,cluster_summary
Offline extraction always measures low, medium and high; other inputs use --tier.
Online runs require RRSS_EVAL_BASE_URL and RRSS_EVAL_MODEL; an API key is optional.";

pub struct Cli {
    pub offline: bool,
    pub tier: InputTier,
    pub variants: Vec<Variant>,
    pub sections: BTreeSet<String>,
    pub concurrency: usize,
    pub out: PathBuf,
    pub compare: Option<PathBuf>,
}

impl Cli {
    pub fn parse(args: &[String], root: &Path) -> Result<Self> {
        let mut values = HashMap::new();
        let mut offline = false;
        let mut i = 0;
        while i < args.len() {
            let flag = args[i].as_str();
            if flag == "--offline" {
                offline = true;
                i += 1;
                continue;
            }
            if ![
                "--tier",
                "--variants",
                "--only",
                "--concurrency",
                "--out",
                "--compare",
            ]
            .contains(&flag)
            {
                bail!("unknown option {flag}; see --help");
            }
            let value = args
                .get(i + 1)
                .filter(|v| !v.starts_with("--"))
                .with_context(|| format!("{flag} requires a value"))?;
            values.entry(flag).or_insert(value.as_str());
            i += 2;
        }
        let tier = InputTier::parse(values.get("--tier").copied().unwrap_or("medium"))
            .context("--tier must be low, medium or high")?;
        let variants = values.get("--variants").map_or_else(
            || Ok(Variant::ALL.to_vec()),
            |list| {
                list.split(',')
                    .map(|id| {
                        Variant::ALL
                            .into_iter()
                            .find(|v| v.id() == id.trim())
                            .ok_or_else(|| anyhow!("unknown variant {id}"))
                    })
                    .collect::<Result<Vec<_>>>()
            },
        )?;
        let allowed = if offline {
            OFFLINE_SECTIONS
        } else {
            ONLINE_SECTIONS
        };
        let sections: BTreeSet<String> = values.get("--only").map_or_else(
            || allowed.iter().map(|s| s.to_string()).collect(),
            |list| list.split(',').map(|s| s.trim().to_string()).collect(),
        );
        for section in &sections {
            if !allowed.contains(&section.as_str()) {
                bail!(
                    "unknown {} section {section}",
                    if offline { "offline" } else { "online" }
                );
            }
        }
        let concurrency = values
            .get("--concurrency")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(4)
            .max(1);
        Ok(Self {
            offline,
            tier,
            variants,
            sections,
            concurrency,
            out: values
                .get("--out")
                .map(PathBuf::from)
                .unwrap_or_else(|| root.join("test-results/prompt-eval")),
            compare: values.get("--compare").map(PathBuf::from),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli> {
        Cli::parse(
            &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            Path::new("."),
        )
    }

    #[test]
    fn old_options_and_new_sections_remain_selectable() {
        let c = parse(&[
            "--variants",
            "en-zh",
            "--only",
            "weekly_digests,article_summaries,digests_cold",
            "--out",
            "report",
            "--concurrency",
            "2",
            "--tier",
            "high",
        ])
        .unwrap();
        assert_eq!(c.variants, vec![Variant::EnZh]);
        assert_eq!(c.sections.len(), 3);
        assert_eq!(c.tier, InputTier::High);
        assert_eq!(c.concurrency, 2);
        assert_eq!(c.out, PathBuf::from("report"));
        assert_eq!(
            parse(&["--out", "first", "--out", "second"]).unwrap().out,
            PathBuf::from("first")
        );
        assert_eq!(parse(&["--concurrency", "invalid"]).unwrap().concurrency, 4);
        assert_eq!(parse(&["--concurrency", "0"]).unwrap().concurrency, 1);
    }

    #[test]
    fn offline_defaults_do_not_require_model_configuration() {
        let c = parse(&["--offline", "--compare", "baseline.json"]).unwrap();
        assert!(c.offline);
        assert_eq!(c.tier, InputTier::Medium);
        assert_eq!(c.sections.len(), 4);
        assert_eq!(c.compare, Some(PathBuf::from("baseline.json")));
        assert!(parse(&["--tier", "huge"]).is_err());
        assert!(parse(&["--only", "missing"]).is_err());
        assert!(parse(&["--out"]).is_err());
    }
}
