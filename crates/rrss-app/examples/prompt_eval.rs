//! AI output language eval: runs the background-job prompts on fixed inputs in three variants and
//! writes a side-by-side HTML report for a person to review.
//!
//! Variants: `baseline` (the Chinese prompts before English prompts, kept in
//! `testdata/eval/baseline-zh.json`), `en-zh` (English prompts, Simplified Chinese output) and
//! `en-en` (English prompts, English output). Each variant runs the same pipeline on its own
//! throwaway database: summaries, then the daily digest (pick and write), related coverage
//! overviews and memory candidates. Inputs are the real feed snapshots in `testdata/feeds/real/`
//! plus fictional cases in `testdata/eval/cases.json`. It never opens the everyday database.
//!
//! ```text
//! RRSS_EVAL_BASE_URL=https://api.deepseek.com RRSS_EVAL_MODEL=deepseek-chat RRSS_EVAL_API_KEY=… \
//!   cargo run -p rrss-app --example prompt_eval
//! ```
//!
//! Options: `--variants baseline,en-zh,en-en`, `--only summaries,digests,clusters,memory`,
//! `--concurrency 4`, `--out <dir>` (default `test-results/prompt-eval`). The report is
//! `report.html` in the output directory, with the raw results in `results.json`.

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

const TIER: InputTier = InputTier::Medium;

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
}

#[derive(Deserialize, Clone)]
struct MemoryCase {
    name: String,
    transcript: String,
}

#[derive(Deserialize)]
struct Baseline {
    templates: HashMap<String, String>,
    input: BaselineInput,
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
            Variant::Baseline => "Chinese prompts (before)",
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
                    fill(&map.event_with_count, &[number, tail.trim_end_matches(" articles")])
                } else {
                    fill(&map.event, &[number])
                }
            } else if let (Some(rest), true) = (line.strip_prefix("Topic: "), line.ends_with(" articles)")) {
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
}

impl Model {
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
            ..Call::default()
        };
        match rrss_ai::complete(&self.client, &req).await {
            Ok(out) => {
                call.content = out.content;
                call.parsed = out.parsed;
                call.prompt_tokens = out.prompt_tokens;
                call.completion_tokens = out.completion_tokens;
            }
            Err(error) => call.error = Some(error.to_string()),
        }
        call.ms = started.elapsed().as_millis();
        let mut usage = self.usage.lock().expect("usage");
        let entry = usage.entry(tag).or_default();
        entry.0 += u64::from(call.prompt_tokens);
        entry.1 += u64::from(call.completion_tokens);
        entry.2 += 1;
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
    let found: Vec<&str> = LEAKS.iter().copied().filter(|leak| text.contains(leak)).collect();
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
    })
}

fn summary_output(call: Call, language: OutputLanguage) -> Output {
    if let Some(failed) = call_failed(&call) {
        return failed;
    }
    let p = &call.parsed;
    let text = |key: &str| p.get(key).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let list = |key: &str| -> Vec<String> {
        p.get(key)
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default()
    };
    let (summary, one_liner, points, facts) = (text("summary"), text("one_liner"), list("key_points"), list("facts"));
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
    let all = [summary.clone(), one_liner.clone(), points.join(" "), facts.join(" ")].join(" ");
    let n = sentences(&summary);
    Output {
        checks: vec![
            check(
                "fields",
                !summary.is_empty() && !one_liner.is_empty() && p.get("key_points").is_some() && p.get("facts").is_some(),
                "",
            ),
            language_check(&all, language),
            check("length", (1..=5).contains(&n), format!("{n} sentences, {} points, {} facts", points.len(), facts.len())),
            check("limits", points.len() <= 3 && facts.len() <= 5, ""),
            leak_check(&all),
        ],
        shown,
        calls: vec![call],
    }
}

// ---------- seeding ----------

struct Seeded {
    _dir: tempfile::TempDir,
    conn: db::PooledConnection,
    ids: HashMap<String, i64>,
}

