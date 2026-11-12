//! 存储占用：库内各用途占的页字节数，以及数据目录下文件的体积。只读统计，不做清理。
//!
//! 库内体积来自 SQLite `dbstat`（按 B 树统计实际页大小，含索引与 FTS 影子表），
//! 与空闲页相加等于库文件的页数 × 页大小。统计要扫一遍所有页，只在用户打开页面时调用。

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rusqlite::Connection;
use serde::Serialize;

use crate::error::Result;

/// 库内按用途归类。顺序即展示顺序。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum DbPart {
    /// 文章元数据与正文，以及挂在文章上的标签、赞踩、兴趣命中与聚类。
    Articles,
    /// `articles_fts` 及其影子表。
    SearchIndex,
    /// 分段文本与向量。
    Vectors,
    /// 译文缓存。
    Translations,
    /// AI 调用结果缓存。
    AiCache,
    /// 对话、消息、记忆与兴趣。
    Conversations,
    /// 简报。
    Digests,
    /// 曝光、行为与阅读会话日志，含索引。
    RecommendationLogs,
    /// 设置、订阅、规则、用量等其余表与 SQLite 自身的结构。
    Other,
    /// 早期版本留下、当前代码不读写的表，见 [`LEGACY_TABLES`]。
    Legacy,
}

pub const DB_PARTS: [DbPart; 10] = [
    DbPart::Articles,
    DbPart::SearchIndex,
    DbPart::Vectors,
    DbPart::Translations,
    DbPart::AiCache,
    DbPart::Conversations,
    DbPart::Digests,
    DbPart::RecommendationLogs,
    DbPart::Other,
    DbPart::Legacy,
];

