//! Offline structural evaluation of the production focus pagination path.
//! cargo test -p rrss-app --test focus_stream_eval -- --include-ignored --nocapture
//! Only disposable synthetic databases; no input paths, scheduler, models or downloads.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Instant,
};

use rrss_app::{queries, views::ArticleView};
use rrss_core::{
    ai_store,
    articles::{self, FocusFilter, ListScope, ROW_CARD},
    db,
    feed_scope::FeedScope,
    feeds, interests, rank,
};
use rusqlite::{Connection, params};

const DAY: i64 = 86_400_000;
// Contract expectations below are authored independently of production selectors.
const SEGMENTS: [(usize, usize); 3] = [(0, 10), (10, 40), (40, 80)];

#[derive(Clone)]
struct Item {
    id: i64,
    feed: i64,
    published: Option<i64>,
    created: i64,
    score: i64,
    read: bool,
    starred: bool,
    exposures: usize,
    exposure_age: i64,
    exposure_surface: &'static str,
    exposure_variant: &'static str,
    expired: bool,
    active: bool,
    cluster: Option<i64>,
    // Fixture assumptions only. Never inferred from rank or treated as user labels.
    relevant: bool,
    quiet: bool,
    representative: bool,
}

struct Fixture {
    name: &'static str,
    scope: ListScope,
    items: Vec<Item>,
    anchor: i64,
    rebuild_rank: bool,
}

impl Fixture {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            scope: ListScope::All,
            items: Vec::new(),
            anchor: feeds::now_ms() - 60_000,
            rebuild_rank: false,
        }
    }

    fn add(&mut self, feed: i64, count: usize, age: i64, score: i64, relevant: bool) {
        for n in 0..count {
            self.items.push(Item {
                id: self.items.len() as i64 + 1,
                feed,
                published: Some(self.anchor - age - n as i64),
                created: self.anchor - age - n as i64,
                score,
                read: false,
                starred: false,
                exposures: 0,
                exposure_age: DAY,
                exposure_surface: "focus",
                exposure_variant: "baseline",
                expired: false,
                active: true,
                cluster: None,
                relevant,
                quiet: feed != 1,
                representative: true,
            });
        }
    }

    fn eligible(&self) -> Vec<Item> {
        self.items
            .iter()
            .filter(|a| {
                a.representative
                    && !a.expired
                    && a.score > -10
                    && (self.scope != ListScope::Unread || !a.read)
                    && (self.scope != ListScope::Starred || a.starred)
            })
            .cloned()
            .collect()
    }

    fn database(&self) -> (tempfile::TempDir, db::PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("stream.db")).unwrap();
        for key in [
            "pi_auto_download",
            "feed_icons_enabled",
            ai_store::FOCUS_AUTO_SUMMARY.id,
            ai_store::CLUSTER_AUTO_SUMMARY.id,
            ai_store::GENERATION_BUDGET.id,
            ai_store::EMBED_BUDGET.id,
            db::HIDDEN_SPOT_CHECK_RATE_KEY,
        ] {
            db::set_setting(&conn, key, "0").unwrap();
        }
        articles::set_focus_filter(&conn, FocusFilter::HideFaded).unwrap();
        articles::set_list_scope(&conn, self.scope).unwrap();
        interests::set_rerank_enabled(&conn, false).unwrap();
        interests::set_cluster_group_enabled(&conn, true).unwrap();
        let tx = db::write_transaction(&conn).unwrap();
        for feed in self.items.iter().map(|a| a.feed).collect::<BTreeSet<_>>() {
            tx.execute(
                "INSERT INTO feeds(id,url,title,created_at) VALUES(?1,?2,?3,?4)",
                params![
                    feed,
                    format!("https://source-{feed}.example/feed"),
                    format!("Source {feed}"),
                    self.anchor
                ],
            )
            .unwrap();
        }
        for a in &self.items {
            tx.execute(
                "INSERT INTO articles(id,feed_id,title,published_at,score,read,starred,cluster_id,created_at,expires_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![a.id, a.feed, format!("Synthetic article {}", a.id), a.published, a.score, a.read, a.starred, a.cluster, a.created, a.expired.then_some(self.anchor - DAY)],
            )
            .unwrap();
            for session in 0..a.exposures {
                tx.execute("INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
                    VALUES(?1,?2,0,?3,0,?3,?4,?5,?6)", params![a.id,a.exposure_surface,a.score,a.exposure_variant,format!("session-{session}"),self.anchor-a.exposure_age-session as i64 * 1000]).unwrap();
            }
        }
        tx.execute_batch(
            "INSERT INTO clusters(id,member_count,created_at)
             SELECT cluster_id,COUNT(*),0 FROM articles WHERE cluster_id IS NOT NULL GROUP BY cluster_id;",
        )
        .unwrap();
        tx.commit().unwrap();
        if self.rebuild_rank {
            assert!(rank::store::rebuild(&conn, self.anchor).unwrap() > 0);
        }
        // Reading the pages must not mutate even this disposable fixture.
        conn.pragma_update(None, "query_only", true).unwrap();
        (dir, conn)
    }
}

