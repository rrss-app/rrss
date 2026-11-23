//! 焦点页组装的测试；卡片与可见性本身的测试在 core 的 `articles`。
use super::*;
use rrss_core::articles::*;
use rrss_core::db::{PooledConnection, open_at};
use rrss_core::feed_scope::FeedScope;
use rrss_core::feeds::{SubscribeRequest, subscribe};
use rrss_core::parse::ParsedEntry;
use rrss_core::rules::{Dimension, NewRule, Polarity, insert_rule};
use rusqlite::{Connection, params};
use tempfile::tempdir;

fn open_temp() -> (tempfile::TempDir, PooledConnection) {
    let dir = tempdir().unwrap();
    let conn = open_at(&dir.path().join("rrss.db")).unwrap();
    (dir, conn)
}

fn seed(conn: &Connection) {
    let feed = subscribe(
        conn,
        SubscribeRequest {
            url: "https://news.example.com/rss.xml".into(),
            folder_id: None,
            as_rsshub: false,
            rsshub_instance_id: None,
        },
    )
    .unwrap();
    insert_rule(
        conn,
        NewRule::manual_global(Polarity::Like, Dimension::TitleKw, "focus"),
    )
    .unwrap();
    insert_rule(
        conn,
        NewRule::manual_global(Polarity::Dislike, Dimension::TitleKw, "noise"),
    )
    .unwrap();
    let entries = [
        entry("focus piece", "https://news.example.com/a"),
        entry("neutral piece", "https://news.example.com/b"),
        entry("noise piece", "https://news.example.com/c"),
    ];
    rrss_core::ingest::upsert_entries(conn, feed, &entries, now_ms()).unwrap();
}

fn entry(title: &str, url: &str) -> ParsedEntry {
    ParsedEntry {
        guid: url.into(),
        canonical_url: Some(url.into()),
        title: title.into(),
        author: None,
        published_at: Some(now_ms() - 60_000),
        content_html: Some("<p>x</p>".into()),
        hash: rrss_core::parse::entry_hash(title, Some(url), Some("<p>x</p>")),
        tags: vec![],
    }
}

#[test]
fn learned_boost_changes_order_only_never_visibility_or_membership() {
    let dir = tempfile::tempdir().unwrap();
    let conn = rrss_core::db::open_at(&dir.path().join("rank.db")).unwrap();
    conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://rank.invalid/feed',0);
        INSERT INTO articles(id,feed_id,title,score,interest_boost,created_at,starred) VALUES
        (1,1,'hidden',-10,0,0,0),(2,1,'neutral',0,0,0,0),(3,1,'liked',10,0,0,1),(4,1,'liked second',10,0,0,1);").unwrap();
    conn.execute("UPDATE articles SET created_at=?1", [now_ms()])
        .unwrap();
    rrss_core::db::set_setting(&conn, rrss_core::db::HIDDEN_SPOT_CHECK_RATE_KEY, "0").unwrap();
    let ids = || {
        list_focus_page(&conn, 40, None, &[], &[], FeedScope::All)
            .unwrap()
            .0
            .into_iter()
            .map(|c| c.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(), vec![4, 3, 2]);
    conn.execute_batch("INSERT INTO article_rank VALUES(1,1000,'[]',2,0),(2,100,'[]',2,0),(3,10,'[]',2,0),(4,-100,'[]',2,0);").unwrap();
    assert_eq!(ids(), vec![2, 3, 4]);
    set_focus_filter(&conn, FocusFilter::FocusOnly).unwrap();
    assert_eq!(ids(), vec![3, 4]); // neutral +100 不能变成关注，liked -100 不能掉出。
    rrss_core::db::set_setting(&conn, rrss_core::db::LIST_SCOPE_KEY, "starred").unwrap();
    assert_eq!(ids(), vec![3, 4]);
    conn.execute("UPDATE articles SET read=1 WHERE id=3", [])
        .unwrap();
    rrss_core::db::set_setting(&conn, rrss_core::db::LIST_SCOPE_KEY, "unread").unwrap();
    assert_eq!(ids(), vec![4]);
}

#[test]
fn slider_filters() {
    let (_dir, conn) = open_temp();
    seed(&conn);
    assert_eq!(focus_filter(&conn).unwrap(), FocusFilter::HideFaded);
    let faded = list_focus(&conn, 10).unwrap();
    let titles: Vec<_> = faded.iter().map(|c| c.title.as_str()).collect();
    assert_eq!(titles, vec!["focus piece", "neutral piece"]);

    set_focus_filter(&conn, FocusFilter::FocusOnly).unwrap();
    let focus = list_focus(&conn, 10).unwrap();
    assert_eq!(focus.len(), 1);
    assert_eq!(focus[0].title, "focus piece");
    assert_eq!(focus[0].score, 10);

    set_focus_filter(&conn, FocusFilter::ShowAll).unwrap();
    let all = list_focus(&conn, 10).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].title, "focus piece");
    assert_eq!(all[2].title, "noise piece");
    assert_eq!(all[2].score, -10);
}