impl DbPart {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Articles => "articles",
            Self::SearchIndex => "search_index",
            Self::Vectors => "vectors",
            Self::Translations => "translations",
            Self::AiCache => "ai_cache",
            Self::Conversations => "conversations",
            Self::Digests => "digests",
            Self::RecommendationLogs => "recommendation_logs",
            Self::Other => "other",
            Self::Legacy => "legacy",
        }
    }

    /// 按所属表归类；索引按它所在的表算。
    pub fn of_table(table: &str) -> Self {
        if table == "articles_fts" || table.starts_with("articles_fts_") {
            return Self::SearchIndex;
        }
        match table {
            "articles" | "article_tags" | "article_interests" | "feedback" | "clusters" => {
                Self::Articles
            }
            "chunks" | "vector_spaces" | "vector_space_probes" | "vector_staging" => Self::Vectors,
            "article_translations" => Self::Translations,
            "translations" | "feed_sub_records" => Self::Legacy,
            "ai_cache" => Self::AiCache,
            "threads" | "messages" | "memories" | "interests" => Self::Conversations,
            "digests" => Self::Digests,
            "exposures" | "article_events" | "read_sessions" => Self::RecommendationLogs,
            _ => Self::Other,
        }
    }

    /// 用来给出条数的表；多表合计，没有明确「条」的类别返回空切片。
    fn count_tables(self) -> &'static [&'static str] {
        match self {
            Self::Articles => &["articles"],
            Self::Vectors => &["chunks"],
            Self::Translations => &["article_translations"],
            Self::AiCache => &["ai_cache"],
            Self::Conversations => &["threads"],
            Self::Digests => &["digests"],
            Self::RecommendationLogs => &RECOMMENDATION_TABLES,
            Self::SearchIndex | Self::Other | Self::Legacy => &[],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DbPartUsage {
    pub part: DbPart,
    pub bytes: i64,
    pub rows: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DbUsage {
    /// 页数 × 页大小，即库主文件应有的大小（不含 WAL）。
    pub file_bytes: i64,
    /// 空闲页。删除数据后留下，`VACUUM` 才会还给磁盘。
    pub free_bytes: i64,
    /// 按 [`DB_PARTS`] 顺序，每类一项（0 字节也列出）。
    pub parts: Vec<DbPartUsage>,
}

pub fn db_usage(conn: &Connection) -> Result<DbUsage> {
    let page_size: i64 = conn.pragma_query_value(None, "page_size", |row| row.get(0))?;
    let page_count: i64 = conn.pragma_query_value(None, "page_count", |row| row.get(0))?;
    let freelist: i64 = conn.pragma_query_value(None, "freelist_count", |row| row.get(0))?;

    let owners: BTreeMap<String, String> = {
        let mut stmt = conn.prepare("SELECT name, tbl_name FROM sqlite_schema")?;
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?
    };
    let mut bytes: BTreeMap<DbPart, i64> = DB_PARTS.iter().map(|p| (*p, 0)).collect();
    let mut stmt = conn.prepare("SELECT name, pgsize FROM dbstat WHERE aggregate = TRUE")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (name, size) = row?;
        let table = owners
            .get(&name)
            .map(String::as_str)
            .unwrap_or(name.as_str());
        *bytes.entry(DbPart::of_table(table)).or_default() += size;
    }

    let mut parts = Vec::with_capacity(DB_PARTS.len());
    for part in DB_PARTS {
        let mut rows = None;
        for table in part.count_tables() {
            if !owners.contains_key(*table) {
                continue;
            }
            let count: i64 =
                conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })?;
            rows = Some(rows.unwrap_or(0) + count);
        }
        parts.push(DbPartUsage {
            part,
            bytes: bytes[&part],
            rows,
        });
    }
    Ok(DbUsage {
        file_bytes: page_count * page_size,
        free_bytes: freelist * page_size,
        parts,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MediaUsage {
    /// 文章配图（`kind = 'image'`）。
    pub image_bytes: i64,
    /// 直链音视频（`kind = 'video'`）。
    pub video_bytes: i64,
    /// 封面元信息等其余登记文件，不参与配图 / 音视频淘汰。
    pub other_bytes: i64,
    /// 媒体目录里没有登记在 `media` 表中、且不是刚写入的文件，例如中断的下载。
    pub untracked_bytes: i64,
    pub untracked_files: i64,
}

/// 刚写入的文件可能还没来得及登记，不算未登记。
const UNTRACKED_MIN_AGE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// 媒体缓存按类别统计。多行共用同一个文件时只算一次。
pub fn media_usage(conn: &Connection, data_dir: &Path) -> Result<MediaUsage> {
    let mut usage = MediaUsage::default();
    let mut stmt = conn.prepare(
        "SELECT kind, COALESCE(SUM(bytes), 0) FROM (
             SELECT MAX(kind) AS kind, MAX(COALESCE(bytes, 0)) AS bytes FROM media
             WHERE local_path IS NOT NULL AND local_path != ''
             GROUP BY local_path
         )
         GROUP BY kind",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (kind, sum) = row?;
        match kind.as_str() {
            "image" => usage.image_bytes += sum,
            "video" => usage.video_bytes += sum,
            _ => usage.other_bytes += sum,
        }
    }
    for (_, len) in untracked_media_files(conn, data_dir)? {
        usage.untracked_bytes += len as i64;
        usage.untracked_files += 1;
    }
    Ok(usage)
}

/// 媒体目录中未登记、且修改时间早于 [`UNTRACKED_MIN_AGE`] 的普通文件。
pub fn untracked_media_files(conn: &Connection, data_dir: &Path) -> Result<Vec<(PathBuf, u64)>> {
    let dir = crate::media::cache_dir(data_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };
    let tracked: HashSet<OsString> = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT local_path FROM media WHERE local_path IS NOT NULL AND local_path != ''",
        )?;
        stmt.query_map([], |row| row.get::<_, String>(0))?
            .filter_map(|p| p.ok())
            .filter_map(|p| Path::new(&p).file_name().map(|n| n.to_os_string()))
            .collect()
    };
    let now = SystemTime::now();
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Ok(meta) = entry.path().symlink_metadata() else {
            continue;
        };
        if !meta.is_file() || tracked.contains(&entry.file_name()) {
            continue;
        }
        let fresh = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_none_or(|age| age < UNTRACKED_MIN_AGE);
        if !fresh {
            out.push((entry.path(), meta.len()));
        }
    }
    Ok(out)
}

/// 目录下所有普通文件的字节数之和，不跟随符号链接；目录不存在时为 0。
pub fn dir_bytes(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        let Ok(meta) = entry.path().symlink_metadata() else {
            continue;
        };
        if meta.is_dir() {
            total += dir_bytes(&entry.path());
        } else if meta.is_file() {
            total += meta.len();
        }
    }
    total
}

