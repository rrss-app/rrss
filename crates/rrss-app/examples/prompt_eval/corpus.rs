use super::*;
use metrics::{chars, normalize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Body,
    Table,
    Code,
    Lead,
    Tail,
}

impl Category {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Body => "body",
            Self::Table => "table",
            Self::Code => "code",
            Self::Lead => "lead",
            Self::Tail => "tail",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Fact {
    pub text: String,
    pub category: Category,
    /// Fixed textual equivalents for online output, including translated units.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alternatives: Vec<String>,
}

impl Fact {
    pub fn hit(&self, text: &str) -> bool {
        let normalized = normalize(text);
        std::iter::once(&self.text)
            .chain(&self.alternatives)
            .filter(|fact| !fact.trim().is_empty())
            .any(|fact| {
                let needle = normalize(fact);
                normalized.match_indices(&needle).any(|(start, _)| {
                    let before = normalized[..start].chars().next_back();
                    let after = normalized[start + needle.len()..].chars().next();
                    let first = needle.chars().next().unwrap();
                    let last = needle.chars().next_back().unwrap();
                    !(first.is_ascii_alphanumeric()
                        && before.is_some_and(|c| c.is_ascii_alphanumeric())
                        || last.is_ascii_alphanumeric()
                            && after.is_some_and(|c| c.is_ascii_alphanumeric()))
                })
            })
    }
}

#[derive(Clone, Deserialize)]
pub struct Part {
    pub text: String,
    #[serde(default = "one")]
    pub repeat: usize,
}
fn one() -> usize {
    1
}

#[derive(Clone, Deserialize)]
pub struct InputCase {
    pub id: String,
    pub language: String,
    pub title: String,
    pub source: String,
    pub url: String,
    pub format: String,
    pub parts: Vec<Part>,
    pub must_keep_facts: Vec<Fact>,
    #[serde(default)]
    pub link_stub: bool,
    #[serde(default)]
    pub near_budget_tier: Option<String>,
}

pub struct PreparedInput {
    pub blocks: String,
    pub html: Option<String>,
    pub full_text: String,
}

impl InputCase {
    pub fn prepare(&self) -> Result<PreparedInput> {
        let content: String = self.parts.iter().map(|p| p.text.repeat(p.repeat)).collect();
        let doc = match self.format.as_str() {
            "markdown" => rrss_render::markdown_to_document(&content),
            "html" => rrss_render::html_to_document(&content, &rrss_render::RenderOpts::default()),
            format => bail!("unsupported corpus format {format}"),
        };
        Ok(PreparedInput {
            blocks: rrss_render::document_to_json(&doc)?,
            html: (self.format == "html").then_some(content),
            full_text: rrss_render::document_to_plain(&doc),
        })
    }
}

#[derive(Deserialize)]
pub struct Corpus {
    pub version: u32,
    pub articles: Vec<InputCase>,
}

impl Corpus {
    pub fn load(root: &Path) -> Result<Self> {
        let corpus: Self = serde_json::from_str(&std::fs::read_to_string(
            root.join("testdata/eval/input-cases.json"),
        )?)?;
        anyhow::ensure!(corpus.version == 1, "unsupported corpus version");
        let mut ids = BTreeSet::new();
        for case in &corpus.articles {
            anyhow::ensure!(
                !case.source.trim().is_empty(),
                "missing fictional corpus source"
            );
            anyhow::ensure!(ids.insert(&case.id), "duplicate input id {}", case.id);
            let url = url::Url::parse(&case.url)?;
            anyhow::ensure!(
                url.host_str()
                    .is_some_and(|host| host.ends_with(".example")),
                "fictional corpus URLs must use .example"
            );
            anyhow::ensure!(
                ["zh", "en"].contains(&case.language.as_str()),
                "unsupported corpus language"
            );
            let prepared = case.prepare()?;
            for fact in &case.must_keep_facts {
                anyhow::ensure!(
                    !fact.text.trim().is_empty() && fact.hit(&prepared.full_text),
                    "{} fact absent from rendered input: {}",
                    case.id,
                    fact.text
                );
            }
            if let Some(tier) = &case.near_budget_tier {
                let budget = InputTier::parse(tier)
                    .context("invalid near-budget tier")?
                    .summary_budget(false);
                let utilization = chars(&prepared.full_text) as f64 / budget as f64;
                anyhow::ensure!(
                    (1.05..=1.15).contains(&utilization),
                    "{} must be 5–15% over its target budget, got {utilization}",
                    case.id
                );
            }
        }
        Ok(corpus)
    }
}

pub fn fact_counts(facts: &[Fact], text: &str) -> (usize, usize) {
    (facts.iter().filter(|f| f.hit(text)).count(), facts.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fact_hits_accept_fixed_translations_without_empty_matches() {
        let f = Fact {
            text: "142 ms".into(),
            category: Category::Body,
            alternatives: vec!["142 毫秒".into()],
        };
        assert!(f.hit("Latency: 142  ms."));
        assert!(f.hit("142 毫秒"));
        assert!(!f.hit("143 ms"));
        assert!(!f.hit("1142 ms"));
        assert!(!f.hit("142 msuffix"));
        let empty = Fact {
            text: " ".into(),
            category: Category::Body,
            alternatives: vec![],
        };
        assert!(!empty.hit("anything"));
    }

    #[test]
    fn committed_corpus_has_rendered_facts_and_budget_boundary_cases() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let corpus = Corpus::load(&root).unwrap();
        assert!(
            corpus
                .articles
                .iter()
                .filter(|c| c.near_budget_tier.is_some())
                .count()
                >= 3
        );
        assert!(corpus.articles.iter().any(|c| c.link_stub));
    }
}
