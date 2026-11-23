//! Offline focus pagination benchmark. Fixture construction and connection opening are untimed.
//! CARGO_TARGET_DIR=target CARGO_BUILD_JOBS=4 flock /tmp/rrss-cargo.lock cargo test --offline --locked -p rrss-core --test focus_query_perf -- --ignored --nocapture --test-threads=1
//!
//! Uses the production core pagination entry point, including cluster projection and source mixing.
//! Does not measure the app view adapter, IPC, rendering, background jobs, or rank rebuilding.
//! `sqlite_cold` opens a fresh read-only connection for each page, without flushing OS caches.
//! `warm` reuses the seeded pooled connection after one three-page warmup. Both use SQLite's
//! default cache size. The clock-relative exposure anchor keeps the same eligibility on reruns;
//! all IDs, scores, source weights, and relative timestamps are deterministic.

use std::{collections::HashSet, path::Path, time::Instant};

use rrss_core::{
    ai_store,
    articles::{self, ArticleCard, FOCUS_PAGE_SIZE, FocusFilter, ListScope, ROW_CARD},
    db,
    feed_scope::FeedScope,
    feeds, interests,
};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::json;

const REPEATS: usize = 21;
const PAGES: usize = 3;
const DAY: i64 = 86_400_000;

fn seed(conn: &Connection, count: i64, now: i64) {
    for key in [
        "pi_auto_download",
        "feed_icons_enabled",
        ai_store::FOCUS_AUTO_SUMMARY.id,
        ai_store::CLUSTER_AUTO_SUMMARY.id,
        ai_store::GENERATION_BUDGET.id,
        ai_store::EMBED_BUDGET.id,
        db::HIDDEN_SPOT_CHECK_RATE_KEY,
    ] {
        db::set_setting(conn, key, "0").unwrap();
    }
    articles::set_focus_filter(conn, FocusFilter::HideFaded).unwrap();
    articles::set_list_scope(conn, ListScope::All).unwrap();
    interests::set_boost_enabled(conn, true).unwrap();
    interests::set_cluster_group_enabled(conn, true).unwrap();
    interests::set_hide_noisy_enabled(conn, false).unwrap();
    let tx = db::write_transaction(conn).unwrap();
    for feed in 1..=60 {
        tx.execute(
            "INSERT INTO feeds(id,url,title,created_at) VALUES(?1,?2,?3,?4)",
            params![
                feed,
                format!("https://source-{feed}.example/rss"),
                format!("Source {feed}"),
                now - 30 * DAY
            ],
        )
        .unwrap();
    }
    // Each ten-article block has a three-member, sometimes cross-source reporting group.
    for cluster in 1..=count / 10 {
        tx.execute(
            "INSERT INTO clusters(id,label,member_count,first_published_at,last_published_at,created_at)
             VALUES(?1,?2,3,?3,?4,?4)",
            params![cluster, format!("Report {cluster}"), now - 30 * DAY, now],
        ).unwrap();
    }
    {
        let mut article = tx
            .prepare(
                "INSERT INTO articles(id,feed_id,guid,canonical_url,title,published_at,created_at,
             score,interest_boost,read,starred,cluster_id,summary,content_html,expires_at)
             VALUES(?1,?2,?3,?4,?5,?6,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            )
            .unwrap();
        let mut rank = tx
            .prepare(
                "INSERT INTO article_rank(article_id,boost,reasons_json,algo_version,computed_at)
             VALUES(?1,?2,?3,?4,?5)",
            )
            .unwrap();
        let mut exposure = tx
            .prepare(
                "INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,
             variant,session_id,shown_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            )
            .unwrap();
        let body = format!(
            "<p>{}</p>",
            "Synthetic article body for pagination measurements. ".repeat(40)
        );
        for id in 1..=count {
            // 60% from one high-volume source, 25% from nine, 15% from fifty.
            let slot = (id - 1) % 100;
            let feed = match slot {
                0..=59 => 1,
                60..=84 => 2 + (id / 100 * 25 + slot - 60) % 9,
                _ => 11 + (id / 100 * 15 + slot - 85) % 50,
            };
            let score = if id % 17 == 0 { -20 } else { (id * 7 % 19) - 3 };
            let interest = id * 3 % 7;
            let boost = id * 11 % 9 - 4;
            let published = now - 60_000 - (id * 7919 % (30 * 24 * 60)) * 60_000;
            let cluster = ((id - 1) % 10 < 3).then_some((id - 1) / 10 + 1);
            article
                .execute(params![
                    id,
                    feed,
                    format!("article-{id}"),
                    format!("https://source-{feed}.example/a/{id}"),
                    format!(
                        "Topic {} report {id}: synthetic compiler storage analysis",
                        id % 23
                    ),
                    published,
                    score,
                    interest,
                    id % 4 == 0,
                    id % 13 == 0,
                    cluster,
                    format!(
                        "Synthetic summary for topic {}. {}",
                        id % 23,
                        "Measured offline pagination. ".repeat(12)
                    ),
                    body,
                    (id % 29 == 0).then_some(now - DAY),
                ])
                .unwrap();
            // Ten percent lack a rank row, covering the production LEFT JOIN fallback.
            if id % 10 != 0 {
                rank.execute(params![id, boost,
                    json!([{"component":"source","code":"rank_source","params":{"habit":"read"},"points":boost}]).to_string(),
                    rrss_core::rank::combine::ALGO_VERSION, now - 3_600_000]).unwrap();
            }
            // 3.6 exposures/article: recent repeats, first opportunities, stale history,
            // and unrelated surfaces/variants. Only baseline focus rows can cool an article.
            let (sessions, surface, variant, age) = match id % 5 {
                0 => (4, "focus", "baseline", DAY),
                1 => (1, "focus", "baseline", DAY),
                2 => (6, "focus", "baseline", 14 * DAY),
                3 => (4, "timeline", "baseline", DAY),
                _ => (3, "focus", "spot_check", DAY),
            };
            for session in 0..sessions {
                exposure
                    .execute(params![
                        id,
                        surface,
                        id % 40,
                        score,
                        interest,
                        score + interest + boost,
                        variant,
                        format!("session-{session}"),
                        now - age - session * 3_600_000
                    ])
                    .unwrap();
            }
        }
    }
    tx.commit().unwrap();
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    let counts: Vec<i64> = ["articles", "feeds", "clusters", "article_rank", "exposures"]
        .iter()
        .map(|table| {
            conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        })
        .collect();
    assert_eq!(
        counts,
        [count, 60, count / 10, count * 9 / 10, count * 18 / 5]
    );
    println!(
        "FOCUS_FIXTURE {}",
        json!({"articles":count,"feeds":60,"largest_feed_percent":60,
        "clusters":counts[2],"rank_rows":counts[3],"exposures":counts[4],"read_percent":25,
        "body_bytes":body_size(),"anchor_ms":now,"sqlite":rusqlite::version(),
        "debug_assertions":cfg!(debug_assertions),"page_size":FOCUS_PAGE_SIZE,
        "cache_size":conn.pragma_query_value(None,"cache_size",|r|r.get::<_,i64>(0)).unwrap()})
    );
}

