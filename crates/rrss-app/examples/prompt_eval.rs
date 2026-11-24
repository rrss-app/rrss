//! Quantitative RRSS input/output evaluation on throwaway databases.
//! Offline mode makes zero model calls and needs no API configuration:
//!
//! ```text
//! cargo run -p rrss-app --example prompt_eval -- --offline
//! cargo run -p rrss-app --example prompt_eval -- --offline --tier high --compare baseline.json
//! ```
//!
//! Online mode reads RRSS_EVAL_BASE_URL, RRSS_EVAL_MODEL and optional
//! RRSS_EVAL_API_KEY. Existing variants and sections remain available; weekly
//! digests, manual article summaries and cold daily digests add coverage.
//! Use --help for all options. Outputs go to test-results/prompt-eval by default.
//! Fixtures are fictional (.example) or existing testdata/feeds/real snapshots.
//! This executable never opens the everyday database or starts background jobs.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use rrss_ai::{CompleteRequest, Protocol};
use rrss_core::ai_store::{self, DigestItem, InputTier, OutputLanguage};
use rrss_core::feed_scope::FeedScope;
use rrss_core::{cluster_summary, db, summary_extract};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Semaphore;

#[path = "prompt_eval/cli.rs"]
mod cli;
#[path = "prompt_eval/compare.rs"]
mod compare;
#[path = "prompt_eval/corpus.rs"]
mod corpus;
#[path = "prompt_eval/fixtures.rs"]
mod fixtures;
#[path = "prompt_eval/metrics.rs"]
mod metrics;
#[path = "prompt_eval/offline.rs"]
mod offline;
#[path = "prompt_eval/online.rs"]
mod online;

use metrics::Metrics;

// ---------- inputs ----------

#[derive(Deserialize)]
struct Cases {
    feeds: Vec<FeedCase>,
    clusters: Vec<ClusterCase>,
    articles: Vec<ArticleCase>,
    summaries: Vec<String>,
    digests: Vec<DigestCase>,
    clusters_to_summarize: Vec<i64>,
    memory: Vec<MemoryCase>,
    #[serde(default)]
    citations: Vec<CitationCase>,
    #[serde(default)]
    weekly_digests: Vec<fixtures::WeeklyCase>,
    #[serde(default)]
    article_summaries: Vec<String>,
    #[serde(default)]
    digests_cold: Vec<DigestCase>,
}

/// A chat question with a citation, asked with the chat assistant's system prompt and no tools.
#[derive(Deserialize, Clone)]
struct CitationCase {
    name: String,
    #[serde(default)]
    article: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    excerpt: String,
    /// Attach the paragraph around the selection, as the app does.
    #[serde(default)]
    paragraph: bool,
    question: String,
    /// `zh-CN`, `en`, or `fallback` (the variant's default reply language).
    expect: String,
}

#[derive(Deserialize)]
struct FeedCase {
    key: String,
    title: String,
    url: String,
}

#[derive(Deserialize)]
struct ClusterCase {
    id: i64,
    label: String,
}

#[derive(Deserialize)]
struct ArticleCase {
    key: String,
    feed: String,
    #[serde(default)]
    cluster: Option<i64>,
    hours_ago: i64,
    title: String,
    html: String,
}

#[derive(Deserialize, Clone)]
struct DigestCase {
    name: String,
    pick_max: usize,
    pick_min: usize,
    articles: Vec<String>,
    #[serde(default)]
    high_value_events: Vec<Vec<String>>,
    #[serde(default)]
    min_high_value_events: usize,
    #[serde(default)]
    avoid_articles: Vec<String>,
}

#[derive(Deserialize, Clone)]
struct MemoryPrevTurn {
    #[serde(default)]
    user: Option<String>,
    assistant: String,
}

#[derive(Deserialize, Clone)]
struct MemoryExistingFixture {
    id: i64,
    kind: String,
    content: String,
    #[serde(default)]
    pending: bool,
}

#[derive(Deserialize, Clone)]
struct MemoryCase {
    name: String,
    user_message: String,
    #[serde(default)]
    previous_turn: Option<MemoryPrevTurn>,
    #[serde(default)]
    existing_memories: Vec<MemoryExistingFixture>,
    #[serde(default)]
    pending_count: usize,
    #[serde(default = "default_true")]
    expect_trigger: bool,
    #[serde(default)]
    expect_empty: bool,
    #[serde(default)]
    expect_action: Option<String>,
    #[serde(default)]
    expect_replaces_id: Option<i64>,
    #[serde(default)]
    expect_kind: Option<String>,
    #[serde(default)]
    expect_lang: Option<String>,
    #[serde(default)]
    expect_not_contains: Vec<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
struct Baseline {
    templates: HashMap<String, String>,
    input: BaselineInput,
    #[serde(default)]
    pi_system: String,
    #[serde(default)]
    citation: Option<BaselineCitation>,
}

/// The citation format the baseline chat prompt was written for.
#[derive(Deserialize)]
struct BaselineCitation {
    no_question: String,
    header: String,
    untitled: String,
    line: String,
    whole: String,
    digest_label: String,
    selected: String,
    excerpt: String,
    paragraph: String,
}

#[derive(Deserialize)]
struct BaselineInput {
    no_body: String,
    labels: Vec<(String, String)>,
    pick_header: String,
    issue_header: String,
    cluster_header: String,
    event_with_count: String,
    event: String,
    topic: String,
}

// ---------- variants ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
enum Variant {
    #[serde(rename = "baseline")]
    Baseline,
    #[serde(rename = "en-zh")]
    EnZh,
    #[serde(rename = "en-en")]
    EnEn,
}

impl Variant {
    const ALL: [Variant; 3] = [Variant::Baseline, Variant::EnZh, Variant::EnEn];

    fn id(self) -> &'static str {
        match self {
            Variant::Baseline => "baseline",
            Variant::EnZh => "en-zh",
            Variant::EnEn => "en-en",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Variant::Baseline => "Baseline → Chinese",
            Variant::EnZh => "English prompts → Chinese",
            Variant::EnEn => "English prompts → English",
        }
    }

    fn language(self) -> OutputLanguage {
        match self {
            Variant::EnEn => OutputLanguage::En,
            _ => OutputLanguage::ZhCn,
        }
    }
}

struct Prompts {
    variant: Variant,
    english: HashMap<String, String>,
    baseline: Arc<Baseline>,
}

impl Prompts {
    /// System prompt for a template slug; English prompts get the language instruction when the
    /// app would add one for that task.
    fn system(&self, slug: &str, task: &str) -> Result<String> {
        if self.variant == Variant::Baseline {
            return self
                .baseline
                .templates
                .get(slug)
                .cloned()
                .with_context(|| format!("baseline has no template {slug}"));
        }
        let body = self
            .english
            .get(slug)
            .with_context(|| format!("no template {slug}"))?;
        Ok(if rrss_app::ai_language::applies(task) {
            rrss_app::ai_language::with_directive(body, self.variant.language())
        } else {
            body.clone()
        })
    }

    fn user(&self, text: &str) -> String {
        if self.variant == Variant::Baseline {
            to_baseline_input(text, &self.baseline.input)
        } else {
            text.to_string()
        }
    }
}

