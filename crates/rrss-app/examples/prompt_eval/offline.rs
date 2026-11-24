use super::*;
use corpus::{Corpus, Fact};
use fixtures::{FixtureDb, NOW};
use metrics::{Direction, Metrics, chars, estimate_tokens, normalize, put, ratio};

#[derive(Serialize)]
struct FactHit {
    text: String,
    category: String,
    hit: bool,
}

#[derive(Serialize)]
struct Payload {
    budget: usize,
    actual_chars: usize,
    full_chars: usize,
    over_budget: bool,
    utilization: f64,
    estimated_tokens: usize,
    facts_hit: usize,
    facts_total: usize,
    fact_recall: f64,
    facts: Vec<FactHit>,
    text: String,
}

fn payload(text: String, full_chars: usize, budget: usize, facts: &[Fact]) -> Payload {
    let hits: Vec<_> = facts
        .iter()
        .map(|fact| FactHit {
            text: fact.text.clone(),
            category: fact.category.as_str().into(),
            hit: fact.hit(&text),
        })
        .collect();
    let matched = hits.iter().filter(|fact| fact.hit).count();
    Payload {
        actual_chars: chars(&text),
        full_chars,
        over_budget: full_chars > budget,
        utilization: ratio(chars(&text), budget),
        estimated_tokens: estimate_tokens(&text),
        facts_hit: matched,
        facts_total: hits.len(),
        fact_recall: ratio(matched, hits.len()),
        facts: hits,
        text,
        budget,
    }
}

#[derive(Serialize)]
struct Extraction {
    id: String,
    language: String,
    tier: String,
    link_stub_expected: bool,
    link_stub_detected: bool,
    near_budget: bool,
    summary: Payload,
    excerpt: Payload,
    segmented: Option<Payload>,
    segment_count: usize,
    segment_chars: Vec<usize>,
    orphan_headings: usize,
    segments: Vec<String>,
}

fn extract(corpus: &Corpus) -> Result<Vec<Extraction>> {
    let mut rows = Vec::new();
    for tier in [InputTier::Low, InputTier::Medium, InputTier::High] {
        for case in &corpus.articles {
            let input = case.prepare()?;
            let summary = summary_extract::extract_summary_payload(
                &case.title,
                Some(&input.blocks),
                input.html.as_deref(),
                false,
                tier,
            );
            let excerpt = summary_extract::extract_body_excerpt(
                Some(&input.blocks),
                input.html.as_deref(),
                tier.digest_item_chars(),
            );
            let segments = summary_extract::split_segments(
                Some(&input.blocks),
                input.html.as_deref(),
                tier.summary_budget(false),
            )
            .unwrap_or_default();
            let segmented = (!segments.is_empty()).then(|| {
                payload(
                    segments.join("\n\n"),
                    chars(&input.full_text),
                    segments.len() * tier.summary_budget(false),
                    &case.must_keep_facts,
                )
            });
            rows.push(Extraction {
                id: case.id.clone(),
                language: case.language.clone(),
                tier: tier.as_str().into(),
                link_stub_expected: case.link_stub,
                link_stub_detected: summary.contains(summary_extract::LINK_STUB_NOTE),
                near_budget: case.near_budget_tier.as_deref() == Some(tier.as_str()),
                summary: payload(
                    summary,
                    chars(&input.full_text) + chars(&case.title) + 2,
                    tier.summary_budget(false),
                    &case.must_keep_facts,
                ),
                excerpt: payload(
                    excerpt,
                    chars(&input.full_text),
                    tier.digest_item_chars(),
                    &case.must_keep_facts,
                ),
                segmented,
                segment_count: segments.len(),
                segment_chars: segments.iter().map(|s| chars(s)).collect(),
                orphan_headings: metrics::orphan_headings(&segments),
                segments,
            });
        }
    }
    Ok(rows)
}

