use super::*;
use corpus::{Corpus, Fact};
use fixtures::{NOW, WeeklyCase};
use metrics::{Direction, Metrics, put, ratio};
use rrss_core::digest::DigestDeepInput;

impl Prompts {
    /// The legacy snapshot covers the original tasks only. New sections use
    /// current production templates in baseline's Chinese output language too.
    pub fn evaluation_system(&self, slug: &str, task: &str) -> Result<String> {
        if self.variant == Variant::Baseline && !self.baseline.templates.contains_key(slug) {
            let body = self
                .english
                .get(slug)
                .with_context(|| format!("no template {slug}"))?;
            return Ok(if rrss_app::ai_language::applies(task) {
                rrss_app::ai_language::with_directive(body, self.variant.language())
            } else {
                body.clone()
            });
        }
        self.system(slug, task)
    }
}

fn string_array(value: &Value) -> bool {
    value.as_array().is_some_and(|items| {
        items
            .iter()
            .all(|v| v.as_str().is_some_and(|s| !s.trim().is_empty()))
    })
}

pub fn summary_fields(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == 4
            && ["summary", "one_liner"].iter().all(|key| {
                object
                    .get(*key)
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.trim().is_empty())
            })
            && ["key_points", "facts"]
                .iter()
                .all(|key| object.get(*key).is_some_and(string_array))
    })
}

pub fn segment_fields(value: &Value) -> bool {
    value.as_object().is_some_and(|object| object.len() == 2)
        && value["notes"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
        && string_array(&value["facts"])
        && value["facts"]
            .as_array()
            .is_some_and(|facts| facts.len() <= 5)
}

fn summary_text(value: &Value) -> String {
    let mut text = value["summary"].as_str().unwrap_or("").to_string();
    // one_liner is deliberately excluded from the requested fact-recall score.
    for key in ["key_points", "facts"] {
        for fact in value[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            text.push('\n');
            text.push_str(fact);
        }
    }
    text
}

pub fn segment_leaks(text: &str) -> Vec<&'static str> {
    let lower = text.to_lowercase();
    [
        "[part ",
        "(part ",
        "part 1",
        "第一部分",
        "第1部分",
        "第二部分",
        "第2部分",
        "notes on the full text",
        "parts in the original order",
    ]
    .into_iter()
    .filter(|marker| lower.contains(marker))
    .collect()
}