fn fixtures() -> Vec<Fixture> {
    let mut cases = Vec::new();
    for (name, news) in [("small", 8), ("burst", 130)] {
        let mut f = Fixture::new(name);
        f.add(1, news, 0, 0, true);
        f.add(2, 2, 7 * DAY, 0, true);
        f.add(3, 1, 14 * DAY, 0, true);
        cases.push(f);
    }
    let mut f = Fixture::new("sustained");
    f.add(1, 130, 0, 0, true);
    f.add(2, 60, 7 * DAY, 0, true);
    cases.push(f);
    let mut f = Fixture::new("single-source");
    f.add(1, 130, 0, 0, true);
    cases.push(f);
    let mut f = Fixture::new("quiet-off-topic");
    f.add(1, 130, 0, 10, true);
    f.add(2, 2, 7 * DAY, 0, false);
    f.add(3, 1, 14 * DAY, 0, false);
    cases.push(f);
    let mut f = Fixture::new("major-news");
    f.add(1, 8, 0, 10, true);
    f.add(1, 122, DAY, 0, true);
    f.add(2, 2, 7 * DAY, 0, true);
    f.add(3, 1, 14 * DAY, 0, true);
    cases.push(f);
    for (name, scope) in [
        ("read-all", ListScope::All),
        ("read-unread", ListScope::Unread),
        ("read-starred", ListScope::Starred),
    ] {
        let mut f = Fixture::new(name);
        f.scope = scope;
        f.add(1, 40, 0, 10, true);
        for a in &mut f.items {
            a.read = true;
        }
        f.add(1, 90, DAY, 0, true);
        f.add(2, 2, 7 * DAY, 0, true);
        f.add(3, 1, 14 * DAY, 0, true);
        if scope == ListScope::Starred {
            for a in &mut f.items {
                a.starred = true;
            }
        }
        cases.push(f);
    }
    let mut f = Fixture::new("cross-source-group");
    f.add(1, 130, 0, 0, true);
    f.add(2, 2, 7 * DAY, 0, true);
    f.add(3, 1, 14 * DAY, 0, true);
    // One high-score read member and two unread members occupy one slot.
    f.items[0].read = true;
    f.items[0].score = 10;
    f.items[0].cluster = Some(7);
    f.items[0].representative = false;
    f.items[130].score = 1;
    f.items[130].cluster = Some(7);
    f.items[132].cluster = Some(7);
    f.items[132].representative = false;
    cases.push(f);
    cases
}

fn collect_pages(conn: &Connection, expected: &[Item]) -> Vec<ArticleView> {
    collect_scope(conn, expected, FeedScope::All)
}