#[test]
fn list_focus_groups_cross_feed_cluster() {
    let (_dir, conn) = open_temp();
    let a = subscribe(
        &conn,
        SubscribeRequest {
            url: "https://a.example.com/rss.xml".into(),
            folder_id: None,
            as_rsshub: false,
            rsshub_instance_id: None,
        },
    )
    .unwrap();
    let b = subscribe(
        &conn,
        SubscribeRequest {
            url: "https://b.example.com/rss.xml".into(),
            folder_id: None,
            as_rsshub: false,
            rsshub_instance_id: None,
        },
    )
    .unwrap();
    let url = "https://news.example.com/library";
    rrss_core::ingest::upsert_entries(&conn, a, &[entry("市政厅新图书馆开放", url)], now_ms())
        .unwrap();
    rrss_core::ingest::upsert_entries(&conn, b, &[entry("市政厅开放新图书馆", url)], now_ms())
        .unwrap();
    set_focus_filter(&conn, FocusFilter::ShowAll).unwrap();
    let rows = list_focus(&conn, 10).unwrap();
    assert!(
        rows.iter().any(|r| r.row_kind == ROW_HEADER),
        "expected cluster header, got {:?}",
        rows.iter()
            .map(|r| (r.row_kind, r.title.clone()))
            .collect::<Vec<_>>()
    );
    let cid = rows
        .iter()
        .find(|r| r.row_kind == ROW_HEADER)
        .and_then(|r| r.cluster_id)
        .unwrap();
    // 折叠时只有标题行和代表文章，没有第二个展开入口。
    let collapsed: Vec<i32> = rows
        .iter()
        .filter(|r| r.cluster_id == Some(cid))
        .map(|r| r.row_kind)
        .collect();
    assert_eq!(collapsed, vec![ROW_HEADER, ROW_CARD]);
    let expanded = list_focus_stream(&conn, 10, Some(cid)).unwrap();
    let members = expanded
        .iter()
        .filter(|r| r.row_kind == ROW_CARD && r.cluster_id == Some(cid))
        .count();
    assert_eq!(members, 2);
    assert!(
        expanded
            .iter()
            .all(|r| r.row_kind == ROW_CARD || r.row_kind == ROW_HEADER)
    );
}