fn extraction_metrics(rows: &[Extraction], metrics: &mut Metrics) {
    for tier in ["low", "medium", "high"] {
        let selected: Vec<_> = rows.iter().filter(|row| row.tier == tier).collect();
        if selected.is_empty() {
            continue;
        }
        let prefix = format!("summary_extract.{tier}");
        for path in ["summary", "excerpt", "segments"] {
            let payloads: Vec<_> = selected
                .iter()
                .filter_map(|row| match path {
                    "summary" => Some((*row, &row.summary)),
                    "excerpt" => Some((*row, &row.excerpt)),
                    _ => row.segmented.as_ref().map(|payload| (*row, payload)),
                })
                .collect();
            for language in ["all", "zh", "en"] {
                for category in ["all", "body", "table", "code", "lead", "tail"] {
                    let facts: Vec<_> = payloads
                        .iter()
                        .filter(|(row, _)| language == "all" || row.language == language)
                        .flat_map(|(_, payload)| &payload.facts)
                        .filter(|fact| category == "all" || fact.category == category)
                        .collect();
                    if facts.is_empty() {
                        continue;
                    }
                    let key = format!("{prefix}.{path}.{language}.{category}.fact_recall");
                    put(
                        metrics,
                        key,
                        ratio(facts.iter().filter(|f| f.hit).count(), facts.len()),
                        "ratio",
                        Direction::Higher,
                    );
                }
                let oversized: Vec<_> = payloads
                    .iter()
                    .filter(|(row, p)| {
                        p.over_budget
                            && !row.link_stub_expected
                            && (language == "all" || row.language == language)
                    })
                    .map(|(_, p)| *p)
                    .collect();
                if !oversized.is_empty() {
                    let utilization = oversized.iter().map(|p| p.utilization).sum::<f64>()
                        / oversized.len() as f64;
                    let tokens = oversized.iter().map(|p| p.estimated_tokens).sum::<usize>() as f64
                        / oversized.len() as f64;
                    put(
                        metrics,
                        format!("{prefix}.{path}.{language}.over_budget_utilization"),
                        utilization,
                        "ratio",
                        Direction::TowardOne,
                    );
                    put(
                        metrics,
                        format!("{prefix}.{path}.{language}.mean_estimated_tokens"),
                        tokens,
                        "tokens",
                        Direction::Observe,
                    );
                }
            }
            let mean_tokens = |lang: &str| {
                let values: Vec<_> = payloads
                    .iter()
                    .filter(|(row, p)| {
                        row.language == lang && p.over_budget && !row.link_stub_expected
                    })
                    .map(|(_, p)| p.estimated_tokens)
                    .collect();
                if values.is_empty() {
                    0.0
                } else {
                    values.iter().sum::<usize>() as f64 / values.len() as f64
                }
            };
            let en = mean_tokens("en");
            if en > 0.0 {
                put(
                    metrics,
                    format!("{prefix}.{path}.zh_en_token_ratio"),
                    mean_tokens("zh") / en,
                    "ratio",
                    Direction::Observe,
                );
            }
        }
        let triggered = selected.iter().filter(|r| r.segment_count > 0).count();
        put(
            metrics,
            format!("{prefix}.segment_trigger_ratio"),
            ratio(triggered, selected.len()),
            "ratio",
            Direction::Observe,
        );
        put(
            metrics,
            format!("{prefix}.orphan_headings"),
            selected.iter().map(|r| r.orphan_headings).sum::<usize>() as f64,
            "items",
            Direction::Lower,
        );
        for count in 0..=summary_extract::MAX_SEGMENTS {
            put(
                metrics,
                format!("{prefix}.segment_count.{count}"),
                selected.iter().filter(|r| r.segment_count == count).count() as f64,
                "articles",
                Direction::Observe,
            );
        }
        let tp = selected
            .iter()
            .filter(|r| r.link_stub_expected && r.link_stub_detected)
            .count();
        let fp = selected
            .iter()
            .filter(|r| !r.link_stub_expected && r.link_stub_detected)
            .count();
        let fn_count = selected
            .iter()
            .filter(|r| r.link_stub_expected && !r.link_stub_detected)
            .count();
        let tn = selected
            .iter()
            .filter(|r| !r.link_stub_expected && !r.link_stub_detected)
            .count();
        for (key, value, direction) in [
            ("link_true_positive", tp, Direction::Observe),
            ("link_false_positive", fp, Direction::Lower),
            ("link_false_negative", fn_count, Direction::Lower),
            ("link_true_negative", tn, Direction::Observe),
        ] {
            put(
                metrics,
                format!("{prefix}.{key}"),
                value as f64,
                "articles",
                direction,
            );
        }
        put(
            metrics,
            format!("{prefix}.link_accuracy"),
            ratio(tp + tn, selected.len()),
            "ratio",
            Direction::Higher,
        );
        put(
            metrics,
            format!("{prefix}.link_recall"),
            ratio(tp, tp + fn_count),
            "ratio",
            Direction::Higher,
        );
        let near: Vec<_> = selected.iter().filter(|r| r.near_budget).collect();
        put(
            metrics,
            format!("{prefix}.near_budget_segment_trigger_ratio"),
            ratio(
                near.iter().filter(|r| r.segment_count > 0).count(),
                near.len(),
            ),
            "ratio",
            Direction::Observe,
        );
    }
}

#[derive(Debug, Serialize, Default)]
pub struct Health {
    pub count: usize,
    pub noisy_feed_count: usize,
    pub noisy_feed_ratio: f64,
    pub disliked_count: usize,
    pub read_count: usize,
    pub link_stub_count: usize,
    pub unique_feeds: usize,
    pub max_single_feed_share: f64,
    pub article_ids: Vec<i64>,
}

pub fn health(fixture: &FixtureDb, ids: &[i64]) -> Result<Health> {
    let noisy = rrss_core::feeds::noisy_feed_ids(&fixture.conn)?;
    let mut out = Health {
        count: ids.len(),
        article_ids: ids.to_vec(),
        ..Health::default()
    };
    let mut feeds = BTreeMap::new();
    for id in ids {
        let (feed, read, disliked): (i64,bool,bool) = fixture.conn.query_row(
            "SELECT feed_id,read,EXISTS(SELECT 1 FROM feedback WHERE article_id=a.id AND kind='down') FROM articles a WHERE id=?1",
            [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        *feeds.entry(feed).or_insert(0usize) += 1;
        out.noisy_feed_count += usize::from(noisy.contains(&feed));
        out.disliked_count += usize::from(disliked);
        out.read_count += usize::from(read);
        out.link_stub_count += usize::from(fixture.link_stubs.contains(id));
    }
    out.unique_feeds = feeds.len();
    out.max_single_feed_share = ratio(feeds.values().copied().max().unwrap_or(0), ids.len());
    out.noisy_feed_ratio = ratio(out.noisy_feed_count, ids.len());
    Ok(out)
}

fn health_metrics(metrics: &mut Metrics, prefix: &str, health: &Health) {
    for (key, value, direction, unit) in [
        ("count", health.count as f64, Direction::Observe, "articles"),
        (
            "noisy_feed_ratio",
            health.noisy_feed_ratio,
            Direction::Lower,
            "ratio",
        ),
        (
            "disliked_count",
            health.disliked_count as f64,
            Direction::Lower,
            "articles",
        ),
        (
            "read_count",
            health.read_count as f64,
            Direction::Observe,
            "articles",
        ),
        (
            "link_stub_count",
            health.link_stub_count as f64,
            Direction::Lower,
            "articles",
        ),
        (
            "unique_feeds",
            health.unique_feeds as f64,
            Direction::Higher,
            "feeds",
        ),
        (
            "max_single_feed_share",
            health.max_single_feed_share,
            Direction::Lower,
            "ratio",
        ),
    ] {
        put(metrics, format!("{prefix}.{key}"), value, unit, direction);
    }
}

#[derive(Serialize)]
struct Catalog {
    duplicate_title_items: usize,
    noisy_items: usize,
    noisy_identifiable_items: usize,
    noisy_identifiable_ratio: f64,
    unsummarized_items: usize,
    unsummarized_with_lead_or_no_body: usize,
    unsummarized_evidence_ratio: f64,
    prompt: String,
}

fn item_catalog<'a>(catalog: &'a str, id: i64) -> &'a str {
    let marker = format!("[id:{id}]");
    let Some(start) = catalog.find(&marker) else {
        return "";
    };
    let rest = &catalog[start..];
    let next = rest[marker.len()..]
        .find("\n[id:")
        .map(|i| i + marker.len());
    let topic = rest[marker.len()..]
        .find("\n\nTopic:")
        .map(|i| i + marker.len());
    &rest[..next.into_iter().chain(topic).min().unwrap_or(rest.len())]
}

