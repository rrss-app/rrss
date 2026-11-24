# AI input/output evaluation

`cargo run -p rrss-app --example prompt_eval` evaluates production extraction,
candidate selection, packing and model output on throwaway SQLite databases.
It never opens the everyday database, starts a scheduler or downloads content.
Offline runs make **zero model requests** and do not read or require
`RRSS_EVAL_BASE_URL`, `RRSS_EVAL_MODEL` or `RRSS_EVAL_API_KEY`.

```sh
# Establish the current deterministic baseline (medium digest/cluster tier).
cargo run -p rrss-app --example prompt_eval -- --offline

# Compare against a saved baseline; use a separate output directory.
cargo run -p rrss-app --example prompt_eval -- --offline \
  --compare test-results/prompt-eval/offline-results.json \
  --out test-results/prompt-eval-compare

# Change budgets, or run one offline group.
cargo run -p rrss-app --example prompt_eval -- --offline --tier high
cargo run -p rrss-app --example prompt_eval -- --offline --only summary_extract

# Evaluation unit tests, including local mock-model chains; no paid API calls.
cargo test -p rrss-app --example prompt_eval
```

If tasks share a machine, use a separate worktree and `target/` per task and
serialize Cargo with `CARGO_TARGET_DIR=target CARGO_BUILD_JOBS=4 flock
/tmp/rrss-cargo.lock cargo …`. Generated `test-results/` artifacts are ignored
and must not be committed.

| File | Purpose |
| --- | --- |
| [input-cases.json](input-cases.json) | Deterministic Chinese/English long articles, Markdown/HTML tables and code, oversized lead/tail paragraphs, three 5–15% budget boundaries, nested headings, link cards and short articles. `parts[].repeat` expands fixed text; `must_keep_facts` labels body/table/code/lead/tail facts. All names and `.example` sources are fictional. |
| [cases.json](cases.json) | Original summary, daily digest, cluster, memory and citation inputs plus weekly, manual-summary and cold-digest sections. `real:<file>#<index>` refers only to an existing [real feed snapshot](../feeds/README.md). |
| [baseline-zh.json](baseline-zh.json) | Archived Chinese prompts and input labels for the original online sections. |
| [fixtures.rs](../../crates/rrss-app/examples/prompt_eval/fixtures.rs) | Deterministic temporary database factories: mixed/cold pools, a 35-article single-feed flood, a production-classified noisy feed, downvotes, read articles, old reingestion, a seven-day 70-item weekly pool and 3/15/30-member clusters. |

## Offline measurements

`--offline` writes `offline-results.json`, `offline-report.md` and a standalone
`offline-report.html`. The fixed evaluation clock uses the existing evaluation
clock feature, enabled only for this executable's development dependencies.
No production defaults or business behavior change.

| `--only` group | Measurements |
| --- | --- |
| `summary_extract` | Calls `extract_summary_payload`, `extract_body_excerpt` and `split_segments` at **all three tiers**, regardless of `--tier`. Reports fact recall by path/tier/language/category, utilization for oversized articles, deterministic token estimates and zh/en ratios, segment rate/counts/lengths, trailing isolated `### ` blocks and link-card TP/FP/FN/TN. Summary payload uses the summary budget; the body excerpt uses the digest per-item budget; segments use the summary budget. |
| `digests` | Calls `digest_candidates`, `digest_representatives`, catalog/deep formatters, `week_daily_lines` and `format_weekly_input`. Retains raw, representative and selected pool health, title repetition, noisy-source identifiability, missing-summary evidence, excerpt utilization, short-body summary redundancy, publication-time coverage and same-cluster weekly collisions. Full weekly input is split into daily items, source summaries and supplementary excerpts, with framing reported separately. |
| `auto_summary` | Calls `due_focus_summaries_excluding` and counts read, downvoted, noisy-feed and link-only selections in the production batch size. |
| `cluster_summary` | Calls `visible_members` and `build_input` for 3, 15 and 30 members, counting input characters/tokens, membership, distinct feeds and same-feed repetition. |

Facts use case-insensitive, whitespace-normalized literal matching with ASCII word/number
boundaries; optional
fixed `alternatives` support translated units in online output. Recall is
micro-averaged over annotated facts, with explicit numerator/denominator in each
case. An absent category is omitted from the aggregate map rather than treated
as perfect recall. The estimator weights CJK characters (including punctuation,
kana and hangul) at 1 token, ASCII/Latin at 0.25 and other characters at 0.5,
rounding the total up once. It is a reproducible proxy, not a model tokenizer.
The zh/en ratio compares the oversized subsets in this corpus.