#[test]
fn preload_ids_cover_focus_and_starred() {
    let (_dir, conn) = open_temp();
    seed(&conn);
    let ids = list_preload_article_ids(&conn).unwrap();
    let titles: Vec<String> = ids
        .iter()
        .map(|id| get_article(&conn, *id).unwrap().title)
        .collect();
    assert!(titles.iter().any(|t| t.contains("focus")));
    assert!(!titles.iter().any(|t| t.contains("noise")));
    let noise_id: i64 = conn
        .query_row(
            "SELECT id FROM articles WHERE title LIKE '%noise%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    set_starred(&conn, noise_id, true).unwrap();
    let ids = list_preload_article_ids(&conn).unwrap();
    assert!(ids.contains(&noise_id));
}

#[test]
fn mix_keeps_quiet_feed_when_loud_feed_floods() {
    let (_dir, conn) = open_temp();
    set_focus_filter(&conn, FocusFilter::ShowAll).unwrap();
    rrss_core::interests::set_cluster_group_enabled(&conn, false).unwrap();
    let loud = subscribe(
        &conn,
        SubscribeRequest {
            url: "https://loud.example.com/rss.xml".into(),
            folder_id: None,
            as_rsshub: false,
            rsshub_instance_id: None,
        },
    )
    .unwrap();
    let quiet = subscribe(
        &conn,
        SubscribeRequest {
            url: "https://quiet.example.com/rss.xml".into(),
            folder_id: None,
            as_rsshub: false,
            rsshub_instance_id: None,
        },
    )
    .unwrap();
    let loud_entries: Vec<ParsedEntry> = (1..=20)
        .map(|i| {
            let mut e = entry(
                &format!("loud {i:02}"),
                &format!("https://loud.example.com/{i}"),
            );
            e.published_at = Some(now_ms() - 60_000 + 1_000 + i);
            e
        })
        .collect();
    rrss_core::ingest::upsert_entries(&conn, loud, &loud_entries, now_ms()).unwrap();
    rrss_core::ingest::upsert_entries(
        &conn,
        quiet,
        &[
            {
                let mut e = entry("quiet alpha", "https://quiet.example.com/a");
                e.published_at = Some(now_ms() - 60_000 + 1);
                e
            },
            {
                let mut e = entry("quiet beta", "https://quiet.example.com/b");
                e.published_at = Some(now_ms() - 60_000 + 2);
                e
            },
        ],
        now_ms(),
    )
    .unwrap();

    let mixed = list_focus(&conn, 10).unwrap();
    assert_eq!(mixed.len(), 10);
    let quiet_n = mixed.iter().filter(|c| c.feed_id == quiet).count();
    assert!(
        quiet_n >= 2,
        "quiet feed should survive mix, got {:?}",
        mixed.iter().map(|c| c.title.as_str()).collect::<Vec<_>>()
    );
}

#[test]
fn focus_page_then_more_does_not_repeat_ids() {
    let (_dir, conn) = open_temp();
    seed(&conn);
    set_focus_filter(&conn, FocusFilter::ShowAll).unwrap();
    rrss_core::interests::set_cluster_group_enabled(&conn, false).unwrap();
    let feed: i64 = conn
        .query_row("SELECT id FROM feeds LIMIT 1", [], |r| r.get(0))
        .unwrap();
    for i in 0..12 {
        let title = format!("extra {i}");
        let url = format!("https://news.example.com/extra/{i}");
        rrss_core::ingest::upsert_entries(
            &conn,
            feed,
            &[ParsedEntry {
                guid: url.clone(),
                canonical_url: Some(url.clone()),
                title: title.clone(),
                author: None,
                published_at: Some(now_ms() - 30_000 + i),
                content_html: Some("<p>x</p>".into()),
                hash: rrss_core::parse::entry_hash(&title, Some(&url), Some("<p>x</p>")),
                tags: vec![],
            }],
            now_ms(),
        )
        .unwrap();
    }
    let (first, has_more) = list_focus_page(&conn, 5, None, &[], &[], FeedScope::All).unwrap();
    assert!(has_more);
    assert_eq!(first.len(), 5);
    let seen: Vec<i64> = first
        .iter()
        .filter(|c| c.row_kind == ROW_CARD)
        .map(|c| c.id)
        .collect();
    let (second, second_more) =
        list_focus_page(&conn, 5, None, &seen, &[], FeedScope::All).unwrap();
    let again: Vec<i64> = second
        .iter()
        .filter(|c| c.row_kind == ROW_CARD)
        .map(|c| c.id)
        .collect();
    assert!(!again.is_empty());
    assert!(second_more);
    for id in &again {
        assert!(!seen.contains(id), "id {id} repeated across pages");
    }
    let mut all = seen;
    all.extend(again);
    let (third, third_more) = list_focus_page(&conn, 5, None, &all, &[], FeedScope::All).unwrap();
    assert_eq!(third.len(), 5);
    assert!(!third_more);
    for card in &third {
        assert!(!all.contains(&card.id));
    }
}

#[test]
fn hide_noisy_drops_flagged_feed() {
    let (_dir, conn) = open_temp();
    insert_rule(
        &conn,
        NewRule::manual_global(Polarity::Dislike, Dimension::TitleKw, "ad"),
    )
    .unwrap();
    let feed = subscribe(
        &conn,
        SubscribeRequest {
            url: "https://ads.example.com/rss.xml".into(),
            folder_id: None,
            as_rsshub: false,
            rsshub_instance_id: None,
        },
    )
    .unwrap();
    let now = rrss_core::feeds::now_ms();
    let entries: Vec<ParsedEntry> = (0..8)
        .map(|i| {
            let url = format!("https://ads.example.com/{i}");
            let mut item = entry(&format!("ad promo {i}"), &url);
            item.published_at = Some(now);
            item
        })
        .collect();
    rrss_core::ingest::upsert_entries(&conn, feed, &entries, now).unwrap();
    rrss_core::articles::set_focus_filter(&conn, FocusFilter::ShowAll).unwrap();
    rrss_core::interests::set_cluster_group_enabled(&conn, false).unwrap();
    let shown = list_focus(&conn, 20).unwrap();
    assert!(shown.iter().any(|c| c.noisy));
    rrss_core::interests::set_hide_noisy_enabled(&conn, true).unwrap();
    let hidden = list_focus(&conn, 20).unwrap();
    assert!(hidden.is_empty(), "got {:?}", hidden.len());
}

#[test]
fn cluster_header_lists_sources_and_group_read_respects_scope_and_rules() {
    let (_dir, conn) = open_temp();
    conn.execute_batch(
        "INSERT INTO folders(id,parent_id,name,created_at) VALUES (1,NULL,'in',0),(2,NULL,'out',0);
         INSERT INTO feeds(id,url,title,folder_id,created_at) VALUES
         (10,'https://a.example/rss','甲',1,0),(11,'https://b.example/rss','乙',1,0),
         (12,'https://c.example/rss','丙',2,0);
         INSERT INTO clusters(id,label,member_count,created_at) VALUES (7,'同题',5,0);
         INSERT INTO articles(id,feed_id,title,score,published_at,cluster_id,read,created_at) VALUES
         (1,10,'甲一',5,1000,7,0,1),(2,11,'乙一',5,2000,7,0,1),(3,10,'甲二',5,3000,7,1,1),
         (4,12,'丙一',5,4000,7,0,1),(5,11,'被隐藏',-100,5000,7,0,1);",
    )
    .unwrap();
    conn.execute(
        "UPDATE articles SET published_at=?1+published_at,created_at=?2",
        params![now_ms() - 60_000, now_ms()],
    )
    .unwrap();
    let header = |scope| {
        list_focus_page(&conn, 40, None, &[], &[], scope)
            .unwrap()
            .0
            .into_iter()
            .find(|card| card.row_kind == ROW_HEADER)
            .unwrap()
    };
    // 按成员发布先后去重；被规则隐藏的成员不算来源。
    assert_eq!(
        header(FeedScope::All).cluster_feeds,
        vec![
            (10, "甲".to_string()),
            (11, "乙".to_string()),
            (12, "丙".to_string())
        ]
    );
    assert_eq!(
        header(FeedScope::Folder(1)).cluster_feeds,
        vec![(10, "甲".to_string()), (11, "乙".to_string())]
    );
    // 只标范围内可见的未读：已读、隐藏、范围外的都不动。
    assert_eq!(
        mark_cluster_read(&conn, 7, FeedScope::Folder(1)).unwrap(),
        vec![1, 2]
    );
    let read = |id: i64| -> i64 {
        conn.query_row("SELECT read FROM articles WHERE id=?1", [id], |r| r.get(0))
            .unwrap()
    };
    assert_eq!((read(4), read(5)), (0, 0));
    assert_eq!(
        mark_cluster_read(&conn, 7, FeedScope::All).unwrap(),
        vec![4]
    );
    assert!(
        mark_cluster_read(&conn, 7, FeedScope::All)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn folder_scope_limits_cards_clusters_and_skips_spot_checks() {
    let (_dir, conn) = open_temp();
    conn.execute_batch(
        "INSERT INTO folders(id,parent_id,name,created_at) VALUES
         (1,NULL,'parent',0),(2,1,'child',0),(3,NULL,'other',0);
         INSERT INTO feeds(id,url,folder_id,created_at) VALUES
         (10,'https://in.example/rss',1,0),(11,'https://child.example/rss',2,0),
         (12,'https://out.example/rss',3,0);
         INSERT INTO clusters(id,label,member_count,created_at) VALUES
         (7,'同题',3,0),(8,'只剩一篇',2,0);
         INSERT INTO articles(id,feed_id,title,score,published_at,cluster_id,created_at) VALUES
         (1,10,'in parent',5,3000,7,1),(2,11,'in child',5,2000,7,1),
         (3,12,'outside cluster',5,4000,7,1),(4,10,'solo in',5,1000,8,1),
         (5,12,'solo out',5,5000,8,1),(6,12,'plain out',5,6000,NULL,1);",
    )
    .unwrap();
    conn.execute(
        "UPDATE articles SET published_at=?1+published_at,created_at=?2",
        params![now_ms() - 60_000, now_ms()],
    )
    .unwrap();
    rrss_core::db::set_setting(&conn, rrss_core::db::HIDDEN_SPOT_CHECK_RATE_KEY, "5").unwrap();
    let now = rrss_core::feeds::now_ms();
    conn.execute(
        "WITH RECURSIVE ids(id) AS (SELECT 100 UNION ALL SELECT id+1 FROM ids WHERE id<500)
         INSERT INTO articles(id,feed_id,title,score,created_at)
         SELECT id,10,'hidden',-10,?1 FROM ids",
        [now],
    )
    .unwrap();
    let page = |scope| {
        list_focus_page(&conn, 40, Some(7), &[], &[], scope)
            .unwrap()
            .0
    };
    let cards = |rows: &[ArticleCard]| {
        rows.iter()
            .filter(|card| card.row_kind == ROW_CARD)
            .map(|card| card.id)
            .collect::<Vec<_>>()
    };
    let scoped = page(FeedScope::Folder(1));
    assert_eq!(cards(&scoped), vec![2, 1, 4]);
    let headers: Vec<_> = scoped
        .iter()
        .filter(|card| card.row_kind == ROW_HEADER)
        .collect();
    assert_eq!(
        headers
            .iter()
            .map(|card| (card.cluster_id, card.cluster_size))
            .collect::<Vec<_>>(),
        vec![(Some(7), 2)]
    );
    assert_eq!(
        focus_cluster_members(&conn, 7, FeedScope::Folder(1))
            .unwrap()
            .into_iter()
            .map(|card| card.id)
            .collect::<Vec<_>>(),
        vec![2, 1]
    );
    let all = page(FeedScope::All);
    assert!(all.iter().any(|card| card.spot_check) && scoped.iter().all(|card| !card.spot_check));
    assert_eq!(cards(&page(FeedScope::Feed(12))), cards(&all));
}

/// 与 core `interests` 测试里的同名辅助相同：每篇一个源，可选加一条「rust」赞规则。
fn seed_article(conn: &Connection, title: &str, url: &str, like: bool) -> i64 {
    let feed = subscribe(
        conn,
        SubscribeRequest {
            url: format!("{url}/rss.xml"),
            folder_id: None,
            as_rsshub: false,
            rsshub_instance_id: None,
        },
    )
    .unwrap();
    if like {
        insert_rule(
            conn,
            NewRule::manual_global(Polarity::Like, Dimension::TitleKw, "rust"),
        )
        .unwrap();
    }
    rrss_core::ingest::upsert_entries(
        conn,
        feed,
        &[ParsedEntry {
            guid: url.into(),
            canonical_url: Some(url.into()),
            title: title.into(),
            author: None,
            published_at: Some(rrss_core::feeds::now_ms() - 60_000),
            content_html: Some("<p>body</p>".into()),
            hash: rrss_core::parse::entry_hash(title, Some(url), Some("<p>body</p>")),
            tags: vec![],
        }],
        rrss_core::feeds::now_ms(),
    )
    .unwrap();
    conn.query_row(
        "SELECT id FROM articles WHERE canonical_url = ?1",
        [url],
        |r| r.get(0),
    )
    .unwrap()
}

#[test]
fn toggling_boost_changes_order_without_clearing_column() {
    let (_dir, conn) = open_temp();
    let low = seed_article(&conn, "alpha rust", "https://lab.example/low", true);
    let high = seed_article(&conn, "beta rust", "https://lab.example/high", true);
    conn.execute(
        "UPDATE articles SET score = 10, interest_boost = 100 WHERE id = ?1",
        [low],
    )
    .unwrap();
    conn.execute(
        "UPDATE articles SET score = 20, interest_boost = 0 WHERE id = ?1",
        [high],
    )
    .unwrap();
    rrss_core::articles::set_focus_filter(&conn, rrss_core::articles::FocusFilter::ShowAll)
        .unwrap();
    rrss_core::interests::set_cluster_group_enabled(&conn, false).unwrap();
    rrss_core::interests::set_boost_enabled(&conn, true).unwrap();
    let on = crate::list_focus_stream(&conn, 20, None).unwrap();
    assert_eq!(on[0].id, low, "boost on: 10+100 should beat 20");
    rrss_core::interests::set_boost_enabled(&conn, false).unwrap();
    let stored: i64 = conn
        .query_row(
            "SELECT interest_boost FROM articles WHERE id = ?1",
            [low],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, 100);
    let off = crate::list_focus_stream(&conn, 20, None).unwrap();
    assert_eq!(off[0].id, high, "boost off: sort by score only");
}