fn body_size() -> usize {
    7 + "Synthetic article body for pagination measurements. ".len() * 40
}

#[derive(Clone, Default)]
struct Cursor {
    ids: Vec<i64>,
    clusters: Vec<i64>,
}

impl Cursor {
    fn append(&mut self, cards: &[ArticleCard]) {
        // Same exclusion contract as the UI: ordered article IDs and unique cluster IDs.
        for card in cards {
            if card.row_kind == ROW_CARD {
                assert!(
                    !self.ids.contains(&card.id),
                    "duplicate article across pages"
                );
                self.ids.push(card.id);
            }
            if let Some(cluster) = card.cluster_id {
                if !self.clusters.contains(&cluster) {
                    self.clusters.push(cluster);
                }
            }
        }
    }
}

fn page(conn: &Connection, cursor: &Cursor) -> (Vec<ArticleCard>, bool) {
    articles::list_focus_page(
        conn,
        FOCUS_PAGE_SIZE,
        None,
        &cursor.ids,
        &cursor.clusters,
        FeedScope::All,
    )
    .unwrap()
}

fn signature(cards: &[ArticleCard]) -> Vec<(i32, i64, Option<i64>)> {
    cards
        .iter()
        .map(|c| (c.row_kind, c.id, c.cluster_id))
        .collect()
}

