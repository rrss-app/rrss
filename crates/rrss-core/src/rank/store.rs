//! 阶段 23「接入方式 · article_rank」：可重建的排序缓存，事务保证旧结果不会半写入。
//!
//! 模型加载与打分只在读快照里做，写锁只包住核对与写回（阶段 25「大库演进」）。
//! 写回前核对开关与整表刷新标记：其间有整表刷新或开关变化就丢掉结果重算；
//! 其余输入（行为、新文章）的滞后与原先一样，由防抖 / 每小时整表刷新收敛。

use crate::error::settings::SettingProblem;
use super::{
    combine::{ALGO_VERSION, Switches},
    model::Model,
};
use crate::{Result, db};
use rusqlite::{Connection, params};

/// 最近一次整表刷新的时间；整表刷新与它在同一写事务里更新。
pub const REFRESHED_KEY: &str = "rank_refreshed_at";
// 开关切换的代次：即使 A → B → A，也不能让切换前算出的后台结果重新覆盖缓存。
const SWITCH_REVISION_KEY: &str = "rank_switch_revision";
/// 快照重算上限；耗尽后在写锁内计算一次，保证后台刷新推进、用户开关得到保存。
const SNAPSHOT_ATTEMPTS: usize = 3;

pub fn rebuild(conn: &Connection, now: i64) -> Result<usize> {
    refresh(conn, None, now, |_| Ok(()))
}

/// 算法版本不一致时自动整表重建；不存在 / 已删除文章直接跳过。
pub fn update(conn: &Connection, ids: &[i64], now: i64) -> Result<usize> {
    refresh(conn, Some(ids), now, |_| Ok(()))
}

/// 设置成功时开关和全部排序结果一起生效；重计算通常不持写锁。
/// 只接受独立调用，不能在已有写事务中重新包裹这条用户路径。
pub fn set_switch(conn: &Connection, key: &str, enabled: bool) -> Result<()> {
    set_switch_with(conn, key, enabled, || Ok(()))
}

/// `between` 在每次快照计算后、取写锁前调用，只供测试注入并发写入。
fn set_switch_with(
    conn: &Connection,
    key: &str,
    enabled: bool,
    between: impl Fn() -> Result<()>,
) -> Result<()> {
    if !conn.is_autocommit() {
        return Err(crate::Error::InvalidSetting(SettingProblem::RankTransactionRequired));
    }
    for _ in 0..SNAPSHOT_ATTEMPTS {
        let change = db::read_snapshot(conn, |snapshot| {
            SwitchChange::prepare(snapshot, key, enabled)
        })?;
        between()?;
        if db::atomic_write(conn, |tx| change.commit(tx))? {
            return Ok(());
        }
    }
    // 后台持续入库时快照总会失效；与后台整表刷新一样在写锁内算一次，保证用户的设置能保存。
    // 命令已在阻塞线程执行，持锁只让其他写入稍等，不占界面线程。
    db::atomic_write(conn, |tx| {
        SwitchChange::prepare(tx, key, enabled)?.apply(tx)
    })
}

struct SwitchChange<'a> {
    key: &'a str,
    enabled: bool,
    // data_version 覆盖其他连接提交；total_changes 覆盖同一连接的本地写入。
    version: (i64, u64),
    plan: Plan,
}

fn input_version(conn: &Connection) -> Result<(i64, u64)> {
    Ok((
        conn.query_row("PRAGMA data_version", [], |row| row.get(0))?,
        conn.total_changes(),
    ))
}

