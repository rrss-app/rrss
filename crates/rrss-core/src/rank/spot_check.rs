//! R1：当前范围内近七天隐藏文章的每日稳定抽查，只放回焦点首屏。

use rusqlite::{Connection, params_from_iter};

use crate::articles::{self, ArticleCard, FocusFilter};
use crate::{Result, db};

// 固定 FNV-1a：文章 id 的小端字节 + 本地日期 YYYYMMDD，不依赖进程种子。
fn daily_hash(id: i64, day: &str) -> u64 {
    id.to_le_bytes()
        .iter()
        .chain(day.as_bytes())
        .fold(0xcbf29ce484222325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
}

pub(crate) fn inject(
    conn: &Connection,
    cards: &mut Vec<ArticleCard>,
    filter: FocusFilter,
    threshold: i64,
    exclude_feeds: &[i64],
    first_page: bool,
) -> Result<()> {
    let rate = db::hidden_spot_check_rate(conn)?;
    if !first_page || filter == FocusFilter::ShowAll || rate == 0 || cards.len() < 2 {
        return Ok(());
    }
    let now = chrono::Local::now();
    let day = now.format("%Y%m%d").to_string();
    let scope_sql = articles::list_scope(conn)?.filter_sql();
    let noisy_sql = if exclude_feeds.is_empty() {
        String::new()
    } else {
        let marks = (4..4 + exclude_feeds.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(",");
        format!(" AND a.feed_id NOT IN ({marks})")
    };
    let sql = format!(
        "SELECT a.id, a.title, COALESCE(f.title, f.url), a.published_at, a.canonical_url,
                a.read, a.starred, a.score, a.interest_boost, COALESCE(a.summary, ''),
                a.feed_id, a.cluster_id, a.interest_reasons_json, COALESCE(r.boost, 0), r.reasons_json
         FROM articles a
         JOIN feeds f ON f.id = a.feed_id
         LEFT JOIN article_rank r ON r.article_id = a.id
         WHERE a.expires_at IS NULL AND a.score <= ?1 AND a.released_at IS NULL AND a.created_at BETWEEN ?2 AND ?3
           {scope_sql} {noisy_sql}
         ORDER BY a.id"
    );
    let now_ms = now.timestamp_millis();
    let mut bind = vec![threshold, now_ms - 7 * 86_400_000, now_ms];
    bind.extend_from_slice(exclude_feeds);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query(params_from_iter(bind))?;
    let mut selected = Vec::new();
    while let Some(row) = rows.next()? {
        let id = row.get(0)?;
        let hash = daily_hash(id, &day);
        if hash % 100 < rate as u64
            && !cards
                .iter()
                .any(|card| card.id == id && card.row_kind == articles::ROW_CARD)
        {
            selected.push((hash, articles::map_card(row)?));
            selected.sort_unstable_by_key(|(hash, card)| (*hash, card.id));
            selected.truncate(2);
        }
    }
    // 在排序、聚类投影之后插入，抽查按单篇处理，续查也不排除它原来的整个簇。
    for (hash, mut card) in selected {
        card.spot_check = true;
        card.cluster_id = None;
        card.feed_title = crate::feeds::display_title(&card.feed_title, "");
        let position = 1 + (hash % (cards.len() - 1) as u64) as usize;
        cards.insert(position, card);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::articles::ListScope;
    use crate::feed_scope::FeedScope;
    use rusqlite::params;

    fn fixture() -> (tempfile::TempDir, db::PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("spot.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,title,created_at) VALUES
             (1,'https://one.invalid','One',0), (2,'https://two.invalid','Two',0),
             (3,'https://noisy.invalid','Noisy',0);",
        )
        .unwrap();
        let now = crate::feeds::now_ms();
        let tx = db::write_transaction(&conn).unwrap();
        for feed in 1..=3 {
            for n in 0..4 {
                tx.execute(
                    "INSERT INTO articles(id,feed_id,title,score,read,starred,created_at)
                     VALUES(?1,?2,'visible',5,?3,?4,?5)",
                    params![feed * 10 + n, feed, n % 2, n / 2, now],
                )
                .unwrap();
            }
            for n in 0..2000 {
                tx.execute(
                    "INSERT INTO articles(id,feed_id,title,summary,score,read,starred,published_at,created_at)
                     VALUES(?1,?2,'hidden','summary',-10,?3,?4,?5,?6)",
                    params![
                        feed * 10_000 + n,
                        feed,
                        n % 2,
                        (n / 2) % 2,
                        if feed == 3 { Some(now) } else { None },
                        now
                    ],
                )
                .unwrap();
            }
        }
        tx.commit().unwrap();
        (dir, conn)
    }

    fn page(conn: &Connection, ids: &[i64], clusters: &[i64]) -> Vec<ArticleCard> {
        articles::list_focus_page(conn, 8, None, ids, clusters, FeedScope::All)
            .unwrap()
            .0
    }

    fn spots(cards: &[ArticleCard]) -> Vec<i64> {
        cards
            .iter()
            .filter(|c| c.spot_check)
            .map(|c| c.id)
            .collect()
    }

    #[test]
    fn same_day_is_stable_and_sampling_respects_rate() {
        // 固定读数锁定跨进程 / 平台的字节序和算法；比例也检查足够大的完整样本。
        assert_eq!(daily_hash(42, "20261012"), 0xdde1eb3844f7ceeb);
        let mut previous = std::collections::HashSet::new();
        for rate in [0, 1, 3, 5] {
            let selected: std::collections::HashSet<_> = (1..=10_000)
                .filter(|id| daily_hash(*id, "20261012") % 100 < rate)
                .collect();
            assert!(previous.is_subset(&selected));
            assert!(selected.len().abs_diff(rate as usize * 100) < 50);
            previous = selected;
        }
        assert_ne!(daily_hash(42, "20261012"), daily_hash(42, "20261013"));
        let (_dir, conn) = fixture();
        for rate in [1, 3, 5] {
            db::set_setting(&conn, db::HIDDEN_SPOT_CHECK_RATE_KEY, &rate.to_string()).unwrap();
            let first = page(&conn, &[], &[]);
            let again = page(&conn, &[], &[]);
            assert_eq!(spots(&first).len(), 2);
            assert_eq!(
                serde_json::to_value(&first).unwrap(),
                serde_json::to_value(&again).unwrap()
            );
            let day = chrono::Local::now().format("%Y%m%d").to_string();
            assert!(
                spots(&again)
                    .iter()
                    .all(|id| daily_hash(*id, &day) % 100 < rate as u64)
            );
        }
    }

    #[test]
    fn only_first_page_and_non_show_all_filters_inject() {
        let (_dir, conn) = fixture();
        for filter in [
            FocusFilter::FocusOnly,
            FocusFilter::HideFaded,
            FocusFilter::ShowAll,
        ] {
            articles::set_focus_filter(&conn, filter).unwrap();
            assert_eq!(
                spots(&page(&conn, &[], &[])).len(),
                if filter == FocusFilter::ShowAll { 0 } else { 2 }
            );
            for (ids, clusters) in [
                (&[11][..], &[][..]),
                (&[][..], &[1][..]),
                (&[11][..], &[1][..]),
            ] {
                assert!(spots(&page(&conn, ids, clusters)).is_empty());
            }
        }
    }

    #[test]
    fn at_most_two_standalone_spots_preserve_first_and_normal_order() {
        let (_dir, conn) = fixture();
        conn.execute_batch(
            "INSERT INTO clusters(id,label,member_count,created_at) VALUES(1,'cluster',2004,0);
             UPDATE articles SET cluster_id=1;",
        )
        .unwrap();
        db::set_setting_enabled(&conn, db::FOCUS_RERANK_ENABLED_KEY, true).unwrap();
        db::set_setting(&conn, db::HIDDEN_SPOT_CHECK_RATE_KEY, "0").unwrap();
        let baseline = page(&conn, &[], &[]);
        assert_eq!(baseline[0].row_kind, articles::ROW_HEADER);
        db::set_setting(&conn, db::HIDDEN_SPOT_CHECK_RATE_KEY, "5").unwrap();
        let injected = page(&conn, &[], &[]);
        assert_eq!(spots(&injected).len(), 2);
        assert!(!injected[0].spot_check);
        assert_eq!(injected.len(), baseline.len() + 2);
        for card in injected.iter().filter(|c| c.spot_check) {
            assert_eq!(card.row_kind, articles::ROW_CARD);
            assert_eq!(card.cluster_id, None);
            assert_eq!(card.cluster_size, 1);
            assert_eq!(card.summary, "summary");
        }
        let normal: Vec<_> = injected.into_iter().filter(|c| !c.spot_check).collect();
        assert_eq!(
            serde_json::to_value(normal).unwrap(),
            serde_json::to_value(baseline).unwrap()
        );
        // 一条或空列表没有合法插入位置。
        db::set_setting_enabled(&conn, db::CLUSTER_GROUP_ENABLED_KEY, false).unwrap();
        assert!(
            spots(
                &articles::list_focus_page(&conn, 1, None, &[], &[], FeedScope::All)
                    .unwrap()
                    .0
            )
            .is_empty()
        );
        conn.execute("DELETE FROM articles WHERE score > -10", [])
            .unwrap();
        assert!(page(&conn, &[], &[]).is_empty());
    }

    #[test]
    fn candidate_scope_matches_normal_feed_list_scope_and_noise_exclusions() {
        let (_dir, conn) = fixture();
        let now = crate::feeds::now_ms();
        conn.execute(
            "UPDATE articles SET created_at=?1 WHERE score <= -10 AND id%3=0",
            [now - 8 * 86_400_000],
        )
        .unwrap();
        conn.execute(
            "UPDATE articles SET created_at=?1 WHERE score <= -10 AND id%3=1",
            [now + 86_400_000],
        )
        .unwrap();
        db::set_setting_enabled(&conn, db::HIDE_NOISY_KEY, true).unwrap();
        assert_eq!(crate::feeds::noisy_feed_ids(&conn).unwrap(), [3].into());
        for scope in [ListScope::All, ListScope::Unread, ListScope::Starred] {
            articles::set_list_scope(&conn, scope).unwrap();
            let cards = page(&conn, &[], &[]);
            assert_eq!(spots(&cards).len(), 2);
            for card in cards.iter().filter(|c| c.row_kind == articles::ROW_CARD) {
                assert_ne!(card.feed_id, 3);
                match scope {
                    ListScope::Unread => assert!(!card.read),
                    ListScope::Starred => assert!(card.starred),
                    ListScope::All => {}
                }
                if card.spot_check {
                    let created: i64 = conn
                        .query_row(
                            "SELECT created_at FROM articles WHERE id=?1",
                            [card.id],
                            |r| r.get(0),
                        )
                        .unwrap();
                    assert!(created >= now - 7 * 86_400_000 && created <= now);
                    assert_eq!(card.score, -10); // 阈值边界也属于隐藏候选。
                    assert!(!card.noisy);
                }
            }
        }
    }

    #[test]
    fn zero_rate_never_injects() {
        let (_dir, conn) = fixture();
        db::set_setting(&conn, db::HIDDEN_SPOT_CHECK_RATE_KEY, "0").unwrap();
        for filter in [
            FocusFilter::FocusOnly,
            FocusFilter::HideFaded,
            FocusFilter::ShowAll,
        ] {
            articles::set_focus_filter(&conn, filter).unwrap();
            let cards = page(&conn, &[], &[]);
            assert!(!cards.is_empty());
            assert!(spots(&cards).is_empty());
        }
    }

    #[test]
    fn released_and_temporary_articles_are_never_spot_checked() {
        let (_dir, conn) = fixture();
        db::set_setting(&conn, db::HIDDEN_SPOT_CHECK_RATE_KEY, "5").unwrap();
        articles::set_focus_filter(&conn, FocusFilter::HideFaded).unwrap();
        assert!(!spots(&page(&conn, &[], &[])).is_empty());
        conn.execute("UPDATE articles SET released_at=CASE WHEN id%2=0 THEN 1 END, expires_at=CASE WHEN id%2=1 THEN 999 END WHERE score<0", [])
            .unwrap();
        // 放行文章正常可见，临时文章仍不可见；两者都不能作为抽查放回。
        assert!(spots(&page(&conn, &[], &[])).is_empty());
    }
}