fn fresh_reader(path: &Path) -> Connection {
    // db::open_at is pooled: dropping and reacquiring it would NOT produce a cold connection.
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    conn.pragma_update(None, "foreign_keys", true).unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(30))
        .unwrap();
    conn
}

fn measure(conn: &Connection, path: &Path, count: i64, rerank: bool) {
    interests::set_rerank_enabled(conn, rerank).unwrap();
    let mut cursor = Cursor::default();
    let mut inputs = Vec::new();
    let mut expected = Vec::new();
    for _ in 0..PAGES {
        inputs.push(cursor.clone());
        let (cards, more) = page(conn, &cursor);
        assert!(more && !cards.is_empty());
        let ids: Vec<_> = cards
            .iter()
            .filter(|c| c.row_kind == ROW_CARD)
            .map(|c| c.id)
            .collect();
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        assert!(ids.len() <= FOCUS_PAGE_SIZE as usize);
        expected.push(signature(&cards));
        cursor.append(&cards);
    }
    let mut samples = vec![vec![Vec::new(); PAGES]; 2];
    // Alternate cache-mode order between repetitions to avoid a fixed order bias.
    for repeat in 0..REPEATS {
        for offset in 0..2 {
            let mode = (repeat + offset) % 2;
            for index in 0..PAGES {
                let fresh = (mode == 0).then(|| fresh_reader(path));
                let reader = fresh.as_ref().unwrap_or(conn);
                let start = Instant::now();
                let (cards, more) = page(reader, &inputs[index]);
                let ms = start.elapsed().as_secs_f64() * 1000.0;
                assert!(more);
                assert_eq!(
                    signature(&cards),
                    expected[index],
                    "fixed-state page changed"
                );
                samples[mode][index].push(ms);
            }
        }
    }
    for (mode, pages) in samples.iter_mut().enumerate() {
        for (index, times) in pages.iter_mut().enumerate() {
            let raw = times.clone();
            times.sort_by(f64::total_cmp);
            println!(
                "FOCUS_PERF {}",
                json!({"articles":count,"rerank":rerank,
                "cache":if mode==0 {"sqlite_cold"} else {"warm"},"page":index+1,
                "repeats":REPEATS,"per_connection_warmups":if mode==0 {0} else {1},"p50_ms":times[REPEATS/2],
                "p95_ms":times[(REPEATS*95).div_ceil(100)-1],"max_ms":times[REPEATS-1],
                "excluded_articles":inputs[index].ids.len(),"excluded_clusters":inputs[index].clusters.len(),
                "returned_cards":expected[index].iter().filter(|(kind,_,_)| *kind==ROW_CARD).count(),
                "returned_rows":expected[index].len(),"samples_ms":raw})
            );
        }
    }
}

#[test]
#[ignore = "explicit offline focus pagination benchmark: 5k and 30k synthetic articles"]
fn focus_pagination_5k_30k() {
    let now = feeds::now_ms();
    for count in [5_000, 30_000] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("focus-perf.db");
        let conn = db::open_at(&path).unwrap();
        seed(&conn, count, now);
        for rerank in [false, true] {
            measure(&conn, &path, count, rerank);
        }
    }
}