/// 单个文件的字节数；不存在时为 0。用于库主文件与 `-wal`。
pub fn file_bytes(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// 可以在存储页清理的内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cleanup {
    AiCache,
    RecommendationLogs,
    Translations,
    Images,
    Videos,
    UntrackedMedia,
    /// 不是当前嵌入模型生成的向量；分段文本保留，之后按当前模型重新生成。
    OldVectors,
    /// 早期版本留下、当前代码不读写的表。
    LegacyTables,
}

impl Cleanup {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "ai_cache" => Self::AiCache,
            "recommendation_logs" => Self::RecommendationLogs,
            "translations" => Self::Translations,
            "images" => Self::Images,
            "videos" => Self::Videos,
            "untracked_media" => Self::UntrackedMedia,
            "old_vectors" => Self::OldVectors,
            "legacy_tables" => Self::LegacyTables,
            _ => return None,
        })
    }
}

/// 当前代码不再读写的遗留表；只在存在时返回。
pub const LEGACY_TABLES: [&str; 2] = ["translations", "feed_sub_records"];

// 先删引用曝光的两张表，最后删曝光；统计与清理共用同一范围。
const RECOMMENDATION_TABLES: [&str; 3] = ["article_events", "read_sessions", "exposures"];

pub fn legacy_tables(conn: &Connection) -> Result<Vec<&'static str>> {
    let mut out = Vec::new();
    for table in LEGACY_TABLES {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = ?1",
            [table],
            |row| row.get(0),
        )?;
        if n > 0 {
            out.push(table);
        }
    }
    Ok(out)
}

const OLD_SPACES: &str =
    "space_id IS NULL OR space_id NOT IN (SELECT id FROM vector_spaces WHERE state!='retired')";

/// 退役空间残留的暂存行与没有生效空间归属的向量。
pub fn old_vector_count(conn: &Connection) -> Result<i64> {
    let mut n = 0;
    for table in ["chunks", "interests", "memories", "vector_staging"] {
        n += conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE embedding IS NOT NULL AND ({OLD_SPACES})"),
            [],
            |r| r.get::<_, i64>(0),
        )?;
    }
    Ok(n)
}

/// 执行一项清理，返回删除的条数（文件或行）。模型参数暂留兼容调用方，旧向量只看空间状态。
pub fn clean(
    conn: &Connection,
    data_dir: &Path,
    what: Cleanup,
    _current_embed_model: Option<&str>,
) -> Result<i64> {
    let n = match what {
        Cleanup::AiCache => conn.execute("DELETE FROM ai_cache", [])? as i64,
        Cleanup::RecommendationLogs => {
            let tx = crate::db::write_transaction(conn)?;
            let mut removed = 0;
            for table in RECOMMENDATION_TABLES {
                removed += tx.execute(&format!("DELETE FROM {table}"), [])? as i64;
            }
            tx.commit()?;
            removed
        }
        Cleanup::Translations => conn.execute("DELETE FROM article_translations", [])? as i64,
        Cleanup::Images => clear_media_kind(conn, "image")?,
        Cleanup::Videos => clear_media_kind(conn, "video")?,
        Cleanup::UntrackedMedia => {
            let mut n = 0;
            for (path, _) in untracked_media_files(conn, data_dir)? {
                if std::fs::remove_file(&path).is_ok() {
                    n += 1;
                }
            }
            n
        }
        Cleanup::OldVectors => crate::db::atomic_write(conn, |conn| {
            let count = old_vector_count(conn)?;
            for table in ["chunks", "interests", "memories"] {
                let reset = if table == "chunks" {
                    ",model=NULL,attempts=0,last_error=NULL"
                } else {
                    ""
                };
                conn.execute(&format!("UPDATE {table} SET embedding=NULL,space_id=NULL{reset} WHERE embedding IS NOT NULL AND ({OLD_SPACES})"),[])?;
            }
            conn.execute(
                &format!("DELETE FROM vector_staging WHERE {OLD_SPACES}"),
                [],
            )?;
            conn.execute("DELETE FROM vector_space_probes WHERE space_id IN (SELECT id FROM vector_spaces WHERE state='retired')",[])?;
            Ok(count)
        })?,
        Cleanup::LegacyTables => {
            let tables = legacy_tables(conn)?;
            for table in &tables {
                conn.execute_batch(&format!("DROP TABLE {table}"))?;
            }
            tables.len() as i64
        }
    };
    tracing::info!(?what, n, "storage cleanup");
    Ok(n)
}