fn catalog(fixture: &FixtureDb, items: &[DigestItem]) -> Result<Catalog> {
    let mut noisy = rrss_core::feeds::noisy_feed_titles(&fixture.conn)?;
    noisy.sort();
    let prompt = ai_store::format_digest_input(items, &noisy, fixtures::PICK_MAX);
    let body = prompt.split("\n\nNoisy sources").next().unwrap_or(&prompt);
    let mut duplicates = 0;
    let mut noisy_items = 0;
    let mut identifiable = 0;
    let mut unsummarized = 0;
    let mut evidence = 0;
    for item in items {
        let line = item_catalog(body, item.article_id);
        let entry = line.lines().next().unwrap_or("");
        if let Some((title, summary)) = entry
            .split_once("] ")
            .and_then(|(_, entry)| entry.split_once(" | "))
        {
            duplicates += usize::from(normalize(title) == normalize(summary));
        }
        if noisy.contains(&item.feed_title) {
            noisy_items += 1;
            identifiable += usize::from(line.contains(&item.feed_title));
        }
        let (summary, blocks, html): (Option<String>, Option<String>, Option<String>) =
            fixture.conn.query_row(
                "SELECT summary,blocks_json,content_html FROM articles WHERE id=?1",
                [item.article_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
        if summary.is_none_or(|s| s.trim().is_empty()) {
            unsummarized += 1;
            let doc = blocks
                .as_deref()
                .and_then(|b| rrss_render::document_from_json(b).ok())
                .unwrap_or_else(|| {
                    rrss_render::html_to_document(
                        html.as_deref().unwrap_or(""),
                        &rrss_render::RenderOpts::default(),
                    )
                });
            let lead = doc
                .blocks
                .iter()
                .find_map(|block| match block {
                    rrss_render::Block::Paragraph { .. } => Some(rrss_render::block_plain(block)),
                    _ => None,
                })
                .unwrap_or_default();
            let lead = rrss_core::text::truncate_chars(&lead, 80);
            evidence += usize::from(
                line.contains("[No body text]")
                    || (!lead.is_empty() && normalize(line).contains(&normalize(&lead))),
            );
        }
    }
    Ok(Catalog {
        duplicate_title_items: duplicates,
        noisy_items,
        noisy_identifiable_items: identifiable,
        noisy_identifiable_ratio: ratio(identifiable, noisy_items),
        unsummarized_items: unsummarized,
        unsummarized_with_lead_or_no_body: evidence,
        unsummarized_evidence_ratio: ratio(evidence, unsummarized),
        prompt,
    })
}

#[derive(Debug, Serialize)]
struct Efficiency {
    total_budget: usize,
    per_item_budget: usize,
    actual_excerpt_chars: usize,
    utilization: f64,
    short_unused_chars: usize,
    truncated_long_articles: usize,
    unretained_long_chars: usize,
    wasted_budget_chars: usize,
    redundant_summary_chars: usize,
    multi_report_members: usize,
    members_with_publication_time: usize,
    publication_time_ratio: f64,
    prompt_chars: usize,
    estimated_tokens: usize,
}

fn efficiency(
    fixture: &FixtureDb,
    deep: &rrss_core::digest::DigestDeepInput,
    tier: InputTier,
) -> Result<Efficiency> {
    let items = &deep.numbered_items;
    let per_item = (tier.digest_total_chars() / items.len().max(1)).min(tier.digest_item_chars());
    let mut actual = 0;
    let mut short_unused = 0;
    let mut long_missing = 0;
    let mut truncated = 0;
    let mut redundant = 0;
    let mut multi_members = 0;
    let mut timed = 0;
    for (i, item) in items.iter().enumerate() {
        let (blocks, html, published): (Option<String>, Option<String>, Option<i64>) =
            fixture.conn.query_row(
                "SELECT blocks_json,content_html,published_at FROM articles WHERE id=?1",
                [item.article_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
        let excerpt =
            summary_extract::extract_body_excerpt(blocks.as_deref(), html.as_deref(), per_item);
        let full =
            summary_extract::extract_body_excerpt(blocks.as_deref(), html.as_deref(), usize::MAX);
        actual += chars(&excerpt);
        if chars(&full) <= per_item {
            short_unused += per_item.saturating_sub(chars(&excerpt));
            let (one, points) = ai_store::parse_summary_details_for_digest(
                &item.summary,
                item.summary_json.as_deref(),
            );
            let mut candidates: BTreeSet<String> = std::iter::once(one)
                .chain(points)
                .chain(rrss_core::digest::summary_facts(
                    item.summary_json.as_deref(),
                ))
                .collect();
            candidates.retain(|s| !s.is_empty());
            redundant += candidates
                .into_iter()
                .filter(|s| normalize(&excerpt).contains(&normalize(s)) && deep.prompt.contains(s))
                .map(|s| chars(&s))
                .sum::<usize>();
        } else if chars(&full) > chars(&excerpt) {
            truncated += 1;
            long_missing += chars(&full).saturating_sub(chars(&excerpt));
        }
        if item.cluster_id.is_some()
            && items
                .iter()
                .filter(|other| other.cluster_id == item.cluster_id)
                .count()
                > 1
        {
            multi_members += 1;
            let start_marker = format!("〈{}〉", i + 1);
            let next_marker = format!("〈{}〉", i + 2);
            let block = deep
                .prompt
                .split_once(&start_marker)
                .map(|(_, rest)| rest.split(&next_marker).next().unwrap_or(rest))
                .unwrap_or("");
            if let Some(time) = published.and_then(chrono::DateTime::from_timestamp_millis) {
                let time = time.with_timezone(&chrono::Local);
                timed += usize::from(
                    block.contains(&time.format("%m-%d %H:%M").to_string())
                        || block.contains(&time.to_rfc3339()),
                );
            }
        }
    }
    let wasted = short_unused
        .min(long_missing)
        .min(tier.digest_total_chars().saturating_sub(actual));
    Ok(Efficiency {
        total_budget: tier.digest_total_chars(),
        per_item_budget: per_item,
        actual_excerpt_chars: actual,
        utilization: ratio(actual, tier.digest_total_chars()),
        short_unused_chars: short_unused,
        truncated_long_articles: truncated,
        unretained_long_chars: long_missing,
        wasted_budget_chars: wasted,
        redundant_summary_chars: redundant,
        multi_report_members: multi_members,
        members_with_publication_time: timed,
        publication_time_ratio: ratio(timed, multi_members),
        prompt_chars: chars(&deep.prompt),
        estimated_tokens: estimate_tokens(&deep.prompt),
    })
}

#[derive(Serialize)]
struct DigestRow {
    scene: String,
    period: String,
    candidates: Health,
    representatives: Health,
    selected: Health,
    catalog: Catalog,
    efficiency: Efficiency,
}

fn digest_rows(corpus: &Corpus, tier: InputTier, metrics: &mut Metrics) -> Result<Vec<DigestRow>> {
    let mut rows = Vec::new();
    for scene in ["mixed", "cold", "polluted"] {
        let fixture = fixtures::scene(corpus, scene)?;
        for period in ["daily", "weekly"] {
            let limit = if period == "daily" {
                fixtures::DAILY_CANDIDATES
            } else {
                fixtures::WEEKLY_CANDIDATES
            };
            let window = ai_store::DigestWindow::for_period(period, NOW, NOW, limit);
            let candidates = ai_store::digest_candidates(&fixture.conn, limit, &window)?;
            let kept = fixtures::representatives(&fixture.conn, &candidates, limit as usize)?;
            // No simulated editorial intelligence: use the ordered prefix as fixed picks.
            let picks: Vec<_> = kept
                .iter()
                .take(fixtures::PICK_MAX)
                .map(|i| i.article_id)
                .collect();
            let selected = ai_store::apply_digest_picks(&kept, &picks, fixtures::PICK_MAX, 6);
            let deep = ai_store::format_digest_deep_input(&fixture.conn, &selected, tier)?;
            let row = DigestRow {
                scene: scene.into(),
                period: period.into(),
                candidates: health(
                    &fixture,
                    &candidates.iter().map(|i| i.article_id).collect::<Vec<_>>(),
                )?,
                representatives: health(
                    &fixture,
                    &kept.iter().map(|i| i.article_id).collect::<Vec<_>>(),
                )?,
                selected: health(
                    &fixture,
                    &selected.iter().map(|i| i.article_id).collect::<Vec<_>>(),
                )?,
                catalog: catalog(&fixture, &kept)?,
                efficiency: efficiency(&fixture, &deep, tier)?,
            };
            let prefix = format!("digests.{scene}.{period}");
            health_metrics(metrics, &format!("{prefix}.candidates"), &row.candidates);
            health_metrics(
                metrics,
                &format!("{prefix}.representatives"),
                &row.representatives,
            );
            health_metrics(metrics, &format!("{prefix}.selected"), &row.selected);
            for (key, value, unit, direction) in [
                (
                    "catalog.duplicate_title_items",
                    row.catalog.duplicate_title_items as f64,
                    "items",
                    Direction::Lower,
                ),
                (
                    "deep.utilization",
                    row.efficiency.utilization,
                    "ratio",
                    Direction::TowardOne,
                ),
                (
                    "deep.wasted_budget_chars",
                    row.efficiency.wasted_budget_chars as f64,
                    "chars",
                    Direction::Lower,
                ),
                (
                    "deep.redundant_summary_chars",
                    row.efficiency.redundant_summary_chars as f64,
                    "chars",
                    Direction::Lower,
                ),
                (
                    "deep.publication_time_ratio",
                    row.efficiency.publication_time_ratio,
                    "ratio",
                    Direction::Higher,
                ),
                (
                    "deep.prompt_chars",
                    row.efficiency.prompt_chars as f64,
                    "chars",
                    Direction::Observe,
                ),
                (
                    "deep.estimated_tokens",
                    row.efficiency.estimated_tokens as f64,
                    "tokens",
                    Direction::Observe,
                ),
            ] {
                put(metrics, format!("{prefix}.{key}"), value, unit, direction);
            }
            if row.catalog.noisy_items > 0 {
                put(
                    metrics,
                    format!("{prefix}.catalog.noisy_identifiable_ratio"),
                    row.catalog.noisy_identifiable_ratio,
                    "ratio",
                    Direction::Higher,
                );
            }
            if row.catalog.unsummarized_items > 0 {
                put(
                    metrics,
                    format!("{prefix}.catalog.unsummarized_evidence_ratio"),
                    row.catalog.unsummarized_evidence_ratio,
                    "ratio",
                    Direction::Higher,
                );
            }
            rows.push(row);
        }
    }
    Ok(rows)
}

#[derive(Debug, Serialize)]
pub struct WeeklyParts {
    pub daily_chars: usize,
    pub daily_tokens: usize,
    pub sources_chars: usize,
    pub sources_tokens: usize,
    pub supplements_chars: usize,
    pub supplements_tokens: usize,
    pub framing_chars: usize,
    pub total_chars: usize,
    pub total_tokens: usize,
}

pub fn weekly_parts(prompt: &str) -> WeeklyParts {
    let portion = |heading: &str| {
        prompt
            .split_once(heading)
            .map(|(_, rest)| rest.split("\n\n## ").next().unwrap_or(rest).trim())
            .unwrap_or("")
    };
    let daily = portion("## Daily digest items\n\n");
    let sources = portion("## Source articles\n\n");
    let supplements = prompt
        .split_once("## Supplementary articles")
        .map(|(_, rest)| {
            rest.split_once("\n\n")
                .map(|(_, body)| body.trim())
                .unwrap_or("")
        })
        .unwrap_or("");
    WeeklyParts {
        daily_chars: chars(daily),
        daily_tokens: estimate_tokens(daily),
        sources_chars: chars(sources),
        sources_tokens: estimate_tokens(sources),
        supplements_chars: chars(supplements),
        supplements_tokens: estimate_tokens(supplements),
        framing_chars: chars(prompt)
            .saturating_sub(chars(daily) + chars(sources) + chars(supplements)),
        total_chars: chars(prompt),
        total_tokens: estimate_tokens(prompt),
    }
}

pub fn cluster_collisions(
    conn: &rusqlite::Connection,
    daily: &[rrss_core::digest_sources::DailyDigestLine],
    supplements: &[DigestItem],
) -> Result<Vec<i64>> {
    let ids: BTreeSet<_> = daily
        .iter()
        .flat_map(|line| &line.article_ids)
        .copied()
        .collect();
    let source_items = ai_store::digest_items_by_ids(conn, &ids.into_iter().collect::<Vec<_>>())?;
    let clusters: BTreeSet<_> = source_items
        .iter()
        .filter_map(|item| item.cluster_id)
        .collect();
    Ok(supplements
        .iter()
        .filter(|item| item.cluster_id.is_some_and(|id| clusters.contains(&id)))
        .map(|item| item.article_id)
        .collect())
}

#[derive(Serialize)]
struct WeeklyRow {
    daily_lines: usize,
    source_articles: usize,
    supplements: Health,
    cluster_collision_count: usize,
    cluster_collision_ids: Vec<i64>,
    parts: WeeklyParts,
    prompt: String,
}

fn weekly_row(corpus: &Corpus, tier: InputTier, metrics: &mut Metrics) -> Result<WeeklyRow> {
    let fixture = fixtures::full_week(corpus)?;
    let daily = ai_store::week_daily_lines(&fixture.conn, NOW - 7 * fixtures::DAY, NOW)?;
    let mut window =
        ai_store::DigestWindow::for_period("weekly", NOW, NOW, fixtures::SUPPLEMENT_CANDIDATES);
    window.exclude_daily_before = Some(i64::MAX);
    let candidates =
        ai_store::digest_candidates(&fixture.conn, fixtures::SUPPLEMENT_CANDIDATES, &window)?;
    let kept = fixtures::representatives(
        &fixture.conn,
        &candidates,
        fixtures::SUPPLEMENT_CANDIDATES as usize,
    )?;
    let collisions = cluster_collisions(&fixture.conn, &daily, &kept)?;
    let picks: Vec<_> = kept
        .iter()
        .take(fixtures::SUPPLEMENT_MAX)
        .map(|i| i.article_id)
        .collect();
    let selected = ai_store::apply_digest_picks(&kept, &picks, fixtures::SUPPLEMENT_MAX, 0);
    let deep = ai_store::format_weekly_input(&fixture.conn, &daily, &selected, tier)?;
    let row = WeeklyRow {
        daily_lines: daily.len(),
        source_articles: daily
            .iter()
            .flat_map(|d| &d.article_ids)
            .collect::<BTreeSet<_>>()
            .len(),
        supplements: health(
            &fixture,
            &kept.iter().map(|i| i.article_id).collect::<Vec<_>>(),
        )?,
        cluster_collision_count: collisions.len(),
        cluster_collision_ids: collisions,
        parts: weekly_parts(&deep.prompt),
        prompt: deep.prompt,
    };
    health_metrics(
        metrics,
        "weekly.full.supplement_candidates",
        &row.supplements,
    );
    put(
        metrics,
        "weekly.full.cluster_collision_count",
        row.cluster_collision_count as f64,
        "articles",
        Direction::Lower,
    );
    for (key, value, unit) in [
        ("daily_chars", row.parts.daily_chars, "chars"),
        ("daily_tokens", row.parts.daily_tokens, "tokens"),
        ("sources_chars", row.parts.sources_chars, "chars"),
        ("sources_tokens", row.parts.sources_tokens, "tokens"),
        ("supplements_chars", row.parts.supplements_chars, "chars"),
        ("supplements_tokens", row.parts.supplements_tokens, "tokens"),
        ("total_chars", row.parts.total_chars, "chars"),
        ("total_tokens", row.parts.total_tokens, "tokens"),
    ] {
        put(
            metrics,
            format!("weekly.full.{key}"),
            value as f64,
            unit,
            Direction::Observe,
        );
    }
    Ok(row)
}

fn auto_summary(corpus: &Corpus, metrics: &mut Metrics) -> Result<Health> {
    let fixture = fixtures::scene(corpus, "polluted")?;
    let due = rrss_core::articles::due_focus_summaries_excluding(
        &fixture.conn,
        ai_store::FOCUS_BATCH,
        &[],
    )?;
    let row = health(&fixture, &due.iter().map(|d| d.id).collect::<Vec<_>>())?;
    health_metrics(metrics, "auto_summary.batch", &row);
    for (key, value) in [
        ("read_count", row.read_count),
        ("noisy_feed_count", row.noisy_feed_count),
    ] {
        put(
            metrics,
            format!("auto_summary.batch.{key}"),
            value as f64,
            "articles",
            Direction::Lower,
        );
    }
    Ok(row)
}

#[derive(Serialize)]
struct ClusterRow {
    requested_members: usize,
    members: usize,
    unique_feeds: usize,
    same_feed_duplicate_members: usize,
    unsummarized_members: usize,
    prompt_chars: usize,
    estimated_tokens: usize,
    prompt: String,
}

fn clusters(corpus: &Corpus, tier: InputTier, metrics: &mut Metrics) -> Result<Vec<ClusterRow>> {
    let mut rows = Vec::new();
    for count in [3, 15, 30] {
        let mut fixture = FixtureDb::new()?;
        let input = corpus
            .articles
            .iter()
            .find(|a| a.id == "en-depth")
            .context("missing cluster body")?;
        for i in 0..count {
            fixture.from_corpus(
                input,
                &format!("cluster-member-{i}"),
                ["cedar", "willow", "fir", "alder"][i % 4],
                10,
                Some(9900),
                i % 3 != 0,
            )?;
        }
        fixture.refresh_clusters()?;
        let members = cluster_summary::visible_members(&fixture.conn, 9900, FeedScope::All)?;
        let input = cluster_summary::build_input(&fixture.conn, &members, tier)?;
        let unique = input
            .members
            .iter()
            .map(|m| m.feed_id)
            .collect::<BTreeSet<_>>()
            .len();
        let row = ClusterRow {
            requested_members: count,
            members: input.members.len(),
            unique_feeds: unique,
            same_feed_duplicate_members: input.members.len().saturating_sub(unique),
            unsummarized_members: input
                .members
                .iter()
                .filter(|m| m.one_liner.is_empty())
                .count(),
            prompt_chars: chars(&input.prompt),
            estimated_tokens: estimate_tokens(&input.prompt),
            prompt: input.prompt,
        };
        for (key, value, direction, unit) in [
            ("members", row.members, Direction::Observe, "articles"),
            ("unique_feeds", row.unique_feeds, Direction::Higher, "feeds"),
            (
                "same_feed_duplicate_members",
                row.same_feed_duplicate_members,
                Direction::Lower,
                "articles",
            ),
            (
                "prompt_chars",
                row.prompt_chars,
                Direction::Observe,
                "chars",
            ),
            (
                "estimated_tokens",
                row.estimated_tokens,
                Direction::Observe,
                "tokens",
            ),
        ] {
            put(
                metrics,
                format!("cluster_summary.{count}.{key}"),
                value as f64,
                unit,
                direction,
            );
        }
        rows.push(row);
    }
    Ok(rows)
}

pub fn run(root: &Path, cli: &cli::Cli, baseline: Option<&Value>) -> Result<()> {
    let corpus = Corpus::load(root)?;
    let (extraction, digests, weekly, auto, clusters, metrics) =
        rrss_core::evaluation_clock::at_time(NOW, || {
            let mut metrics = Metrics::new();
            put(
                &mut metrics,
                "model_requests",
                0.0,
                "requests",
                Direction::Lower,
            );
            let extraction = if cli.sections.contains("summary_extract") {
                extract(&corpus)?
            } else {
                Vec::new()
            };
            extraction_metrics(&extraction, &mut metrics);
            let digests = if cli.sections.contains("digests") {
                digest_rows(&corpus, cli.tier, &mut metrics)?
            } else {
                Vec::new()
            };
            let weekly = if cli.sections.contains("digests") {
                Some(weekly_row(&corpus, cli.tier, &mut metrics)?)
            } else {
                None
            };
            let auto = if cli.sections.contains("auto_summary") {
                Some(auto_summary(&corpus, &mut metrics)?)
            } else {
                None
            };
            let clusters = if cli.sections.contains("cluster_summary") {
                clusters(&corpus, cli.tier, &mut metrics)?
            } else {
                Vec::new()
            };
            Ok::<_, anyhow::Error>((extraction, digests, weekly, auto, clusters, metrics))
        })?;
    let mut result = json!({
        "schema_version":1,"mode":"offline","tier":cli.tier.as_str(),"extraction_tiers":["low","medium","high"],
        "source_revision":metrics::revision(root),"fixture_hash":metrics::fixture_hash(root)?,
        "production_hash":metrics::production_hash(root)?,
        "evaluation_time_ms":NOW,"token_estimator":"CJK=1, ASCII/Latin=0.25, other=0.5; ceil once",
        "sections":cli.sections,"metrics":metrics,"summary_extract":extraction,"digests":digests,
        "weekly":weekly,"auto_summary":auto,"cluster_summary":clusters,
    });
    let mut report = report(&result);
    if let Some(baseline) = baseline {
        let comparison = compare::compare(baseline, &result)?;
        let markdown = comparison.markdown();
        println!("{markdown}");
        report.push_str("\n");
        report.push_str(&markdown);
        result["comparison"] = serde_json::to_value(comparison)?;
    }
    std::fs::create_dir_all(&cli.out)?;
    std::fs::write(
        cli.out.join("offline-results.json"),
        serde_json::to_string_pretty(&result)? + "\n",
    )?;
    std::fs::write(cli.out.join("offline-report.md"), &report)?;
    std::fs::write(
        cli.out.join("offline-report.html"),
        format!(
            "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><title>Offline input evaluation</title><style>{STYLE}</style><main><pre>{}</pre></main></html>\n",
            escape(&report)
        ),
    )?;
    println!(
        "{}",
        report
            .split("\n## Extraction cases")
            .next()
            .unwrap_or(&report)
    );
    eprintln!(
        "offline report: {}",
        cli.out.join("offline-report.md").display()
    );
    Ok(())
}

fn report(result: &Value) -> String {
    let metrics: Metrics =
        serde_json::from_value(result["metrics"].clone()).expect("serialized metrics");
    let value = |key: &str| metrics.get(key).map(|m| m.value).unwrap_or(0.0);
    let mut out = format!(
        "# RRSS deterministic input baseline\n\nProduction revision: {}. Digest/cluster tier: **{}**; extraction tiers: low, medium, high. Model requests: **0**.\n\nFixed evaluation clock: {} ms. Fixture hash: {}.\n\nFact recall is micro-averaged over annotated facts; absent categories are omitted from the metrics map. The token estimate is CJK=1, ASCII/Latin=0.25, other=0.5, rounded up once. Token ratios compare the oversized Chinese and English subsets of this corpus.\n\n",
        result["source_revision"].as_str().unwrap_or("unknown"),
        result["tier"].as_str().unwrap_or("medium"),
        NOW,
        result["fixture_hash"].as_str().unwrap_or("")
    );
    out.push_str("## Extraction summary\n\n| Tier | Summary recall | Body | Table | Code | Lead | Tail | Oversized utilization | zh/en tokens | Split rate | Orphan headings | Link TP / FP / FN |\n| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |\n");
    for tier in ["low", "medium", "high"] {
        if !metrics.contains_key(&format!(
            "summary_extract.{tier}.summary.all.all.fact_recall"
        )) {
            continue;
        }
        let p = format!("summary_extract.{tier}");
        out.push_str(&format!("| {tier} | {:.1}% | {:.1}% | {:.1}% | {:.1}% | {:.1}% | {:.1}% | {:.1}% | {:.2} | {:.1}% | {:.0} | {:.0} / {:.0} / {:.0} |\n",
            value(&format!("{p}.summary.all.all.fact_recall"))*100.0,value(&format!("{p}.summary.all.body.fact_recall"))*100.0,
            value(&format!("{p}.summary.all.table.fact_recall"))*100.0,value(&format!("{p}.summary.all.code.fact_recall"))*100.0,
            value(&format!("{p}.summary.all.lead.fact_recall"))*100.0,value(&format!("{p}.summary.all.tail.fact_recall"))*100.0,
            value(&format!("{p}.summary.all.over_budget_utilization"))*100.0,value(&format!("{p}.summary.zh_en_token_ratio")),
            value(&format!("{p}.segment_trigger_ratio"))*100.0,value(&format!("{p}.orphan_headings")),
            value(&format!("{p}.link_true_positive")),value(&format!("{p}.link_false_positive")),value(&format!("{p}.link_false_negative"))));
    }
    out.push_str("\n## Digest candidates and packing\n\nFixed editorial picks are the ordered candidate prefix; these numbers evaluate backend inputs before any model selection. Raw candidate and representative health are both retained in JSON. Wasted budget is min(short-article unused allocation, omitted long-text characters, remaining total budget); summaries overlapping complete short bodies count as redundancy.\n\n| Scene / period | Candidates | Noisy | Down | Max feed | Feeds | Repeated titles | No-summary evidence | Utilization | Wasted chars | Redundant chars | Timed / multi-report |\n| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |\n");
    for row in result["digests"].as_array().into_iter().flatten() {
        out.push_str(&format!("| {} / {} | {} | {:.1}% | {} | {:.1}% | {} | {} | {:.1}% | {:.1}% | {} | {} | {} / {} |\n",
            row["scene"].as_str().unwrap_or(""),row["period"].as_str().unwrap_or(""),row["candidates"]["count"],
            row["candidates"]["noisy_feed_ratio"].as_f64().unwrap_or(0.0)*100.0,row["candidates"]["disliked_count"],
            row["candidates"]["max_single_feed_share"].as_f64().unwrap_or(0.0)*100.0,row["candidates"]["unique_feeds"],
            row["catalog"]["duplicate_title_items"],row["catalog"]["unsummarized_evidence_ratio"].as_f64().unwrap_or(0.0)*100.0,
            row["efficiency"]["utilization"].as_f64().unwrap_or(0.0)*100.0,row["efficiency"]["wasted_budget_chars"],
            row["efficiency"]["redundant_summary_chars"],row["efficiency"]["members_with_publication_time"],row["efficiency"]["multi_report_members"]));
    }
    if !result["weekly"].is_null() {
        let w = &result["weekly"];
        out.push_str(&format!("\n## Full weekly input\n\n{} daily lines, {} source articles, {} supplement candidates; **{} same-cluster collisions**.\n\n| Portion | Characters | Estimated tokens |\n| --- | ---: | ---: |\n",
            w["daily_lines"],w["source_articles"],w["supplements"]["count"],w["cluster_collision_count"]));
        for (label, c, t) in [
            ("Daily items", "daily_chars", "daily_tokens"),
            ("Source summaries", "sources_chars", "sources_tokens"),
            (
                "Supplement excerpts",
                "supplements_chars",
                "supplements_tokens",
            ),
            ("Total (including framing)", "total_chars", "total_tokens"),
        ] {
            out.push_str(&format!(
                "| {label} | {} | {} |\n",
                w["parts"][c], w["parts"][t]
            ));
        }
    }
    if !result["auto_summary"].is_null() {
        let a = &result["auto_summary"];
        out.push_str(&format!("\n## Automatic summary batch\n\n{} selected: **{} read, {} downvoted, {} from noisy feeds, {} link-only**.\n",
            a["count"],a["read_count"],a["disliked_count"],a["noisy_feed_count"],a["link_stub_count"]));
    }
    if result["cluster_summary"]
        .as_array()
        .is_some_and(|rows| !rows.is_empty())
    {
        out.push_str("\n## Related coverage inputs\n\n| Members | Unique feeds | Same-feed duplicate members | No summary | Characters | Estimated tokens |\n| ---: | ---: | ---: | ---: | ---: | ---: |\n");
        for row in result["cluster_summary"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} |\n",
                row["members"],
                row["unique_feeds"],
                row["same_feed_duplicate_members"],
                row["unsummarized_members"],
                row["prompt_chars"],
                row["estimated_tokens"]
            ));
        }
    }
    out.push_str("\n## Extraction cases\n\nEach JSON row retains the exact summary payload, body excerpt and segments, with fact-level hits. No production behavior is changed by the evaluator.\n\n| Case | Language | Tier | Summary facts | Body facts | Segment facts | Segment lengths | Orphans |\n| --- | --- | --- | --- | --- | --- | --- | ---: |\n");
    for row in result["summary_extract"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "| {} | {} | {} | {} / {} | {} / {} | {} | {} | {} |\n",
            row["id"].as_str().unwrap_or(""),
            row["language"].as_str().unwrap_or(""),
            row["tier"].as_str().unwrap_or(""),
            row["summary"]["facts_hit"],
            row["summary"]["facts_total"],
            row["excerpt"]["facts_hit"],
            row["excerpt"]["facts_total"],
            if row["segmented"].is_null() {
                "—".into()
            } else {
                format!(
                    "{} / {}",
                    row["segmented"]["facts_hit"], row["segmented"]["facts_total"]
                )
            },
            row["segment_chars"],
            row["orphan_headings"]
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use corpus::Category;

    #[test]
    fn recall_is_micro_averaged_and_budget_uses_unicode_characters() {
        let facts = vec![
            Fact {
                text: "73".into(),
                category: Category::Lead,
                alternatives: vec![],
            },
            Fact {
                text: "尾".into(),
                category: Category::Tail,
                alternatives: vec![],
            },
        ];
        let p = payload("中文73".into(), 10, 8, &facts);
        assert_eq!(p.actual_chars, 4);
        assert_eq!(p.fact_recall, 0.5);
        assert_eq!(p.utilization, 0.5);
        assert!(p.over_budget);
    }

    #[test]
    fn aggregate_recall_weights_each_fact_instead_of_each_article() {
        let make = |id: &str, facts: Vec<Fact>| Extraction {
            id: id.into(),
            language: "en".into(),
            tier: "low".into(),
            link_stub_expected: false,
            link_stub_detected: false,
            near_budget: false,
            summary: payload("73".into(), 10, 8, &facts),
            excerpt: payload("73".into(), 10, 8, &facts),
            segmented: None,
            segment_count: 0,
            segment_chars: vec![],
            orphan_headings: 0,
            segments: vec![],
        };
        let fact = |text: &str| Fact {
            text: text.into(),
            category: Category::Body,
            alternatives: vec![],
        };
        let rows = [
            make("one", vec![fact("73")]),
            make("three", vec![fact("73"), fact("99"), fact("88")]),
        ];
        let mut metrics = Metrics::new();
        extraction_metrics(&rows, &mut metrics);
        assert_eq!(
            metrics["summary_extract.low.summary.all.body.fact_recall"].value,
            0.5
        );
        assert!(!metrics.contains_key("summary_extract.low.summary.all.code.fact_recall"));
    }

    #[test]
    fn weekly_portions_exclude_framing_and_count_unicode_correctly() {
        let prompt = "intro\n\n## Daily digest items\n\n中文\n\n## Source articles\n\nabcd\n\n## Supplementary articles (not in the daily digests)\n\n正文";
        let p = weekly_parts(prompt);
        assert_eq!(
            (p.daily_chars, p.sources_chars, p.supplements_chars),
            (2, 4, 2)
        );
        assert_eq!(
            (p.daily_tokens, p.sources_tokens, p.supplements_tokens),
            (2, 1, 2)
        );
        assert_eq!(
            p.framing_chars + p.daily_chars + p.sources_chars + p.supplements_chars,
            p.total_chars
        );
    }

    #[test]
    fn budget_waste_requires_short_unused_space_and_truncated_long_text() {
        rrss_core::evaluation_clock::at_time(NOW, || {
            let mut fixture = FixtureDb::new().unwrap();
            fixture
                .insert(&fixtures::simple(
                    "short",
                    "cedar",
                    "The audit saved 46 hours.".into(),
                    10,
                    true,
                ))
                .unwrap();
            fixture
                .insert(&fixtures::simple(
                    "long",
                    "willow",
                    "The longer audit retains every device record. ".repeat(300),
                    10,
                    false,
                ))
                .unwrap();
            let ids: Vec<_> = fixture.ids.values().copied().collect();
            let items = ai_store::digest_items_by_ids(&fixture.conn, &ids).unwrap();
            let deep = ai_store::format_digest_deep_input(&fixture.conn, &items, InputTier::Medium)
                .unwrap();
            let e = efficiency(&fixture, &deep, InputTier::Medium).unwrap();
            assert!(e.wasted_budget_chars > 0);
            assert!(e.redundant_summary_chars > 0);
            let deep =
                ai_store::format_digest_deep_input(&fixture.conn, &items[..1], InputTier::Medium)
                    .unwrap();
            assert_eq!(
                efficiency(&fixture, &deep, InputTier::Medium)
                    .unwrap()
                    .wasted_budget_chars,
                0
            );
        });
    }
}