impl<'a> SwitchChange<'a> {
    fn prepare(conn: &Connection, key: &'a str, enabled: bool) -> Result<Self> {
        let version = input_version(conn)?;
        // 每次重试在快照建立后取新的截止时间，覆盖等待期间刚入库的文章与行为。
        let now = crate::feeds::now_ms();
        let mut switches = db::rank_switches(conn)?;
        let selected = match key {
            db::RANK_FEATURES_ENABLED_KEY => &mut switches.features,
            db::RANK_SOURCE_ENABLED_KEY => &mut switches.source,
            db::RANK_COVERAGE_ENABLED_KEY => &mut switches.coverage,
            db::RANK_FRESHNESS_ENABLED_KEY => &mut switches.freshness,
            db::RANK_AFFINITY_ENABLED_KEY => &mut switches.affinity,
            _ => return Err(crate::Error::InvalidSetting(SettingProblem::RankSwitchUnknown)),
        };
        *selected = enabled;
        Ok(Self {
            key,
            enabled,
            version,
            plan: plan_with_switches(conn, None, now, switches)?,
        })
    }

    fn commit(&self, tx: &Connection) -> Result<bool> {
        if input_version(tx)? != self.version {
            return Ok(false);
        }
        self.apply(tx)?;
        Ok(true)
    }

    fn apply(&self, tx: &Connection) -> Result<()> {
        db::set_setting_enabled(tx, self.key, self.enabled)?;
        self.plan.write(tx)?;
        let revision = self
            .plan
            .switch_revision
            .as_deref()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        db::set_setting(
            tx,
            SWITCH_REVISION_KEY,
            &revision.saturating_add(1).to_string(),
        )?;
        Ok(())
    }
}

/// `ids` 为 `None` 时整表重算。`after` 与写回在同一写事务内执行，
/// 用于把作业标记与结果一起提交。在调用方事务内时整段在该事务里完成。
pub fn refresh(
    conn: &Connection,
    ids: Option<&[i64]>,
    now: i64,
    after: impl Fn(&Connection) -> Result<()>,
) -> Result<usize> {
    if conn.is_autocommit() {
        for _ in 0..SNAPSHOT_ATTEMPTS {
            let plan = db::read_snapshot(conn, |snapshot| plan(snapshot, ids, now))?;
            let written = db::atomic_write(conn, |tx| {
                if !plan.still_valid(tx)? {
                    return Ok(None);
                }
                let n = plan.write(tx)?;
                after(tx)?;
                Ok(Some(n))
            })?;
            if let Some(n) = written {
                return Ok(n);
            }
        }
    }
    db::atomic_write(conn, |tx| {
        let n = plan(tx, ids, now)?.write(tx)?;
        after(tx)?;
        Ok(n)
    })
}

/// 在某个快照上算好的写回内容。
struct Plan {
    switches: Switches,
    switch_revision: Option<String>,
    refreshed: Option<String>,
    // 整表重算时为快照里的最大文章 ID：只替换它及以下的行，
    // 快照之后入库、已由增量写入的文章保留。
    replace_through: Option<i64>,
    rows: Vec<(i64, i64, String)>,
    now: i64,
}

fn plan(conn: &Connection, ids: Option<&[i64]>, now: i64) -> Result<Plan> {
    plan_with_switches(conn, ids, now, db::rank_switches(conn)?)
}

fn plan_with_switches(
    conn: &Connection,
    ids: Option<&[i64]>,
    now: i64,
    switches: Switches,
) -> Result<Plan> {
    let refreshed = db::get_setting(conn, REFRESHED_KEY)?;
    let mut plan = Plan {
        switches,
        switch_revision: db::get_setting(conn, SWITCH_REVISION_KEY)?,
        refreshed,
        replace_through: None,
        rows: Vec::new(),
        now,
    };
    let on = switches;
    if !on.features && !on.source && !on.coverage && !on.freshness && !on.affinity {
        plan.replace_through = Some(i64::MAX);
        return Ok(plan);
    }
    let outdated: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM article_rank WHERE algo_version<>?1)",
        [ALGO_VERSION],
        |r| r.get(0),
    )?;
    let all = ids.is_none() || outdated;
    let model = Model::load_with_affinity(
        conn,
        now.saturating_add(1),
        on.affinity,
        if all { None } else { ids },
    )?;
    let targets: Vec<_> = if all {
        plan.replace_through =
            Some(conn.query_row("SELECT COALESCE(MAX(id),0) FROM articles", [], |r| r.get(0))?);
        model.article_ids().collect()
    } else {
        ids.unwrap_or_default().to_vec()
    };
    for id in targets {
        let Some(boost) = model.article_boost(id, now, on) else {
            continue;
        };
        let reasons = serde_json::to_string(&boost.reasons)
            .map_err(|e| crate::Error::Parse(e.to_string()))?;
        plan.rows.push((id, boost.score, reasons));
    }
    Ok(plan)
}

