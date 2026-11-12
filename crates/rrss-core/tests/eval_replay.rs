//! 阶段 23「M2 离线回放」「接入方式」：零模型费用、只读的开发命令与固定夹具。
//! cargo test -p rrss-core --test eval_replay -- --ignored --nocapture

use std::path::Path;

use rrss_core::{
    db,
    eval::replay::{Config, Replay, open_read_only},
    rag,
    rules::{self, Dimension, NewRule, Polarity},
};
use rusqlite::{Connection, params};

const NOW: i64 = 1_800_000_000_000;
fn configs() -> [Config; 7] {
    [
        (false, false, false, -10),
        (true, false, false, -10),
        (false, true, false, -10),
        (true, true, false, -10),
        (true, true, false, 0),
        (false, false, true, -10),
        (false, false, true, -10),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (rules, interests, b1, hide_threshold))| Config {
        b2: i == 6,
        rules,
        interests,
        b1,
        hide_threshold,
        b1_components: rrss_core::rank::combine::Switches::ALL,
    })
    .collect::<Vec<_>>()
    .try_into()
    .unwrap()
}

fn fixture(path: &Path, count: i64) {
    let conn = db::open_at(path).unwrap();
    db::set_setting(&conn, "pi_auto_download", "0").unwrap();
    conn.execute(
        "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://eval.invalid/feed',0),(2,'https://other.invalid/feed',0)",
        [],
    )
    .unwrap();
    rules::insert_rule(
        &conn,
        NewRule::manual_global(Polarity::Like, Dimension::TitleKw, "positive"),
    )
    .unwrap();
    conn.execute("INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at) VALUES(1,'','fixture',2,1,0,'active',0)", []).unwrap();
    let iid = rrss_core::interests::insert_interest(&conn, "fixture", "manual").unwrap();
    rag::set_interest_embedding(&conn, iid, &[1.0, 0.0]).unwrap();
    for id in 1..=count {
        let positive = id <= count / 2;
        conn.execute(
            "INSERT INTO articles(id,feed_id,title,published_at,created_at,score,interest_boost,author)
            VALUES(?1,?4,?2,?3,0,777,888,?5)",
            params![
                id,
                if positive { "positive" } else { "negative" },
                NOW - count + id,
                if positive {1} else {2},
                if positive {"Ada"} else {"Noise"}
            ],
        )
        .unwrap();
        conn.execute("INSERT INTO exposures(id,article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
            VALUES(?1,?1,'focus',0,777,888,1665,'baseline','fixture',?2)",params![id,NOW]).unwrap();
        // exposure_id=NULL 也是有效的 E0 输入；按文章 / 时间恢复关联。
        conn.execute(
            "INSERT INTO article_events(article_id,kind,at) VALUES(?1,?2,?3)",
            params![id, if positive { "up" } else { "down" }, NOW + 1],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO chunks(article_id,ordinal,kind,text,embedding,model,space_id)
            VALUES(?1,0,'title_summary','fixture',?2,'fixture',1)",
            params![
                id,
                rag::f32s_to_le(if positive { &[1.0, 0.0] } else { &[0.0, 1.0] })
            ],
        )
        .unwrap();
    }
    // 训练严格位于 30 天回放窗口之前，同时落在源偏好的 60 天窗口内。
    let history_at = NOW - 40 * 86_400_000;
    for id in 1001..=1040 {
        let positive = id <= 1020;
        conn.execute("INSERT INTO articles(id,feed_id,title,author,published_at,created_at) VALUES(?1,?2,'history',?3,?4,?4)",
            params![id,if positive {1} else {2},if positive {"Ada"} else {"Noise"},history_at-86_400_000]).unwrap();
        conn.execute("INSERT INTO exposures(id,article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at) VALUES(?1,?1,'focus',0,0,0,0,'baseline','history',?2)",params![id,history_at]).unwrap();
        conn.execute(
            "INSERT INTO chunks(article_id,ordinal,kind,text,embedding,model,space_id)
            VALUES(?1,0,'title_summary','history',?2,'fixture',1)",
            params![
                id,
                rag::f32s_to_le(if positive { &[1.0, 0.0] } else { &[0.0, 1.0] })
            ],
        )
        .unwrap();
        if positive {
            conn.execute(
                "INSERT INTO article_events(article_id,kind,at) VALUES(?1,'open',?2)",
                params![id, history_at + 1],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO article_events(article_id,kind,at) VALUES(?1,'up',?2)",
                params![id, history_at + 2],
            )
            .unwrap();
        } else {
            // E0 之前赞踩回退，同篇即使有曝光也不能丢掉 feedback。
            conn.execute(
                "INSERT INTO feedback(article_id,kind,created_at) VALUES(?1,'down',?2)",
                params![id, history_at + 2],
            )
            .unwrap();
        }
    }
}

#[test]
#[ignore = "explicit offline replay; RRSS_EVAL_DB or fixed disposable fixture; no models"]
fn eval_replay() {
    // 仅消费用户明确指定的 DB 入口，不打印路径或读取其他环境值。
    if let Some(path) = std::env::var_os("RRSS_EVAL_DB") {
        let conn =
            open_read_only(Path::new(&path)).expect("cannot open evaluation database read-only");
        println!(
            "{}",
            Replay::latest(&conn)
                .expect("cannot load evaluation window")
                .report(&configs())
        );
    } else {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("eval.db");
        fixture(&path, 100);
        let conn = open_read_only(&path).unwrap();
        let replay = Replay::latest(&conn).unwrap();
        println!("{}", replay.report(&configs()));
        let rules = replay.evaluate(configs()[1]);
        let off = replay.evaluate(configs()[0]);
        assert_eq!(replay.evaluate(configs()[5]).ranked.unwrap().auc, Some(1.0));
        let ranked = rules.ranked.unwrap();
        assert!(ranked.ndcg.unwrap() > off.ranked.unwrap().ndcg.unwrap());
        assert!(ranked.ndcg.unwrap() > rules.chronological.unwrap().ndcg.unwrap());
        // 一半窗口内正例的词面属性、源相同于负例，B2 可补足 B1；历史训练不变。
        let writer = Connection::open(&path).unwrap();
        writer
            .execute(
                "UPDATE articles SET author='Noise',feed_id=2 WHERE id<=50 AND id%2=1",
                [],
            )
            .unwrap();
        let b2 = Replay::latest(&conn).unwrap();
        println!("{}", b2.report(&configs()[5..]));
        let on = b2.evaluate(configs()[6]).ranked.unwrap();
        let off = b2
            .evaluate(Config {
                b2: false,
                ..configs()[6]
            })
            .ranked
            .unwrap();
        assert!(on.auc.unwrap() > off.auc.unwrap());
        // 窗口内投票与向量不可以给 B2 训练；去掉历史后即退回同一 B1。
        writer
            .execute("DELETE FROM article_events WHERE at<?1", [NOW])
            .unwrap();
        writer.execute("DELETE FROM feedback", []).unwrap();
        let untrained = Replay::latest(&conn).unwrap();
        assert_eq!(
            untrained.evaluate(configs()[6]).ranked.unwrap().auc,
            untrained
                .evaluate(Config {
                    b2: false,
                    ..configs()[6]
                })
                .ranked
                .unwrap()
                .auc
        );
    }
}

#[test]
fn replay_recomputes_rules_and_interests_and_matches_focus_order() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("eval.db");
    fixture(&path, 100);
    let conn = Connection::open(&path).unwrap();
    // fixture 故意填错缓存；正常重算与回放都应忽略这些旧值。
    rrss_core::score::rescore_all(&conn).unwrap();
    rrss_core::interests::recompute_interest_boost(&conn).unwrap();
    let replay = Replay::load(&conn, NOW, NOW + 2).unwrap();
    for config in &configs()[1..4] {
        let result = replay.evaluate(*config);
        assert_eq!(result.ranked.unwrap().ndcg, Some(1.0));
        assert_eq!(result.chronological.unwrap().auc, Some(0.0));
    }
    let off = replay.evaluate(configs()[0]).ranked.unwrap();
    assert_eq!(off.auc, Some(0.0));
    assert_eq!(off.ndcg, Some(0.0));
}

