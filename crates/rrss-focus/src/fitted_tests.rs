//! Synthetic contracts for the opt-in fitted scorer.
use super::*;
use crate::{evidence::DAY_MS, store};
use rrss_core::db;
use rusqlite::params;

const H: i64 = 3_600_000;
const NOW: i64 = 60 * DAY_MS + 12 * H;
const DIMS: usize = 50;
const TOPICS: [&str; 6] = [
    "smartphone launch",
    "chipmaker export",
    "robotaxi rollout",
    "celebrity wedding",
    "football transfer",
    "lottery jackpot",
];
const VARIANTS: [&str; 8] = [
    "rumor", "review", "probe", "leak", "deal", "delay", "ban", "outlook",
];
const WEEKLY: [&str; 9] = [
    "orbital mechanics",
    "medieval glassmaking",
    "soil microbes",
    "typeface history",
    "tide prediction",
    "bee navigation",
    "origami engineering",
    "volcanic lightning",
    "desert acoustics",
];
const IGNORED: [&str; 9] = [
    "harbor zoning",
    "pension audit",
    "parking levy",
    "sewer bond",
    "library hours",
    "bridge paint",
    "council minutes",
    "tax notice",
    "school calendar",
];
const UNSEEN: [&str; 9] = [
    "knitting patterns",
    "chess openings",
    "bonsai pruning",
    "stamp auction",
    "model trains",
    "birdsong recording",
    "sourdough starter",
    "kite design",
    "pottery glaze",
];
const BLOG_WORDS: [&str; 20] = [
    "compiler", "memory", "garden", "violin", "glacier", "harbor", "lantern", "meadow", "nebula",
    "orchard", "pigment", "quarry", "saddle", "thimble", "tundra", "velvet", "walnut", "yarrow",
    "zephyr", "basalt",
];

#[derive(Clone, Copy, PartialEq)]
enum Weekly {
    FocusSameDay,
    Mixed,
}