fn clear_media_kind(conn: &Connection, kind: &str) -> Result<i64> {
    let rows: Vec<(i64, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, local_path FROM media
             WHERE kind = ?1 AND local_path IS NOT NULL AND local_path != ''",
        )?;
        stmt.query_map([kind], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<std::result::Result<_, _>>()?
    };
    for (id, local_path) in &rows {
        crate::media::remove_cached(conn, *id, local_path)?;
    }
    Ok(rows.len() as i64)
}

/// 删除 `older_than_ms` 之前写入的 AI 缓存，返回删除条数。
pub fn prune_ai_cache(conn: &Connection, older_than_ms: i64) -> Result<i64> {
    Ok(conn.execute(
        "DELETE FROM ai_cache WHERE created_at < ?1",
        [older_than_ms],
    )? as i64)
}

/// 把空闲页还给磁盘并截断 WAL。需要约等于库大小的临时空间；空间不足或库正忙时报错，原库不变。
pub fn vacuum(conn: &Connection) -> Result<()> {
    conn.execute_batch("VACUUM")?;
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
    Ok(())
}

/// 剩余可用空间由平台模块取（各平台实现不同）。
pub use crate::platform::available_bytes;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn part(usage: &DbUsage, part: DbPart) -> &DbPartUsage {
        usage.parts.iter().find(|p| p.part == part).unwrap()
    }

    #[test]
    fn classifies_tables_indexes_and_fts_shadows() {
        assert_eq!(DbPart::of_table("articles"), DbPart::Articles);
        assert_eq!(DbPart::of_table("articles_fts"), DbPart::SearchIndex);
        assert_eq!(DbPart::of_table("articles_fts_data"), DbPart::SearchIndex);
        assert_eq!(DbPart::of_table("chunks"), DbPart::Vectors);
        assert_eq!(DbPart::of_table("translations"), DbPart::Legacy);
        assert_eq!(DbPart::of_table("sqlite_schema"), DbPart::Other);
        assert_eq!(DbPart::of_table("feeds"), DbPart::Other);
        for table in ["exposures", "article_events", "read_sessions"] {
            assert_eq!(DbPart::of_table(table), DbPart::RecommendationLogs);
        }
    }

    #[test]
    fn recommendation_logs_include_all_three_tables_and_clean_only_their_rows() {
        let dir = tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("storage.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://test.invalid/rss',0);
             INSERT INTO articles(id,feed_id,title,created_at) VALUES(1,1,'article',0);
             INSERT INTO feedback(article_id,kind,created_at) VALUES(1,'up',0);
             INSERT INTO ai_cache(cache_key,value,created_at) VALUES('keep','value',0);
             INSERT INTO exposures(id,article_id,surface,position,score,interest_boost,rank_score,
                 variant,session_id,shown_at) VALUES(1,1,'focus',0,0,0,0,'baseline','session',0);
             INSERT INTO article_events(article_id,kind,exposure_id,at) VALUES
                 (1,'open',1,0),(1,'star',NULL,0);
             INSERT INTO read_sessions(article_id,exposure_id,opened_at,active_ms,idle_ms,
                 read_chars,skimmed_chars,total_chars,max_block,reached_end,translated,ended_by,via)
                 VALUES(1,1,0,1000,0,10,0,100,0,0,0,'back','focus');",
        )
        .unwrap();
        let before = db_usage(&conn).unwrap();
        let signal_bytes: i64 = conn.query_row("SELECT SUM(pgsize) FROM dbstat WHERE name IN
            (SELECT name FROM sqlite_schema WHERE tbl_name IN ('exposures','article_events','read_sessions'))", [], |r| r.get(0)).unwrap();
        assert!(signal_bytes > 0);
        assert_eq!(
            part(&before, DbPart::RecommendationLogs).bytes,
            signal_bytes
        );
        assert_eq!(part(&before, DbPart::RecommendationLogs).rows, Some(4));
        assert_eq!(
            Cleanup::parse("recommendation_logs"),
            Some(Cleanup::RecommendationLogs)
        );
        assert_eq!(
            clean(&conn, dir.path(), Cleanup::RecommendationLogs, None).unwrap(),
            4
        );
        let after = db_usage(&conn).unwrap();
        assert_eq!(part(&after, DbPart::RecommendationLogs).rows, Some(0));
        for table in ["articles", "feedback", "feeds", "ai_cache"] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 1, "{table} must be preserved");
        }
        assert_eq!(
            part(&before, DbPart::Other).bytes,
            part(&after, DbPart::Other).bytes
        );
        assert_eq!(
            after.parts.iter().map(|p| p.bytes).sum::<i64>() + after.free_bytes,
            after.file_bytes
        );
    }

    #[test]
    fn parts_and_free_pages_add_up_to_file() {
        let dir = tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("rrss.db")).unwrap();
        conn.execute(
            "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://a.example/feed', 1)",
            [],
        )
        .unwrap();
        let body = "正文段落 ".repeat(2000);
        for i in 0..20 {
            conn.execute(
                "INSERT INTO articles (feed_id, guid, title, content_html, created_at)
                 VALUES (1, ?1, 'title', ?2, 1)",
                rusqlite::params![format!("g{i}"), body],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chunks (article_id, ordinal, text, embedding)
                 VALUES (last_insert_rowid(), 0, 'chunk', zeroblob(3072))",
                [],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO ai_cache (cache_key, value, created_at) VALUES ('k', 'v', 1)",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM articles WHERE guid IN ('g0', 'g1', 'g2')", [])
            .unwrap();

        let usage = db_usage(&conn).unwrap();
        let sum: i64 = usage.parts.iter().map(|p| p.bytes).sum();
        assert_eq!(sum + usage.free_bytes, usage.file_bytes);
        assert_eq!(usage.parts.len(), DB_PARTS.len());
        assert!(usage.free_bytes > 0, "deleted rows should leave free pages");

        let articles = part(&usage, DbPart::Articles);
        assert_eq!(articles.rows, Some(17));
        assert!(
            articles.bytes > 17 * 4096,
            "bodies should dominate: {articles:?}"
        );
        assert!(part(&usage, DbPart::SearchIndex).bytes > 0);
        assert_eq!(part(&usage, DbPart::Vectors).rows, Some(17));
        assert_eq!(part(&usage, DbPart::AiCache).rows, Some(1));
        assert_eq!(part(&usage, DbPart::SearchIndex).rows, None);
    }

    fn age(path: &Path, secs: u64) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - std::time::Duration::from_secs(secs))
            .unwrap();
    }

    fn media_fixture() -> (tempfile::TempDir, crate::db::PooledConnection, PathBuf) {
        let dir = tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("rrss.db")).unwrap();
        let cache = crate::media::cache_dir(dir.path());
        std::fs::create_dir_all(&cache).unwrap();
        for (name, len) in [
            ("img", 100),
            ("vid", 300),
            ("meta-x", 7),
            ("stale.part", 50),
            ("fresh.part", 20),
        ] {
            std::fs::write(cache.join(name), vec![0u8; len]).unwrap();
            if name != "fresh.part" {
                age(&cache.join(name), 3600);
            }
        }
        let p = |n: &str| cache.join(n).to_string_lossy().into_owned();
        conn.execute(
            "INSERT INTO media (url, local_path, kind, bytes, created_at) VALUES
                 ('https://a/1.png', ?1, 'image', 100, 1),
                 ('https://b/1.png', ?1, 'image', 100, 1),
                 ('https://a/2.mp4', ?2, 'video', 300, 1),
                 ('https://a/v', ?3, 'other', 7, 1),
                 ('https://a/3.png', NULL, 'image', NULL, 1)",
            rusqlite::params![p("img"), p("vid"), p("meta-x")],
        )
        .unwrap();
        (dir, conn, cache)
    }

    #[test]
    fn media_usage_splits_kinds_and_skips_fresh_untracked_files() {
        let (dir, conn, _cache) = media_fixture();
        let usage = media_usage(&conn, dir.path()).unwrap();
        assert_eq!(
            usage,
            MediaUsage {
                image_bytes: 100,
                video_bytes: 300,
                other_bytes: 7,
                untracked_bytes: 50,
                untracked_files: 1,
            }
        );
    }

    #[test]
    fn clean_media_kinds_and_untracked_files() {
        let (dir, conn, cache) = media_fixture();
        assert_eq!(
            clean(&conn, dir.path(), Cleanup::UntrackedMedia, None).unwrap(),
            1
        );
        assert!(!cache.join("stale.part").exists());
        assert!(cache.join("fresh.part").exists(), "fresh downloads stay");

        assert_eq!(clean(&conn, dir.path(), Cleanup::Images, None).unwrap(), 2);
        assert!(!cache.join("img").exists());
        assert!(cache.join("vid").exists() && cache.join("meta-x").exists());
        assert_eq!(clean(&conn, dir.path(), Cleanup::Videos, None).unwrap(), 1);
        assert!(!cache.join("vid").exists());
        let usage = media_usage(&conn, dir.path()).unwrap();
        assert_eq!(
            (usage.image_bytes, usage.video_bytes, usage.other_bytes),
            (0, 0, 7)
        );
    }

    #[test]
    fn clean_old_vectors_keeps_active_space_and_chunk_text() {
        let dir = tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("rrss.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO feeds (id, url, created_at) VALUES (1, 'https://a.example/feed', 1);
             INSERT INTO articles (id, feed_id, title, created_at) VALUES (1, 1, 't', 1);
             INSERT INTO chunks (article_id, ordinal, text, embedding, model) VALUES
                 (1, 0, 'a', x'00', 'old'),
                 (1, 1, 'b', x'00', 'new'),
                 (1, 2, 'c', NULL, NULL);",
        )
        .unwrap();
        conn.execute_batch("INSERT INTO vector_spaces(id,base_url,model,dims,prefix_version,normalized,state,created_at) VALUES
            (1,'','old',0,1,0,'retired',0), (2,'','new',0,1,0,'active',0);
            UPDATE chunks SET space_id = CASE model WHEN 'old' THEN 1 WHEN 'new' THEN 2 END;").unwrap();
        assert_eq!(old_vector_count(&conn).unwrap(), 1);
        assert_eq!(
            clean(&conn, dir.path(), Cleanup::OldVectors, None).unwrap(),
            1
        );
        assert_eq!(
            clean(&conn, dir.path(), Cleanup::OldVectors, Some("new")).unwrap(),
            0
        );
        let left: Vec<(String, Option<String>)> = conn
            .prepare("SELECT text, model FROM chunks ORDER BY ordinal")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            left,
            vec![
                ("a".into(), None),
                ("b".into(), Some("new".into())),
                ("c".into(), None)
            ]
        );
    }

    #[test]
    fn clean_caches_legacy_tables_and_prune_ai_cache() {
        let dir = tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("rrss.db")).unwrap();
        conn.execute_batch(
            "INSERT INTO ai_cache (cache_key, value, created_at) VALUES ('old', 'v', 10), ('new', 'v', 100);
             CREATE TABLE feed_sub_records (id INTEGER PRIMARY KEY);",
        )
        .unwrap();
        assert_eq!(prune_ai_cache(&conn, 50).unwrap(), 1);
        assert_eq!(clean(&conn, dir.path(), Cleanup::AiCache, None).unwrap(), 1);
        assert_eq!(legacy_tables(&conn).unwrap(), vec!["feed_sub_records"]);
        assert_eq!(
            clean(&conn, dir.path(), Cleanup::LegacyTables, None).unwrap(),
            1
        );
        assert!(legacy_tables(&conn).unwrap().is_empty());
        assert_eq!(
            clean(&conn, dir.path(), Cleanup::Translations, None).unwrap(),
            0
        );
    }

    #[test]
    fn vacuum_returns_free_pages() {
        let dir = tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("rrss.db")).unwrap();
        conn.execute(
            "INSERT INTO ai_cache (cache_key, value, created_at) VALUES ('k', ?1, 1)",
            [&"x".repeat(200_000)],
        )
        .unwrap();
        conn.execute("DELETE FROM ai_cache", []).unwrap();
        assert!(db_usage(&conn).unwrap().free_bytes > 0);
        vacuum(&conn).unwrap();
        assert_eq!(db_usage(&conn).unwrap().free_bytes, 0);
        assert!(available_bytes(dir.path()).is_none_or(|n| n > 0));
    }

    #[test]
    fn dir_bytes_is_zero_for_missing_dir() {
        let dir = tempdir().unwrap();
        assert_eq!(dir_bytes(&dir.path().join("missing")), 0);
    }
}