#[test]
fn threshold_hides_by_rule_score_and_counts_positive_labels() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("eval.db");
    fixture(&path, 100);
    let conn = Connection::open(&path).unwrap();
    conn.execute("UPDATE articles SET title='positive bad' WHERE id=1", [])
        .unwrap();
    let mut dislike = NewRule::manual_global(Polarity::Dislike, Dimension::TitleKw, "bad");
    dislike.weight = 11; // 规则分 -1，兴趣加分 +2；仍须被 -1 阈值隐藏。
    rules::insert_rule(&conn, dislike).unwrap();
    let replay = Replay::load(&conn, NOW, NOW + 2).unwrap();
    let hidden = replay.evaluate(Config {
        rules: true,
        interests: true,
        b1: true,
        b2: false,
        b1_components: rrss_core::rank::combine::Switches::ALL,
        hide_threshold: -1,
    });
    assert_eq!(hidden.hidden_positives, 1);
    assert_eq!(hidden.labels, 99);
    assert_eq!(hidden.chronological.unwrap().auc, Some(0.0));
    assert_eq!(hidden.ranked.unwrap().ndcg, Some(1.0));
    let visible = replay.evaluate(configs()[3]);
    assert_eq!(visible.hidden_positives, 0);
    assert_eq!(visible.labels, 100);
}