Offline editorial picks are the ordered candidate prefix, so the report isolates
backend input behavior before any model judgment. Wasted budget is the minimum
of unused short-article allocations, omitted long-text characters and remaining
total budget; unused space caused solely by the per-item cap is excluded.
Noise comes from the production classifier, not an added schema field. No
embeddings are generated: representatives exercise the no-embedding path.

## Online sections

Online runs require an explicitly configured chat-completions endpoint and
model through `RRSS_EVAL_BASE_URL` and `RRSS_EVAL_MODEL`; set
`RRSS_EVAL_API_KEY` only if required. They incur that service's normal usage.
The evaluation client uses the production `NetworkScope::Api` client builder
with an explicit direct route on Windows and Linux, without reading system or
environment proxy settings. Request errors use the shared credential scrubber.
Development validation uses local mocks only.

```sh
# Run after configuring the endpoint and model yourself.
cargo run -p rrss-app --example prompt_eval -- \
  --variants en-zh,en-en --tier medium --concurrency 4 \
  --only weekly_digests,article_summaries,digests_cold

cargo run -p rrss-app --example prompt_eval -- \
  --only article_summaries --compare saved-results.json --out test-results/after
cargo run -p rrss-app --example prompt_eval -- --help
```

All original flags remain available: `--variants`, `--only`, `--out` and
`--concurrency`. `--tier low|medium|high` defaults to `medium` and is recorded in
JSON and report headers. Without `--only`, online mode runs all eight sections.

| Section | Pipeline and checks |
| --- | --- |
| `summaries` | Original focus summaries, fields/limits, output language and prompt leakage. |
| `digests` | Original article pools with prerequisite summaries, pick then write, verified v2 sources, fixture-specific item ranges, high-value event coverage and low-quality selections. Pipeline cost metrics include prerequisite summaries. |
| `clusters` | Original related coverage synthesis and output checks. |
| `memory` | Original memory-candidate gate, parsing and output checks. |
| `citations` | Original citation questions with read-only tools on the temporary database. |
| `weekly_digests` | Four verified v2 daily issues with a cross-day evolving event, disposable entries and missed high-value/duplicate/noisy candidates. Uses production weekly pick, apply, pack and draft functions. Checks raw JSON and local refs, verified v2 alignment, unsourced items, item range, cross-day source merging, high-value supplements, input-structure words, output language and prompt leakage. |
| `article_summaries` | Four long Chinese/English inputs. Each has a single-excerpt row and, when the production policy permits splitting, a segment-compression row using the production notes formatter and manual-summary template. Checks JSON/types/limits, facts in `summary + key_points + facts` (excluding `one_liner`), segment markers, language and leakage; retains every request, tokens and path latency. Low tier runs the single path only, matching production. |
| `digests_cold` | Exactly the same pools as `digests` in an isolated database with no prerequisite summaries. Runs pick then write and checks core-event coverage, link/low-quality selection, source completeness and usage. |

Variants remain `baseline`, `en-zh` and `en-en`. The archived `baseline`
contains only the original five task templates. Newly covered weekly and
manual-summary tasks use the **current production English template with Chinese
output** for `baseline`; they are not a historical Chinese-prompt comparison.
The report identifies this boundary. All variants use production input budgets.
Online output files are `results.json`, `report.md` and `report.html`; raw calls
and checks remain available. Reported per-digest pipeline costs add that issue's
prerequisite summaries, which can be shared across issues; global request usage
counts each actual request once. Concurrent compression records wall latency
and summed service latency separately. Automatic checks are deterministic
heuristics; semantic quality still needs human review.

## Comparing runs

`--compare <baseline.json>` accepts offline `offline-results.json` or online
`results.json`, including older online files without a metrics map. Comparison
must use the same mode. Matched metrics produce baseline/current/delta/percentage
rows in stdout and the Markdown/HTML reports; JSON contains the full diff too.
Lower recall/pass rates, more contamination/waste/cost/latency and utilization
farther from 1 are explicitly marked **REGRESSED**. Observational measurements
such as input size and segment count are marked changed without a quality claim.
New and missing metrics remain visible. Unchanged differences are normalized
to zero to absorb JSON floating-point roundoff. Zero baselines have no percentage change
unless both values are zero. Different tiers or fixture hashes (including the temporary database factory)
produce a visible comparability note. A production source hash also records
uncommitted backend changes independently of the Git revision. Legacy files recover checks, request tokens and latency,
but cannot recover unreported prerequisite calls.