fn numbers(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in line.chars() {
        if ch.is_ascii_digit() {
            current.push(ch);
        } else if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn fill(template: &str, values: &[&str]) -> String {
    values
        .iter()
        .enumerate()
        .fold(template.to_string(), |acc, (i, value)| {
            acc.replace(&format!("{{{i}}}"), value)
        })
}

/// The English input markers mapped back to the Chinese ones the baseline prompts were written for.
fn to_baseline_input(text: &str, map: &BaselineInput) -> String {
    let mut text = text.replace(summary_extract::LINK_STUB_NOTE, &map.no_body);
    for (from, to) in &map.labels {
        text = text.replace(from, to);
    }
    text.lines()
        .map(|line| {
            let n = numbers(line);
            let n: Vec<&str> = n.iter().map(String::as_str).collect();
            if line.starts_with("This issue: ") && n.len() >= 2 {
                fill(&map.issue_header, &n[..2])
            } else if line.contains(" candidates; pick ") && n.len() >= 2 {
                fill(&map.pick_header, &n[..2])
            } else if line.contains(" reports of the same event (from ") && n.len() >= 2 {
                fill(&map.cluster_header, &n[..2])
            } else if let Some(rest) = line.strip_prefix("【Event ") {
                let (number, tail) = rest.split_once('】').unwrap_or((rest, ""));
                if tail.ends_with(" articles") {
                    fill(
                        &map.event_with_count,
                        &[number, tail.trim_end_matches(" articles")],
                    )
                } else {
                    fill(&map.event, &[number])
                }
            } else if let (Some(rest), true) =
                (line.strip_prefix("Topic: "), line.ends_with(" articles)"))
            {
                let (title, count) = rest.rsplit_once(" (").unwrap_or((rest, ""));
                fill(&map.topic, &[title, count.trim_end_matches(" articles)")])
            } else if line.starts_with("[id:") {
                line.replacen(" | ", "｜", 1)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------- model calls ----------

struct Model {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    model: String,
    gate: Semaphore,
    /// Per variant: prompt tokens, completion tokens, requests.
    usage: std::sync::Mutex<BTreeMap<&'static str, (u64, u64, usize)>>,
}

#[derive(Debug, Clone, Serialize, Default)]
struct Call {
    system: String,
    user: String,
    content: String,
    #[serde(skip)]
    parsed: Value,
    error: Option<String>,
    prompt_tokens: u32,
    completion_tokens: u32,
    ms: u128,
    requests: usize,
    /// Article ids the chat tools returned, which the reply may link to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_ids: Vec<i64>,
}

impl Model {
    /// A plain-text chat completion with the chat assistant's read-only tools, run against the eval
    /// database for up to four rounds, as the assistant would in the app.
    async fn chat(
        &self,
        tag: &'static str,
        system: String,
        user: String,
        conn: &rusqlite::Connection,
    ) -> Call {
        let _permit = self.gate.acquire().await.expect("semaphore");
        let started = Instant::now();
        let base = self.base_url.trim_end_matches('/');
        let url = if base.ends_with("/v1") {
            format!("{base}/chat/completions")
        } else {
            format!("{base}/v1/chat/completions")
        };
        let mut messages = vec![
            json!({"role": "system", "content": system}),
            json!({"role": "user", "content": user}),
        ];
        let mut call = Call {
            system,
            user,
            ..Call::default()
        };
        let mut used = Vec::new();
        for round in 0..4 {
            call.requests += 1;
            let mut body = json!({ "model": self.model, "messages": messages });
            // The last round withholds tools so the model has to answer.
            if round < 3 {
                body["tools"] = chat_tools();
            }
            let mut request = self.client.post(&url).json(&body);
            if let Some(key) = &self.api_key {
                request = request.bearer_auth(key);
            }
            let response = async {
                let response = request.send().await?.error_for_status()?;
                response.json::<Value>().await
            }
            .await;
            let value = match response {
                Ok(value) => value,
                Err(error) => {
                    call.error = Some(rrss_core::logsafe::scrub_secrets_with(
                        &error.without_url().to_string(),
                        self.api_key.as_deref(),
                    ));
                    break;
                }
            };
            call.prompt_tokens += value["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32;
            call.completion_tokens +=
                value["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32;
            let message = value["choices"][0]["message"].clone();
            let calls = message["tool_calls"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if calls.is_empty() {
                let text = message["content"].as_str().unwrap_or("");
                call.content = rrss_ai::strip_thinking(text).trim().to_string();
                if call.content.is_empty() {
                    call.error = Some("empty reply".into());
                }
                break;
            }
            messages.push(json!({"role": "assistant", "content": message["content"].clone(), "tool_calls": calls}));
            for tool_call in &calls {
                let name = tool_call["function"]["name"].as_str().unwrap_or("");
                let arguments: Value = serde_json::from_str(
                    tool_call["function"]["arguments"].as_str().unwrap_or("{}"),
                )
                .unwrap_or(json!({}));
                used.push(format!("{name}({arguments})"));
                let mut args = vec!["rrss".to_string(), "tool".into(), name.to_string()];
                if let Some(object) = arguments.as_object() {
                    for (key, value) in object {
                        args.push(format!("--{key}"));
                        args.push(
                            value
                                .as_str()
                                .map_or_else(|| value.to_string(), str::to_string),
                        );
                    }
                }
                let output = rrss_app::tool::run_on(conn, &args)
                    .unwrap_or_else(|error| format!("error: {error}"));
                if let Ok(parsed) = serde_json::from_str::<Value>(&output) {
                    let rows = parsed
                        .as_array()
                        .cloned()
                        .unwrap_or_else(|| vec![parsed.clone()]);
                    call.tool_ids
                        .extend(rows.iter().filter_map(|row| row["id"].as_i64()));
                }
                messages.push(json!({"role": "tool", "tool_call_id": tool_call["id"].clone(), "content": output}));
            }
        }
        if !used.is_empty() {
            call.system
                .push_str(&format!("\n\nTOOLS CALLED\n{}", used.join("\n")));
        }
        call.ms = started.elapsed().as_millis();
        let mut usage = self.usage.lock().expect("usage");
        let entry = usage.entry(tag).or_default();
        entry.0 += u64::from(call.prompt_tokens);
        entry.1 += u64::from(call.completion_tokens);
        entry.2 += call.requests;
        call
    }

    async fn call(&self, tag: &'static str, system: String, user: String, schema: Value) -> Call {
        let _permit = self.gate.acquire().await.expect("semaphore");
        let started = Instant::now();
        let req = CompleteRequest {
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            model: self.model.clone(),
            system: system.clone(),
            user: user.clone(),
            json_schema: Some(schema),
            protocol: Protocol::Chat,
        };
        let mut call = Call {
            system,
            user,
            requests: 1,
            ..Call::default()
        };
        match rrss_ai::complete(&self.client, &req).await {
            Ok(out) => {
                call.content = out.content;
                call.parsed = out.parsed;
                call.prompt_tokens = out.prompt_tokens;
                call.completion_tokens = out.completion_tokens;
            }
            Err(error) => {
                call.error = Some(rrss_core::logsafe::scrub_secrets_with(
                    &error.to_string(),
                    self.api_key.as_deref(),
                ))
            }
        }
        call.ms = started.elapsed().as_millis();
        let mut usage = self.usage.lock().expect("usage");
        let entry = usage.entry(tag).or_default();
        entry.0 += u64::from(call.prompt_tokens);
        entry.1 += u64::from(call.completion_tokens);
        entry.2 += call.requests;
        call
    }
}

// ---------- results ----------

#[derive(Debug, Clone, Serialize)]
struct Check {
    name: &'static str,
    ok: bool,
    note: String,
}

fn check(name: &'static str, ok: bool, note: impl Into<String>) -> Check {
    Check {
        name,
        ok,
        note: note.into(),
    }
}

#[derive(Debug, Clone, Serialize)]
struct Output {
    /// What a reader would see, as plain text or Markdown.
    shown: String,
    checks: Vec<Check>,
    calls: Vec<Call>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    metrics: Metrics,
}

#[derive(Debug, Clone, Serialize)]
struct Case {
    section: &'static str,
    key: String,
    title: String,
    input: String,
    outputs: BTreeMap<&'static str, Output>,
}

// ---------- automatic checks ----------

/// Share of Han characters among letters.
fn han_share(text: &str) -> f64 {
    let mut han = 0usize;
    let mut letters = 0usize;
    for ch in text.chars() {
        if ('\u{4e00}'..='\u{9fff}').contains(&ch) || ('\u{3400}'..='\u{4dbf}').contains(&ch) {
            han += 1;
            letters += 1;
        } else if ch.is_alphabetic() {
            letters += 1;
        }
    }
    if letters == 0 {
        0.0
    } else {
        han as f64 / letters as f64
    }
}

fn language_check(text: &str, language: OutputLanguage) -> Check {
    let share = han_share(text);
    let ok = match language {
        // Chinese prose keeps product and company names in Latin letters.
        OutputLanguage::ZhCn => share >= 0.3,
        OutputLanguage::En => share <= 0.05,
    };
    check(
        "language",
        ok || text.trim().is_empty(),
        format!("Han share {:.0}%", share * 100.0),
    )
}

const LEAKS: &[&str] = &[
    "Output only JSON",
    "只输出 JSON",
    "Output language",
    "〈",
    "【Event",
    "article_id",
    "[id:",
    "[No body text]",
    "【无正文】",
];

fn leak_check(text: &str) -> Check {
    let found: Vec<&str> = LEAKS
        .iter()
        .copied()
        .filter(|leak| text.contains(leak))
        .collect();
    check("no prompt leak", found.is_empty(), found.join(", "))
}

/// Sentence count: a full stop ends a sentence only when followed by whitespace or the end, so
/// version numbers such as 2.0 do not count.
fn sentences(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut count = 0;
    let mut length = 0;
    for (i, ch) in chars.iter().enumerate() {
        let next = chars.get(i + 1);
        let end = matches!(ch, '。' | '！' | '？')
            || (matches!(ch, '.' | '!' | '?') && next.is_none_or(|c| c.is_whitespace()));
        if end {
            if length > 3 {
                count += 1;
            }
            length = 0;
        } else if !ch.is_whitespace() {
            length += 1;
        }
    }
    count + usize::from(length > 3)
}

fn call_failed(call: &Call) -> Option<Output> {
    call.error.as_ref().map(|error| Output {
        shown: String::new(),
        checks: vec![check("request", false, error.clone())],
        calls: vec![call.clone()],
        metrics: Metrics::new(),
    })
}

fn summary_output(call: Call, language: OutputLanguage) -> Output {
    if let Some(failed) = call_failed(&call) {
        return failed;
    }
    let p = &call.parsed;
    let text = |key: &str| {
        p.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let list = |key: &str| -> Vec<String> {
        p.get(key)
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };
    let (summary, one_liner, points, facts) = (
        text("summary"),
        text("one_liner"),
        list("key_points"),
        list("facts"),
    );
    let mut shown = format!("{summary}\n\nOne-liner: {one_liner}");
    for point in &points {
        shown.push_str(&format!("\n• {point}"));
    }
    if !facts.is_empty() {
        shown.push_str("\n\nFacts:");
        for fact in &facts {
            shown.push_str(&format!("\n- {fact}"));
        }
    }
    let all = [
        summary.clone(),
        one_liner.clone(),
        points.join(" "),
        facts.join(" "),
    ]
    .join(" ");
    let n = sentences(&summary);
    Output {
        checks: vec![
            check(
                "fields",
                !summary.is_empty()
                    && !one_liner.is_empty()
                    && p.get("key_points").is_some()
                    && p.get("facts").is_some(),
                "",
            ),
            language_check(&all, language),
            check(
                "length",
                (1..=5).contains(&n),
                format!(
                    "{n} sentences, {} points, {} facts",
                    points.len(),
                    facts.len()
                ),
            ),
            check("limits", points.len() <= 3 && facts.len() <= 5, ""),
            leak_check(&all),
        ],
        shown,
        calls: vec![call],
        metrics: Metrics::new(),
    }
}

// ---------- seeding ----------

struct Seeded {
    conn: db::PooledConnection,
    _dir: tempfile::TempDir,
    ids: HashMap<String, i64>,
}

fn seed(root: &Path, cases: &Cases) -> Result<Seeded> {
    let dir = tempfile::tempdir()?;
    let conn = db::open_at(&dir.path().join("eval.db"))?;
    for key in [
        ai_store::FOCUS_AUTO_SUMMARY.id,
        ai_store::CLUSTER_AUTO_SUMMARY.id,
    ] {
        db::set_setting(&conn, key, "0")?;
    }
    let now = chrono::Utc::now().timestamp_millis();
    let mut feeds = HashMap::new();
    for feed in &cases.feeds {
        conn.execute(
            "INSERT INTO feeds (url, title, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![feed.url, feed.title, now],
        )?;
        feeds.insert(feed.key.clone(), conn.last_insert_rowid());
    }
    for cluster in &cases.clusters {
        conn.execute(
            "INSERT INTO clusters (id, label, member_count, created_at) VALUES (?1, ?2, 0, ?3)",
            rusqlite::params![cluster.id, cluster.label, now],
        )?;
    }
    let mut ids = HashMap::new();
    for article in &cases.articles {
        let feed = *feeds
            .get(&article.feed)
            .with_context(|| format!("unknown feed {}", article.feed))?;
        let at = now - article.hours_ago * 3_600_000;
        conn.execute(
            "INSERT INTO articles (feed_id, title, hash, created_at, published_at, score, cluster_id, content_html, canonical_url)
             VALUES (?1, ?2, ?3, ?4, ?4, 5, ?5, NULLIF(?6, ''), ?7)",
            rusqlite::params![
                feed,
                article.title,
                format!("eval-{}", article.key),
                at,
                article.cluster,
                article.html,
                format!("https://{}.example/{}", article.feed, article.key)
            ],
        )?;
        ids.insert(article.key.clone(), conn.last_insert_rowid());
    }
    conn.execute(
        "UPDATE clusters SET member_count = (SELECT COUNT(*) FROM articles a WHERE a.cluster_id = clusters.id)",
        [],
    )?;
    // Real snapshots: only the files the cases use.
    let mut files = BTreeSet::new();
    for key in cases
        .summaries
        .iter()
        .chain(cases.digests.iter().flat_map(|d| d.articles.iter()))
    {
        if let Some((file, _)) = key
            .strip_prefix("real:")
            .and_then(|rest| rest.split_once('#'))
        {
            files.insert(file.to_string());
        }
    }
    for file in files {
        let path = root.join("testdata/feeds/real").join(&file);
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let parsed = rrss_core::parse_feed_bytes(&bytes, None)?;
        conn.execute(
            "INSERT INTO feeds (url, title, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                format!("https://snapshot.example/{file}"),
                parsed.title.clone().unwrap_or_else(|| file.clone()),
                now
            ],
        )?;
        let feed = conn.last_insert_rowid();
        rrss_core::ingest::upsert_entries(&conn, feed, &parsed.entries, now)?;
        for (index, entry) in parsed.entries.iter().enumerate() {
            let id: Option<i64> = conn
                .query_row(
                    "SELECT id FROM articles WHERE feed_id = ?1 AND hash = ?2",
                    rusqlite::params![feed, entry.hash],
                    |row| row.get(0),
                )
                .ok();
            if let Some(id) = id {
                ids.insert(format!("real:{file}#{index}"), id);
            }
        }
    }
    Ok(Seeded {
        _dir: dir,
        conn,
        ids,
    })
}

fn english_templates(conn: &rusqlite::Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT slug, body FROM prompt_templates")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---------- pipeline ----------

#[derive(Clone)]
struct Options {
    sections: BTreeSet<String>,
    tier: InputTier,
}

async fn run_variant(
    variant: Variant,
    root: PathBuf,
    cases: Arc<Cases>,
    baseline: Arc<Baseline>,
    model: Arc<Model>,
    options: Options,
) -> Result<Vec<Case>> {
    let seeded = seed(&root, &cases)?;
    let prompts = Prompts {
        variant,
        english: english_templates(&seeded.conn)?,
        baseline,
    };
    let language = variant.language();
    let id = |key: &str| {
        seeded
            .ids
            .get(key)
            .copied()
            .with_context(|| format!("no article for {key}"))
    };
    let mut out = Vec::new();
    // Isolate cold digests even when the default run also generates warm summaries.
    if options.sections.contains("digests_cold") {
        let cold = seed(&root, &cases)?;
        let issues = if cases.digests_cold.is_empty() {
            &cases.digests
        } else {
            &cases.digests_cold
        };
        for digest in issues {
            let ids = digest
                .articles
                .iter()
                .map(|key| {
                    cold.ids
                        .get(key)
                        .copied()
                        .with_context(|| format!("no article for {key}"))
                })
                .collect::<Result<Vec<_>>>()?;
            let items = ai_store::digest_items_by_ids(&cold.conn, &ids)?;
            let mut row = run_digest(
                &prompts,
                &model,
                &cold.conn,
                digest,
                &items,
                language,
                options.tier,
                "digests_cold",
                &cold.ids,
            )
            .await?;
            let output = row.outputs.get_mut(variant.id()).unwrap();
            online::pipeline_cost(output, &[]);
            let summaries: i64 = cold.conn.query_row(
                "SELECT COUNT(*) FROM articles WHERE COALESCE(summary,'') != ''",
                [],
                |r| r.get(0),
            )?;
            output.checks.push(check(
                "cold pool",
                summaries == 0,
                "automatic summaries disabled; no prerequisite summaries",
            ));
            out.push(row);
        }
    }
    eprintln!("[{}] summaries", variant.id());

    // Summaries: every article a later stage needs, so the digest and overviews read this
    // variant's own summaries, as they would in the app.
    let mut keys: Vec<String> = if options.sections.contains("summaries") {
        cases.summaries.clone()
    } else {
        Vec::new()
    };
    if options.sections.contains("digests") || options.sections.contains("clusters") {
        for key in cases.digests.iter().flat_map(|d| d.articles.iter()) {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
        for article in &cases.articles {
            if article.cluster.is_some() && !keys.contains(&article.key) {
                keys.push(article.key.clone());
            }
        }
    }
    let system = prompts.system(ai_store::SLUG_FOCUS_SUMMARY, ai_store::TASK_FOCUS_SUMMARY)?;
    let mut jobs = Vec::new();
    for key in &keys {
        let (title, blocks, html): (String, Option<String>, Option<String>) =
            seeded.conn.query_row(
                "SELECT title, blocks_json, content_html FROM articles WHERE id = ?1",
                [id(key)?],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
        let input = ai_store::summary_payload(
            &title,
            blocks.as_deref(),
            html.as_deref(),
            false,
            options.tier,
        );
        jobs.push((key.clone(), title, input));
    }
    let mut handles = Vec::new();
    for (_, _, input) in &jobs {
        let (model, system, user) = (model.clone(), system.clone(), prompts.user(input));
        let tag = variant.id();
        handles.push(tokio::spawn(async move {
            model
                .call(tag, system, user, rrss_ai::summary_json_schema())
                .await
        }));
    }
    let mut calls = Vec::new();
    for handle in handles {
        calls.push(handle.await?);
    }
    let mut summary_calls = HashMap::new();
    for ((key, title, input), call) in jobs.into_iter().zip(calls) {
        summary_calls.insert(key.clone(), call.clone());
        if call.error.is_none()
            && let Some(summary) = call.parsed.get("summary").and_then(Value::as_str)
        {
            let raw = serde_json::to_string(&call.parsed)?;
            rrss_core::articles::set_summary_and_json(
                &seeded.conn,
                id(&key)?,
                summary.trim(),
                Some(&raw),
            )?;
        }
        if options.sections.contains("summaries") && cases.summaries.contains(&key) {
            out.push(Case {
                section: "summaries",
                key,
                title,
                input,
                outputs: BTreeMap::from([(variant.id(), summary_output(call, language))]),
            });
        }
    }

    if options.sections.contains("digests") {
        for digest in &cases.digests {
            eprintln!("[{}] digest: {}", variant.id(), digest.name);
            let ids: Vec<i64> = digest
                .articles
                .iter()
                .map(|key| id(key))
                .collect::<Result<_>>()?;
            let items = ai_store::digest_items_by_ids(&seeded.conn, &ids)?;
            let mut row = run_digest(
                &prompts,
                &model,
                &seeded.conn,
                digest,
                &items,
                language,
                options.tier,
                "digests",
                &seeded.ids,
            )
            .await?;
            let prerequisites = digest
                .articles
                .iter()
                .filter_map(|key| summary_calls.get(key))
                .cloned()
                .collect::<Vec<_>>();
            online::pipeline_cost(row.outputs.get_mut(variant.id()).unwrap(), &prerequisites);
            out.push(row);
        }
    }

    if options.sections.contains("weekly_digests") {
        out.extend(
            online::weekly_digests(&prompts, &model, &cases.weekly_digests, options.tier).await?,
        );
    }
    if options.sections.contains("article_summaries") {
        let corpus = corpus::Corpus::load(&root)?;
        out.extend(
            online::article_summaries(
                &prompts,
                &model,
                &corpus,
                &cases.article_summaries,
                options.tier,
            )
            .await?,
        );
    }
    if options.sections.contains("clusters") {
        let system = prompts.system(
            ai_store::SLUG_CLUSTER_SUMMARY,
            ai_store::TASK_CLUSTER_SUMMARY,
        )?;
        for cluster in &cases.clusters_to_summarize {
            eprintln!("[{}] related coverage {cluster}", variant.id());
            let members = cluster_summary::visible_members(&seeded.conn, *cluster, FeedScope::All)?;
            let input = cluster_summary::build_input(&seeded.conn, &members, options.tier)?;
            let call = model
                .call(
                    variant.id(),
                    system.clone(),
                    prompts.user(&input.prompt),
                    cluster_summary::json_schema(),
                )
                .await;
            let label = cases
                .clusters
                .iter()
                .find(|c| c.id == *cluster)
                .map_or(String::new(), |c| c.label.clone());
            out.push(Case {
                section: "clusters",
                key: format!("cluster-{cluster}"),
                title: label,
                input: input.prompt.clone(),
                outputs: BTreeMap::from([(
                    variant.id(),
                    cluster_output(call, &input.members, language),
                )]),
            });
        }
    }

    if options.sections.contains("memory") {
        let system = prompts.system(
            ai_store::SLUG_MEMORY_CANDIDATE,
            ai_store::TASK_MEMORY_CANDIDATE,
        )?;
        let mut handles = Vec::new();
        for memory in &cases.memory {
            let memory = memory.clone();
            let clean = rrss_core::memories::clean_user_message(&memory.user_message);
            let triggered =
                rrss_core::memories::should_extract_memory(&clean, memory.pending_count);
            let existing: Vec<rrss_core::memories::ExistingMemorySummary> = memory
                .existing_memories
                .iter()
                .map(|m| rrss_core::memories::ExistingMemorySummary {
                    id: m.id,
                    kind: if m.kind == "procedural" {
                        "procedural"
                    } else {
                        "semantic"
                    },
                    content: m.content.clone(),
                    pending: m.pending,
                })
                .collect();
            let prev = memory
                .previous_turn
                .as_ref()
                .map(|p| rrss_core::memories::PreviousTurn {
                    user: p.user.clone(),
                    assistant: p.assistant.clone(),
                });
            let payload = serde_json::to_string(&rrss_core::memories::ExtractionInput {
                user_message: &clean,
                previous_turn: prev.as_ref(),
                existing_memories: &existing,
            })?;
            let model = model.clone();
            let system = system.clone();
            let tag = variant.id();
            handles.push(tokio::spawn(async move {
                eprintln!("[{tag}] memory: {}", memory.name);
                let output = if !triggered {
                    Output {
                        checks: vec![check(
                            "gate",
                            !memory.expect_trigger,
                            if memory.expect_trigger {
                                "skipped unexpectedly"
                            } else {
                                "skipped by gate"
                            },
                        )],
                        shown: "(skipped by pre-filter gate)".into(),
                        calls: Vec::new(),
                        metrics: Metrics::new(),
                    }
                } else {
                    let call = model
                        .call(tag, system, payload.clone(), rrss_ai::facts_json_schema())
                        .await;
                    memory_output(call, &memory, &clean, &existing, language)
                };
                Case {
                    section: "memory",
                    key: memory.name.clone(),
                    title: memory.name,
                    input: payload,
                    outputs: BTreeMap::from([(tag, output)]),
                }
            }));
        }
        for handle in handles {
            out.push(handle.await?);
        }
    }

    if options.sections.contains("citations") {
        let system = if variant == Variant::Baseline {
            prompts.baseline.pi_system.clone()
        } else {
            rrss_app::pi::workspace::system_prompt("", language)
        };
        for case in &cases.citations {
            eprintln!("[{}] citation: {}", variant.id(), case.name);
            let (snaps, passage) = citation_input(&seeded.conn, case, &id)?;
            let user = match (&prompts.baseline.citation, variant) {
                (Some(map), Variant::Baseline) => {
                    baseline_citation(map, &case.question, &snaps, passage.as_deref())
                }
                _ => {
                    rrss_core::cite::expand_user_prompt(&case.question, &snaps, |_| passage.clone())
                }
            };
            let call = model
                .chat(variant.id(), system.clone(), user.clone(), &seeded.conn)
                .await;
            let expect = match case.expect.as_str() {
                "fallback" => language,
                tag => OutputLanguage::from_tag(tag),
            };
            let cited: Vec<i64> = snaps.iter().map(|s| s.article_id).collect();
            out.push(Case {
                section: "citations",
                key: case.name.clone(),
                title: case.name.clone(),
                input: user,
                outputs: BTreeMap::from([(variant.id(), citation_output(call, expect, &cited))]),
            });
        }
    }
    Ok(out)
}

/// The chat assistant's read-only tools, from the same definitions the app gives Pi.
fn chat_tools() -> Value {
    let tools: Vec<Value> = rrss_core::pi_store::DEFAULT_TOOLS
        .iter()
        .filter(|tool| {
            matches!(
                tool.name,
                "search_articles" | "get_article" | "search_memory"
            )
        })
        .map(|tool| {
            let properties: serde_json::Map<String, Value> = tool
                .parameters
                .iter()
                .map(|(name, kind, description)| {
                    (
                        (*name).to_string(),
                        json!({"type": kind, "description": description}),
                    )
                })
                .collect();
            let required: Vec<&str> = tool.parameters.iter().map(|(name, _, _)| *name).collect();
            json!({"type": "function", "function": {
                "name": tool.name,
                "description": tool.description,
                "parameters": {"type": "object", "properties": properties, "required": required},
            }})
        })
        .collect();
    Value::Array(tools)
}

/// The citation snapshot the app would attach, and the paragraph around the selection.
fn citation_input(
    conn: &rusqlite::Connection,
    case: &CitationCase,
    id: &impl Fn(&str) -> Result<i64>,
) -> Result<(Vec<rrss_core::CitationSnap>, Option<String>)> {
    let Some(key) = &case.article else {
        return Ok((Vec::new(), None));
    };
    let article_id = id(key)?;
    let (title, feed, html): (String, String, Option<String>) = conn.query_row(
        "SELECT a.title, COALESCE(f.title, ''), a.content_html FROM articles a JOIN feeds f ON f.id = a.feed_id WHERE a.id = ?1",
        [article_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let passage = if case.paragraph {
        html.unwrap_or_default()
            .split("</p>")
            .map(plain_text)
            .find(|paragraph| paragraph.contains(&case.excerpt))
            .and_then(|paragraph| rrss_core::cite::passage_around(&paragraph, &case.excerpt, 600))
    } else {
        None
    };
    let snap = rrss_core::CitationSnap {
        kind: case.kind.clone().unwrap_or_else(|| {
            if case.excerpt.is_empty() {
                "article".into()
            } else {
                "span".into()
            }
        }),
        article_id,
        title,
        feed_title: feed,
        excerpt: case.excerpt.clone(),
        block_id: if case.kind.as_deref() == Some("digest") {
            -1
        } else {
            1
        },
        ..rrss_core::CitationSnap::default()
    };
    Ok((vec![snap], passage))
}

fn plain_text(html: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for ch in html.chars() {
        match ch {
            '<' => tag = true,
            '>' => tag = false,
            _ if !tag => out.push(ch),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// The citation message in the format used before English prompts.
fn baseline_citation(
    map: &BaselineCitation,
    question: &str,
    snaps: &[rrss_core::CitationSnap],
    passage: Option<&str>,
) -> String {
    let question = question.trim();
    if snaps.is_empty() {
        return question.to_string();
    }
    let mut out = if question.is_empty() {
        map.no_question.clone()
    } else {
        question.to_string()
    };
    out.push_str("\n\n");
    out.push_str(&map.header);
    for (i, c) in snaps.iter().enumerate() {
        let title = if c.title.trim().is_empty() {
            map.untitled.as_str()
        } else {
            c.title.trim()
        };
        let source = if c.feed_title.trim().is_empty() {
            String::new()
        } else {
            format!(" — {}", c.feed_title.trim())
        };
        let whole = if c.excerpt.trim().is_empty() {
            map.whole.as_str()
        } else {
            ""
        };
        let n = (i + 1).to_string();
        let id = c.article_id.to_string();
        out.push('\n');
        out.push_str(&fill(&map.line, &[&n, title, &source, &id, whole]));
        if c.excerpt.trim().is_empty() {
            continue;
        }
        let label = if c.kind == "digest" {
            map.digest_label.as_str()
        } else {
            map.selected.as_str()
        };
        out.push('\n');
        out.push_str(&fill(&map.excerpt, &[label, c.excerpt.trim()]));
        if let Some(paragraph) = passage.filter(|p| !p.trim().is_empty()) {
            out.push('\n');
            out.push_str(&fill(&map.paragraph, &[paragraph.trim()]));
        }
    }
    out
}

/// Phrases that point out what the article leaves out, which the chat prompt asks the model not to list.
const LEAVES_OUT: &[&str] = &[
    "文章没有",
    "文中没有",
    "没有提到",
    "未提及",
    "没有说明",
    "doesn't say",
    "does not say",
    "doesn't mention",
    "does not mention",
    "not mentioned",
];
/// Openings that restate the question or the passage instead of answering it.
/// Tool calls written out as text instead of made through the API.
const FAKE_TOOL_CALLS: &[&str] = &[
    "<tool_call",
    "<tool_use",
    "get_article(",
    "\"name\": \"get_article\"",
];
const RESTATES: &[&str] = &[
    "这段",
    "这个片段",
    "这段片段",
    "你问的",
    "您问的",
    "The passage",
    "This passage",
    "You asked",
    "In this passage",
];

fn citation_output(call: Call, expect: OutputLanguage, cited: &[i64]) -> Output {
    if let Some(failed) = call_failed(&call) {
        return failed;
    }
    let text = call.content.clone();
    let headings = text
        .lines()
        .filter(|line| line.trim_start().starts_with('#'))
        .count();
    let links: Vec<i64> = text
        .match_indices("rrss://article/")
        .filter_map(|(at, m)| {
            text[at + m.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        })
        .collect();
    let invented: Vec<String> = links
        .iter()
        .filter(|id| !cited.contains(id) && !call.tool_ids.contains(id))
        .map(|id| id.to_string())
        .collect();
    let leaves_out: Vec<&str> = LEAVES_OUT
        .iter()
        .copied()
        .filter(|p| text.contains(p))
        .collect();
    let opening = text.chars().take(20).collect::<String>();
    let restates: Vec<&str> = RESTATES
        .iter()
        .copied()
        .filter(|p| opening.starts_with(p))
        .collect();
    let length = match expect {
        OutputLanguage::ZhCn => format!(
            "{} characters",
            text.chars().filter(|c| !c.is_whitespace()).count()
        ),
        OutputLanguage::En => format!("{} words", text.split_whitespace().count()),
    };
    Output {
        checks: vec![
            language_check(&text, expect),
            check("no headings", headings == 0, length),
            check("answers first", restates.is_empty(), restates.join(", ")),
            check(
                "no missing-info remarks",
                leaves_out.is_empty(),
                leaves_out.join(", "),
            ),
            check("only known IDs", invented.is_empty(), invented.join(", ")),
            check(
                "no tool calls in text",
                !FAKE_TOOL_CALLS.iter().any(|p| text.contains(p)),
                "",
            ),
            leak_check(&text),
        ],
        shown: text,
        calls: vec![call],
        metrics: Metrics::new(),
    }
}

async fn run_digest(
    prompts: &Prompts,
    model: &Model,
    conn: &rusqlite::Connection,
    digest: &DigestCase,
    items: &[DigestItem],
    language: OutputLanguage,
    tier: InputTier,
    section: &'static str,
    ids: &HashMap<String, i64>,
) -> Result<Case> {
    let started = Instant::now();
    let noisy = rrss_core::feeds::noisy_feed_titles(conn)?;
    let shallow = ai_store::format_digest_input(items, &noisy, digest.pick_max);
    let pick = model
        .call(
            prompts.variant.id(),
            prompts.system(
                ai_store::SLUG_DAILY_DIGEST_PICK,
                ai_store::TASK_DIGEST_PICK_DAILY,
            )?,
            prompts.user(&shallow),
            rrss_ai::digest_pick_json_schema(),
        )
        .await;
    let picks = if pick.error.is_none() {
        ai_store::parse_digest_picks(&pick.parsed)
    } else {
        Vec::new()
    };
    let selected = ai_store::apply_digest_picks(items, &picks, digest.pick_max, digest.pick_min);
    let deep = ai_store::format_digest_deep_input(conn, &selected, tier)?;
    let write = model
        .call(
            prompts.variant.id(),
            prompts.system(ai_store::SLUG_DAILY_DIGEST, ai_store::TASK_DIGEST_DAILY)?,
            prompts.user(&deep.prompt),
            rrss_ai::digest_json_schema(),
        )
        .await;
    let input = format!(
        "Pick input:\n\n{shallow}\n\n— — —\n\nWrite input:\n\n{}",
        deep.prompt
    );
    let output = online::daily_output(
        pick,
        write,
        &deep,
        items,
        &selected,
        digest,
        ids,
        conn,
        language,
        started.elapsed().as_millis(),
    )?;
    Ok(Case {
        section,
        key: digest.name.clone(),
        title: digest.name.clone(),
        input,
        outputs: BTreeMap::from([(prompts.variant.id(), output)]),
    })
}

fn cluster_output(
    call: Call,
    members: &[cluster_summary::Member],
    language: OutputLanguage,
) -> Output {
    if let Some(failed) = call_failed(&call) {
        return failed;
    }
    match cluster_summary::parse_output(&call.parsed, members) {
        Some(parsed) => {
            let title = |id: i64| {
                members
                    .iter()
                    .find(|m| m.article_id == id)
                    .map_or(String::new(), |m| m.feed_title.clone())
            };
            let mut shown = parsed.overview.clone();
            for addition in &parsed.additions {
                shown.push_str(&format!(
                    "\n\n+ [{}] {}",
                    title(addition.article_id),
                    addition.text
                ));
            }
            for conflict in &parsed.conflicts {
                shown.push_str(&format!("\n\n≠ {}", conflict.text));
            }
            let text = [
                parsed.overview.clone(),
                parsed
                    .additions
                    .iter()
                    .map(|a| a.text.clone())
                    .collect::<Vec<_>>()
                    .join(" "),
                parsed
                    .conflicts
                    .iter()
                    .map(|c| c.text.clone())
                    .collect::<Vec<_>>()
                    .join(" "),
            ]
            .join(" ");
            let n = sentences(&parsed.overview);
            Output {
                checks: vec![
                    check("fields", !parsed.overview.trim().is_empty(), ""),
                    language_check(&text, language),
                    check(
                        "length",
                        (1..=5).contains(&n),
                        format!(
                            "{n} sentences, {} additions, {} conflicts",
                            parsed.additions.len(),
                            parsed.conflicts.len()
                        ),
                    ),
                    leak_check(&text),
                ],
                shown,
                calls: vec![call],
                metrics: Metrics::new(),
            }
        }
        None => Output {
            shown: call.content.clone(),
            checks: vec![check("fields", false, "overview missing or unusable")],
            calls: vec![call],
            metrics: Metrics::new(),
        },
    }
}

fn memory_output(
    call: Call,
    case: &MemoryCase,
    clean_user_message: &str,
    existing: &[rrss_core::memories::ExistingMemorySummary],
    fallback_language: OutputLanguage,
) -> Output {
    if let Some(failed) = call_failed(&call) {
        return failed;
    }
    let parsed =
        rrss_app::jobs::parse_memory_candidates(&call.parsed, clean_user_message, existing);
    let raw_count = call.parsed["candidates"].as_array().map_or(0, Vec::len);
    let (valid_schema, candidates) = match parsed {
        Ok(items) => (true, items),
        Err(_) => (false, Vec::new()),
    };
    let shown = if candidates.is_empty() {
        format!("(no candidates; raw={raw_count})")
    } else {
        candidates
            .iter()
            .map(|c| match c.replaces_id {
                Some(id) => format!("- [update #{id} · {}] {}", c.kind.as_str(), c.content),
                None => format!("- [add · {}] {}", c.kind.as_str(), c.content),
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let all: String = candidates
        .iter()
        .map(|c| c.content.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let mut checks = vec![
        check(
            "gate",
            case.expect_trigger,
            if case.expect_trigger {
                ""
            } else {
                "unexpected trigger"
            },
        ),
        check("fields", valid_schema, ""),
        check(
            "limits",
            candidates.len() <= 2,
            format!("{} candidates", candidates.len()),
        ),
        leak_check(&all),
    ];
    if case.expect_empty {
        checks.push(check(
            "empty",
            candidates.is_empty(),
            format!("got {}", candidates.len()),
        ));
    } else {
        checks.push(check(
            "non-empty",
            !candidates.is_empty(),
            "expected at least 1",
        ));
    }
    if let Some(action) = case.expect_action.as_deref() {
        let ok = candidates.iter().any(|c| {
            let actual = if c.replaces_id.is_some() {
                "update"
            } else {
                "add"
            };
            actual == action
                && case
                    .expect_replaces_id
                    .map_or(true, |id| c.replaces_id == Some(id))
        });
        checks.push(check("action", ok, format!("want {action}")));
    }
    if let Some(kind) = case.expect_kind.as_deref() {
        let ok = candidates.iter().any(|c| c.kind.as_str() == kind);
        checks.push(check("kind", ok, format!("want {kind}")));
    }
    if !all.is_empty() {
        let lang = case
            .expect_lang
            .as_deref()
            .map(OutputLanguage::from_tag)
            .unwrap_or(fallback_language);
        checks.push(language_check(&all, lang));
        let lower = all.to_ascii_lowercase();
        let leaked_deictic = [
            "这篇文章",
            "这篇",
            "刚才",
            "这一段",
            "这段",
            "今天",
            "这次",
            "偏这边",
            "this article",
            "that paragraph",
            "just now",
            "today",
            "this chat",
        ]
        .iter()
        .find(|d| lower.contains(*d));
        checks.push(check(
            "de-contextualized",
            leaked_deictic.is_none(),
            leaked_deictic.map_or(String::new(), |d| format!("contains {d:?}")),
        ));
        if !case.expect_not_contains.is_empty() {
            let hit = case
                .expect_not_contains
                .iter()
                .find(|s| lower.contains(&s.to_ascii_lowercase()));
            checks.push(check(
                "forbidden",
                hit.is_none(),
                hit.map_or(String::new(), |s| format!("contains {s:?}")),
            ));
        }
    }
    Output {
        checks,
        shown,
        calls: vec![call],
        metrics: Metrics::new(),
    }
}

// ---------- report ----------

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const STYLE: &str = r#"
:root{--bg:#fbfaf7;--fg:#1f1d1a;--muted:#6b665e;--line:#e3dfd6;--card:#ffffff;--ok:#2f7d4f;--bad:#b3261e;--code:#f3f0ea}
@media (prefers-color-scheme:dark){:root{--bg:#171614;--fg:#ece8e1;--muted:#a29c92;--line:#34312c;--card:#1f1e1b;--ok:#6cc391;--bad:#f2867d;--code:#262420}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.6 system-ui,-apple-system,"Segoe UI","Noto Sans SC",sans-serif}
main{max-width:1500px;margin:0 auto;padding:24px 16px 64px}h1{font-size:24px;margin:0 0 4px}h2{font-size:19px;margin:40px 0 12px;border-bottom:1px solid var(--line);padding-bottom:6px}
h3{font-size:16px;margin:28px 0 8px}.muted{color:var(--muted)}table{border-collapse:collapse;margin:12px 0}td,th{border:1px solid var(--line);padding:4px 10px;text-align:left}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(320px,1fr));gap:12px}.col{background:var(--card);border:1px solid var(--line);border-radius:8px;padding:12px;min-width:0}
.col h4{margin:0 0 8px;font-size:13px;color:var(--muted);font-weight:600}.out{white-space:pre-wrap;overflow-wrap:anywhere}
.checks{display:flex;flex-wrap:wrap;gap:4px;margin:0 0 8px}.chk{font-size:12px;border-radius:4px;padding:1px 6px;border:1px solid currentColor}
.chk.ok{color:var(--ok)}.chk.bad{color:var(--bad)}details{margin:6px 0}summary{cursor:pointer;color:var(--muted)}
pre{white-space:pre-wrap;overflow-wrap:anywhere;background:var(--code);padding:10px;border-radius:6px;font-size:13px;max-height:420px;overflow:auto}
"#;

fn render(
    cases: &[Case],
    variants: &[Variant],
    model: &str,
    tier: InputTier,
    totals: &BTreeMap<&'static str, (u64, u64, usize)>,
) -> String {
    let mut html = String::new();
    html.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Prompt eval</title><style>");
    html.push_str(STYLE);
    html.push_str("</style></head><body><main>");
    html.push_str(&format!(
        "<h1>AI input/output evaluation</h1><p class=\"muted\">Model {} · {} · input tier {}</p>",
        escape(model),
        chrono::Local::now().format("%Y-%m-%d %H:%M"),
        tier.as_str()
    ));
    html.push_str("<p>Review input packing, fact coverage, sources, language and request cost. Automatic checks are deterministic hints for human review.</p>");
    html.push_str("<p class=\"muted\">The archived baseline covers the original tasks. Weekly and manual article summaries use the current production template with Chinese output for baseline.</p>");
    html.push_str("<table><tr><th>Variant</th><th>Checks passed</th><th>Prompt tokens</th><th>Completion tokens</th><th>Requests</th></tr>");
    for variant in variants {
        let (passed, total) = cases
            .iter()
            .filter_map(|case| case.outputs.get(variant.id()))
            .flat_map(|out| out.checks.iter())
            .fold((0, 0), |(p, t), c| (p + usize::from(c.ok), t + 1));
        let (prompt, completion, requests) = totals.get(variant.id()).copied().unwrap_or_default();
        html.push_str(&format!(
            "<tr><td>{}</td><td>{passed} / {total}</td><td>{prompt}</td><td>{completion}</td><td>{requests}</td></tr>",
            variant.label()
        ));
    }
    html.push_str("</table>");
    for (section, heading) in [
        ("summaries", "Summaries"),
        ("digests", "Daily digests"),
        ("digests_cold", "Daily digests without automatic summaries"),
        ("weekly_digests", "Weekly digests"),
        (
            "article_summaries",
            "Manual article summaries: single excerpt and compression",
        ),
        ("clusters", "Related coverage overviews"),
        ("memory", "Memory candidates"),
        (
            "citations",
            "Citation Q&A (chat assistant prompt with read-only tools)",
        ),
    ] {
        let rows: Vec<&Case> = cases.iter().filter(|c| c.section == section).collect();
        if rows.is_empty() {
            continue;
        }
        html.push_str(&format!("<h2>{heading}</h2>"));
        for case in rows {
            html.push_str(&format!("<h3>{}</h3>", escape(&case.title)));
            html.push_str(&format!(
                "<details><summary>Input</summary><pre>{}</pre></details><div class=\"grid\">",
                escape(&case.input)
            ));
            for variant in variants {
                let Some(out) = case.outputs.get(variant.id()) else {
                    continue;
                };
                html.push_str(&format!(
                    "<div class=\"col\"><h4>{}</h4><div class=\"checks\">",
                    variant.label()
                ));
                for c in &out.checks {
                    html.push_str(&format!(
                        "<span class=\"chk {}\" title=\"{}\">{}{}</span>",
                        if c.ok { "ok" } else { "bad" },
                        escape(&c.note),
                        if c.ok { "✓ " } else { "✗ " },
                        c.name
                    ));
                }
                html.push_str("</div>");
                let notes: Vec<String> = out
                    .checks
                    .iter()
                    .filter(|c| !c.note.is_empty())
                    .map(|c| format!("{}: {}", c.name, c.note))
                    .collect();
                if !notes.is_empty() {
                    html.push_str(&format!(
                        "<p class=\"muted\" style=\"font-size:12px\">{}</p>",
                        escape(&notes.join(" · "))
                    ));
                }
                html.push_str(&format!("<div class=\"out\">{}</div>", escape(&out.shown)));
                for (i, call) in out.calls.iter().enumerate() {
                    html.push_str(&format!(
                        "<details><summary>Request {} · {} ms · {} + {} tokens</summary><pre>{}</pre><pre>{}</pre></details>",
                        i + 1,
                        call.ms,
                        call.prompt_tokens,
                        call.completion_tokens,
                        escape(&format!("SYSTEM\n{}", call.system)),
                        escape(&format!("RESPONSE\n{}", if call.content.is_empty() { call.error.clone().unwrap_or_default() } else { call.content.clone() }))
                    ));
                }
                html.push_str("</div>");
            }
            html.push_str("</div>");
        }
    }
    html.push_str("</main></body></html>\n");
    html
}

// ---------- main ----------

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help") {
        println!("{}", cli::HELP);
        return Ok(());
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cli = cli::Cli::parse(&args, &root)?;
    let comparison_baseline = cli.compare.as_deref().map(compare::load).transpose()?;
    if let Some(baseline) = &comparison_baseline {
        compare::validate(baseline, cli.offline)?;
    }
    if cli.offline {
        return offline::run(&root, &cli, comparison_baseline.as_ref());
    }
    // Configuration and the API client are only created after the offline return.
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    let (Some(base_url), Some(model_name)) = (env("RRSS_EVAL_BASE_URL"), env("RRSS_EVAL_MODEL"))
    else {
        bail!(
            "set RRSS_EVAL_BASE_URL and RRSS_EVAL_MODEL (and RRSS_EVAL_API_KEY if the service needs one)"
        );
    };
    let cases: Arc<Cases> = Arc::new(serde_json::from_str(&std::fs::read_to_string(
        root.join("testdata/eval/cases.json"),
    )?)?);
    let baseline: Arc<Baseline> = Arc::new(serde_json::from_str(&std::fs::read_to_string(
        root.join("testdata/eval/baseline-zh.json"),
    )?)?);
    let model = Arc::new(Model {
        client: rrss_core::network::ProxyConfig::default().build_client_with_env(
            rrss_core::network::NetworkScope::Api,
            reqwest::Client::builder().timeout(std::time::Duration::from_secs(300)),
            &rrss_core::network::ProxyEnvironment::default(),
        )?,
        base_url,
        api_key: env("RRSS_EVAL_API_KEY"),
        model: model_name.clone(),
        gate: Semaphore::new(cli.concurrency),
        usage: Default::default(),
    });
    let options = Options {
        sections: cli.sections.clone(),
        tier: cli.tier,
    };
    let local = tokio::task::LocalSet::new();
    let results = local
        .run_until(async {
            let handles: Vec<_> = cli
                .variants
                .iter()
                .map(|variant| {
                    tokio::task::spawn_local(run_variant(
                        *variant,
                        root.clone(),
                        cases.clone(),
                        baseline.clone(),
                        model.clone(),
                        options.clone(),
                    ))
                })
                .collect();
            let mut results = Vec::new();
            for handle in handles {
                results.push(handle.await);
            }
            results
        })
        .await;
    let mut merged: Vec<Case> = Vec::new();
    for (variant, result) in cli.variants.iter().zip(results) {
        for case in result?.with_context(|| format!("variant {}", variant.id()))? {
            match merged
                .iter_mut()
                .find(|c| c.section == case.section && c.key == case.key)
            {
                Some(existing) => existing.outputs.extend(case.outputs),
                None => merged.push(case),
            }
        }
    }
    let totals = model.usage.lock().expect("usage").clone();
    let revision = metrics::revision(&root);
    let mut result = json!({ "schema_version":1, "mode":"online", "model":model_name,
        "tier":cli.tier.as_str(), "source_revision":revision, "fixture_hash":metrics::fixture_hash(&root)?,
        "production_hash":metrics::production_hash(&root)?,
        "sections":cli.sections, "variants":cli.variants, "usage":totals,
        "metrics":metrics::online_metrics(&merged, &totals), "cases":merged });
    let mut html = render(&merged, &cli.variants, &model_name, cli.tier, &totals);
    let mut markdown = online::markdown(&merged, cli.tier, &model_name, &revision);
    if let Some(baseline) = comparison_baseline {
        let comparison = compare::compare(&baseline, &result)?;
        let diff = comparison.markdown();
        println!("{diff}");
        markdown.push_str("\n");
        markdown.push_str(&diff);
        html = html.replace(
            "</main>",
            &format!("<h2>Comparison</h2><pre>{}</pre></main>", escape(&diff)),
        );
        result["comparison"] = serde_json::to_value(comparison)?;
    }
    std::fs::create_dir_all(&cli.out)?;
    std::fs::write(
        cli.out.join("results.json"),
        serde_json::to_string_pretty(&result)? + "\n",
    )?;
    std::fs::write(cli.out.join("report.html"), html)?;
    std::fs::write(cli.out.join("report.md"), markdown)?;
    for (variant, (prompt, completion, requests)) in &totals {
        eprintln!(
            "{variant}: {requests} requests, {prompt} prompt + {completion} completion tokens"
        );
    }
    eprintln!("report: {}", cli.out.join("report.html").display());
    Ok(())
}
