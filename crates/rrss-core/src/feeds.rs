//! 文件夹、源、RSSHub 实例。日志 target：`rrss_core::feeds`。

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::db::HIDE_THRESHOLD_KEY;
use crate::error::{Error, Result};
use crate::kind::FeedKind;
use crate::url_norm::normalize_feed_url;

pub const NOISE_WINDOW_DAYS: i64 = 14;
pub const NOISE_MIN_ARTICLES: i64 = 8;
pub const NOISE_HIDE_RATE: f64 = 0.40;
pub const NOISE_DISLIKE_RATE: f64 = 0.50;
const MS_PER_DAY: i64 = 86_400_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedRow {
    pub id: i64,
    pub url: String,
    pub title: Option<String>,
    pub kind: FeedKind,
    pub site_url: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub interval_secs: i64,
    pub folder_id: Option<i64>,
    pub error: Option<String>,
    pub rsshub_instance_id: Option<i64>,
    pub last_fetched_at: Option<i64>,
    pub position: i64,
    pub next_fetch_at: i64,
    pub slow_fail_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderView {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub label: String,
    pub indent: i64,
    pub position: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderRow {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub position: i64,
}

#[derive(Debug, Clone)]
pub struct RsshubInstance {
    pub id: i64,
    pub name: String,
    pub base_url: String,
}

#[derive(Debug, Clone)]
pub struct SubscribeRequest {
    pub url: String,
    pub folder_id: Option<i64>,
    pub as_rsshub: bool,
    pub rsshub_instance_id: Option<i64>,
}

/// Internal placeholder, never fetched. Identity is enforced by the unique manual-kind index.
pub const MANUAL_URL: &str = "rrss:manual";

pub fn manual_feed(conn: &Connection) -> Result<i64> {
    conn.execute(
        "INSERT INTO feeds(url,title,kind,title_custom,created_at) VALUES(?1,'手动收录','manual',1,?2) ON CONFLICT DO NOTHING",
        params![MANUAL_URL, now_ms()],
    )?;
    Ok(conn.query_row("SELECT id FROM feeds WHERE kind='manual'", [], |r| r.get(0))?)
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn subscribe(conn: &Connection, req: SubscribeRequest) -> Result<i64> {
    let url = normalize_feed_url(&req.url)?;
    if let Some(folder_id) = req.folder_id {
        ensure_folder(conn, folder_id)?;
    }
    let mut kind = FeedKind::Rss;
    let mut instance_id = req.rsshub_instance_id;
    if req.as_rsshub || instance_id.is_some() {
        kind = FeedKind::Rsshub;
    }
    if instance_id.is_none()
        && let Some(id) = match_rsshub_instance(conn, &url)?
    {
        instance_id = Some(id);
        kind = FeedKind::Rsshub;
    }
    let created = now_ms();
    crate::db::atomic_write(conn, |conn| {
        let position = crate::tree_order::end_position(conn, req.folder_id)?;
        match conn.execute(
            "INSERT INTO feeds (url, kind, folder_id, rsshub_instance_id, position, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                url,
                kind.as_str(),
                req.folder_id,
                instance_id,
                position,
                created
            ],
        ) {
            Ok(_) => Ok(conn.last_insert_rowid()),
            Err(err) if is_unique(&err) => Err(Error::DuplicateFeed(url)),
            Err(err) => Err(err.into()),
        }
    })
}

pub fn unsubscribe(conn: &Connection, feed_id: i64) -> Result<()> {
    let _ = crate::media::cleanup_feed_media(conn, feed_id);
    let n = conn.execute("DELETE FROM feeds WHERE id = ?1", [feed_id])?;
    if n == 0 {
        Err(Error::FeedNotFound)
    } else {
        Ok(())
    }
}

pub fn rename_feed(conn: &Connection, feed_id: i64, title: &str) -> Result<()> {
    let trimmed = title.trim();
    let stored: Option<&str> = if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    };
    // 命名后刷新不再覆盖；清空则恢复跟随源标题。
    let n = conn.execute(
        "UPDATE feeds SET title = ?1, title_custom = ?2 WHERE id = ?3",
        params![stored, stored.is_some(), feed_id],
    )?;
    if n == 0 {
        Err(Error::FeedNotFound)
    } else {
        Ok(())
    }
}

pub fn update_feed_url(conn: &Connection, feed_id: i64, url: &str) -> Result<()> {
    let url = normalize_feed_url(url)?;
    match conn.execute(
        "UPDATE feeds SET url = ?1, etag = NULL, last_modified = NULL, error = NULL
         WHERE id = ?2",
        params![url, feed_id],
    ) {
        Ok(0) => Err(Error::FeedNotFound),
        Ok(_) => Ok(()),
        Err(err) if is_unique(&err) => Err(Error::DuplicateFeed(url)),
        Err(err) => Err(err.into()),
    }
}

pub fn get_feed(conn: &Connection, feed_id: i64) -> Result<FeedRow> {
    conn.query_row(
        "SELECT id, url, title, kind, site_url, etag, last_modified, interval_secs,
                folder_id, error, rsshub_instance_id, last_fetched_at, position, next_fetch_at, slow_fail_count
         FROM feeds WHERE id = ?1",
        [feed_id],
        feed_from_row,
    )
    .optional()?
    .ok_or(Error::FeedNotFound)
}

pub fn list_feeds(conn: &Connection) -> Result<Vec<FeedRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, url, title, kind, site_url, etag, last_modified, interval_secs,
                folder_id, error, rsshub_instance_id, last_fetched_at, position, next_fetch_at, slow_fail_count
         FROM feeds ORDER BY position, id",
    )?;
    let rows = stmt.query_map([], feed_from_row)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn set_folder(conn: &Connection, feed_id: i64, folder_id: Option<i64>) -> Result<()> {
    if let Some(id) = folder_id {
        ensure_folder(conn, id)?;
    }
    crate::db::atomic_write(conn, |conn| {
        let current: Option<Option<i64>> = conn
            .query_row(
                "SELECT folder_id FROM feeds WHERE id = ?1",
                [feed_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(current) = current else {
            return Err(Error::FeedNotFound);
        };
        if current == folder_id {
            return Ok(());
        }
        let position = crate::tree_order::end_position(conn, folder_id)?;
        conn.execute(
            "UPDATE feeds SET folder_id = ?1, position = ?2 WHERE id = ?3",
            params![folder_id, position, feed_id],
        )?;
        Ok(())
    })
}

pub fn create_folder(conn: &Connection, name: &str, parent_id: Option<i64>) -> Result<i64> {
    insert_folder(conn, name, parent_id, false)
}

/// 界面上的手动新建：放在父级最前，同级其余项顺延。
pub fn create_folder_first(conn: &Connection, name: &str, parent_id: Option<i64>) -> Result<i64> {
    insert_folder(conn, name, parent_id, true)
}

fn insert_folder(
    conn: &Connection,
    name: &str,
    parent_id: Option<i64>,
    at_front: bool,
) -> Result<i64> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::InvalidFolderName);
    }
    crate::db::atomic_write(conn, |conn| {
        if let Some(id) = parent_id {
            ensure_folder(conn, id)?;
        }
        let position = if at_front {
            crate::tree_order::shift_front(conn, parent_id)?;
            0
        } else {
            crate::tree_order::end_position(conn, parent_id)?
        };
        conn.execute(
            "INSERT INTO folders (parent_id, name, position, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![parent_id, name, position, now_ms()],
        )?;
        Ok(conn.last_insert_rowid())
    })
}

