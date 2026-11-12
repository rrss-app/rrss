//! 阶段 23「数据底座 E0」「阅读会话」「接入方式」：原始信号的类型、批量写入与保留清理。

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef};
use rusqlite::{Connection, OptionalExtension, Row, ToSql, params};
use serde::{Deserialize, Serialize};

use crate::rank::engagement::{self, Language, SessionSummary};
use crate::{Error, Result, db};

pub const RETENTION_MS: i64 = 180 * 24 * 3600 * 1000;

macro_rules! signal_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name {
            $(#[serde(rename = $value)] $variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $value),+ }
            }

            pub fn parse_str(value: &str) -> Result<Self> {
                match value {
                    $($value => Ok(Self::$variant)),+,
                    _ => Err(Error::Parse(concat!("unknown signal ", stringify!($name)).into())),
                }
            }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
                Ok(ToSqlOutput::from(self.as_str()))
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
                Self::parse_str(value.as_str()?).map_err(|e| FromSqlError::Other(Box::new(e)))
            }
        }
    };
}

signal_enum!(Surface {
    Focus => "focus",
    Drawer => "drawer",
    Search => "search",
    Digest => "digest",
    Feeds => "feeds",
    FocusColumn => "focus_column",
});
signal_enum!(EventKind {
    Open => "open", Star => "star", Unstar => "unstar", Up => "up", Down => "down",
    Cite => "cite", Ask => "ask", OpenOriginal => "open_original",
});
signal_enum!(EndedBy { Back => "back", Next => "next", Close => "close", FocusTimeout => "focus_timeout" });
signal_enum!(Via {
    Focus => "focus",
    Drawer => "drawer",
    Search => "search",
    Digest => "digest",
    Citation => "citation",
    Feeds => "feeds",
    FocusColumn => "focus_column",
});