fn collect_scope(conn: &Connection, expected: &[Item], scope: FeedScope) -> Vec<ArticleView> {
    let mut cards = Vec::new();
    let mut seen_ids = Vec::new();
    let mut seen_clusters = Vec::new();
    loop {
        let page = queries::focus_page(
            conn,
            None,
            &seen_ids,
            &seen_clusters,
            !seen_ids.is_empty(),
            scope,
        )
        .unwrap();
        let before = cards.len();
        for a in page.articles.into_iter().filter(|a| a.row_kind == ROW_CARD) {
            assert!(!a.spot_check, "exploration is disabled in this evaluation");
            assert!(!seen_ids.contains(&a.id), "duplicate across pages");
            seen_ids.push(a.id);
            if a.cluster_id != 0 {
                assert!(!seen_clusters.contains(&a.cluster_id), "repeated group");
                seen_clusters.push(a.cluster_id);
            }
            cards.push(a);
        }
        assert!(cards.len() <= expected.len(), "unexpected candidates");
        assert!(cards.len() - before <= articles::FOCUS_PAGE_SIZE as usize);
        if !page.has_more {
            break;
        }
        assert!(cards.len() > before, "pagination made no progress");
    }
    assert_eq!(
        seen_ids.into_iter().collect::<BTreeSet<_>>(),
        expected.iter().map(|a| a.id).collect(),
        "must exhaust the independently specified candidate set"
    );
    let terminal = queries::focus_page(
        conn,
        None,
        &cards.iter().map(|a| a.id).collect::<Vec<_>>(),
        &seen_clusters,
        true,
        scope,
    )
    .unwrap();
    assert!(
        !terminal.has_more && terminal.articles.is_empty(),
        "exhausted pool must not refill from the archive"
    );
    cards
}

fn ordered_items(cards: &[ArticleView], expected: &[Item]) -> Vec<Item> {
    cards
        .iter()
        .map(|card| {
            let item = expected.iter().find(|a| a.id == card.id).unwrap();
            assert_eq!(card.feed_id, item.feed);
            assert_eq!(card.read, item.read);
            assert_eq!(card.published_at, item.published);
            item.clone()
        })
        .collect()
}

#[derive(Debug, PartialEq)]
struct Metrics {
    n: usize,
    sources: BTreeMap<i64, usize>,
    longest: usize,
    read: usize,
    relevant: usize,
    // Slots with at least one other-source, relevant, unread candidate remaining.
    alternative: usize,
}

fn metrics(order: &[Item], start: usize, end: usize) -> Metrics {
    let mut result = Metrics {
        n: 0,
        sources: BTreeMap::new(),
        longest: 0,
        read: 0,
        relevant: 0,
        alternative: 0,
    };
    let (mut previous, mut run) = (None, 0);
    for (i, a) in order.iter().enumerate().take(end).skip(start) {
        result.n += 1;
        *result.sources.entry(a.feed).or_default() += 1;
        run = if previous == Some(a.feed) { run + 1 } else { 1 };
        previous = Some(a.feed);
        result.longest = result.longest.max(run);
        result.read += usize::from(a.read);
        result.relevant += usize::from(a.relevant);
        result.alternative += usize::from(
            order[i + 1..]
                .iter()
                .any(|b| b.feed != a.feed && b.relevant && !b.read),
        );
    }
    result
}