fn article_output(
    calls: Vec<Call>,
    facts: &[Fact],
    language: OutputLanguage,
    elapsed: u128,
    segments: usize,
) -> Output {
    let final_call = calls.last().expect("article path has a request");
    let mut metrics = metrics::call_metrics(&calls);
    let service_ms = metrics["latency_ms"].value;
    put(
        &mut metrics,
        "service_latency_ms",
        service_ms,
        "ms",
        Direction::Lower,
    );
    put(
        &mut metrics,
        "latency_ms",
        elapsed as f64,
        "ms",
        Direction::Lower,
    );
    put(
        &mut metrics,
        "segments",
        segments as f64,
        "items",
        Direction::Observe,
    );
    let failed: Vec<_> = calls.iter().filter_map(|c| c.error.as_ref()).collect();
    if !failed.is_empty() {
        put(&mut metrics, "fact_recall", 0.0, "ratio", Direction::Higher);
        put(&mut metrics, "facts_hit", 0.0, "facts", Direction::Higher);
        put(
            &mut metrics,
            "facts_total",
            facts.len() as f64,
            "facts",
            Direction::Observe,
        );
        return Output {
            shown: String::new(),
            checks: vec![check(
                "request",
                false,
                failed
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("; "),
            )],
            calls,
            metrics,
        };
    }
    let p = &final_call.parsed;
    let text = summary_text(p);
    let all_fields = format!("{text}\n{}", p["one_liner"].as_str().unwrap_or(""));
    let (hits, total) = corpus::fact_counts(facts, &text);
    put(
        &mut metrics,
        "facts_hit",
        hits as f64,
        "facts",
        Direction::Higher,
    );
    put(
        &mut metrics,
        "facts_total",
        total as f64,
        "facts",
        Direction::Observe,
    );
    put(
        &mut metrics,
        "fact_recall",
        ratio(hits, total),
        "ratio",
        Direction::Higher,
    );
    let mut checks = vec![
        check(
            "fields",
            summary_fields(p),
            "summary, one_liner and string arrays; no extra fields",
        ),
        check(
            "limits",
            p["key_points"].as_array().is_some_and(|a| a.len() <= 3)
                && p["facts"].as_array().is_some_and(|a| a.len() <= 5),
            "at most 3 points and 5 facts",
        ),
        check(
            "length",
            (4..=8).contains(&sentences(p["summary"].as_str().unwrap_or(""))),
            format!(
                "{} sentences",
                sentences(p["summary"].as_str().unwrap_or(""))
            ),
        ),
        check(
            "must_keep_facts",
            hits == total,
            format!(
                "{hits}/{total} facts; missed: {}",
                facts
                    .iter()
                    .filter(|f| !f.hit(&text))
                    .map(|f| f.text.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
        language_check(&all_fields, language),
        leak_check(&all_fields),
        check(
            "no segment markers",
            segment_leaks(&all_fields).is_empty(),
            segment_leaks(&all_fields).join(", "),
        ),
    ];
    for (i, call) in calls.iter().take(segments).enumerate() {
        checks.push(check(
            "segment fields",
            segment_fields(&call.parsed),
            format!("part {}", i + 1),
        ));
    }
    let shown = format!(
        "{}\n\nOne-liner: {}",
        text,
        p["one_liner"].as_str().unwrap_or("")
    );
    Output {
        shown,
        checks,
        calls,
        metrics,
    }
}

pub async fn article_summaries(
    prompts: &Prompts,
    model: &Arc<Model>,
    corpus: &Corpus,
    keys: &[String],
    tier: InputTier,
) -> Result<Vec<Case>> {
    let system = prompts.evaluation_system(
        ai_store::SLUG_ARTICLE_SUMMARY,
        ai_store::TASK_ARTICLE_SUMMARY,
    )?;
    let segment_system = prompts.evaluation_system(
        ai_store::SLUG_ARTICLE_SEGMENT,
        ai_store::TASK_ARTICLE_SEGMENT,
    )?;
    let mut out = Vec::new();
    for key in keys {
        let case = corpus
            .articles
            .iter()
            .find(|c| c.id == *key)
            .with_context(|| format!("unknown manual-summary case {key}"))?;
        let prepared = case.prepare()?;
        let input = summary_extract::extract_summary_payload(
            &case.title,
            Some(&prepared.blocks),
            prepared.html.as_deref(),
            false,
            tier,
        );
        let started = Instant::now();
        let call = model
            .call(
                prompts.variant.id(),
                system.clone(),
                prompts.user(&input),
                rrss_ai::summary_json_schema(),
            )
            .await;
        out.push(Case {
            section: "article_summaries",
            key: format!("{key}/single"),
            title: format!("{} · single excerpt", case.title),
            input,
            outputs: BTreeMap::from([(
                prompts.variant.id(),
                article_output(
                    vec![call],
                    &case.must_keep_facts,
                    prompts.variant.language(),
                    started.elapsed().as_millis(),
                    0,
                ),
            )]),
        });
        // Match the production manual-summary policy: low never adds compression calls.
        let segments = (tier != InputTier::Low)
            .then(|| {
                summary_extract::split_segments(
                    Some(&prepared.blocks),
                    prepared.html.as_deref(),
                    tier.summary_budget(false),
                )
            })
            .flatten();
        let Some(segments) = segments else {
            continue;
        };
        let started = Instant::now();
        let count = segments.len();
        let mut handles = Vec::new();
        for (i, segment) in segments.into_iter().enumerate() {
            let model = model.clone();
            let user = prompts.user(&format!(
                "{}\n\n(part {} of {count})\n\n{segment}",
                case.title,
                i + 1
            ));
            let system = segment_system.clone();
            let tag = prompts.variant.id();
            handles.push(tokio::spawn(async move {
                model
                    .call(tag, system, user, rrss_ai::segment_json_schema())
                    .await
            }));
        }
        let mut calls = Vec::new();
        for handle in handles {
            calls.push(handle.await?);
        }
        let valid = calls
            .iter()
            .all(|call| call.error.is_none() && segment_fields(&call.parsed));
        let notes = calls
            .iter()
            .map(|call| call.parsed.clone())
            .collect::<Vec<_>>();
        let input = rrss_app::jobs::summaries::format_segment_notes(&case.title, &notes);
        let output = if valid {
            calls.push(
                model
                    .call(
                        prompts.variant.id(),
                        system.clone(),
                        prompts.user(&input),
                        rrss_ai::summary_json_schema(),
                    )
                    .await,
            );
            article_output(
                calls,
                &case.must_keep_facts,
                prompts.variant.language(),
                started.elapsed().as_millis(),
                count,
            )
        } else {
            let mut metrics = metrics::call_metrics(&calls);
            put(&mut metrics, "fact_recall", 0.0, "ratio", Direction::Higher);
            put(&mut metrics, "facts_hit", 0.0, "facts", Direction::Higher);
            put(
                &mut metrics,
                "facts_total",
                case.must_keep_facts.len() as f64,
                "facts",
                Direction::Observe,
            );
            put(
                &mut metrics,
                "latency_ms",
                started.elapsed().as_millis() as f64,
                "ms",
                Direction::Lower,
            );
            Output {
                shown: String::new(),
                checks: vec![check(
                    "segment fields",
                    false,
                    "compression failed or returned invalid notes; final summary request was skipped",
                )],
                calls,
                metrics,
            }
        };
        out.push(Case {
            section: "article_summaries",
            key: format!("{key}/segmented"),
            title: format!("{} · segment compression", case.title),
            input,
            outputs: BTreeMap::from([(prompts.variant.id(), output)]),
        });
    }
    Ok(out)
}

pub fn pick_fields(parsed: &Value, allowed: &[DigestItem], max: usize) -> bool {
    parsed.as_object().is_some_and(|object| object.len() == 1)
        && parsed["picks"].as_array().is_some_and(|picks| {
            let mut seen = BTreeSet::new();
            picks.len() <= max
                && picks.iter().all(|pick| {
                    pick.as_object().is_some_and(|o| o.len() == 2)
                        && pick["reason"].as_str().is_some()
                        && pick["article_id"].as_i64().is_some_and(|id| {
                            seen.insert(id) && allowed.iter().any(|item| item.article_id == id)
                        })
                })
        })
}

pub fn digest_fields(parsed: &Value, numbered: &[DigestItem]) -> bool {
    parsed.as_object().is_some_and(|object| object.len() == 1)
        && parsed["sections"].as_array().is_some_and(|sections| {
            !sections.is_empty()
                && sections.iter().all(|section| {
                    section.as_object().is_some_and(|o| o.len() == 2)
                        && section["title"]
                            .as_str()
                            .is_some_and(|s| !s.trim().is_empty())
                        && section["items"].as_array().is_some_and(|items| {
                            !items.is_empty()
                                && items.iter().all(|item| {
                                    item.as_object().is_some_and(|o| o.len() == 2)
                                        && item["text"]
                                            .as_str()
                                            .is_some_and(|s| !s.trim().is_empty())
                                        && item["refs"].as_array().is_some_and(|refs| {
                                            (1..=3).contains(&refs.len())
                                                && refs.iter().all(|r| {
                                                    r.as_u64().is_some_and(|n| {
                                                        n > 0 && n as usize <= numbered.len()
                                                    })
                                                })
                                        })
                                })
                        })
                })
        })
}

pub struct DigestAssessment {
    pub shown: String,
    pub checks: Vec<Check>,
    pub metrics: Metrics,
    pub sources: Vec<BTreeSet<i64>>,
    pub raw_text: String,
}

pub fn assess_digest(
    call: &Call,
    numbered: &[DigestItem],
    language: OutputLanguage,
    min: usize,
    max: usize,
) -> DigestAssessment {
    let mut out = DigestAssessment {
        shown: String::new(),
        checks: Vec::new(),
        metrics: Metrics::new(),
        sources: Vec::new(),
        raw_text: String::new(),
    };
    if let Some(error) = &call.error {
        out.checks.push(check("request", false, error));
        return out;
    }
    let raw_texts: Vec<_> = call.parsed["sections"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|section| section["items"].as_array().into_iter().flatten())
        .filter_map(|item| item["text"].as_str())
        .collect();
    out.raw_text = raw_texts.join("\n");
    out.checks.push(check(
        "JSON fields",
        digest_fields(&call.parsed, numbered),
        "sections, text and 1–3 valid local reference numbers",
    ));
    match ai_store::prepare_digest_draft(&call.parsed, numbered, language) {
        Err(error) => out
            .checks
            .push(check("structure", false, error.to_string())),
        Ok(draft) => {
            let dropped = raw_texts
                .iter()
                .filter(|text| !draft.body.contains(text.trim()))
                .count();
            let doc = rrss_render::markdown_to_document(&draft.body);
            let decoded = rrss_core::digest_sources::decode_digest_sources(
                &draft.body,
                &draft.items_json,
                &doc,
            );
            let raw: Value = serde_json::from_str(&draft.items_json).unwrap_or(Value::Null);
            out.sources = decoded
                .lines
                .iter()
                .map(|refs| refs.iter().map(|r| r.article_id).collect())
                .collect();
            let count = out.sources.len();
            let unsourced = out.sources.iter().filter(|refs| refs.is_empty()).count();
            let allowed: BTreeSet<_> = numbered.iter().map(|item| item.article_id).collect();
            let invalid = out
                .sources
                .iter()
                .flatten()
                .filter(|id| !allowed.contains(id))
                .count();
            out.checks.extend([
                check(
                    "structure",
                    raw["v"] == 2 && decoded.state == rrss_core::digest_sources::SourceState::V2,
                    "verified v2 source package and body hash",
                ),
                check(
                    "sources",
                    unsourced == 0 && invalid == 0,
                    format!("{unsourced} unsourced; {invalid} invalid sources"),
                ),
                check(
                    "item range",
                    (min..=max).contains(&count),
                    format!("{count} items; expected {min}–{max}"),
                ),
                language_check(&draft.body, language),
                leak_check(&out.raw_text),
                leak_check(&draft.body),
                check(
                    "cleanup",
                    true,
                    format!("{dropped} input items changed or removed"),
                ),
            ]);
            put(
                &mut out.metrics,
                "items_count",
                count as f64,
                "items",
                Direction::Observe,
            );
            put(
                &mut out.metrics,
                "unsourced_count",
                unsourced as f64,
                "items",
                Direction::Lower,
            );
            put(
                &mut out.metrics,
                "invalid_source_count",
                invalid as f64,
                "items",
                Direction::Lower,
            );
            put(
                &mut out.metrics,
                "source_completeness",
                ratio(count - unsourced, count),
                "ratio",
                Direction::Higher,
            );
            out.shown = draft.body;
        }
    }
    out
}

fn link_only(conn: &rusqlite::Connection, id: i64) -> Result<bool> {
    let (blocks, html): (Option<String>, Option<String>) = conn.query_row(
        "SELECT blocks_json,content_html FROM articles WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let excerpt = summary_extract::extract_body_excerpt(blocks.as_deref(), html.as_deref(), 200);
    Ok(excerpt.trim().is_empty() || excerpt.contains(summary_extract::LINK_STUB_NOTE))
}

pub fn daily_output(
    pick: Call,
    write: Call,
    deep: &DigestDeepInput,
    items: &[DigestItem],
    selected: &[DigestItem],
    digest: &DigestCase,
    ids: &HashMap<String, i64>,
    conn: &rusqlite::Connection,
    language: OutputLanguage,
    elapsed: u128,
) -> Result<Output> {
    let mut assessment = assess_digest(
        &write,
        &deep.numbered_items,
        language,
        digest.pick_min,
        digest.pick_max,
    );
    assessment.checks.insert(
        0,
        check(
            "picks",
            pick.error.is_none() && pick_fields(&pick.parsed, items, digest.pick_max),
            format!("{} selected of {} candidates", selected.len(), items.len()),
        ),
    );
    let cited: BTreeSet<_> = assessment.sources.iter().flatten().copied().collect();
    let event_hits = digest
        .high_value_events
        .iter()
        .filter(|event| {
            event
                .iter()
                .filter_map(|key| ids.get(key))
                .any(|id| cited.contains(id))
        })
        .count();
    let avoided: BTreeSet<i64> = digest
        .avoid_articles
        .iter()
        .filter_map(|key| ids.get(key))
        .copied()
        .collect();
    let mut low_quality = avoided;
    for item in items {
        if link_only(conn, item.article_id)? {
            low_quality.insert(item.article_id);
        }
    }
    let bad_picks = selected
        .iter()
        .filter(|item| low_quality.contains(&item.article_id))
        .count();
    let bad_refs = cited.intersection(&low_quality).count();
    if !digest.high_value_events.is_empty() {
        assessment.checks.push(check(
            "core events",
            event_hits >= digest.min_high_value_events,
            format!(
                "{event_hits}/{} high-value events",
                digest.high_value_events.len()
            ),
        ));
    }
    assessment.checks.push(check(
        "low quality",
        bad_picks == 0 && bad_refs == 0,
        format!("{bad_picks} selected; {bad_refs} cited link-only or low-quality articles"),
    ));
    for (key, value, unit, direction) in [
        (
            "core_event_hits",
            event_hits as f64,
            "events",
            Direction::Higher,
        ),
        (
            "core_event_recall",
            ratio(event_hits, digest.high_value_events.len()),
            "ratio",
            Direction::Higher,
        ),
        (
            "low_quality_selected",
            bad_picks as f64,
            "articles",
            Direction::Lower,
        ),
        (
            "low_quality_cited",
            bad_refs as f64,
            "articles",
            Direction::Lower,
        ),
        ("latency_ms", elapsed as f64, "ms", Direction::Lower),
    ] {
        put(&mut assessment.metrics, key, value, unit, direction);
    }
    Ok(Output {
        shown: assessment.shown,
        checks: assessment.checks,
        calls: vec![pick, write],
        metrics: assessment.metrics,
    })
}

pub fn pipeline_cost(output: &mut Output, prerequisites: &[Call]) {
    let direct = metrics::call_metrics(&output.calls);
    let before = metrics::call_metrics(prerequisites);
    for key in [
        "requests",
        "prompt_tokens",
        "completion_tokens",
        "latency_ms",
    ] {
        let unit = direct[key].unit.clone();
        put(
            &mut output.metrics,
            format!("pre_summary_{key}"),
            before[key].value,
            &unit,
            Direction::Lower,
        );
        put(
            &mut output.metrics,
            format!("pipeline_{key}"),
            direct[key].value + before[key].value,
            &unit,
            Direction::Lower,
        );
    }
}

pub fn weekly_leaks(text: &str) -> Vec<&'static str> {
    let lower = text.to_lowercase();
    [
        "周一",
        "周二",
        "周三",
        "周四",
        "周五",
        "周六",
        "周日",
        "日报称",
        "日报指出",
        "日报提到",
        "daily digest",
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
        "## source articles",
        "supplementary articles",
        "this week's daily",
    ]
    .into_iter()
    .filter(|word| lower.contains(word))
    .collect()
}

pub fn cross_day_metrics(sources: &[BTreeSet<i64>], target: &BTreeSet<i64>) -> (usize, usize) {
    let touched = sources
        .iter()
        .filter(|refs| !refs.is_disjoint(target))
        .count();
    let hits = sources
        .iter()
        .flat_map(|refs| refs.intersection(target))
        .copied()
        .collect::<BTreeSet<_>>()
        .len();
    (touched, hits)
}

pub async fn weekly_digests(
    prompts: &Prompts,
    model: &Model,
    cases: &[WeeklyCase],
    tier: InputTier,
) -> Result<Vec<Case>> {
    let mut out = Vec::new();
    for case in cases {
        let started = Instant::now();
        let fixture = rrss_core::evaluation_clock::at_time(NOW, || fixtures::weekly(case))?;
        let daily = ai_store::week_daily_lines(&fixture.conn, NOW - 7 * fixtures::DAY, NOW)?;
        let mut window =
            ai_store::DigestWindow::for_period("weekly", NOW, NOW, fixtures::SUPPLEMENT_CANDIDATES);
        if !daily.is_empty() {
            window.exclude_daily_before = Some(i64::MAX);
        }
        let candidates =
            ai_store::digest_candidates(&fixture.conn, fixtures::SUPPLEMENT_CANDIDATES, &window)?;
        let kept = fixtures::representatives(
            &fixture.conn,
            &candidates,
            fixtures::SUPPLEMENT_CANDIDATES as usize,
        )?;
        let noisy = rrss_core::evaluation_clock::at_time(NOW, || {
            rrss_core::feeds::noisy_feed_titles(&fixture.conn)
        })?;
        let shallow = ai_store::format_digest_input(&kept, &noisy, fixtures::SUPPLEMENT_MAX);
        let pick = model
            .call(
                prompts.variant.id(),
                prompts.evaluation_system(
                    ai_store::SLUG_WEEKLY_DIGEST_PICK,
                    ai_store::TASK_DIGEST_PICK_WEEKLY,
                )?,
                prompts.user(&shallow),
                rrss_ai::digest_pick_json_schema(),
            )
            .await;
        let picks = ai_store::parse_digest_picks(&pick.parsed);
        let selected = ai_store::apply_digest_picks(&kept, &picks, fixtures::SUPPLEMENT_MAX, 0);
        let deep = ai_store::format_weekly_input(&fixture.conn, &daily, &selected, tier)?;
        let write = model
            .call(
                prompts.variant.id(),
                prompts.evaluation_system(
                    ai_store::SLUG_WEEKLY_DIGEST,
                    ai_store::TASK_DIGEST_WEEKLY,
                )?,
                prompts.user(&deep.prompt),
                rrss_ai::digest_json_schema(),
            )
            .await;
        let mut assessed = assess_digest(
            &write,
            &deep.numbered_items,
            prompts.variant.language(),
            case.item_min,
            case.item_max,
        );
        assessed.checks.insert(
            0,
            check(
                "picks",
                pick.error.is_none() && pick_fields(&pick.parsed, &kept, fixtures::SUPPLEMENT_MAX),
                format!(
                    "{} supplement picks from {} candidates",
                    selected.len(),
                    kept.len()
                ),
            ),
        );
        let target = case
            .cross_day_articles
            .iter()
            .map(|key| fixture.id(key))
            .collect::<Result<BTreeSet<_>>>()?;
        let (target_items, target_hits) = cross_day_metrics(&assessed.sources, &target);
        assessed.checks.push(check(
            "cross-day merge",
            target_items == 1 && target_hits >= 2,
            format!(
                "{target_hits}/{} target sources in {target_items} items",
                target.len()
            ),
        ));
        let cited: BTreeSet<_> = assessed.sources.iter().flatten().copied().collect();
        let high: BTreeSet<_> = case
            .high_value_supplements
            .iter()
            .map(|key| fixture.id(key))
            .collect::<Result<_>>()?;
        let avoid: BTreeSet<_> = case
            .avoid_articles
            .iter()
            .map(|key| fixture.id(key))
            .collect::<Result<_>>()?;
        let high_hits = cited.intersection(&high).count();
        let bad_picks = selected
            .iter()
            .filter(|item| avoid.contains(&item.article_id))
            .count();
        assessed.checks.extend([
            check(
                "high-value supplements",
                high_hits == high.len(),
                format!("{high_hits}/{} high-value supplements", high.len()),
            ),
            check(
                "low-quality supplements",
                bad_picks == 0,
                format!("{bad_picks} reprint, poll or link-only supplements"),
            ),
            check(
                "no weekly structure words",
                weekly_leaks(&assessed.raw_text).is_empty()
                    && weekly_leaks(&assessed.shown).is_empty(),
                weekly_leaks(&assessed.raw_text).join(", "),
            ),
        ]);
        let parts = offline::weekly_parts(&deep.prompt);
        let collisions = offline::cluster_collisions(&fixture.conn, &daily, &kept)?;
        for (key, value, unit, direction) in [
            (
                "cross_day_event_items",
                target_items as f64,
                "items",
                Direction::Observe,
            ),
            (
                "cross_day_source_hits",
                target_hits as f64,
                "articles",
                Direction::Higher,
            ),
            (
                "high_value_supplement_hits",
                high_hits as f64,
                "articles",
                Direction::Higher,
            ),
            (
                "low_quality_selected",
                bad_picks as f64,
                "articles",
                Direction::Lower,
            ),
            (
                "cluster_collision_count",
                collisions.len() as f64,
                "articles",
                Direction::Lower,
            ),
            (
                "daily_chars",
                parts.daily_chars as f64,
                "chars",
                Direction::Observe,
            ),
            (
                "sources_chars",
                parts.sources_chars as f64,
                "chars",
                Direction::Observe,
            ),
            (
                "supplements_chars",
                parts.supplements_chars as f64,
                "chars",
                Direction::Observe,
            ),
            (
                "input_estimated_tokens",
                parts.total_tokens as f64,
                "tokens",
                Direction::Observe,
            ),
            (
                "latency_ms",
                started.elapsed().as_millis() as f64,
                "ms",
                Direction::Lower,
            ),
        ] {
            put(&mut assessed.metrics, key, value, unit, direction);
        }
        out.push(Case {
            section: "weekly_digests",
            key: case.name.clone(),
            title: case.name.clone(),
            input: format!(
                "Pick input:\n\n{shallow}\n\nWrite input:\n\n{}",
                deep.prompt
            ),
            outputs: BTreeMap::from([(
                prompts.variant.id(),
                Output {
                    shown: assessed.shown,
                    checks: assessed.checks,
                    calls: vec![pick, write],
                    metrics: assessed.metrics,
                },
            )]),
        });
    }
    Ok(out)
}

pub fn markdown(cases: &[Case], tier: InputTier, model: &str, revision: &str) -> String {
    let mut out = format!(
        "# RRSS online input/output evaluation\n\nModel: {model}; tier: {}; source revision: {revision}.\n\nLegacy baseline templates cover the original sections. For new weekly and manual-summary tasks, baseline uses the current production template with Chinese output.\n\nRequest usage includes actual model requests. Per-digest pipeline costs add its prerequisite summaries, which may be shared by other report rows. Latency is measured per path; summed service latency is separate for concurrent compression calls.\n\n| Section / case | Variant | Checks | Facts / core events | Requests | Prompt tokens | Completion tokens | Latency ms | Pipeline prompt tokens |\n| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |\n",
        tier.as_str()
    );
    for case in cases {
        for (variant, output) in &case.outputs {
            let calls = metrics::call_metrics(&output.calls);
            let get = |key: &str| {
                output
                    .metrics
                    .get(key)
                    .or_else(|| calls.get(key))
                    .map(|m| format!("{:.3}", m.value))
                    .unwrap_or_else(|| "—".into())
            };
            out.push_str(&format!(
                "| {} / {} | {} | {} / {} | {} | {} | {} | {} | {} | {} |\n",
                case.section,
                case.key.replace('|', "\\|"),
                variant,
                output.checks.iter().filter(|c| c.ok).count(),
                output.checks.len(),
                if output.metrics.contains_key("fact_recall") {
                    get("fact_recall")
                } else {
                    get("core_event_hits")
                },
                get("requests"),
                get("prompt_tokens"),
                get("completion_tokens"),
                get("latency_ms"),
                get("pipeline_prompt_tokens")
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use fixtures::FixtureDb;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_string_contains, method, path},
    };

    fn item(id: i64) -> DigestItem {
        DigestItem {
            article_id: id,
            title: format!("Alder report {id}"),
            summary: "A verified change.".into(),
            summary_json: None,
            cluster_id: Some(1),
            feed_title: "Cedar Signal".into(),
        }
    }

    #[test]
    fn parsers_reject_malformed_types_and_out_of_range_sources() {
        assert!(!summary_fields(
            &json!({"summary":"a","one_liner":"b","key_points":{},"facts":[]})
        ));
        assert!(!segment_fields(&json!({"notes":"ok","facts":[17]})));
        assert!(segment_fields(
            &json!({"notes":"Verified data.","facts":[]})
        ));
        let numbered = vec![item(10)];
        assert!(digest_fields(
            &json!({"sections":[{"title":"Updates","items":[{"text":"Alder changed.","refs":[1]}]}]}),
            &numbered
        ));
        assert!(!digest_fields(
            &json!({"sections":[{"title":"Updates","items":[{"text":"Alder changed.","refs":[2]}]}]}),
            &numbered
        ));
        assert!(!pick_fields(
            &json!({"picks":[{"article_id":99,"reason":"bad"}]}),
            &numbered,
            5
        ));
    }

    #[test]
    fn source_validation_sees_unsourced_items_and_cross_day_splits() {
        let numbered = vec![item(10), item(11), item(12)];
        let call = Call {
            parsed: json!({"sections":[{"title":"Updates","items":[
                {"text":"Alder began the trial.","refs":[1,2]},{"text":"Willow completed verification.","refs":[]}
            ]}]}),
            ..Call::default()
        };
        let assessed = assess_digest(&call, &numbered, OutputLanguage::En, 1, 5);
        assert_eq!(assessed.metrics["unsourced_count"].value, 1.0);
        assert!(assessed.checks.iter().any(|c| c.name == "sources" && !c.ok));
        let target = BTreeSet::from([10, 11]);
        assert_eq!(cross_day_metrics(&assessed.sources, &target), (1, 2));
        assert_eq!(
            cross_day_metrics(&[BTreeSet::from([10]), BTreeSet::from([11])], &target),
            (2, 2)
        );
        assert!(!weekly_leaks("According to the daily digest, Monday brought a fix.").is_empty());
        assert!(!segment_leaks("[Part 1] The rollout began.").is_empty());
    }

    #[test]
    fn one_liner_is_excluded_from_recall_but_included_in_leak_checks() {
        let fact = Fact {
            text: "142 ms".into(),
            category: corpus::Category::Body,
            alternatives: vec![],
        };
        let call = Call {
            parsed: json!({"summary":"Alder finished verification. The operators checked the records. A repair was accepted. Recovery remains under observation.",
            "one_liner":"[Part 1] Latency reached 142 ms.","key_points":[],"facts":[]}),
            requests: 1,
            ..Call::default()
        };
        let output = article_output(vec![call], &[fact], OutputLanguage::En, 1, 0);
        assert_eq!(output.metrics["fact_recall"].value, 0.0);
        assert!(
            output
                .checks
                .iter()
                .any(|check| check.name == "no segment markers" && !check.ok)
        );
    }

    #[tokio::test]
    async fn cold_section_skips_all_prerequisite_model_calls() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut cases: Cases = serde_json::from_str(
            &std::fs::read_to_string(root.join("testdata/eval/cases.json")).unwrap(),
        )
        .unwrap();
        cases.summaries.clear();
        cases
            .articles
            .retain(|article| ["wren-en", "huichuan-zh"].contains(&article.key.as_str()));
        cases.digests = vec![DigestCase {
            name: "Cold fixture".into(),
            pick_max: 2,
            pick_min: 1,
            articles: vec!["wren-en".into(), "huichuan-zh".into()],
            high_value_events: vec![vec!["wren-en".into()], vec!["huichuan-zh".into()]],
            min_high_value_events: 2,
            avoid_articles: vec![],
        }];
        cases.digests_cold = cases.digests.clone();
        let seeded = seed(&root, &cases).unwrap();
        let server = MockServer::start().await;
        let picks = cases.digests[0]
            .articles
            .iter()
            .map(|key| json!({"article_id":seeded.ids[key],"reason":"Concrete change"}))
            .collect::<Vec<_>>();
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_string_contains("candidates; pick"))
            .respond_with(response(json!({"picks":picks})))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST")).and(path("/v1/chat/completions")).and(body_string_contains("This issue:"))
            .respond_with(response(json!({"sections":[{"title":"Updates","items":[
                {"text":"Wren 12 adds per-site memory limits and pauses background scripts when a tab exceeds its limit.","refs":[1]},
                {"text":"Huichuan raises 200 million dollars for licensing and teams in Southeast Asia.","refs":[2]}
            ]}]}))).expect(1).mount(&server).await;
        let model = model(&server);
        let fixture = FixtureDb::new().unwrap();
        let prompts = prompts(&fixture);
        let rows = run_variant(
            Variant::EnEn,
            root,
            Arc::new(cases),
            prompts.baseline,
            model.clone(),
            Options {
                sections: BTreeSet::from(["digests_cold".into()]),
                tier: InputTier::Medium,
            },
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        let output = &rows[0].outputs["en-en"];
        assert!(output.checks.iter().all(|c| c.ok), "{:?}", output.checks);
        assert_eq!(output.metrics["pre_summary_requests"].value, 0.0);
        assert_eq!(output.metrics["pipeline_requests"].value, 2.0);
        assert_eq!(model.usage.lock().unwrap()["en-en"].2, 2);
    }

    fn response(value: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "choices":[{"message":{"role":"assistant","content":value.to_string()}}],
            "usage":{"prompt_tokens":100,"completion_tokens":40}
        }))
    }

    fn model(server: &MockServer) -> Arc<Model> {
        Arc::new(Model {
            client: rrss_core::network::ProxyConfig::default()
                .build_client_with_env(
                    rrss_core::network::NetworkScope::Api,
                    reqwest::Client::builder(),
                    &rrss_core::network::ProxyEnvironment::default(),
                )
                .unwrap(),
            base_url: server.uri(),
            api_key: None,
            model: "fixture-model".into(),
            gate: Semaphore::new(4),
            usage: Default::default(),
        })
    }

    fn prompts(fixture: &FixtureDb) -> Prompts {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        Prompts {
            variant: Variant::EnEn,
            english: english_templates(&fixture.conn).unwrap(),
            baseline: Arc::new(
                serde_json::from_str(
                    &std::fs::read_to_string(root.join("testdata/eval/baseline-zh.json")).unwrap(),
                )
                .unwrap(),
            ),
        }
    }

    #[tokio::test]
    async fn weekly_chain_uses_local_model_and_production_source_numbering() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let cases: Cases = serde_json::from_str(
            &std::fs::read_to_string(root.join("testdata/eval/cases.json")).unwrap(),
        )
        .unwrap();
        let case = &cases.weekly_digests[0];
        let fixture = rrss_core::evaluation_clock::at_time(NOW, || fixtures::weekly(case)).unwrap();
        let daily =
            ai_store::week_daily_lines(&fixture.conn, NOW - 7 * fixtures::DAY, NOW).unwrap();
        let supplement_ids: Vec<_> = case
            .high_value_supplements
            .iter()
            .map(|key| fixture.id(key).unwrap())
            .collect();
        let supplements = ai_store::digest_items_by_ids(&fixture.conn, &supplement_ids).unwrap();
        let deep =
            ai_store::format_weekly_input(&fixture.conn, &daily, &supplements, InputTier::Medium)
                .unwrap();
        let n = |key: &str| {
            deep.numbered_items
                .iter()
                .position(|item| item.article_id == fixture.id(key).unwrap())
                .unwrap()
                + 1
        };
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/v1/chat/completions")).and(body_string_contains("picking supplementary articles"))
            .respond_with(response(json!({"picks":supplement_ids.iter().map(|id| json!({"article_id":id,"reason":"Material safety result"})).collect::<Vec<_>>()})))
            .expect(1).mount(&server).await;
        Mock::given(method("POST")).and(path("/v1/chat/completions")).and(body_string_contains("writing this week's digest"))
            .respond_with(response(json!({"sections":[{"title":"Developments","items":[
                {"text":"Alder began a 120-sensor trial, paused for certificate failures and resumed after the repair.","refs":case.cross_day_articles.iter().map(|key| n(key)).collect::<Vec<_>>()},
                {"text":"Maple recalls 24000 cooling adapters after a thermal fuse defect.","refs":[n("missed-safety")]},
                {"text":"Willow completed an 11-hour grid trial with 98.4 percent availability.","refs":[n("missed-grid")]},
                {"text":"Maple storage 6.2 adds recovery checkpoints every 45 seconds.","refs":[n("maple-storage")]}
            ]}]}))).expect(1).mount(&server).await;
        let model = model(&server);
        let rows = weekly_digests(
            &prompts(&fixture),
            &model,
            &cases.weekly_digests,
            InputTier::Medium,
        )
        .await
        .unwrap();
        let output = &rows[0].outputs["en-en"];
        assert_eq!(output.calls.iter().map(|c| c.requests).sum::<usize>(), 2);
        assert!(output.checks.iter().all(|c| c.ok), "{:?}", output.checks);
        assert_eq!(output.metrics["cross_day_source_hits"].value, 3.0);
    }

    #[tokio::test]
    async fn manual_summary_chain_counts_segments_and_uses_production_notes() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let corpus = Corpus::load(&root).unwrap();
        let fixture = FixtureDb::new().unwrap();
        let server = MockServer::start().await;
        let prompts = prompts(&fixture);
        let segment_system = prompts
            .evaluation_system(
                ai_store::SLUG_ARTICLE_SEGMENT,
                ai_store::TASK_ARTICLE_SEGMENT,
            )
            .unwrap();
        Mock::given(method("POST")).and(path("/v1/chat/completions")).and(body_string_contains(&segment_system[..70]))
            .respond_with(response(json!({"notes":"Verified controller measurements were retained.","facts":["87.6 million","0.014%","19.2 joules"]})))
            .mount(&server).await;
        let system = prompts
            .evaluation_system(
                ai_store::SLUG_ARTICLE_SUMMARY,
                ai_store::TASK_ARTICLE_SUMMARY,
            )
            .unwrap();
        Mock::given(method("POST")).and(path("/v1/chat/completions")).and(body_string_contains(&system[..70]))
            .respond_with(response(json!({"summary":"Willow tested three controller revisions. Revenue reached 87.6 million. The error rate was 0.014%. Median energy was 19.2 joules.",
                "one_liner":"Willow completes a controlled quarterly evaluation.","key_points":[],"facts":[]})))
            .expect(2).mount(&server).await;
        let model = model(&server);
        let rows = article_summaries(
            &prompts,
            &model,
            &corpus,
            &["markdown-table".into()],
            InputTier::Medium,
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 2);
        let segmented = &rows[1].outputs["en-en"];
        assert!(segmented.calls.len() >= 3);
        assert!(
            segmented.checks.iter().all(|c| c.ok),
            "{:?}",
            segmented.checks
        );
        assert_eq!(segmented.metrics["fact_recall"].value, 1.0);
        assert!(rows[1].input.contains("Notes on the full text"));
        assert_eq!(
            segmented.calls.iter().map(|c| c.prompt_tokens).sum::<u32>(),
            100 * segmented.calls.len() as u32
        );
    }
}