fn seed(root: &Path, cases: &Cases) -> Result<Seeded> {
    let dir = tempfile::tempdir()?;
    let conn = db::open_at(&dir.path().join("eval.db"))?;
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
        let feed = *feeds.get(&article.feed).with_context(|| format!("unknown feed {}", article.feed))?;
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
    for key in cases.summaries.iter().chain(cases.digests.iter().flat_map(|d| d.articles.iter())) {
        if let Some((file, _)) = key.strip_prefix("real:").and_then(|rest| rest.split_once('#')) {
            files.insert(file.to_string());
        }
    }
    for file in files {
        let path = root.join("testdata/feeds/real").join(&file);
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let parsed = rrss_core::parse_feed_bytes(&bytes, None)?;
        conn.execute(
            "INSERT INTO feeds (url, title, created_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![format!("https://snapshot.example/{file}"), parsed.title.clone().unwrap_or_else(|| file.clone()), now],
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
    Ok(Seeded { _dir: dir, conn, ids })
}

fn english_templates(conn: &rusqlite::Connection) -> Result<HashMap<String, String>> {
    let mut stmt = conn.prepare("SELECT slug, body FROM prompt_templates")?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

// ---------- pipeline ----------

#[derive(Clone)]
struct Options {
    sections: BTreeSet<String>,
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
    let id = |key: &str| seeded.ids.get(key).copied().with_context(|| format!("no article for {key}"));
    let mut out = Vec::new();
    eprintln!("[{}] summaries", variant.id());

    // Summaries: every article a later stage needs, so the digest and overviews read this
    // variant's own summaries, as they would in the app.
    let mut keys: Vec<String> = cases.summaries.clone();
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
        let (title, blocks, html): (String, Option<String>, Option<String>) = seeded.conn.query_row(
            "SELECT title, blocks_json, content_html FROM articles WHERE id = ?1",
            [id(key)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let input = ai_store::summary_payload(&title, blocks.as_deref(), html.as_deref(), false, TIER);
        jobs.push((key.clone(), title, input));
    }
    let mut handles = Vec::new();
    for (_, _, input) in &jobs {
        let (model, system, user) = (model.clone(), system.clone(), prompts.user(input));
        let tag = variant.id();
        handles.push(tokio::spawn(async move {
            model.call(tag, system, user, rrss_ai::summary_json_schema()).await
        }));
    }
    let mut calls = Vec::new();
    for handle in handles {
        calls.push(handle.await?);
    }
    for ((key, title, input), call) in jobs.into_iter().zip(calls) {
        if call.error.is_none()
            && let Some(summary) = call.parsed.get("summary").and_then(Value::as_str)
        {
            let raw = serde_json::to_string(&call.parsed)?;
            rrss_core::articles::set_summary_and_json(&seeded.conn, id(&key)?, summary.trim(), Some(&raw))?;
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
            let ids: Vec<i64> = digest.articles.iter().map(|key| id(key)).collect::<Result<_>>()?;
            let items = ai_store::digest_items_by_ids(&seeded.conn, &ids)?;
            out.push(run_digest(&prompts, &model, &seeded.conn, digest, &items, language).await?);
        }
    }

    if options.sections.contains("clusters") {
        let system = prompts.system(ai_store::SLUG_CLUSTER_SUMMARY, ai_store::TASK_CLUSTER_SUMMARY)?;
        for cluster in &cases.clusters_to_summarize {
            eprintln!("[{}] related coverage {cluster}", variant.id());
            let members = cluster_summary::visible_members(&seeded.conn, *cluster, FeedScope::All)?;
            let input = cluster_summary::build_input(&seeded.conn, &members, TIER)?;
            let call = model
                .call(variant.id(), system.clone(), prompts.user(&input.prompt), cluster_summary::json_schema())
                .await;
            let label = cases.clusters.iter().find(|c| c.id == *cluster).map_or(String::new(), |c| c.label.clone());
            out.push(Case {
                section: "clusters",
                key: format!("cluster-{cluster}"),
                title: label,
                input: input.prompt.clone(),
                outputs: BTreeMap::from([(variant.id(), cluster_output(call, &input.members, language))]),
            });
        }
    }

    if options.sections.contains("memory") {
        let system = prompts.system(ai_store::SLUG_MEMORY_CANDIDATE, ai_store::TASK_MEMORY_CANDIDATE)?;
        for memory in &cases.memory {
            eprintln!("[{}] memory: {}", variant.id(), memory.name);
            let call = model
                .call(variant.id(), system.clone(), memory.transcript.clone(), rrss_ai::facts_json_schema())
                .await;
            out.push(Case {
                section: "memory",
                key: memory.name.clone(),
                title: memory.name.clone(),
                input: memory.transcript.clone(),
                outputs: BTreeMap::from([(variant.id(), memory_output(call, language))]),
            });
        }
    }
    Ok(out)
}

async fn run_digest(
    prompts: &Prompts,
    model: &Model,
    conn: &rusqlite::Connection,
    digest: &DigestCase,
    items: &[DigestItem],
    language: OutputLanguage,
) -> Result<Case> {
    let shallow = ai_store::format_digest_input(items, &[], digest.pick_max);
    let pick = model
        .call(
            prompts.variant.id(),
            prompts.system(ai_store::SLUG_DAILY_DIGEST_PICK, ai_store::TASK_DIGEST_PICK_DAILY)?,
            prompts.user(&shallow),
            rrss_ai::digest_pick_json_schema(),
        )
        .await;
    let picks = if pick.error.is_none() { ai_store::parse_digest_picks(&pick.parsed) } else { Vec::new() };
    let selected = ai_store::apply_digest_picks(items, &picks, digest.pick_max, digest.pick_min);
    let deep = ai_store::format_digest_deep_input(conn, &selected, TIER)?;
    let write = model
        .call(
            prompts.variant.id(),
            prompts.system(ai_store::SLUG_DAILY_DIGEST, ai_store::TASK_DIGEST_DAILY)?,
            prompts.user(&deep.prompt),
            rrss_ai::digest_json_schema(),
        )
        .await;
    let input = format!("Pick input:\n\n{shallow}\n\n— — —\n\nWrite input:\n\n{}", deep.prompt);
    let mut checks = vec![check(
        "picks",
        pick.error.is_none() && !picks.is_empty(),
        format!("{} picked of {}, {} used", picks.len(), items.len(), selected.len()),
    )];
    let mut shown = String::new();
    if let Some(error) = &write.error {
        checks.push(check("request", false, error.clone()));
    } else {
        match ai_store::prepare_digest_draft(&write.parsed, &deep.numbered_items, language) {
            Ok(draft) => {
                shown = draft.body.clone();
                let raw: Vec<String> = write.parsed["sections"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|section| section["items"].as_array().cloned().unwrap_or_default())
                    .filter_map(|item| item["text"].as_str().map(str::to_string))
                    .collect();
                let dropped: Vec<&String> = raw.iter().filter(|text| !draft.body.contains(text.trim())).collect();
                let items_json: Value = serde_json::from_str(&draft.items_json).unwrap_or(Value::Null);
                let kept = items_json["items"].as_array().map_or(0, Vec::len);
                let unsourced = items_json["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|item| item["refs"].as_array().is_none_or(Vec::is_empty))
                    .count();
                let events = deep.prompt.matches("【Event ").count();
                checks.push(check("structure", true, format!("{kept} items for {events} events")));
                checks.push(check("sources", unsourced == 0, format!("{unsourced} items without a source")));
                checks.push(language_check(&draft.body, language));
                checks.push(leak_check(&draft.body));
                checks.push(check(
                    "cleanup",
                    true,
                    if dropped.is_empty() {
                        "nothing removed".to_string()
                    } else {
                        format!("removed or changed: {}", dropped.iter().map(|t| t.as_str()).collect::<Vec<_>>().join(" ‖ "))
                    },
                ));
            }
            Err(error) => checks.push(check("structure", false, error.to_string())),
        }
    }
    Ok(Case {
        section: "digests",
        key: digest.name.clone(),
        title: digest.name.clone(),
        input,
        outputs: BTreeMap::from([(
            prompts.variant.id(),
            Output {
                shown,
                checks,
                calls: vec![pick, write],
            },
        )]),
    })
}

fn cluster_output(call: Call, members: &[cluster_summary::Member], language: OutputLanguage) -> Output {
    if let Some(failed) = call_failed(&call) {
        return failed;
    }
    match cluster_summary::parse_output(&call.parsed, members) {
        Some(parsed) => {
            let title = |id: i64| members.iter().find(|m| m.article_id == id).map_or(String::new(), |m| m.feed_title.clone());
            let mut shown = parsed.overview.clone();
            for addition in &parsed.additions {
                shown.push_str(&format!("\n\n+ [{}] {}", title(addition.article_id), addition.text));
            }
            for conflict in &parsed.conflicts {
                shown.push_str(&format!("\n\n≠ {}", conflict.text));
            }
            let text = [
                parsed.overview.clone(),
                parsed.additions.iter().map(|a| a.text.clone()).collect::<Vec<_>>().join(" "),
                parsed.conflicts.iter().map(|c| c.text.clone()).collect::<Vec<_>>().join(" "),
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
                        format!("{n} sentences, {} additions, {} conflicts", parsed.additions.len(), parsed.conflicts.len()),
                    ),
                    leak_check(&text),
                ],
                shown,
                calls: vec![call],
            }
        }
        None => Output {
            shown: call.content.clone(),
            checks: vec![check("fields", false, "overview missing or unusable")],
            calls: vec![call],
        },
    }
}

fn memory_output(call: Call, language: OutputLanguage) -> Output {
    if let Some(failed) = call_failed(&call) {
        return failed;
    }
    let facts: Vec<String> = call.parsed["facts"]
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();
    let shown = if facts.is_empty() {
        "(no facts)".to_string()
    } else {
        facts.iter().map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n")
    };
    let all = facts.join(" ");
    Output {
        checks: vec![
            check("fields", call.parsed["facts"].is_array(), ""),
            language_check(&all, language),
            check("limits", facts.len() <= 3, format!("{} facts", facts.len())),
            leak_check(&all),
        ],
        shown,
        calls: vec![call],
    }
}

// ---------- report ----------

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
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

fn render(cases: &[Case], variants: &[Variant], model: &str, totals: &BTreeMap<&'static str, (u64, u64, usize)>) -> String {
    let mut html = String::new();
    html.push_str("<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Prompt eval</title><style>");
    html.push_str(STYLE);
    html.push_str("</style></head><body><main>");
    html.push_str(&format!(
        "<h1>AI output language eval</h1><p class=\"muted\">Model {} · {} · input tier {}</p>",
        escape(model),
        chrono::Local::now().format("%Y-%m-%d %H:%M"),
        TIER.as_str()
    ));
    html.push_str("<p>Check each case: is the Chinese from English prompts at least as good as before, and is the English natural? Automatic checks are hints, not verdicts.</p>");
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
        ("clusters", "Related coverage overviews"),
        ("memory", "Memory candidates"),
    ] {
        let rows: Vec<&Case> = cases.iter().filter(|c| c.section == section).collect();
        if rows.is_empty() {
            continue;
        }
        html.push_str(&format!("<h2>{heading}</h2>"));
        for case in rows {
            html.push_str(&format!("<h3>{}</h3>", escape(&case.title)));
            html.push_str(&format!("<details><summary>Input</summary><pre>{}</pre></details><div class=\"grid\">", escape(&case.input)));
            for variant in variants {
                let Some(out) = case.outputs.get(variant.id()) else {
                    continue;
                };
                html.push_str(&format!("<div class=\"col\"><h4>{}</h4><div class=\"checks\">", variant.label()));
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
                let notes: Vec<String> = out.checks.iter().filter(|c| !c.note.is_empty()).map(|c| format!("{}: {}", c.name, c.note)).collect();
                if !notes.is_empty() {
                    html.push_str(&format!("<p class=\"muted\" style=\"font-size:12px\">{}</p>", escape(&notes.join(" · "))));
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

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
    let (Some(base_url), Some(model_name)) = (env("RRSS_EVAL_BASE_URL"), env("RRSS_EVAL_MODEL")) else {
        bail!("set RRSS_EVAL_BASE_URL and RRSS_EVAL_MODEL (and RRSS_EVAL_API_KEY if the service needs one)");
    };
    let variants: Vec<Variant> = match arg(&args, "--variants") {
        Some(list) => list
            .split(',')
            .map(|id| Variant::ALL.into_iter().find(|v| v.id() == id.trim()).ok_or_else(|| anyhow!("unknown variant {id}")))
            .collect::<Result<_>>()?,
        None => Variant::ALL.to_vec(),
    };
    let sections: BTreeSet<String> = arg(&args, "--only")
        .unwrap_or_else(|| "summaries,digests,clusters,memory".into())
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    let concurrency: usize = arg(&args, "--concurrency").and_then(|n| n.parse().ok()).unwrap_or(4);
    let out_dir = arg(&args, "--out").map(PathBuf::from).unwrap_or_else(|| root.join("test-results/prompt-eval"));

    let cases: Arc<Cases> = Arc::new(serde_json::from_str(&std::fs::read_to_string(root.join("testdata/eval/cases.json"))?)?);
    let baseline: Arc<Baseline> = Arc::new(serde_json::from_str(&std::fs::read_to_string(root.join("testdata/eval/baseline-zh.json"))?)?);
    let model = Arc::new(Model {
        client: reqwest::Client::builder().timeout(std::time::Duration::from_secs(300)).build()?,
        base_url,
        api_key: env("RRSS_EVAL_API_KEY"),
        model: model_name.clone(),
        gate: Semaphore::new(concurrency.max(1)),
        usage: Default::default(),
    });

    let options = Options { sections };
    // Each variant holds its own SQLite connection across awaits, so the variants run as local
    // tasks on this thread; the requests themselves are network-bound.
    let local = tokio::task::LocalSet::new();
    let results = local
        .run_until(async {
            let handles: Vec<_> = variants
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
    for (variant, result) in variants.iter().zip(results) {
        for case in result?.with_context(|| format!("variant {}", variant.id()))? {
            match merged.iter_mut().find(|c| c.section == case.section && c.key == case.key) {
                Some(existing) => existing.outputs.extend(case.outputs),
                None => merged.push(case),
            }
        }
    }
    let totals = model.usage.lock().expect("usage").clone();
    std::fs::create_dir_all(&out_dir)?;
    std::fs::write(out_dir.join("results.json"), serde_json::to_string_pretty(&json!({ "model": model_name, "cases": merged }))?)?;
    let report = out_dir.join("report.html");
    std::fs::write(&report, render(&merged, &variants, &model_name, &totals))?;
    for (variant, (prompt, completion, requests)) in &totals {
        eprintln!("{variant}: {requests} requests, {prompt} prompt + {completion} completion tokens");
    }
    eprintln!("report: {}", report.display());
    Ok(())
}