fn first_quiet(order: &[Item]) -> String {
    let mut positions = BTreeMap::new();
    for (i, a) in order.iter().enumerate() {
        if a.quiet && a.relevant && !a.read {
            positions.entry(a.feed).or_insert(i + 1);
        }
    }
    if positions.is_empty() {
        "n/a".into()
    } else {
        positions
            .iter()
            .map(|(f, p)| format!("{f}:{p}"))
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn report(name: &str, variant: &str, order: &[Item]) {
    for (start, end) in SEGMENTS {
        let m = metrics(order, start, end);
        let sources = m
            .sources
            .iter()
            .map(|(f, n)| format!("{f}:{n}"))
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "| {name} | {variant} | {}-{end} | {} | {} ({sources}) | {} | {} | {} | {} |",
            start + 1,
            m.n,
            m.sources.len(),
            m.longest,
            m.read,
            m.relevant,
            m.alternative
        );
    }
    let boundary = order
        .iter()
        .enumerate()
        .take(42)
        .skip(38)
        .map(|(i, a)| format!("{}:{}", i + 1, a.feed))
        .collect::<Vec<_>>()
        .join(",");
    println!(
        "DETAIL {name}/{variant}: total={} first_relevant_quiet={} boundary_39_42=[{boundary}]",
        order.len(),
        first_quiet(order)
    );
}

#[test]
#[ignore = "explicit offline structural report; synthetic data, not a human preference study"]
fn focus_stream_report() {
    println!(
        "Synthetic paired evaluation: time vs production focus_page; NOT historical-version replay or human blind evaluation."
    );
    println!(
        "Same complete candidate set, read state, scores and group representatives. Time = published_at DESC, id DESC."
    );
    println!(
        "Feeds 1/2/3 = fictional daily/weekly/monthly. Scores are fixture inputs; relevance/quiet labels are explicit synthetic assumptions."
    );
    println!(
        "No learned rank cache, vectors, interests, exposures, content rerank or exploration; collapsed groups count once using their chosen representative's source/date."
    );
    println!(
        "Runs stop at segment boundaries; boundary_39_42 separately exposes page joins. Empty segments have n=0, never imply success."
    );
    println!(
        "Alternative counts slots with another relevant unread source still remaining, regardless of score gap. First quiet positions use the full stream, not just top 80."
    );
    println!(
        "| case | order | segment | n | sources (feed:count) | max run | read | synthetic relevant | alternative slots |"
    );
    println!("|---|---|---|---:|---|---:|---:|---:|---:|");
    for f in fixtures() {
        let (_dir, conn) = f.database();
        let expected = f.eligible();
        let before = conn.total_changes();
        let cards = collect_pages(&conn, &expected);
        let again = collect_pages(&conn, &expected);
        assert_eq!(
            cards.iter().map(|a| a.id).collect::<Vec<_>>(),
            again.iter().map(|a| a.id).collect::<Vec<_>>(),
            "fixed-state repeatability"
        );
        assert_eq!(conn.total_changes(), before, "queries must be read-only");
        let focus = ordered_items(&cards, &expected);
        let mut time = expected;
        time.sort_by_key(|a| std::cmp::Reverse((a.published, a.id)));
        report(f.name, "time", &time);
        report(f.name, "focus", &focus);
        // Assert semantic boundaries, not exact rank snapshots or fitted weights.
        match f.name {
            "single-source" | "quiet-off-topic" => assert!(focus[..80].iter().all(|a| a.feed == 1)),
            "major-news" => assert!(focus[..8].iter().all(|a| a.score == 10)),
            "read-all" => assert!(focus[..80].iter().all(|a| !a.read)),
            "read-unread" => assert!(focus.iter().all(|a| !a.read)),
            "read-starred" => assert!(focus[..40].iter().all(|a| a.read)),
            "sustained" => {
                for (start, end) in SEGMENTS {
                    assert_eq!(metrics(&focus, start, end).sources.len(), 2);
                }
                assert!(
                    focus[..80]
                        .windows(3)
                        .all(|w| w.iter().any(|a| a.feed != w[0].feed))
                );
            }
            "small" | "burst" => assert_eq!(metrics(&focus, 0, 10).sources.len(), 3),
            "cross-source-group" => {
                assert_eq!(focus.iter().filter(|a| a.cluster == Some(7)).count(), 1)
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn metrics_respect_segment_boundaries_and_unshown_relevant_alternatives() {
    let mut f = Fixture::new("metric-check");
    f.add(1, 3, 0, 0, true);
    f.add(2, 1, DAY, 0, false);
    f.add(3, 1, DAY, 0, true);
    f.items[1].read = true;
    assert_eq!(
        metrics(&f.items, 1, 4),
        Metrics {
            n: 3,
            sources: BTreeMap::from([(1, 2), (2, 1)]),
            longest: 2,
            read: 1,
            relevant: 2,
            alternative: 3,
        }
    );
    // A read alternative is not another unread opportunity; off-topic is not relevant.
    f.items[4].read = true;
    assert_eq!(metrics(&f.items, 0, 3).alternative, 0);
    assert_eq!(metrics(&f.items, 10, 40).n, 0);
}

// These membership flags encode the product contract, not a second selector.
// In particular, the library contains many deliberately ineligible archive rows.
fn active_fixtures() -> Vec<Fixture> {
    let mut cases = Vec::new();
    for (name, blog_score, rebuild) in [
        ("large-relevant", 1, false),
        ("large-off-topic", -1, false),
        ("cold-start-ranked", 0, true),
        ("positive-old-ranked", 1, true),
    ] {
        let mut f = Fixture::new(name);
        f.rebuild_rank = rebuild;
        f.add(1, 3_200, 0, 0, true);
        for a in &mut f.items[160..] {
            a.active = false;
        }
        f.add(2, 3, 45 * DAY, blog_score, blog_score >= 0);
        for (i, a) in f.items[3_200..].iter_mut().enumerate() {
            a.active = i < if blog_score > 0 {
                2
            } else if blog_score == 0 {
                1
            } else {
                0
            };
        }
        f.add(3, 1, 90 * DAY, -1, false);
        f.items.last_mut().unwrap().active = false;
        cases.push(f);
    }
    let mut f = Fixture::new("exposure-opportunities");
    f.add(1, 12, DAY, 0, true);
    f.items[0].exposures = 3;
    f.items[0].active = false;
    f.items[1].exposures = 1;
    f.items[2].exposures = 2;
    f.items[3].exposures = 3;
    f.items[3].score = 1;
    f.items[4].exposures = 3;
    f.items[4].score = -1;
    f.items[4].active = false;
    f.items[5].exposures = 3;
    f.items[5].exposure_age = 8 * DAY;
    f.items[6].exposures = 3;
    f.items[6].exposure_surface = "timeline";
    f.items[7].exposures = 3;
    f.items[7].exposure_variant = "spot_check";
    f.items[8].exposures = 3;
    f.items[8].exposure_age = -DAY;
    f.items[9].read = true;
    f.items[9].exposures = 3;
    f.items[10].starred = true;
    f.items[10].exposures = 3;
    f.items[10].active = false; // Exemption is for the saved-items scope.
    f.add(2, 1, 45 * DAY, 1, true);
    f.items.last_mut().unwrap().exposures = 1;
    f.items.last_mut().unwrap().exposure_age = 35 * DAY;
    f.items.last_mut().unwrap().active = false;
    cases.push(f);

    let mut f = Fixture::new("dates-and-read");
    for (age, read, active) in [
        (6, true, true),
        (8, true, false),
        (29, false, true),
        (31, false, false),
        (181, false, false),
    ] {
        f.add(1, 1, age * DAY, 0, true);
        let a = f.items.last_mut().unwrap();
        a.read = read;
        a.active = active;
    }
    f.add(1, 1, DAY, 0, true); // undated new import
    f.items.last_mut().unwrap().published = None;
    f.add(1, 1, 181 * DAY, 0, true); // undated historical import
    let a = f.items.last_mut().unwrap();
    a.published = None;
    a.active = false;
    f.add(1, 1, 181 * DAY, 0, true); // future publication cannot rejuvenate old ingestion
    let a = f.items.last_mut().unwrap();
    a.published = Some(f.anchor + DAY);
    a.active = false;
    f.add(1, 1, 181 * DAY, 0, true); // importing dated archive now is still archive
    let a = f.items.last_mut().unwrap();
    a.created = f.anchor;
    a.active = false;
    f.add(1, 1, DAY, 0, true);
    f.items.last_mut().unwrap().published = Some(0);
    f.add(1, 1, 0, 0, true);
    let a = f.items.last_mut().unwrap();
    a.created = f.anchor + DAY;
    a.active = false;
    f.add(2, 3, 45 * DAY, 1, true);
    f.items.last_mut().unwrap().active = false;
    f.add(2, 1, 40 * DAY, 0, true); // positive old work displaces neutral fallback
    f.items.last_mut().unwrap().active = false;
    cases.push(f);

    let mut f = Fixture::new("dormancy-all-articles");
    f.rebuild_rank = true;
    for feed in 1..=4 {
        f.add(feed, 1, 45 * DAY, 0, true);
        f.items.last_mut().unwrap().active = feed == 4;
        if feed < 4 {
            f.add(feed, 1, DAY, 0, true);
            let a = f.items.last_mut().unwrap();
            match feed {
                1 => a.score = -100,
                2 => a.read = true,
                3 => a.expired = true,
                _ => unreachable!(),
            }
        }
    }
    cases.push(f);

    let mut f = Fixture::new("single-source-bounded");
    f.add(1, 3_200, DAY, 0, true);
    for a in &mut f.items[160..] {
        a.active = false;
    }
    cases.push(f);

    let mut f = Fixture::new("total-and-recovery-caps");
    for feed in 1..=16 {
        f.add(feed, 200, DAY, 0, true);
        let start = f.items.len() - 200;
        // 1,984 recent positions: equal scores/dates, 124 per source.
        for a in &mut f.items[start + 124..] {
            a.active = false;
        }
    }
    for feed in 17..=56 {
        f.add(feed, 3, 45 * DAY, 1, true);
        let start = f.items.len() - 3;
        // 64 recoveries: all 40 first positions, then 24 second positions.
        f.items[start + 1].active = feed >= 33;
        f.items[start + 2].active = false;
    }
    cases.push(f);

    let mut f = Fixture::new("saved-archive");
    f.scope = ListScope::Starred;
    for feed in 1..=2 {
        f.add(feed, 170, 365 * DAY, 0, true);
    }
    for a in &mut f.items {
        a.starred = true;
        a.read = true;
        a.exposures = 3;
    }
    f.add(1, 1, DAY, 10, true); // nonstarred is excluded by the explicit scope
    cases.push(f);
    cases
}

fn evaluate_active(f: &Fixture, bounded: bool) {
    let (_dir, conn) = f.database();
    let library = f.eligible();
    let expected: Vec<_> = library
        .iter()
        .filter(|a| !bounded || a.active)
        .cloned()
        .collect();
    if f.rebuild_rank {
        let old = f
            .items
            .iter()
            .find(|a| a.created < f.anchor - 30 * DAY)
            .unwrap();
        let (boost, reasons): (i64, String) = conn
            .query_row(
                "SELECT boost,reasons_json FROM article_rank WHERE article_id=?1",
                [old.id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert!(
            boost < 0 && reasons.contains("rank_freshness"),
            "must exercise real freshness cache"
        );
    }
    let before = conn.total_changes();
    let cards = collect_pages(&conn, &expected);
    let again = collect_pages(&conn, &expected);
    assert_eq!(
        cards.iter().map(|a| a.id).collect::<Vec<_>>(),
        again.iter().map(|a| a.id).collect::<Vec<_>>()
    );
    let focus = ordered_items(&cards, &expected);
    let mut time = library.clone();
    time.sort_by_key(|a| std::cmp::Reverse((a.published, a.id)));
    report(f.name, "time-library", &time);
    let mut time_candidates = expected;
    time_candidates.sort_by_key(|a| std::cmp::Reverse((a.published, a.id)));
    report(f.name, "time-candidates", &time_candidates);
    report(
        f.name,
        if bounded {
            "bounded-focus"
        } else {
            "old-focus"
        },
        &focus,
    );
    println!(
        "POOL {} library={} visible_representatives={} candidates={} omitted={} anchor_ms={}",
        f.name,
        f.items.len(),
        library.len(),
        focus.len(),
        library.len() - focus.len(),
        f.anchor
    );
    if bounded {
        if let Some(archive) = library.iter().find(|a| !a.active) {
            assert!(!cards.iter().any(|a| a.id == archive.id));
            let scope = format!("feed:{}", archive.feed);
            let mut page = queries::feeds_page(&conn, &scope, "all", None).unwrap();
            let mut seen = BTreeSet::new();
            loop {
                for a in &page.articles {
                    assert!(seen.insert(a.id));
                }
                if seen.contains(&archive.id) {
                    break;
                }
                assert!(
                    page.has_more,
                    "pool-excluded archive missing from formal timeline"
                );
                page = queries::feeds_page(&conn, &scope, "all", page.next_cursor).unwrap();
            }
            println!(
                "ARCHIVE {} id={} found_in=timeline excluded_from_all_focus_pages=true",
                f.name, archive.id
            );
        }
        if f.name == "total-and-recovery-caps" {
            assert_eq!(cards.len(), 2048);
        }
        if f.name == "saved-archive" {
            assert_eq!(cards.len(), 340);
        }
        if f.name == "single-source-bounded" {
            assert_eq!(cards.len(), 160);
        }
        if f.name == "exposure-opportunities" {
            let fresh = cards.iter().position(|a| a.id == 2).unwrap();
            let repeat_positive = cards.iter().position(|a| a.id == 4).unwrap();
            assert!(
                fresh < repeat_positive,
                "positive repeat remains but is demoted"
            );
        }
    }
    assert_eq!(
        conn.total_changes(),
        before,
        "all verification reads are immutable"
    );
}

#[test]
#[ignore = "explicit synthetic bounded-pool acceptance, not human preference labels"]
fn focus_active_pool_report() {
    for f in active_fixtures() {
        evaluate_active(&f, true);
    }
}

#[test]
#[ignore = "explicit static-history scaling; app query, content rerank enabled, synthetic data"]
fn focus_static_history_perf() {
    for archive_count in [5_000, 30_000, 100_000] {
        let mut f = Fixture::new("static-history");
        for feed in 1..=8 {
            f.add(feed, 160, DAY, 1, true);
        }
        for i in 0..archive_count {
            f.add(i % 8 + 1, 1, 365 * DAY, 1, true);
            let a = f.items.last_mut().unwrap();
            a.active = false;
            if i % 2 == 0 {
                a.published = None;
            }
        }
        let (_dir, conn) = f.database();
        conn.pragma_update(None, "query_only", false).unwrap();
        interests::set_rerank_enabled(&conn, true).unwrap();
        conn.pragma_update(None, "query_only", true).unwrap();
        let first = queries::focus_page(&conn, None, &[], &[], false, FeedScope::All).unwrap();
        let ids: Vec<_> = first
            .articles
            .iter()
            .filter(|a| a.row_kind == ROW_CARD)
            .map(|a| a.id)
            .collect();
        assert_eq!(ids.len(), articles::FOCUS_PAGE_SIZE as usize);
        assert!(first.has_more);
        let second = queries::focus_page(&conn, None, &ids, &[], true, FeedScope::All).unwrap();
        assert!(second.has_more);
        let expected: Vec<Vec<_>> = [&first, &second]
            .iter()
            .map(|p| p.articles.iter().map(|a| a.id).collect())
            .collect();
        let mut samples = [Vec::new(), Vec::new()];
        for _ in 0..21 {
            for page in 0..2 {
                let start = Instant::now();
                let result = queries::focus_page(
                    &conn,
                    None,
                    if page == 0 { &[] } else { &ids },
                    &[],
                    page > 0,
                    FeedScope::All,
                )
                .unwrap();
                samples[page].push(start.elapsed().as_secs_f64() * 1000.0);
                assert_eq!(
                    result.articles.iter().map(|a| a.id).collect::<Vec<_>>(),
                    expected[page]
                );
            }
        }
        for (page, times) in samples.iter_mut().enumerate() {
            let raw = times.clone();
            times.sort_by(f64::total_cmp);
            println!(
                "HISTORY_PERF {}",
                serde_json::json!({"archive":archive_count,"recent":1280,"page":page+1,"repeats":21,"rerank":true,"cache":"warm","p50_ms":times[10],"p95_ms":times[19],"samples_ms":raw})
            );
        }
    }
}