struct Lib {
    _dir: tempfile::TempDir,
    conn: db::PooledConnection,
    read: BTreeSet<i64>,
    published: BTreeMap<i64, i64>,
    feed: BTreeMap<i64, i64>,
    title: BTreeMap<i64, String>,
    tracked: Vec<(&'static str, i64)>,
}

fn axis(i: usize) -> Vec<f32> {
    let mut v = vec![0.0_f32; DIMS];
    v[i] = 1.0;
    v[49] = 0.2;
    v
}

fn noise(id: i64) -> Vec<f32> {
    let mut v = vec![0.0_f32; DIMS];
    let mut x = (id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    for slot in v.iter_mut().take(48).skip(34) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *slot = (x % 1000) as f32 / 1000.0 - 0.5;
    }
    v
}

#[test]
fn unrelated_embedding_completion_does_not_change_fitted_score() {
    let lib = build(Weekly::Mixed);
    lib.conn.execute("UPDATE chunks SET embedding=NULL WHERE article_id NOT IN (SELECT article_id FROM read_sessions)", []).unwrap();
    store::set_switch(&lib.conn, store::FITTED_ENABLED_KEY, true).unwrap();
    let mut model = Model::load(&lib.conn, NOW).unwrap();
    assert!(model.fitted.is_some());
    let id = lib.tracked.iter().find(|(_, id)| lib.feed[id] == 8).unwrap().1;
    let before = model.article_boost(id, NOW, Switches::ALL).unwrap().raw_score;
    let mut unrelated = vec![0.0; DIMS];
    unrelated[48] = 1.0;
    model.vectors.insert(id, unrelated);
    let after = model.article_boost(id, NOW, Switches::ALL).unwrap().raw_score;
    assert!((before - after).abs() < 1e-10, "{before} != {after}");
}

fn build(mode: Weekly) -> Lib {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("fix.db")).unwrap();
    conn.execute_batch("BEGIN").unwrap();
    for f in 1..=10 {
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(?1,?2,0)",
            params![f, format!("https://feed{f}.example/rss")],
        )
        .unwrap();
    }
    conn.execute("INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at)
        VALUES(1,'','fixture',?1,1,0,'active',0)", [DIMS as i64]).unwrap();
    let mut lib = Lib {
        _dir: dir,
        conn,
        read: BTreeSet::new(),
        published: BTreeMap::new(),
        feed: BTreeMap::new(),
        title: BTreeMap::new(),
        tracked: Vec::new(),
    };
    let mut next_id = 1_i64;
    let mut next_ex = 1_i64;
    let mut insert = |lib: &mut Lib, feed: i64, title: String, published: i64, vector: Vec<f32>| {
        let id = next_id;
        next_id += 1;
        lib.conn.execute(
            "INSERT INTO articles(id,feed_id,title,author,canonical_url,fulltext_md,published_at,created_at)
             VALUES(?1,?2,?3,?4,?5,'body',?6,?7)",
            params![id, feed, title, format!("Author {feed}"),
                format!("https://feed{feed}.example/{id}"), published, published + 300_000],
        ).unwrap();
        lib.conn
            .execute(
                "INSERT INTO chunks(article_id,ordinal,kind,text,embedding,model,space_id)
             VALUES(?1,0,'title_summary','',?2,'fixture',1)",
                params![id, rrss_core::rag::f32s_to_le(&vector)],
            )
            .unwrap();
        lib.published.insert(id, published);
        lib.feed.insert(id, feed);
        lib.title.insert(id, title);
        id
    };
    let mut expose = |lib: &Lib, id: i64, at: i64| {
        let ex = next_ex;
        next_ex += 1;
        lib.conn.execute(
            "INSERT INTO exposures(id,article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
             VALUES(?1,?2,'focus',0,0,0,0,'baseline',?3,?4)",
            params![ex, id, format!("s{ex}"), at],
        ).unwrap();
        ex
    };
    let session = |lib: &mut Lib, id: i64, at: i64, via: &str, ex: Option<i64>| {
        lib.conn.execute("INSERT INTO read_sessions(article_id,opened_at,exposure_id,active_ms,read_chars,skimmed_chars,total_chars,reached_end,translated,ended_by,via,idle_ms,max_block)
            VALUES(?1,?2,?3,400000,5700,0,6000,1,0,'back',?4,0,0)", params![id, at, ex, via]).unwrap();
        lib.read.insert(id);
    };
    for day in 0..=60_i64 {
        let today = day == 60;
        // News 1-5: 48/day; reads 3 of 8 on topics 0..3 (~45/day overall).
        for (t, topic) in TOPICS.iter().enumerate() {
            for (v, variant) in VARIANTS.iter().enumerate() {
                let slot = (t * 8 + v) as i64;
                let published = day * DAY_MS + slot * if today { 15 } else { 30 } * 60_000;
                for feed in 1..=5_i64 {
                    let id = insert(
                        &mut lib,
                        feed,
                        format!("{topic} {variant} n{feed}x{slot}d{day}"),
                        published,
                        axis(t),
                    );
                    if today {
                        continue;
                    }
                    let ex = expose(&lib, id, published + 30 * 60_000);
                    if t < 3 && (day + feed + v as i64) % 8 < 3 {
                        session(&mut lib, id, published + H, "focus", Some(ex));
                    }
                }
            }
        }
        // 6: weekly you read every issue; each issue a different topic.
        if day % 7 == 4 || today {
            let k = if today { 8 } else { (day / 7) as usize };
            let published = day * DAY_MS + 6 * H;
            let id = insert(&mut lib, 6, WEEKLY[k].into(), published, axis(6 + k));
            if today {
                lib.tracked.push(("weekly you read (6)", id));
            } else {
                let ex = expose(&lib, id, published + H);
                match (mode, k % 3) {
                    (Weekly::FocusSameDay, _) | (Weekly::Mixed, 0) => {
                        session(&mut lib, id, published + 2 * H, "focus", Some(ex))
                    }
                    (Weekly::Mixed, 1) => session(&mut lib, id, published + 2 * H, "feeds", None),
                    _ => session(&mut lib, id, published + 72 * H, "focus", Some(ex)),
                }
            }
        }
        // 7: weekly shown every issue, never read (off-topic for you).
        if day % 7 == 2 || today {
            let k = if today { 8 } else { (day / 7) as usize };
            let published = day * DAY_MS + 7 * H;
            let id = insert(&mut lib, 7, IGNORED[k].into(), published, axis(16 + k));
            if today {
                lib.tracked.push(("ignored weekly, off-topic (7)", id));
                let on = insert(
                    &mut lib,
                    7,
                    "robotaxi rollout field test".into(),
                    day * DAY_MS + 8 * H,
                    axis(2),
                );
                lib.tracked.push(("ignored weekly, ON-topic piece (7)", on));
            } else {
                expose(&lib, id, published + H);
            }
        }
        // 8: weekly never shown (new or buried subscription).
        if day % 7 == 5 || today {
            let k = if today { 8 } else { (day / 7) as usize };
            let id = insert(
                &mut lib,
                8,
                UNSEEN[k].into(),
                day * DAY_MS + 8 * H,
                axis(25 + k),
            );
            if today {
                lib.tracked.push(("never-shown weekly, off-topic (8)", id));
                let on = insert(
                    &mut lib,
                    8,
                    "smartphone launch teardown".into(),
                    day * DAY_MS + 9 * H,
                    axis(0),
                );
                lib.tracked
                    .push(("never-shown weekly, ON-topic piece (8)", on));
            }
        }
        // 9: mid-frequency blog, 5/day, you read the first post each day.
        for i in 0..5_i64 {
            let published = day * DAY_MS + (1 + 2 * i) * H;
            let n = (day * 5 + i) as usize;
            let title = format!(
                "{} {}",
                BLOG_WORDS[(n * 7) % 20],
                BLOG_WORDS[(n * 13 + 3) % 20]
            );
            let id = insert(&mut lib, 9, title, published, noise(day * 5 + i));
            if today {
                if i == 4 {
                    lib.tracked.push(("mid-frequency blog, newest (9)", id));
                }
                continue;
            }
            let ex = expose(&lib, id, published + 30 * 60_000);
            if i == 0 {
                session(&mut lib, id, published + H, "focus", Some(ex));
            }
        }
        // 10: covers your topic, but you always skip it.
        for hour in [2_i64, 14] {
            if today && hour == 14 {
                continue;
            }
            let published = day * DAY_MS + if today { 10 * H } else { hour * H };
            let id = insert(
                &mut lib,
                10,
                format!("robotaxi rollout hype h{day}x{hour}"),
                published,
                axis(2),
            );
            if today {
                lib.tracked
                    .push(("on-topic source you always skip (10)", id));
            } else {
                expose(&lib, id, published + 30 * 60_000);
            }
        }
    }
    lib.conn.execute_batch("COMMIT").unwrap();
    lib
}


// What the model learns, without the daily source draw; the draw has its own
// contract below.
#[test]
fn fitted_ten_source_contract() {
    let tuning = crate::Tuning { source_draw: (0.0, f64::INFINITY), ..crate::Tuning::DEFAULT };
    crate::tuning::with(tuning, ten_source_contract);
}

#[test]
fn daily_source_draw_rotates_near_ties_and_keeps_clear_preferences() {
    for (mode, unshown) in [(Weekly::FocusSameDay, false), (Weekly::Mixed, true)] {
        let lib = build(mode);
        if unshown {
            lib.conn.execute("DELETE FROM exposures WHERE article_id IN (SELECT article_id FROM read_sessions WHERE via='feeds')", []).unwrap();
        }
        store::set_switch(&lib.conn, store::FITTED_ENABLED_KEY, true).unwrap();
        let model = Model::load(&lib.conn, NOW + 1).unwrap();
        let ids: Vec<_> = lib
            .published
            .iter()
            .filter(|(id, p)| !lib.read.contains(id) && **p >= NOW - 30 * DAY_MS && **p <= NOW)
            .map(|(&id, &p)| (id, p))
            .collect();
        let tracked = |feed, on: Option<bool>| {
            lib.tracked
                .iter()
                .find(|(name, id)| lib.feed[id] == feed && on.is_none_or(|on| name.contains("ON-topic") == on))
                .unwrap()
                .1
        };
        let (weekly, rejected, on_topic) = (tracked(6, None), tracked(10, None), tracked(7, Some(true)));
        let (mut weekly_ranks, mut rejected_ranks, mut on_topic_ranks, mut leaders) = (vec![], vec![], vec![], BTreeSet::new());
        for day in 0..28 {
            let at = NOW + day * DAY_MS;
            let mut ranked: Vec<_> = ids
                .iter()
                .map(|&(id, p)| (model.article_boost(id, at, Switches::ALL).unwrap().score, p, id))
                .collect();
            ranked.sort_by(|a, b| b.cmp(a));
            let rank = |id| ranked.iter().position(|r| r.2 == id).unwrap() + 1;
            weekly_ranks.push(rank(weekly));
            rejected_ranks.push(rank(rejected));
            on_topic_ranks.push(rank(on_topic));
            leaders.insert(lib.feed[&ranked.iter().find(|r| lib.feed[&r.2] <= 5).unwrap().2]);
        }
        println!("draw unshown={unshown} weekly={weekly_ranks:?} rejected_min={} on_topic={on_topic_ranks:?} news_leaders={leaders:?}",
            rejected_ranks.iter().min().unwrap());
        // Clear preferences survive every day's draw; near ties take turns.
        assert!(weekly_ranks.iter().all(|r| *r <= 10));
        assert!(weekly_ranks.iter().filter(|r| **r <= 3).count() >= 19);
        assert!(rejected_ranks.iter().all(|r| *r > 100));
        // The skipped source's on-topic article reaches the top ten on some days.
        assert!(on_topic_ranks.iter().filter(|r| **r <= 10).count() >= 6);
        assert!(leaders.len() >= 3, "{leaders:?}");
    }
}

fn ten_source_contract() {
    for mode in [Weekly::FocusSameDay, Weekly::Mixed] {
        let lib = build(mode);
        for unshown in [false, true] {
            if unshown {
                if mode != Weekly::Mixed {
                    continue;
                }
                lib.conn.execute("DELETE FROM exposures WHERE article_id IN (SELECT article_id FROM read_sessions WHERE via='feeds')", []).unwrap();
            }
            store::set_switch(&lib.conn, store::FITTED_ENABLED_KEY, true).unwrap();
            let model = Model::load(&lib.conn, NOW + 1).unwrap();
            let fit = model.fitted.as_ref().expect("fitted model");
            // Shelf life replaces both bounded freshness features.
            assert!(matches!(model.policy, freshness::Policy::Shelf { quadratic: true }));
            assert_eq!(fit.mean[4], 0.0);
            assert!(fit.mean[2] <= 0.0);
            let mut ranked: Vec<_> = lib
                .published
                .iter()
                .filter(|(id, p)| !lib.read.contains(id) && **p >= NOW - 30 * DAY_MS && **p <= NOW)
                .map(|(&id, &p)| {
                    (
                        model.article_boost(id, NOW, Switches::ALL).unwrap().score,
                        p,
                        id,
                    )
                })
                .collect();
            ranked.sort_by(|a, b| b.cmp(a));
            if !unshown {
                report_cases(&lib, &model, &ranked, mode);
            }
            let tracked = |feed| {
                lib.tracked
                    .iter()
                    .find(|(_, id)| lib.feed[id] == feed)
                    .unwrap()
                    .1
            };
            let (weekly, rejected) = (tracked(6), tracked(10));
            let rank = |id| ranked.iter().position(|r| r.2 == id).unwrap() + 1;
            let news = ranked
                .iter()
                .find(|r| {
                    lib.feed[&r.2] <= 5
                        && TOPICS[..3].iter().any(|t| lib.title[&r.2].starts_with(t))
                })
                .unwrap()
                .2;
            println!(
                "contract mixed={} unshown={unshown} samples={} positive={} candidates={} weekly_rank={} rejected_rank={} news_rank={}",
                mode == Weekly::Mixed,
                fit.report.samples,
                fit.report.positives,
                ranked.len(),
                rank(weekly),
                rank(rejected),
                rank(news)
            );
            assert!(fit.sources[&6] > 0.0 && fit.sources[&10] < 0.0);
            assert!(fit.sources[&7].abs() < fit.sources[&6] / 2.0);
            assert!(fit.report.beta[0] > 0.0);
            assert!(rank(weekly) <= 3 && rank(rejected) > rank(news) && rank(news) <= 10);
            // Attribution shares fitted lexical/vector terms; vector availability
            // is not an interest topic. Cached contributions survive round trips.
            let mut checked = 0;
            for &(_, _, id) in &ranked {
                let Some((_, Some(topic))) = model.article_rank(id, NOW, Switches::ALL) else {
                    continue;
                };
                let a = &model.articles[&id];
                assert_eq!(
                    topic.features,
                    fit.report.beta[0] * (model.features.raw_score(&a.chips) - fit.mean[0])
                );
                let vector = model
                    .vectors
                    .get(&id)
                    .map(|v| {
                        affinity::combined_score(
                            model.affinity.score(v).0,
                            model.recent_affinity.score(v).0,
                        )
                    })
                    .unwrap_or(0.0);
                assert_eq!(topic.affinity, fit.report.beta[1] * (vector - fit.mean[1]));
                assert!(topic.valid());
                assert!(
                    crate::topics::Lane::decode(
                        &serde_json::to_string(&crate::topics::Lane::from_evidence(&topic))
                            .unwrap()
                    )
                    .is_some()
                );
                checked += 1;
            }
            assert!(checked > 0);
        }
    }
}

// Compare identical unread candidates, before pool admission/mixing/calibration.
fn report_cases(lib: &Lib, model: &Model, ranked: &[(i64, i64, i64)], mode: Weekly) {
    let current = Model::load_with_fitted(&lib.conn, NOW + 1, true, None, false).unwrap();
    let mut baseline: Vec<_> = ranked
        .iter()
        .map(|&(_, p, id)| {
            (
                current.article_boost(id, NOW, Switches::ALL).unwrap().score,
                p,
                id,
            )
        })
        .collect();
    baseline.sort_by(|a, b| b.cmp(a));
    let mut chrono = ranked.to_vec();
    chrono.sort_by_key(|r| std::cmp::Reverse((r.1, r.2)));
    let lists = [&baseline[..], ranked, &chrono[..]];
    let fit = model.fitted.as_ref().unwrap();
    let old = |id: i64| lib.feed[&id] == 9 && NOW - lib.published[&id] >= DAY_MS;
    let unrelated =
        |id: i64| lib.feed[&id] <= 5 && TOPICS[3..].iter().any(|t| lib.title[&id].starts_with(t));
    println!(
        "cases {}",
        serde_json::json!({
            "mixed": mode == Weekly::Mixed, "candidates": ranked.len(),
            "samples": fit.report.samples, "positives": fit.report.positives,
            "old_candidates": ranked.iter().filter(|r| old(r.2)).count(),
            "old_top40": lists.map(|rows| rows.iter().take(40).filter(|r| old(r.2)).count()),
            "old_top40_age_days": lists.map(|rows| rows.iter().take(40).filter(|r| old(r.2)).map(|r| (NOW - r.1) as f64 / DAY_MS as f64).collect::<Vec<_>>()),
            "unrelated_top10": lists.map(|rows| rows.iter().take(10).filter(|r| unrelated(r.2)).count()),
            "beta": fit.report.beta, "intercept": fit.intercept,
        })
    );
    let news = ranked
        .iter()
        .find(|r| lib.feed[&r.2] <= 5 && !unrelated(r.2))
        .unwrap()
        .2;
    let highest_old = ranked.iter().find(|r| old(r.2)).unwrap().2;
    let rejected_topic = ranked.iter().find(|r| unrelated(r.2)).unwrap().2;
    for id in lib
        .tracked
        .iter()
        .map(|r| r.1)
        .chain([news, highest_old, rejected_topic])
    {
        let detail = fitted_detail(model, id);
        println!(
            "case {}",
            serde_json::json!({
                "mixed": mode == Weekly::Mixed, "id": id, "feed": lib.feed[&id],
                "candidates": ranked.len(), "samples": fit.report.samples,
                "ranks": lists.map(|rows| rows.iter().position(|r| r.2 == id).unwrap() + 1),
                "age_days": (NOW - lib.published[&id]) as f64 / DAY_MS as f64,
                "detail": detail,
            })
        );
    }
    let rank = |id| ranked.iter().position(|r| r.2 == id).unwrap() + 1;
    let on_topic = |feed| {
        lib.tracked
            .iter()
            .find(|(name, id)| lib.feed[id] == feed && name.contains("ON-topic"))
            .unwrap()
            .1
    };
    let newest_blog = lib
        .tracked
        .iter()
        .find(|(_, id)| lib.feed[id] == 9)
        .unwrap()
        .1;
    assert!(
        rank(newest_blog) <= 20,
        "new mid-frequency article must be reachable"
    );
    // The legacy per-source interval freshness gave every quiet feed's fresh
    // article about one log-odds unit over hours-old news, which also lifted
    // this one into the top ten. Under the shelf life both are fresh, and the
    // skipped source's fitted penalty remains: a known gap, tracked with the
    // fitted model's narrow coverage. It must still beat its own source's
    // off-topic issue and every unrelated topic.
    let own_issue = lib
        .tracked
        .iter()
        .find(|(name, id)| lib.feed[id] == 7 && !name.contains("ON-topic"))
        .unwrap()
        .1;
    assert!(
        rank(on_topic(7)) < rank(own_issue)
            && ranked.iter().take(rank(on_topic(7))).all(|r| !unrelated(r.2)),
        "topic match must overcome unrelated source skips"
    );
    assert!(
        rank(on_topic(8)) <= 10,
        "unshown source can transfer topic interest"
    );
    assert_eq!(ranked.iter().take(10).filter(|r| unrelated(r.2)).count(), 0);
}

// Centered logit contributions, before score clipping and integer rounding.
fn fitted_detail(model: &Model, id: i64) -> serde_json::Value {
    let a = &model.articles[&id];
    let fit = model.fitted.as_ref().unwrap();
    let vector = model.vectors.get(&id);
    let published = a.published.or(Some(a.created));
    let (age, absolute) = match model.policy {
        freshness::Policy::Shelf { quadratic } => (
            if model.exempt.contains(&a.feed) {
                0.
            } else {
                -freshness::shelf_cost(published, model.fitted_before, model.windows[&a.feed], quadratic)
            },
            0.,
        ),
        freshness::Policy::Legacy => (
            freshness::value(a.published, model.fitted_before, model.half_lives[&a.feed]),
            freshness::value(a.published, model.fitted_before, fitted::ABSOLUTE_HALF_LIFE),
        ),
    };
    let raw = [
        model.features.raw_score(&a.chips),
        vector
            .map(|v| {
                affinity::combined_score(
                    model.affinity.score(v).0,
                    model.recent_affinity.score(v).0,
                )
            })
            .unwrap_or(0.),
        age,
        coverage::score(
            a.cluster
                .and_then(|c| model.cluster_feeds.get(&c))
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        )
        .1,
        absolute,
    ];

    let (parts, source) = fit.contributions(a.feed, raw);
    let source = source + fitted::source_draw(fit, a.feed, model.fitted_before);
    let boost = model
        .article_boost(id, model.fitted_before, Switches::ALL)
        .unwrap();
    assert!((boost.raw_score - (parts.iter().sum::<f64>() + source).clamp(-8., 8.)).abs() < 1e-10);
    serde_json::json!({"parts":parts,"source":source,"raw_score":boost.raw_score,
        "score":boost.score,"samples":fit.report.samples,"positives":fit.report.positives})
}

#[test]
#[ignore = "synthetic Debug load timing"]
fn fitted_load_performance() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("fitted-perf.db")).unwrap();
    conn.execute_batch("BEGIN").unwrap();
    for feed in 1..=100 {
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(?1,?2,0)",
            params![feed, format!("https://f{feed}.example/rss")],
        )
        .unwrap();
    }
    conn.execute("INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at)
        VALUES(1,'','fixture',?1,1,0,'active',0)", [DIMS as i64]).unwrap();
    for id in 1..=30000_i64 {
        let day = (id - 1) / 500;
        let at = day * DAY_MS + (id % 500) * 60000;
        conn.execute(
            "INSERT INTO articles(id,feed_id,title,fulltext_md,published_at,created_at)
            VALUES(?1,?2,?3,'body',?4,?4)",
            params![
                id,
                1 + (id - 1) % 100,
                format!(
                    "{} {}",
                    TOPICS[(id % 6) as usize],
                    VARIANTS[(id / 6 % 8) as usize]
                ),
                at
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chunks(article_id,ordinal,kind,text,embedding,model,space_id)
            VALUES(?1,0,'title_summary','',?2,'fixture',1)",
            params![id, rrss_core::rag::f32s_to_le(&axis((id % 6) as usize))],
        )
        .unwrap();
        if id % 3 != 0 {
            conn.execute("INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
                VALUES(?1,'focus',?2,0,0,0,'baseline',?3,?4)",params![id,id%40,format!("s{id}"),at+H]).unwrap();
            if id % 6 < 3 {
                conn.execute("INSERT INTO read_sessions(article_id,opened_at,active_ms,idle_ms,read_chars,skimmed_chars,total_chars,max_block,reached_end,translated,ended_by,via)
                    VALUES(?1,?2,400000,0,5700,0,6000,0,1,0,'back','focus')",params![id,at+2*H]).unwrap();
            }
        }
    }
    conn.execute_batch("COMMIT").unwrap();
    store::set_switch(&conn, store::FITTED_ENABLED_KEY, true).unwrap();
    let start = std::time::Instant::now();
    let model = Model::load(&conn, 60 * DAY_MS).unwrap();
    let elapsed = start.elapsed();
    let fit = model.fitted.as_ref().expect("fitted model");
    assert_eq!(fit.report.samples, 20000);
    assert_eq!(fit.sources.len(), 100);
    println!(
        "performance articles=30000 samples=20000 sources=100 load_ms={:.2}",
        elapsed.as_secs_f64() * 1000.0,
    );
}

#[test]
fn weekly_cutoff_labels_and_fallbacks() {
    // The legacy freshness features' training contract (evaluation baseline);
    // the shelf-life default is covered by the ten-source contract.
    let tuning = crate::Tuning { freshness: Some(freshness::Policy::Legacy), ..crate::Tuning::DEFAULT };
    crate::tuning::with(tuning, weekly_cutoff_legacy);
}

fn weekly_cutoff_legacy() -> (tempfile::TempDir, rrss_core::db::PooledConnection) {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("cutoffs.db")).unwrap();
    conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://one.example/rss',0),(2,'https://two.example/rss',0)").unwrap();
    conn.execute("INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at)
        VALUES(1,'','fixture',?1,1,0,'active',0)", [DIMS as i64]).unwrap();
    // All outcomes are in the same UTC epoch week. Training topic features must
    // be zero, although the current scoring profile already knows these topics.
    let at = 7 * DAY_MS + H;
    for id in 1..=100 {
        conn.execute("INSERT INTO articles(id,feed_id,title,fulltext_md,created_at) VALUES(?1,?2,?3,'body',?4)",
            params![id,1+id%2,if id <= 60 { "orbital mechanics" } else { "parking levy" },at-1000]).unwrap();
        conn.execute(
            "INSERT INTO chunks(article_id,ordinal,kind,text,embedding,model,space_id)
            VALUES(?1,0,'title_summary','',?2,'fixture',1)",
            params![id, rrss_core::rag::f32s_to_le(&axis((id % 2) as usize))],
        )
        .unwrap();
        if id <= 80 {
            for (shown, position) in [(at, id % 40), (at + 1, 0)] {
                conn.execute("INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
                    VALUES(?1,'focus',?2,0,0,0,'baseline',?3,?4)",params![id,position,format!("s{id}-{shown}"),shown]).unwrap();
            }
        }
        if id <= 60 || id > 80 {
            conn.execute("INSERT INTO read_sessions(article_id,opened_at,active_ms,idle_ms,read_chars,skimmed_chars,total_chars,max_block,reached_end,translated,ended_by,via)
                VALUES(?1,?2,?3,0,?4,0,6000,0,?5,0,'back','feeds')",
                params![id,at+H,if id > 95 { 500 } else { 400000 },if id > 95 { 0 } else { 5700 },id<=95]).unwrap();
        }
    }
    conn.execute("INSERT INTO article_events(article_id,kind,at) VALUES(1,'down',?1),(70,'up',?1),(71,'star',?1),(71,'unstar',?2)",params![at+2*H,at+3*H]).unwrap();
    store::set_switch(&conn, store::FITTED_ENABLED_KEY, true).unwrap();
    let fitted = Model::load(&conn, at + 4 * H).unwrap();
    store::set_switch(&conn, store::FITTED_ENABLED_KEY, false).unwrap();
    let fit = fitted.fitted.as_ref().unwrap();
    // Exploration is a Focus opportunity, including skipped trials.
    conn.execute(
        "UPDATE exposures SET variant='explore' WHERE article_id%2=0",
        [],
    )
    .unwrap();
    store::set_switch(&conn, store::FITTED_ENABLED_KEY, true).unwrap();
    let exploration = Model::load(&conn, at + 4 * H).unwrap();
    let trial_fit = exploration.fitted.as_ref().unwrap();
    assert_eq!(fit.report.samples, trial_fit.report.samples);
    assert_eq!(fit.report.positives, trial_fit.report.positives);
    assert_eq!(fit.report.beta, trial_fit.report.beta);
    assert_eq!(
        fitted.features.raw_score(&fitted.articles[&80].chips),
        exploration
            .features
            .raw_score(&exploration.articles[&80].chips)
    );
    store::set_switch(&conn, store::FITTED_ENABLED_KEY, false).unwrap();
    assert_eq!(fit.report.samples, 100);
    assert_eq!(fit.report.positives, 75); // 75 deep + up - down; unstar revoked.
    assert_eq!(fit.mean[0], 0.0);
    assert_eq!(fit.mean[1], 0.0);
    assert_eq!(fit.report.beta[1], 2.0 * fit.report.beta[0]);
    let unshown_weight = 15.0 * fitted::SIDEBAR_WEIGHT + 5.0;
    for (j, half_life) in [(2, 14 * DAY_MS), (4, fitted::ABSOLUTE_HALF_LIFE)] {
        let expected = (2_f64.powf(-1000.0 / half_life as f64) * 80.0
            + 2_f64.powf(-(H + 1000) as f64 / half_life as f64) * unshown_weight)
            / (80.0 + unshown_weight);
        assert!((fit.mean[j] - expected).abs() < 1e-12);
    }
    assert!(fitted.features.raw_score(&fitted.articles[&1].chips).abs() > 0.0);
    let off = Switches {
        features: false,
        source: false,
        coverage: false,
        freshness: false,
        affinity: false,
    };
    assert_eq!(fitted.article_boost(1, at, off).unwrap().score, 0);
    // Fitted scores follow the model's cutoff, not the scoring time, except
    // for the source effect's daily draw: fixed within a UTC day.
    let day = at.div_euclid(DAY_MS) * DAY_MS;
    assert_eq!(
        fitted.article_boost(1, day, Switches::ALL),
        fitted.article_boost(1, day + DAY_MS - 1, Switches::ALL)
    );
    let (fit, feed) = (fitted.fitted.as_ref().unwrap(), fitted.articles[&1].feed);
    let raw = |at| fitted.article_boost(1, at, Switches::ALL).unwrap().raw_score;
    let drawn = fitted::source_draw(fit, feed, at + DAY_MS) - fitted::source_draw(fit, feed, at);
    assert!(drawn != 0.0 && (raw(at + DAY_MS) - raw(at) - drawn).abs() < 1e-10);
    // The switch must publish scores from its requested model, even though the
    // stored setting still has its previous value while the plan is computed.
    #[cfg(feature = "evaluation-clock")]
    for enabled in [true, false] {
        let now = at + 4 * H - 1;
        rrss_core::evaluation_clock::at_time(now, || {
            store::set_switch(&conn, store::FITTED_ENABLED_KEY, enabled)
        })
        .unwrap();
        let model = Model::load(&conn, now + 1).unwrap();
        assert_eq!(model.fitted.is_some(), enabled);
        for id in 1..=100 {
            let (boost, topics) = model.article_rank(id, now, Switches::ALL).unwrap();
            let cached: (i64, String) = conn
                .query_row(
                    "SELECT boost,reasons_json FROM article_rank WHERE article_id=?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(
                cached,
                (
                    boost.score,
                    crate::cache::encode(&boost.reasons, topics).unwrap()
                )
            );
        }
    }
    // Future records do not alter a model fitted at the earlier cutoff.
    conn.execute(
        "INSERT INTO article_events(article_id,kind,at) VALUES(2,'down',?1)",
        [at + 5 * H],
    )
    .unwrap();
    store::set_switch(&conn, store::FITTED_ENABLED_KEY, true).unwrap();
    let earlier = Model::load_with_affinity(&conn, at + 4 * H, true, Some(&[1])).unwrap();
    store::set_switch(&conn, store::FITTED_ENABLED_KEY, false).unwrap();
    assert_eq!(
        fit.report.beta,
        earlier.fitted.as_ref().unwrap().report.beta
    );
    assert_eq!(
        fitted.article_boost(1, at, Switches::ALL),
        earlier.article_boost(1, at, Switches::ALL)
    );
    // One-source and <50-positive cases fall back exactly to the legacy scorer.
    for single in [true, false] {
        if single {
            conn.execute("UPDATE articles SET feed_id=1", []).unwrap();
        } else {
            conn.execute("UPDATE articles SET feed_id=1+id%2", [])
                .unwrap();
            conn.execute("DELETE FROM read_sessions WHERE article_id>20", [])
                .unwrap();
        }
        store::set_switch(&conn, store::FITTED_ENABLED_KEY, true).unwrap();
        let fallback = Model::load(&conn, at + 4 * H).unwrap();
        store::set_switch(&conn, store::FITTED_ENABLED_KEY, false).unwrap();
        let baseline = Model::load(&conn, at + 4 * H).unwrap();
        assert!(fallback.fitted.is_none());
        for id in 1..=100 {
            assert_eq!(
                fallback.article_boost(id, at, Switches::ALL),
                baseline.article_boost(id, at, Switches::ALL)
            );
        }
    }
    (dir, conn)
}

#[test]
fn skip_discount_uses_first_showing() {
    let exposures: Vec<_> = [9, 10, 29, 30]
        .into_iter()
        .enumerate()
        .flat_map(|(id, position)| {
            [
                (
                    LabelExposure {
                        id: id as i64,
                        article_id: id as i64,
                        position,
                        shown_at: 1,
                    },
                    1,
                ),
                (
                    LabelExposure {
                        id: id as i64 + 10,
                        article_id: id as i64,
                        position: 0,
                        shown_at: 2,
                    },
                    1,
                ),
            ]
        })
        .collect();
    let mut votes = Vec::new();
    fitted::add_skips(&mut votes, &exposures, &[], &[]);
    assert_eq!(
        votes
            .iter()
            .map(|v| (v.article_id, v.weight))
            .collect::<Vec<_>>(),
        vec![(0, -0.15), (1, -0.075), (2, -0.075)]
    );
}

#[test]
fn repeated_exposures_do_not_multiply_fitted_samples() {
    let lib = build(Weekly::Mixed);
    db::set_setting_enabled(&lib.conn, store::FITTED_ENABLED_KEY, true).unwrap();
    let first = Model::load(&lib.conn, NOW + 1).unwrap();
    let before = first.fitted.as_ref().expect("fitted model");
    let original: i64 = lib
        .conn
        .query_row("SELECT COUNT(*) FROM exposures", [], |r| r.get(0))
        .unwrap();
    // Re-show the same articles on six later days; preserve first showings,
    // outcomes and the training window so only exposure multiplicity changes.
    for day in 1..7 {
        lib.conn.execute(
            "INSERT INTO exposures(article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
             SELECT article_id,surface,0,score,interest_boost,rank_score,variant,
                    'repeat-' || ?1 || '-' || id,shown_at+?1*?2 FROM exposures
             WHERE id<=?3 AND shown_at>=?4-60*?2 AND shown_at<?4-7*?2",
            params![day, DAY_MS, original, NOW + 1],
        ).unwrap();
    }
    let repeated = Model::load(&lib.conn, NOW + 1).unwrap();
    let after = repeated.fitted.as_ref().unwrap();
    assert_eq!(before.report.samples, after.report.samples);
    assert_eq!(before.report.positives, after.report.positives);
    assert_eq!(before.report.beta, after.report.beta);
    assert_eq!(before.sources, after.sources);
    for (_, id) in &lib.tracked {
        assert_eq!(
            first.article_boost(*id, NOW, Switches::ALL),
            repeated.article_boost(*id, NOW, Switches::ALL)
        );
    }
    let total: i64 = lib
        .conn
        .query_row("SELECT COUNT(*) FROM exposures", [], |r| r.get(0))
        .unwrap();
    println!(
        "exposure_multiplicity rows_before={original} rows_after={total} samples={} positives={} tracked={}",
        after.report.samples,
        after.report.positives,
        lib.tracked.len()
    );
}