#[test]
fn fewer_than_fifty_labels_has_no_metrics() {
    for (count, enough) in [(49, false), (50, true)] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("eval.db");
        fixture(&path, count);
        let conn = open_read_only(&path).unwrap();
        let result = Replay::load(&conn, NOW, NOW + 2)
            .unwrap()
            .evaluate(configs()[0]);
        assert_eq!(result.ranked.is_some(), enough);
        assert_eq!(result.chronological.is_some(), enough);
        if !enough {
            let report = Replay::load(&conn, NOW, NOW + 2)
                .unwrap()
                .report(&[configs()[0]]);
            assert!(report.ends_with("样本不足\n"));
            assert!(!report.contains("AUC="));
        }
    }
}

#[test]
fn read_only_replay_tolerates_missing_fields_deleted_articles_and_orphan_signals() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("eval.db");
    fixture(&path, 100);
    let writer = Connection::open(&path).unwrap();
    writer
        .execute_batch(
            "PRAGMA foreign_keys=OFF;
        DELETE FROM articles WHERE id=1;
        UPDATE articles SET published_at=NULL,author=NULL,canonical_url=NULL,content_html=NULL;
        UPDATE article_events SET exposure_id=999999 WHERE article_id=2;
        UPDATE article_events SET kind='future-event' WHERE article_id=3;
        INSERT INTO read_sessions(article_id,exposure_id,opened_at,active_ms,idle_ms,read_chars,
            skimmed_chars,total_chars,max_block,reached_end,translated,ended_by,via)
        VALUES(999999,NULL,1800000000001,1000,0,0,0,1000,0,0,0,'back','focus');
        INSERT INTO articles(id,feed_id,title,created_at) VALUES(101,1,'no exposure',0);
        INSERT INTO read_sessions(article_id,exposure_id,opened_at,active_ms,idle_ms,read_chars,
            skimmed_chars,total_chars,max_block,reached_end,translated,ended_by,via)
        VALUES(101,NULL,1800000000001,200000,0,1000,0,1000,0,1,0,'back','focus');
        PRAGMA user_version=1;",
        )
        .unwrap();
    let before: (i64, i64) = writer
        .query_row(
            "SELECT SUM(score),SUM(interest_boost) FROM articles",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let conn = open_read_only(&path).unwrap();
    let replay = Replay::load(&conn, NOW, NOW + 2).unwrap();
    assert_eq!(replay.evaluate(configs()[0]).labels, 98);
    assert!(
        conn.execute("UPDATE settings SET value='changed'", [])
            .is_err()
    );
    assert_eq!(conn.total_changes(), 0);
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 1);
    let after: (i64, i64) = conn
        .query_row(
            "SELECT SUM(score),SUM(interest_boost) FROM articles",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(before, after);
}