#[derive(Debug, Clone, PartialEq)]
pub struct Exposure {
    pub article_id: i64,
    pub surface: Surface,
    pub position: i64,
    pub variant: String,
    pub session_id: String,
    pub shown_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArticleEvent {
    pub article_id: i64,
    pub kind: EventKind,
    pub exposure_id: Option<i64>,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadSession {
    pub article_id: i64,
    pub exposure_id: Option<i64>,
    pub opened_at: i64,
    pub active_ms: i64,
    pub idle_ms: i64,
    pub read_chars: i64,
    pub skimmed_chars: i64,
    pub total_chars: i64,
    pub max_block: i64,
    pub reached_end: bool,
    pub translated: bool,
    pub ended_by: EndedBy,
    pub via: Via,
}

/// 按输入顺序返回曝光 ID；已删除文章为 None，同会话重复输入返回首次记录的 ID。
/// `score`、`interest_boost` 与 `rank_score`（焦点列表的排序值）取写入时文章及排序缓存的当前值，
/// 前端只报位置；批量在展示后数秒内写入，与展示时一致。
pub fn record_exposures(conn: &Connection, rows: &[Exposure]) -> Result<Vec<Option<i64>>> {
    let tx = db::write_transaction(conn)?;
    let boost_on = crate::interests::boost_enabled(&tx)?;
    let mut ids = Vec::with_capacity(rows.len());
    {
        let mut insert = tx.prepare_cached("INSERT INTO exposures
            (article_id,surface,position,score,interest_boost,rank_score,variant,session_id,shown_at)
            SELECT a.id,?2,?3,a.score,a.interest_boost,
                   a.score + CASE WHEN ?7 THEN a.interest_boost ELSE 0 END + COALESCE(r.boost, 0),?4,?5,?6
            FROM articles a LEFT JOIN article_rank r ON r.article_id=a.id WHERE a.id=?1 AND a.expires_at IS NULL
            ON CONFLICT(session_id,article_id) DO NOTHING")?;
        let mut find =
            tx.prepare_cached("SELECT id FROM exposures WHERE session_id=?1 AND article_id=?2")?;
        for r in rows {
            insert.execute(params![
                r.article_id,
                r.surface,
                r.position,
                r.variant,
                r.session_id,
                r.shown_at,
                boost_on
            ])?;
            ids.push(
                find.query_row(params![r.session_id, r.article_id], |r| r.get(0))
                    .optional()?,
            );
        }
    }
    tx.commit()?;
    Ok(ids)
}

pub fn record_events(conn: &Connection, rows: &[ArticleEvent]) -> Result<usize> {
    let tx = db::write_transaction(conn)?;
    let mut written = 0;
    {
        let mut insert = tx.prepare_cached(
            "INSERT INTO article_events(article_id,kind,exposure_id,at)
            SELECT ?1,?2,(SELECT id FROM exposures WHERE id=?3),?4
            WHERE EXISTS (SELECT 1 FROM articles WHERE id=?1 AND expires_at IS NULL)",
        )?;
        for r in rows {
            written += insert.execute(params![r.article_id, r.kind, r.exposure_id, r.at])?;
        }
    }
    tx.commit()?;
    Ok(written)
}

pub fn record_read_sessions(conn: &Connection, rows: &[ReadSession]) -> Result<usize> {
    let tx = db::write_transaction(conn)?;
    let mut written = 0;
    {
        let mut insert = tx.prepare_cached("INSERT INTO read_sessions
            (article_id,exposure_id,opened_at,active_ms,idle_ms,read_chars,skimmed_chars,total_chars,
             max_block,reached_end,translated,ended_by,via)
            SELECT ?1,(SELECT id FROM exposures WHERE id=?2),?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13
            WHERE EXISTS (SELECT 1 FROM articles WHERE id=?1 AND expires_at IS NULL)")?;
        for r in rows {
            written += insert.execute(params![
                r.article_id,
                r.exposure_id,
                r.opened_at,
                r.active_ms,
                r.idle_ms,
                r.read_chars,
                r.skimmed_chars,
                r.total_chars,
                r.max_block,
                r.reached_end,
                r.translated,
                r.ended_by,
                r.via
            ])?;
        }
    }
    tx.commit()?;
    Ok(written)
}

/// 数据转换失败（空值、未知枚举等）跳过该行，SQL / I/O 错误仍返回。
pub(crate) fn rows<T>(
    conn: &Connection,
    sql: &str,
    since: i64,
    until: i64,
    map: impl FnMut(&Row<'_>) -> rusqlite::Result<T>,
) -> Result<Vec<T>> {
    Ok(conn
        .prepare(sql)?
        .query_map([since, until], map)?
        .filter_map(std::result::Result::ok)
        .collect())
}

pub(crate) fn load_events(conn: &Connection, since: i64, until: i64) -> Result<Vec<ArticleEvent>> {
    rows(
        conn,
        "SELECT article_id,kind,exposure_id,at FROM article_events
         WHERE article_id IN (SELECT id FROM articles WHERE expires_at IS NULL) AND at>=?1 AND at<?2 ORDER BY at,id",
        since,
        until,
        |r| {
            Ok(ArticleEvent {
                article_id: r.get(0)?,
                kind: r.get(1)?,
                exposure_id: r.get(2)?,
                at: r.get(3)?,
            })
        },
    )
}

/// UTC 半开窗口；沿用回放的空值容错、语言推断与摘要源判定。
pub fn load_session_summaries(
    conn: &Connection,
    since: i64,
    until: i64,
) -> Result<Vec<SessionSummary>> {
    rows(
        conn,
        "SELECT r.article_id,r.opened_at,r.active_ms,r.idle_ms,r.read_chars,r.skimmed_chars,
        r.total_chars,r.max_block,r.reached_end,r.translated,
        COALESCE(a.fulltext_md,'')='',COALESCE(a.title,'')||COALESCE(a.content_html,'')
        FROM read_sessions r JOIN articles a ON a.id=r.article_id
        WHERE a.expires_at IS NULL AND r.opened_at>=?1 AND r.opened_at<?2 ORDER BY r.opened_at,r.id",
        since,
        until,
        |r| {
            let article_id = r.get(0)?;
            let opened_at: i64 = r.get(1)?;
            let unsigned = |index| {
                r.get::<_, Option<i64>>(index)
                    .map(|n| n.unwrap_or(0).max(0) as u64)
            };
            let text: String = r.get(11)?;
            // 与前端 features/reader/readingSession.ts 一致：非空白文字中 CJK 占比 > 0.3。
            static CJK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
                regex::Regex::new(r"[\p{Han}\p{Hiragana}\p{Katakana}\p{Hangul}]").unwrap()
            });
            let text: String = crate::score::strip_html(&text)
                .chars()
                .take(2_000)
                .filter(|c| !c.is_whitespace())
                .collect();
            let chars = text.chars().count();
            let lang = if chars > 0 && CJK.find_iter(&text).count() as f64 / chars as f64 > 0.3 {
                Language::Chinese
            } else {
                Language::English
            };
            unsigned(3)?;
            unsigned(7)?;
            let total_chars = unsigned(6)?;
            Ok(SessionSummary {
                article_id,
                opened_at,
                active_ms: unsigned(2)?,
                read_chars: unsigned(4)?,
                skimmed_chars: unsigned(5)?,
                total_chars,
                reached_end: r.get::<_, Option<bool>>(8)?.unwrap_or(false),
                translated: r.get::<_, Option<bool>>(9)?.unwrap_or(false),
                lang,
                summary_only: r.get::<_, bool>(10)?
                    && lang.reading_units(total_chars) <= engagement::SHORT_TEXT_UNITS,
                opened_original: false,
            })
        },
    )
}

/// 三类日志在同一事务中按各自时间列清理，返回删除的总行数。
pub fn prune(conn: &Connection, now_ms: i64) -> Result<usize> {
    let cutoff = now_ms.saturating_sub(RETENTION_MS);
    let tx = db::write_transaction(conn)?;
    let mut removed = 0;
    for (table, time) in [
        ("article_events", "at"),
        ("read_sessions", "opened_at"),
        ("exposures", "shown_at"),
    ] {
        removed += tx.execute(&format!("DELETE FROM {table} WHERE {time} < ?1"), [cutoff])?;
    }
    tx.commit()?;
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn fixture() -> (tempfile::TempDir, db::PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("signals.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://test.invalid/rss',0);
            INSERT INTO articles(id,feed_id,title,created_at) VALUES(1,1,'article',0);",
        )
        .unwrap();
        (dir, conn)
    }

    fn exposure(at: i64) -> Exposure {
        Exposure {
            article_id: 1,
            surface: Surface::Focus,
            position: 3,
            variant: "baseline".into(),
            session_id: "session".into(),
            shown_at: at,
        }
    }

    fn reading(at: i64, exposure_id: Option<i64>) -> ReadSession {
        ReadSession {
            article_id: 1,
            exposure_id,
            opened_at: at,
            active_ms: 10_000,
            idle_ms: 5_000,
            read_chars: 100,
            skimmed_chars: 200,
            total_chars: 500,
            max_block: 4,
            reached_end: false,
            translated: true,
            ended_by: EndedBy::Back,
            via: Via::Focus,
        }
    }

    fn count(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn exposure_scores_come_from_article_and_follow_boost_switch() {
        let (_dir, conn) = fixture();
        conn.execute(
            "UPDATE articles SET score=4, interest_boost=5 WHERE id=1",
            [],
        )
        .unwrap();
        let read = |session: &str| {
            let mut e = exposure(NOW);
            e.session_id = session.into();
            let id = record_exposures(&conn, &[e]).unwrap()[0].unwrap();
            conn.query_row(
                "SELECT score, interest_boost, rank_score FROM exposures WHERE id=?1",
                [id],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, f64>(2)?,
                    ))
                },
            )
            .unwrap()
        };
        assert_eq!(read("on"), (4, 5, 9.0));
        conn.execute(
            "INSERT INTO article_rank(article_id,boost,reasons_json,algo_version,computed_at)
             VALUES(1,3,'[]',?1,?2)",
            params![crate::rank::combine::ALGO_VERSION, NOW],
        )
        .unwrap();
        assert_eq!(
            read("rank"),
            (4, 5, crate::rank::focus::score(4, 5, true, 3) as f64)
        );
        crate::interests::set_boost_enabled(&conn, false).unwrap();
        assert_eq!(
            read("off"),
            (4, 5, crate::rank::focus::score(4, 5, false, 3) as f64)
        );
    }

    #[test]
    fn exposures_keep_first_across_and_within_batches() {
        let (_dir, conn) = fixture();
        let first = exposure(NOW);
        let mut duplicate = exposure(NOW + 1);
        duplicate.position = 0;
        duplicate.surface = Surface::Drawer;
        let ids = record_exposures(&conn, &[first.clone(), duplicate.clone()]).unwrap();
        assert_eq!(ids[0], ids[1]);
        assert_eq!(record_exposures(&conn, &[duplicate]).unwrap(), vec![ids[0]]);
        let mut other_session = first;
        other_session.session_id = "other".into();
        let other = record_exposures(&conn, &[other_session]).unwrap();
        assert_ne!(ids, other);
        assert_eq!(count(&conn, "exposures"), 2);
        assert_eq!(
            conn.query_row(
                "SELECT surface,position,shown_at FROM exposures WHERE id=?1",
                [ids[0]],
                |r| Ok((
                    r.get::<_, Surface>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?
                ))
            )
            .unwrap(),
            (Surface::Focus, 3, NOW)
        );
    }

    #[test]
    fn enums_round_trip_and_reject_unknown_sql_and_json() {
        let conn = Connection::open_in_memory().unwrap();
        macro_rules! check {
            ($ty:ty, [$($variant:expr),+]) => {
                for v in [$($variant),+] {
                    assert_eq!(conn.query_row("SELECT ?1", [v], |r| r.get::<_, $ty>(0)).unwrap(), v);
                    assert_eq!(serde_json::from_str::<$ty>(&serde_json::to_string(&v).unwrap()).unwrap(), v);
                    assert_eq!(<$ty>::parse_str(v.as_str()).unwrap(), v);
                }
                assert!(conn.query_row("SELECT 'unknown'", [], |r| r.get::<_, $ty>(0)).is_err());
                assert!(serde_json::from_str::<$ty>("\"unknown\"").is_err());
            };
        }
        check!(
            Surface,
            [
                Surface::Focus,
                Surface::Drawer,
                Surface::Search,
                Surface::Digest,
                Surface::Feeds,
                Surface::FocusColumn
            ]
        );
        check!(
            EventKind,
            [
                EventKind::Open,
                EventKind::Star,
                EventKind::Unstar,
                EventKind::Up,
                EventKind::Down,
                EventKind::Cite,
                EventKind::Ask,
                EventKind::OpenOriginal
            ]
        );
        check!(
            EndedBy,
            [
                EndedBy::Back,
                EndedBy::Next,
                EndedBy::Close,
                EndedBy::FocusTimeout
            ]
        );
        check!(
            Via,
            [
                Via::Focus,
                Via::Drawer,
                Via::Search,
                Via::Digest,
                Via::Citation,
                Via::Feeds,
                Via::FocusColumn
            ]
        );
    }

    #[test]
    fn batches_skip_deleted_articles_and_detach_expired_exposures() {
        let (_dir, conn) = fixture();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,created_at) VALUES(2,'https://deleted.invalid/rss',0);
            INSERT INTO articles(id,feed_id,title,created_at) VALUES(2,2,'deleted',0);",
        )
        .unwrap();
        let mut deleted = exposure(NOW);
        deleted.article_id = 2;
        crate::feeds::unsubscribe(&conn, 2).unwrap();
        let ids = record_exposures(&conn, &[deleted.clone(), exposure(NOW), deleted]).unwrap();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[0], None);
        assert!(ids[1].is_some());
        assert_eq!(ids[2], None);
        assert_eq!(count(&conn, "exposures"), 1);
        let mut expired = exposure(NOW - RETENTION_MS - 1);
        expired.session_id = "expired".into();
        let expired_id = record_exposures(&conn, &[expired]).unwrap()[0];
        assert!(expired_id.is_some());
        assert_eq!(prune(&conn, NOW).unwrap(), 1);
        let references = [ids[1], expired_id, None];
        let mut events = Vec::new();
        let mut sessions = Vec::new();
        for (i, &exposure_id) in references.iter().enumerate() {
            let good = ArticleEvent {
                article_id: 1,
                kind: EventKind::Open,
                exposure_id,
                at: NOW + i as i64,
            };
            events.push(ArticleEvent {
                article_id: 2,
                ..good.clone()
            });
            events.push(good);
            let good = reading(NOW + i as i64, exposure_id);
            sessions.push(ReadSession {
                article_id: 2,
                ..good.clone()
            });
            sessions.push(good);
        }
        assert_eq!(record_events(&conn, &events).unwrap(), 3);
        assert_eq!(record_read_sessions(&conn, &sessions).unwrap(), 3);
        for table in ["article_events", "read_sessions"] {
            let values = conn
                .prepare(&format!(
                    "SELECT article_id,exposure_id FROM {table} ORDER BY id"
                ))
                .unwrap()
                .query_map([], |r| {
                    Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(values, vec![(1, ids[1]), (1, None), (1, None)]);
        }
        assert_eq!(record_events(&conn, &events[..1]).unwrap(), 0);
        assert_eq!(record_read_sessions(&conn, &sessions[..1]).unwrap(), 0);
    }

    #[test]
    fn retention_keeps_cutoff_and_newer_and_detaches_expired_exposure() {
        let (_dir, conn) = fixture();
        let cutoff = NOW - RETENTION_MS;
        let mut rows = Vec::new();
        for at in [cutoff - 1, cutoff, cutoff + 1] {
            let mut r = exposure(at);
            r.session_id = at.to_string();
            rows.push(r);
        }
        let ids = record_exposures(&conn, &rows).unwrap();
        for at in [cutoff - 1, cutoff, cutoff + 1] {
            record_events(
                &conn,
                &[ArticleEvent {
                    article_id: 1,
                    kind: EventKind::Open,
                    exposure_id: ids[0],
                    at,
                }],
            )
            .unwrap();
            record_read_sessions(&conn, &[reading(at, ids[0])]).unwrap();
        }
        assert_eq!(prune(&conn, NOW).unwrap(), 3);
        for t in ["exposures", "article_events", "read_sessions"] {
            assert_eq!(count(&conn, t), 2);
        }
        for t in ["article_events", "read_sessions"] {
            assert_eq!(
                conn.query_row(
                    &format!("SELECT COUNT(*) FROM {t} WHERE exposure_id IS NULL"),
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                2
            );
        }
        assert_eq!(prune(&conn, NOW).unwrap(), 0);
    }

    #[test]
    fn deleting_article_cascades_all_signals() {
        let (_dir, conn) = fixture();
        let id = record_exposures(&conn, &[exposure(NOW)]).unwrap()[0];
        record_events(
            &conn,
            &[ArticleEvent {
                article_id: 1,
                kind: EventKind::Cite,
                exposure_id: id,
                at: NOW,
            }],
        )
        .unwrap();
        record_read_sessions(&conn, &[reading(NOW, id)]).unwrap();
        conn.execute("DELETE FROM articles WHERE id=1", []).unwrap();
        for t in ["exposures", "article_events", "read_sessions"] {
            assert_eq!(count(&conn, t), 0);
        }
    }
}