impl Plan {
    fn still_valid(&self, tx: &Connection) -> Result<bool> {
        Ok(db::rank_switches(tx)? == self.switches
            && db::get_setting(tx, SWITCH_REVISION_KEY)? == self.switch_revision
            && db::get_setting(tx, REFRESHED_KEY)? == self.refreshed)
    }

    fn write(&self, tx: &Connection) -> Result<usize> {
        if let Some(max) = self.replace_through {
            tx.execute("DELETE FROM article_rank WHERE article_id<=?1", [max])?;
        }
        // 快照之后被删除或转为临时的文章不再写入。
        let mut stmt = tx.prepare_cached(
            "INSERT INTO article_rank(article_id,boost,reasons_json,algo_version,computed_at)
            SELECT ?1,?2,?3,?4,?5 WHERE EXISTS(SELECT 1 FROM articles WHERE id=?1 AND expires_at IS NULL)
            ON CONFLICT(article_id) DO UPDATE SET
            boost=excluded.boost,reasons_json=excluded.reasons_json,algo_version=excluded.algo_version,computed_at=excluded.computed_at",
        )?;
        let mut count = 0;
        for (id, boost, reasons) in &self.rows {
            count += stmt.execute(params![id, boost, reasons, ALGO_VERSION, self.now])?;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, db::PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("rank.db")).unwrap();
        conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://rank.invalid/feed',0);
            INSERT INTO articles(id,feed_id,title,published_at,created_at) VALUES(1,1,'old',0,0),(2,1,'new',86400000,0);").unwrap();
        (dir, conn)
    }