/// 同一层级下按名称找文件夹；已有重名时取最早建的那个。
pub fn find_folder(conn: &Connection, name: &str, parent_id: Option<i64>) -> Result<Option<i64>> {
    conn.query_row(
        "SELECT id FROM folders WHERE parent_id IS ?1 AND name = ?2 ORDER BY id LIMIT 1",
        params![parent_id, name.trim()],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub fn rename_folder(conn: &Connection, folder_id: i64, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::InvalidFolderName);
    }
    let n = conn.execute(
        "UPDATE folders SET name = ?1 WHERE id = ?2",
        params![name, folder_id],
    )?;
    if n == 0 {
        Err(Error::FolderNotFound)
    } else {
        Ok(())
    }
}

pub fn delete_folder(conn: &Connection, folder_id: i64) -> Result<()> {
    ensure_folder(conn, folder_id)?;
    conn.execute("DELETE FROM folders WHERE id = ?1", [folder_id])?;
    Ok(())
}

pub fn list_folders(conn: &Connection) -> Result<Vec<FolderRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, parent_id, name, position FROM folders ORDER BY parent_id, position, id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(FolderRow {
            id: row.get(0)?,
            parent_id: row.get(1)?,
            name: row.get(2)?,
            position: row.get(3)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn list_folder_views(conn: &Connection) -> Result<Vec<FolderView>> {
    let folders = list_folders(conn)?;
    let mut children: std::collections::HashMap<Option<i64>, Vec<&FolderRow>> =
        std::collections::HashMap::new();
    for folder in &folders {
        children.entry(folder.parent_id).or_default().push(folder);
    }
    for list in children.values_mut() {
        list.sort_by_key(|f| (f.position, f.id));
    }
    let mut out = Vec::new();
    walk_folders(&children, None, 0, "", &mut out);
    Ok(out)
}

fn walk_folders<'a>(
    children: &std::collections::HashMap<Option<i64>, Vec<&'a FolderRow>>,
    parent: Option<i64>,
    indent: i64,
    prefix: &str,
    out: &mut Vec<FolderView>,
) {
    let Some(list) = children.get(&parent) else {
        return;
    };
    for folder in list {
        let label = if prefix.is_empty() {
            folder.name.clone()
        } else {
            format!("{prefix} / {}", folder.name)
        };
        out.push(FolderView {
            id: folder.id,
            parent_id: folder.parent_id,
            name: folder.name.clone(),
            label: label.clone(),
            indent,
            position: folder.position,
        });
        walk_folders(children, Some(folder.id), indent + 1, &label, out);
    }
}

pub fn list_rsshub_instances(conn: &Connection) -> Result<Vec<RsshubInstance>> {
    let mut stmt = conn.prepare("SELECT id, name, base_url FROM rsshub_instances ORDER BY id")?;
    let rows = stmt.query_map([], |row| {
        Ok(RsshubInstance {
            id: row.get(0)?,
            name: row.get(1)?,
            base_url: row.get(2)?,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn match_rsshub_instance(conn: &Connection, url: &str) -> Result<Option<i64>> {
    for inst in list_rsshub_instances(conn)? {
        if url.starts_with(&inst.base_url) {
            return Ok(Some(inst.id));
        }
    }
    Ok(None)
}

fn ensure_folder(conn: &Connection, id: i64) -> Result<()> {
    let exists: i64 =
        conn.query_row("SELECT COUNT(*) FROM folders WHERE id = ?1", [id], |row| {
            row.get(0)
        })?;
    if exists == 0 {
        Err(Error::FolderNotFound)
    } else {
        Ok(())
    }
}

fn feed_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FeedRow> {
    let kind: String = row.get(3)?;
    Ok(FeedRow {
        id: row.get(0)?,
        url: row.get(1)?,
        title: row.get(2)?,
        kind: FeedKind::parse_str(&kind).map_err(|err| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, Box::new(err))
        })?,
        site_url: row.get(4)?,
        etag: row.get(5)?,
        last_modified: row.get(6)?,
        interval_secs: row.get(7)?,
        folder_id: row.get(8)?,
        error: row.get(9)?,
        rsshub_instance_id: row.get(10)?,
        last_fetched_at: row.get(11)?,
        position: row.get(12)?,
        next_fetch_at: row.get(13)?,
        slow_fail_count: row.get(14)?,
    })
}

fn is_unique(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

pub fn noisy_feed_ids(conn: &Connection) -> Result<HashSet<i64>> {
    noisy_feed_ids_filtered(conn, None)
}

/// 只统计当前页出现过的源，避免每次焦点刷新都扫近 14 天全文。
pub fn noisy_feed_ids_among(conn: &Connection, feed_ids: &[i64]) -> Result<HashSet<i64>> {
    if feed_ids.is_empty() {
        return Ok(HashSet::new());
    }
    noisy_feed_ids_filtered(conn, Some(feed_ids))
}

fn noisy_feed_ids_filtered(conn: &Connection, feed_ids: Option<&[i64]>) -> Result<HashSet<i64>> {
    let threshold: i64 = crate::db::get_setting(conn, HIDE_THRESHOLD_KEY)?
        .and_then(|s| s.parse().ok())
        .unwrap_or(-10);
    let since = now_ms() - NOISE_WINDOW_DAYS * MS_PER_DAY;
    let feed_sql = if let Some(ids) = feed_ids {
        let marks: String = (3..=ids.len() + 2)
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(",");
        format!(" AND feed_id IN ({marks})")
    } else {
        String::new()
    };
    let sql = format!(
        "SELECT feed_id,
                COUNT(*) AS n,
                SUM(CASE WHEN score <= ?1 THEN 1 ELSE 0 END) AS hidden,
                SUM(CASE WHEN score < 0 THEN 1 ELSE 0 END) AS disliked
         FROM articles
         WHERE expires_at IS NULL AND published_at >= ?2
           {feed_sql}
         GROUP BY feed_id"
    );
    let mut bind = vec![threshold, since];
    if let Some(ids) = feed_ids {
        bind.extend_from_slice(ids);
    }
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(bind), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;
    let mut out = HashSet::new();
    for row in rows {
        let (id, n, hidden, disliked) = row?;
        if n < NOISE_MIN_ARTICLES {
            continue;
        }
        let hide_rate = hidden as f64 / n as f64;
        let dislike_rate = disliked as f64 / n as f64;
        if hide_rate >= NOISE_HIDE_RATE || dislike_rate >= NOISE_DISLIKE_RATE {
            out.insert(id);
        }
    }
    Ok(out)
}

pub fn noisy_feed_titles(conn: &Connection) -> Result<Vec<String>> {
    let ids = noisy_feed_ids(conn)?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut titles = Vec::new();
    for feed in list_feeds(conn)? {
        if ids.contains(&feed.id) {
            titles.push(display_title(
                feed.title.as_deref().unwrap_or(""),
                &feed.url,
            ));
        }
    }
    Ok(titles)
}

pub(crate) fn looks_like_url(value: &str) -> bool {
    let t = value.trim();
    t.contains("://") || t.starts_with("www.")
}

fn host_is_local_or_ip(host: &str) -> bool {
    if host.is_empty() || host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

fn skip_feed_filename(seg: &str) -> bool {
    let lower = seg.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "index.rss"
            | "atom.xml"
            | "feed.xml"
            | "rss.xml"
            | "index.xml"
            | "feed.json"
            | "rss"
            | "atom"
            | "feed"
    ) || lower.ends_with(".xml")
        || lower.ends_with(".rss")
        || lower.ends_with(".json")
}

fn title_from_url(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let mut parts = rest.split('/');
    let host_raw = parts.next().unwrap_or("");
    let host_no_user = host_raw.split('@').next_back().unwrap_or(host_raw);
    let host = host_no_user.split(':').next().unwrap_or(host_no_user);
    let host = host.strip_prefix("www.").unwrap_or(host);
    let segs: Vec<&str> = parts
        .filter(|s| !s.is_empty())
        .filter(|s| !skip_feed_filename(s))
        .collect();
    if host_is_local_or_ip(host) {
        return segs
            .first()
            .map(|s| (*s).to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "未命名源".into());
    }
    if host.is_empty() {
        "未命名源".into()
    } else {
        host.to_string()
    }
}

/// 列表 / Toast 用的短名。没有标题时用主机名或 RSSHub 路径，不用整段 URL。
pub fn display_title(title: &str, url: &str) -> String {
    let t = title.trim();
    if !t.is_empty() && !looks_like_url(t) {
        return t.to_string();
    }
    let from = if url.trim().is_empty() { t } else { url };
    let out = title_from_url(from);
    if out.is_empty() {
        "未命名源".into()
    } else {
        out
    }
}

/// 刷新失败给用户看的短句。原始错误仍可写进 `feeds.error`。
pub fn humanize_refresh_error(raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("database is locked")
        || lower.contains("database lock")
        || lower.contains("sqlite_busy")
        || lower.contains("code: 5")
    {
        return "数据库正忙，请稍后重试".into();
    }
    let http = http_status_code(raw);
    if let Some(code) = http {
        return match code {
            429 => "源限流，请稍后重试".into(),
            401 | 403 => "源拒绝访问".into(),
            404 => "找不到这个源（404）".into(),
            502 | 503 | 504 => format!("源暂时不可用（{code}）"),
            _ => format!("源返回 {code}"),
        };
    }
    let compact: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() > 80 {
        compact.chars().take(77).collect::<String>() + "…"
    } else if compact.is_empty() {
        "刷新失败".into()
    } else {
        compact
    }
}

fn http_status_code(raw: &str) -> Option<u16> {
    if let Some(rest) = raw.strip_prefix("HTTP ") {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            return digits.parse().ok();
        }
    }
    raw.split("status=").nth(1).and_then(|rest| {
        rest.chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_at;
    use crate::parse::ParsedEntry;
    use crate::rules::{Dimension, NewRule, Polarity, insert_rule};
    use tempfile::tempdir;

    #[test]
    fn display_title_avoids_raw_urls() {
        assert_eq!(
            display_title("V2EX", "https://www.v2ex.com/index.xml"),
            "V2EX"
        );
        assert_eq!(
            display_title("", "https://www.solidot.org/index.rss"),
            "solidot.org"
        );
        assert_eq!(
            display_title("", "http://192.0.2.10:1200/theverge"),
            "theverge"
        );
        assert_eq!(
            display_title(
                "http://192.0.2.10:1200/v2ex/topics/hot",
                "http://192.0.2.10:1200/v2ex/topics/hot"
            ),
            "v2ex"
        );
        assert_eq!(display_title("", ""), "未命名源");
    }

    #[test]
    fn humanize_refresh_error_is_short() {
        assert_eq!(
            humanize_refresh_error("database is locked"),
            "数据库正忙，请稍后重试"
        );
        assert_eq!(humanize_refresh_error("HTTP 503"), "源暂时不可用（503）");
        assert_eq!(humanize_refresh_error("HTTP 429"), "源限流，请稍后重试");
    }

    #[test]
    fn flags_feed_when_many_dislike_hits() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
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
        let now = now_ms();
        let entries: Vec<ParsedEntry> = (0..8)
            .map(|i| {
                let url = format!("https://ads.example.com/{i}");
                let title = format!("ad promo {i}");
                ParsedEntry {
                    guid: url.clone(),
                    canonical_url: Some(url.clone()),
                    title,
                    author: None,
                    published_at: Some(now),
                    content_html: Some("<p>x</p>".into()),
                    hash: crate::parse::entry_hash(
                        &format!("ad promo {i}"),
                        Some(&url),
                        Some("<p>x</p>"),
                    ),
                    tags: vec![],
                }
            })
            .collect();
        crate::ingest::upsert_entries(&conn, feed, &entries, now).unwrap();
        let noisy = noisy_feed_ids(&conn).unwrap();
        assert!(noisy.contains(&feed));
        assert_eq!(noisy_feed_titles(&conn).unwrap().len(), 1);
    }

    #[test]
    fn low_volume_not_flagged() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
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
        let url = "https://ads.example.com/1";
        crate::ingest::upsert_entries(
            &conn,
            feed,
            &[ParsedEntry {
                guid: url.into(),
                canonical_url: Some(url.into()),
                title: "ad promo".into(),
                author: None,
                published_at: Some(now_ms()),
                content_html: Some("<p>x</p>".into()),
                hash: crate::parse::entry_hash("ad promo", Some(url), Some("<p>x</p>")),
                tags: vec![],
            }],
            1,
        )
        .unwrap();
        assert!(noisy_feed_ids(&conn).unwrap().is_empty());
    }

    fn sample_req(url: &str, folder_id: Option<i64>) -> SubscribeRequest {
        SubscribeRequest {
            url: url.into(),
            folder_id,
            as_rsshub: false,
            rsshub_instance_id: None,
        }
    }

    #[test]
    fn unsubscribe_cascades_articles() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let feed = subscribe(&conn, sample_req("https://news.example.com/rss.xml", None)).unwrap();
        let url = "https://news.example.com/1";
        crate::ingest::upsert_entries(
            &conn,
            feed,
            &[ParsedEntry {
                guid: url.into(),
                canonical_url: Some(url.into()),
                title: "hello".into(),
                author: None,
                published_at: Some(now_ms()),
                content_html: Some("<p>x</p>".into()),
                hash: crate::parse::entry_hash("hello", Some(url), Some("<p>x</p>")),
                tags: vec![],
            }],
            1,
        )
        .unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM articles", [], |row| row.get(0))
            .unwrap();
        assert_eq!(n, 1);
        unsubscribe(&conn, feed).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM articles", [], |row| row.get(0))
            .unwrap();
        assert_eq!(n, 0);
        assert!(matches!(get_feed(&conn, feed), Err(Error::FeedNotFound)));
    }

    #[test]
    fn unsubscribe_cleans_up_media_files() {
        let dir = tempdir().unwrap();
        let data = dir.path();
        let conn = open_at(&data.join("rrss.db")).unwrap();

        let feed_a = subscribe(&conn, sample_req("https://a.example.com/rss.xml", None)).unwrap();
        let feed_b = subscribe(&conn, sample_req("https://b.example.com/rss.xml", None)).unwrap();

        crate::ingest::upsert_entries(
            &conn,
            feed_a,
            &[crate::parse::ParsedEntry {
                guid: "https://a.example.com/1".into(),
                canonical_url: Some("https://a.example.com/1".into()),
                title: "art_a".into(),
                author: None,
                published_at: Some(now_ms()),
                content_html: Some("<p>a</p>".into()),
                hash: crate::parse::entry_hash(
                    "art_a",
                    Some("https://a.example.com/1"),
                    Some("<p>a</p>"),
                ),
                tags: vec![],
            }],
            1,
        )
        .unwrap();

        crate::ingest::upsert_entries(
            &conn,
            feed_b,
            &[crate::parse::ParsedEntry {
                guid: "https://b.example.com/1".into(),
                canonical_url: Some("https://b.example.com/1".into()),
                title: "art_b".into(),
                author: None,
                published_at: Some(now_ms()),
                content_html: Some("<p>b</p>".into()),
                hash: crate::parse::entry_hash(
                    "art_b",
                    Some("https://b.example.com/1"),
                    Some("<p>b</p>"),
                ),
                tags: vec![],
            }],
            1,
        )
        .unwrap();

        let art_a: i64 = conn
            .query_row(
                "SELECT id FROM articles WHERE feed_id = ?1",
                [feed_a],
                |r| r.get(0),
            )
            .unwrap();
        let art_b: i64 = conn
            .query_row(
                "SELECT id FROM articles WHERE feed_id = ?1",
                [feed_b],
                |r| r.get(0),
            )
            .unwrap();

        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        // feed A 独占图片
        let path_a_only = crate::media::store_image(
            &conn,
            data,
            art_a,
            "https://cdn.example.com/a_only.png",
            &png,
        )
        .unwrap();

        // 共享图片 (feed A 与 feed B 均引用)
        let path_shared = crate::media::store_image(
            &conn,
            data,
            art_a,
            "https://cdn.example.com/shared.png",
            &png,
        )
        .unwrap();
        let _ = crate::media::store_image(
            &conn,
            data,
            art_b,
            "https://cdn.example.com/shared.png",
            &png,
        )
        .unwrap();

        // feed B 独占图片
        let path_b_only = crate::media::store_image(
            &conn,
            data,
            art_b,
            "https://cdn.example.com/b_only.png",
            &png,
        )
        .unwrap();

        assert!(path_a_only.exists());
        assert!(path_shared.exists());
        assert!(path_b_only.exists());

        // 退订 feed A
        unsubscribe(&conn, feed_a).unwrap();

        // feed A 独占图片物理文件应被删除
        assert!(!path_a_only.exists(), "独占图片应被清理");
        // 共享图片物理文件依然存在（因为 feed B 仍然引用）
        assert!(path_shared.exists(), "共享图片应被保留");
        // feed B 独占图片物理文件依然存在
        assert!(path_b_only.exists(), "feed B 图片应被保留");

        // media 表中 feed A 的记录被清理，feed B 的记录保留
        let count_a: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM media WHERE article_id = ?1",
                [art_a],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count_a, 0);

        let count_b: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM media WHERE article_id = ?1",
                [art_b],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count_b, 2);
    }

    #[test]
    fn rename_and_update_url() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let a = subscribe(&conn, sample_req("https://news.example.com/rss.xml", None)).unwrap();
        let b = subscribe(&conn, sample_req("https://lab.example.org/atom.xml", None)).unwrap();
        conn.execute(
            "UPDATE feeds SET etag = 'W/1', last_modified = 'x', error = 'boom' WHERE id = ?1",
            [a],
        )
        .unwrap();
        rename_feed(&conn, a, "  Example  ").unwrap();
        assert_eq!(
            get_feed(&conn, a).unwrap().title.as_deref(),
            Some("Example")
        );
        rename_feed(&conn, a, "   ").unwrap();
        assert_eq!(get_feed(&conn, a).unwrap().title, None);
        update_feed_url(&conn, a, "https://news.example.com/feed.xml").unwrap();
        let row = get_feed(&conn, a).unwrap();
        assert_eq!(row.url, "https://news.example.com/feed.xml");
        assert_eq!(row.etag, None);
        assert_eq!(row.last_modified, None);
        assert_eq!(row.error, None);
        let err = update_feed_url(&conn, a, "https://lab.example.org/atom.xml").unwrap_err();
        assert!(matches!(err, Error::DuplicateFeed(_)));
        let _ = b;
    }

    #[test]
    fn folder_rename_delete_unsets_feed() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let parent = create_folder(&conn, "科技", None).unwrap();
        let child = create_folder(&conn, "Rust", Some(parent)).unwrap();
        rename_folder(&conn, child, "异步").unwrap();
        let views = list_folder_views(&conn).unwrap();
        assert_eq!(views.len(), 2);
        assert_eq!(views[1].label, "科技 / 异步");
        assert_eq!(views[1].indent, 1);
        let feed = subscribe(
            &conn,
            sample_req("https://lab.example.org/atom.xml", Some(child)),
        )
        .unwrap();
        delete_folder(&conn, parent).unwrap();
        assert!(list_folders(&conn).unwrap().is_empty());
        assert_eq!(get_feed(&conn, feed).unwrap().folder_id, None);
        assert!(matches!(
            rename_folder(&conn, parent, "x"),
            Err(Error::FolderNotFound)
        ));
        assert!(matches!(
            rename_folder(&conn, child, "  "),
            Err(Error::InvalidFolderName)
        ));
    }
}