#[test]
fn b1_time_split_legacy_fallback_and_component_switches() {
    use rrss_core::rank::combine::Switches;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("eval.db");
    fixture(&path, 100);
    let conn = Connection::open(&path).unwrap();
    let only_features = Config {
        b1_components: Switches {
            features: true,
            source: false,
            coverage: false,
            freshness: false,
            affinity: false,
        },
        ..configs()[5]
    };
    let only_source = Config {
        b1_components: Switches {
            features: false,
            source: true,
            coverage: false,
            freshness: false,
            affinity: false,
        },
        ..configs()[5]
    };
    for config in [only_features, only_source] {
        assert_eq!(
            Replay::load(&conn, NOW, NOW + 2)
                .unwrap()
                .evaluate(config)
                .ranked
                .unwrap()
                .auc,
            Some(1.0)
        );
    }
    // 隔离历史投票回退：只保留 Noise 的旧踩仍足以排出正确作者偏好。
    conn.execute("DELETE FROM article_events WHERE article_id>=1001", [])
        .unwrap();
    assert_eq!(
        Replay::load(&conn, NOW, NOW + 2)
            .unwrap()
            .evaluate(only_features)
            .ranked
            .unwrap()
            .auc,
        Some(1.0)
    );
    // 同篇已有 E0 行为时不再使用旧 feedback（即使行为没有可判定标签）。
    conn.execute("INSERT INTO article_events(article_id,kind,at) SELECT article_id,'open',created_at FROM feedback",[]).unwrap();
    assert_eq!(
        Replay::load(&conn, NOW, NOW + 2)
            .unwrap()
            .evaluate(only_features)
            .ranked
            .unwrap()
            .auc,
        Some(0.0)
    );
    conn.execute("DELETE FROM article_events WHERE at<?1", [NOW])
        .unwrap();
    conn.execute("DELETE FROM feedback", []).unwrap();
    // 大量窗口内正负票不能给特征或源训练；关掉时效后仍为纯时间逆序。
    for config in [only_features, only_source] {
        assert_eq!(
            Replay::load(&conn, NOW, NOW + 2)
                .unwrap()
                .evaluate(config)
                .ranked
                .unwrap()
                .auc,
            Some(0.0)
        );
    }
}

#[test]
fn b1_learns_reading_without_exposures_and_preserves_unknown_metadata() {
    use rrss_core::rank::combine::Switches;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("eval.db");
    fixture(&path, 100);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "DELETE FROM article_events WHERE article_id>=1001;
        DELETE FROM feedback;
        DELETE FROM exposures WHERE article_id>=1001;
        UPDATE articles SET canonical_url=NULL,published_at=NULL WHERE id>=1001;
        INSERT INTO read_sessions(article_id,exposure_id,opened_at,active_ms,idle_ms,
            read_chars,skimmed_chars,total_chars,max_block,reached_end,translated,ended_by,via)
        SELECT id,NULL,created_at+86400000,CASE WHEN id<=1020 THEN 200000 ELSE 1000 END,0,
            CASE WHEN id<=1020 THEN 1000 ELSE 0 END,0,1000,0,id<=1020,0,'back','focus'
        FROM articles WHERE id>=1001;",
    )
    .unwrap();
    let config = Config {
        b1_components: Switches {
            features: true,
            source: false,
            coverage: false,
            freshness: false,
            affinity: false,
        },
        ..configs()[5]
    };
    assert_eq!(
        Replay::load(&conn, NOW, NOW + 2)
            .unwrap()
            .evaluate(config)
            .ranked
            .unwrap()
            .auc,
        Some(1.0)
    );
}