    #[test]
    fn rebuild_and_incremental_version_upgrade_are_atomic_and_tolerate_deleted_ids() {
        let (_dir, conn) = fixture();
        assert_eq!(rebuild(&conn, 86400000).unwrap(), 2);
        assert_eq!(update(&conn, &[1, 999], 86400001).unwrap(), 1);
        let time = || {
            conn.query_row(
                "SELECT computed_at FROM article_rank WHERE article_id=2",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
        };
        assert_eq!(time(), 86400000);
        conn.execute(
            "UPDATE article_rank SET algo_version=?1,reasons_json=?2 WHERE article_id=1",
            params![
                ALGO_VERSION - 1,
                r#"[{"component":"source","text":"old cache reason"}]"#
            ],
        )
        .unwrap();
        assert_eq!(update(&conn, &[1], 86400002).unwrap(), 2);
        assert_eq!(time(), 86400002);
        let cached: Vec<String> = conn
            .prepare("SELECT reasons_json FROM article_rank")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for json in cached {
            let reasons: serde_json::Value = serde_json::from_str(&json).unwrap();
            for reason in reasons.as_array().unwrap() {
                assert!(reason.get("code").is_some());
                assert!(reason.get("params").is_some());
                assert!(reason.get("text").is_none());
            }
        }
        conn.execute_batch("CREATE TEMP TRIGGER fail_rank BEFORE INSERT ON article_rank BEGIN SELECT RAISE(FAIL,'injected'); END;").unwrap();
        assert!(rebuild(&conn, 86400003).is_err());
        assert_eq!(time(), 86400002);
    }

    #[test]
    fn scoring_runs_on_a_read_snapshot_without_blocking_writers() {
        let (dir, conn) = fixture();
        let path = dir.path().join("rank.db");
        let planned = db::read_snapshot(&conn, |snapshot| {
            let plan = plan(snapshot, None, 86400000)?;
            // 另一连接此时能立即拿到写锁（不等 busy_timeout）。
            let other = db::open_at(&path)?;
            other.busy_timeout(std::time::Duration::ZERO)?;
            other.execute(
                "INSERT INTO articles(id,feed_id,title,created_at) VALUES(3,1,'later',0)",
                [],
            )?;
            Ok(plan)
        })
        .unwrap();
        assert_eq!(planned.rows.len(), 2, "the snapshot predates article 3");
    }

    #[test]
    fn switch_calculation_allows_writers_and_rejects_changed_inputs() {
        let (dir, conn) = fixture();
        rebuild(&conn, 86400000).unwrap();
        let path = dir.path().join("rank.db");
        let key = db::RANK_FRESHNESS_ENABLED_KEY;
        let change = db::read_snapshot(&conn, |snapshot| {
            let change = SwitchChange::prepare(snapshot, key, false)?;
            let other = db::open_at(&path)?;
            other.busy_timeout(std::time::Duration::ZERO)?;
            other.execute_batch(
                "INSERT INTO articles(id,feed_id,title,created_at) VALUES(3,1,'arrived',0);
                INSERT INTO feedback(article_id,kind,created_at) VALUES(1,'up',1);
                UPDATE articles SET read=1 WHERE id=2;",
            )?;
            assert!(
                db::rank_switches(&other)?.freshness,
                "the new switch is not published during calculation"
            );
            Ok(change)
        })
        .unwrap();
        assert!(!db::atomic_write(&conn, |tx| change.commit(tx)).unwrap());
        assert!(db::rank_switches(&conn).unwrap().freshness);
        assert_eq!(
            conn.query_row(
                "SELECT computed_at FROM article_rank WHERE article_id=1",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            86400000
        );
        set_switch(&conn, key, false).unwrap();
        assert!(!db::rank_switches(&conn).unwrap().freshness);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM article_rank", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            3
        );
        let current = plan(&conn, None, crate::feeds::now_ms()).unwrap();
        for (id, boost, _) in current.rows {
            assert_eq!(
                conn.query_row(
                    "SELECT boost FROM article_rank WHERE article_id=?1",
                    [id],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
                boost
            );
        }
    }

    #[test]
    fn switch_and_cache_rollback_together_and_old_background_plans_stay_invalid() {
        let (_dir, conn) = fixture();
        rebuild(&conn, 86400000).unwrap();
        let old = plan(&conn, Some(&[1]), 86400000).unwrap();
        conn.execute_batch("CREATE TEMP TRIGGER fail_rank BEFORE INSERT ON article_rank BEGIN SELECT RAISE(FAIL,'injected'); END;").unwrap();
        let key = db::RANK_FRESHNESS_ENABLED_KEY;
        assert!(set_switch(&conn, key, false).is_err());
        assert!(db::rank_switches(&conn).unwrap().freshness);
        assert!(old.still_valid(&conn).unwrap());
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM article_rank WHERE computed_at=86400000",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            2
        );
        conn.execute_batch("DROP TRIGGER fail_rank;").unwrap();
        set_switch(&conn, key, false).unwrap();
        set_switch(&conn, key, true).unwrap();
        assert!(
            !old.still_valid(&conn).unwrap(),
            "switching back cannot revive stale background work"
        );

        let change = db::read_snapshot(&conn, |snapshot| {
            SwitchChange::prepare(snapshot, key, false)
        })
        .unwrap();
        conn.execute("DELETE FROM articles WHERE id=2", []).unwrap();
        assert!(
            !db::atomic_write(&conn, |tx| change.commit(tx)).unwrap(),
            "same-connection changes also invalidate the plan"
        );
        set_switch(&conn, key, false).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM article_rank", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn switch_still_saves_under_the_write_lock_when_inputs_keep_changing() {
        let (dir, conn) = fixture();
        rebuild(&conn, 86400000).unwrap();
        let other = db::open_at(&dir.path().join("rank.db")).unwrap();
        let next = std::cell::Cell::new(10_i64);
        // 每次快照算完都有别的连接入库，模拟后台抓取持续写入。
        set_switch_with(&conn, db::RANK_FRESHNESS_ENABLED_KEY, false, || {
            other.execute(
                "INSERT INTO articles(id,feed_id,title,created_at) VALUES(?1,1,'busy',0)",
                [next.get()],
            )?;
            next.set(next.get() + 1);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            next.get(),
            10 + SNAPSHOT_ATTEMPTS as i64,
            "every snapshot conflicted"
        );
        assert!(!db::rank_switches(&conn).unwrap().freshness);
        let count = |sql: &str| conn.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();
        assert_eq!(
            count("SELECT COUNT(*) FROM article_rank"),
            count("SELECT COUNT(*) FROM articles")
        );
    }

    #[test]
    fn stale_snapshots_are_recomputed_and_never_drop_newer_rows() {
        let (_dir, conn) = fixture();
        let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        // 整表刷新的快照之后才入库的文章：增量已写的行在整表写回后保留。
        let full = plan(&conn, None, 86400000).unwrap();
        conn.execute(
            "INSERT INTO articles(id,feed_id,title,created_at) VALUES(3,1,'later',0)",
            [],
        )
        .unwrap();
        assert_eq!(update(&conn, &[3], 86400000).unwrap(), 1);
        assert!(full.still_valid(&conn).unwrap());
        assert_eq!(full.write(&conn).unwrap(), 2);
        assert_eq!(count("SELECT COUNT(*) FROM article_rank"), 3);

        // 快照之后被删除的文章不写回。
        let partial = plan(&conn, Some(&[2]), 86400001).unwrap();
        conn.execute("DELETE FROM articles WHERE id=2", []).unwrap();
        assert_eq!(partial.write(&conn).unwrap(), 0);
        assert_eq!(
            count("SELECT COUNT(*) FROM article_rank WHERE article_id=2"),
            0
        );

        // 其间有整表刷新或开关变化：旧结果作废，`refresh` 重算后写回。
        let stale = plan(&conn, Some(&[1]), 86400002).unwrap();
        db::set_setting(&conn, REFRESHED_KEY, "86400002").unwrap();
        assert!(!stale.still_valid(&conn).unwrap());
        let stale = plan(&conn, Some(&[1]), 86400002).unwrap();
        db::set_setting_enabled(&conn, db::RANK_AFFINITY_ENABLED_KEY, false).unwrap();
        assert!(!stale.still_valid(&conn).unwrap());
        assert_eq!(update(&conn, &[1], 86400003).unwrap(), 1);
        assert_eq!(
            count("SELECT computed_at FROM article_rank WHERE article_id=1"),
            86400003
        );
    }

    #[test]
    fn independent_switches_and_all_off_clear_the_cache() {
        let (_dir, conn) = fixture();
        assert_eq!(rebuild(&conn, 86400000).unwrap(), 2);
        for key in [
            db::RANK_FEATURES_ENABLED_KEY,
            db::RANK_SOURCE_ENABLED_KEY,
            db::RANK_COVERAGE_ENABLED_KEY,
            db::RANK_FRESHNESS_ENABLED_KEY,
            db::RANK_AFFINITY_ENABLED_KEY,
        ] {
            db::set_setting_enabled(&conn, key, false).unwrap();
        }
        assert_eq!(rebuild(&conn, 86400000).unwrap(), 0);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM article_rank", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        db::set_setting_enabled(&conn, db::RANK_FRESHNESS_ENABLED_KEY, true).unwrap();
        assert_eq!(rebuild(&conn, 86400000).unwrap(), 2);
        assert_eq!(
            conn.query_row(
                "SELECT boost FROM article_rank WHERE article_id=1",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            -1
        );
    }
}
