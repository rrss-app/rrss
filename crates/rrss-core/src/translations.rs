//! 阅读器按需译文。不覆盖原文 `blocks_json`。日志 target：`rrss_core::translations`。

use rusqlite::{Connection, OptionalExtension, params};

use crate::db::{get_setting, set_setting};
use crate::error::Result;
use crate::feeds::now_ms;

pub const TRANSLATE_ENABLED_KEY: &str = "translate_enabled";
pub const TRANSLATE_TARGET_LANG_KEY: &str = "translate_target_lang";
pub const TRANSLATE_BACKEND_KEY: &str = "translate_backend";
pub const TRANSLATE_DEEPL_URL_KEY: &str = "translate_deepl_url";
pub const TRANSLATE_DEEPL_KEY_KEY: &str = "translate_deepl_key";
pub const TRANSLATE_DEEPLX_URL_KEY: &str = "translate_deeplx_url";
pub const TRANSLATE_DEEPLX_TOKEN_KEY: &str = "translate_deeplx_token";
pub const TRANSLATE_HTTP_URL_KEY: &str = "translate_http_url";
pub const TRANSLATE_HTTP_KEY_KEY: &str = "translate_http_key";
const DEEPL_KEY_CONFIGURED: &str = "translate_deepl_key_configured";
pub const SECRET_KEYS: [&str; 3] = [
    TRANSLATE_DEEPL_KEY_KEY,
    TRANSLATE_DEEPLX_TOKEN_KEY,
    TRANSLATE_HTTP_KEY_KEY,
];
pub fn missing_key(key: &str) -> String {
    format!("{key}_missing")
}

pub const ENGINE_COMPLETE: &str = "complete";
pub const ENGINE_HTTP: &str = "http";
pub const ENGINE_DEEPLX: &str = "deeplx";
pub const ENGINE_DEEPL: &str = "deepl";

#[derive(Debug, Clone)]
pub struct ArticleTranslation {
    pub article_id: i64,
    pub target_lang: String,
    pub source_hash: String,
    pub engine: String,
    pub model: Option<String>,
    pub prompt_version: i64,
    pub title: String,
    pub blocks_json: String,
    pub created_at: i64,
}

pub fn is_enabled(conn: &Connection) -> Result<bool> {
    let raw = get_setting(conn, TRANSLATE_ENABLED_KEY)?.unwrap_or_else(|| "1".into());
    Ok(raw.trim() != "0" && raw.trim().to_ascii_lowercase() != "false")
}

pub fn set_enabled(conn: &Connection, enabled: bool) -> Result<()> {
    set_setting(conn, TRANSLATE_ENABLED_KEY, if enabled { "1" } else { "0" })
}

pub fn target_lang(conn: &Connection) -> Result<String> {
    let raw = get_setting(conn, TRANSLATE_TARGET_LANG_KEY)?.unwrap_or_else(|| "zh".into());
    Ok(normalize_lang(&raw))
}

pub fn set_target_lang(conn: &Connection, lang: &str) -> Result<()> {
    set_setting(conn, TRANSLATE_TARGET_LANG_KEY, &normalize_lang(lang))
}

pub fn backend_id(conn: &Connection) -> Result<String> {
    let raw = get_setting(conn, TRANSLATE_BACKEND_KEY)?.unwrap_or_else(|| ENGINE_COMPLETE.into());
    Ok(match raw.trim() {
        ENGINE_HTTP => ENGINE_HTTP.into(),
        ENGINE_DEEPLX => ENGINE_DEEPLX.into(),
        ENGINE_DEEPL => ENGINE_DEEPL.into(),
        _ => ENGINE_COMPLETE.into(),
    })
}

pub fn set_backend(conn: &Connection, backend: &str) -> Result<()> {
    let id = match backend.trim() {
        ENGINE_HTTP => ENGINE_HTTP,
        ENGINE_DEEPLX => ENGINE_DEEPLX,
        ENGINE_DEEPL => ENGINE_DEEPL,
        _ => ENGINE_COMPLETE,
    };
    set_setting(conn, TRANSLATE_BACKEND_KEY, id)
}

// 老版本允许 DeepL 回退到 HTTP 密钥；显式更换 / 清除或恢复缺失标记之后不再回退。
pub fn deepl_uses_legacy_key(conn: &Connection) -> Result<bool> {
    Ok(
        get_setting(conn, TRANSLATE_DEEPL_KEY_KEY)?.is_none_or(|s| s.trim().is_empty())
            && get_setting(conn, DEEPL_KEY_CONFIGURED)?.as_deref() != Some("1")
            && get_setting(conn, &missing_key(TRANSLATE_DEEPL_KEY_KEY))?.as_deref() != Some("1"),
    )
}

pub fn deepl_config(conn: &Connection) -> Result<(Option<String>, Option<String>)> {
    let url = get_setting(conn, TRANSLATE_DEEPL_URL_KEY)?.filter(|s| !s.trim().is_empty());
    let mut key = get_setting(conn, TRANSLATE_DEEPL_KEY_KEY)?.filter(|s| !s.trim().is_empty());
    if key.is_none() && deepl_uses_legacy_key(conn)? {
        // Fallback to legacy single http_key if present
        key = get_setting(conn, TRANSLATE_HTTP_KEY_KEY)?.filter(|s| !s.trim().is_empty());
    }
    Ok((url, key))
}

fn save_secret(conn: &Connection, key: &str, value: Option<&str>) -> Result<()> {
    if let Some(value) = value {
        set_setting(conn, key, value.trim())?;
        if key == TRANSLATE_DEEPL_KEY_KEY {
            set_setting(conn, DEEPL_KEY_CONFIGURED, "1")?;
        }
        conn.execute("DELETE FROM settings WHERE key = ?1", [missing_key(key)])?;
    }
    Ok(())
}

pub fn set_deepl_config(conn: &Connection, url: Option<&str>, api_key: Option<&str>) -> Result<()> {
    if let Some(url) = url {
        set_setting(conn, TRANSLATE_DEEPL_URL_KEY, url.trim())?;
    }
    save_secret(conn, TRANSLATE_DEEPL_KEY_KEY, api_key)?;
    Ok(())
}

pub fn deeplx_config(conn: &Connection) -> Result<(String, Option<String>)> {
    let mut url = get_setting(conn, TRANSLATE_DEEPLX_URL_KEY)?.unwrap_or_default();
    if url.trim().is_empty() {
        // Fallback to legacy http_url if it looked like deeplx
        let legacy = get_setting(conn, TRANSLATE_HTTP_URL_KEY)?.unwrap_or_default();
        if !legacy.trim().is_empty() && legacy.contains("1188") {
            url = legacy;
        } else {
            url = "http://127.0.0.1:1188".into();
        }
    }
    let token = get_setting(conn, TRANSLATE_DEEPLX_TOKEN_KEY)?.filter(|s| !s.trim().is_empty());
    Ok((url, token))
}

pub fn set_deeplx_config(conn: &Connection, url: Option<&str>, token: Option<&str>) -> Result<()> {
    if let Some(url) = url {
        set_setting(conn, TRANSLATE_DEEPLX_URL_KEY, url.trim())?;
    }
    save_secret(conn, TRANSLATE_DEEPLX_TOKEN_KEY, token)?;
    Ok(())
}

pub fn http_config(conn: &Connection) -> Result<(String, Option<String>)> {
    let mut url = get_setting(conn, TRANSLATE_HTTP_URL_KEY)?.unwrap_or_default();
    if url.trim().is_empty() {
        url = "http://127.0.0.1:5000".into();
    }
    let key = get_setting(conn, TRANSLATE_HTTP_KEY_KEY)?.filter(|s| !s.trim().is_empty());
    Ok((url, key))
}

pub fn set_http_config(conn: &Connection, url: Option<&str>, api_key: Option<&str>) -> Result<()> {
    if let Some(url) = url {
        set_setting(conn, TRANSLATE_HTTP_URL_KEY, url.trim())?;
    }
    save_secret(conn, TRANSLATE_HTTP_KEY_KEY, api_key)?;
    Ok(())
}

pub fn http_url(conn: &Connection) -> Result<String> {
    Ok(get_setting(conn, TRANSLATE_HTTP_URL_KEY)?.unwrap_or_default())
}

pub fn http_key(conn: &Connection) -> Result<Option<String>> {
    Ok(get_setting(conn, TRANSLATE_HTTP_KEY_KEY)?.filter(|s| !s.trim().is_empty()))
}

pub fn set_http_endpoint(conn: &Connection, url: &str, api_key: Option<&str>) -> Result<()> {
    set_setting(conn, TRANSLATE_HTTP_URL_KEY, url.trim())?;
    set_setting(conn, TRANSLATE_HTTP_KEY_KEY, api_key.unwrap_or("").trim())?;
    Ok(())
}

pub fn normalize_lang(lang: &str) -> String {
    let t = lang.trim().to_ascii_lowercase();
    if t.is_empty() {
        return "zh".into();
    }
    t.chars()
        .take(16)
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

pub fn get(
    conn: &Connection,
    article_id: i64,
    target_lang: &str,
    engine: &str,
) -> Result<Option<ArticleTranslation>> {
    conn.query_row(
        "SELECT article_id, target_lang, source_hash, engine, model, prompt_version, title, blocks_json, created_at
         FROM article_translations
         WHERE article_id = ?1 AND target_lang = ?2 AND engine = ?3",
        params![article_id, target_lang, engine],
        map_row,
    )
    .optional()
    .map_err(Into::into)
}

pub fn upsert(conn: &Connection, row: &ArticleTranslation) -> Result<()> {
    conn.execute(
        "INSERT INTO article_translations (
            article_id, target_lang, source_hash, engine, model, prompt_version, title, blocks_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (article_id, target_lang, engine) DO UPDATE SET
            source_hash = excluded.source_hash,
            model = excluded.model,
            prompt_version = excluded.prompt_version,
            title = excluded.title,
            blocks_json = excluded.blocks_json,
            created_at = excluded.created_at",
        params![
            row.article_id,
            row.target_lang,
            row.source_hash,
            row.engine,
            row.model,
            row.prompt_version,
            row.title,
            row.blocks_json,
            row.created_at,
        ],
    )?;
    Ok(())
}

pub fn now() -> i64 {
    now_ms()
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArticleTranslation> {
    Ok(ArticleTranslation {
        article_id: row.get(0)?,
        target_lang: row.get(1)?,
        source_hash: row.get(2)?,
        engine: row.get(3)?,
        model: row.get(4)?,
        prompt_version: row.get(5)?,
        title: row.get(6)?,
        blocks_json: row.get(7)?,
        created_at: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_at;
    use tempfile::tempdir;

    #[test]
    fn legacy_deepl_key_is_preserved_until_explicitly_replaced_or_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("legacy.db")).unwrap();
        set_setting(&conn, TRANSLATE_DEEPL_KEY_KEY, "").unwrap();
        set_setting(&conn, TRANSLATE_HTTP_KEY_KEY, "legacy-key").unwrap();
        assert_eq!(
            deepl_config(&conn).unwrap().1.as_deref(),
            Some("legacy-key")
        );
        set_deepl_config(&conn, Some("https://example.test"), None).unwrap();
        assert_eq!(
            deepl_config(&conn).unwrap().1.as_deref(),
            Some("legacy-key")
        );
        set_deepl_config(&conn, None, Some("")).unwrap();
        assert_eq!(deepl_config(&conn).unwrap().1, None);
        assert_eq!(http_config(&conn).unwrap().1.as_deref(), Some("legacy-key"));
    }

    #[test]
    fn upsert_and_hash_mismatch() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        conn.execute(
            "INSERT INTO folders (id, name, parent_id, created_at) VALUES (1, 'f', NULL, 1)",
            [],
        )
        .ok();
        conn.execute(
            "INSERT INTO feeds (id, url, title, kind, created_at) VALUES (1, 'https://ex.test/f', 'f', 'rss', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO articles (id, feed_id, title, hash, created_at) VALUES (1, 1, 'Hello', 'abc', 1)",
            [],
        )
        .unwrap();
        upsert(
            &conn,
            &ArticleTranslation {
                article_id: 1,
                target_lang: "zh".into(),
                source_hash: "abc".into(),
                engine: ENGINE_COMPLETE.into(),
                model: Some("qwen".into()),
                prompt_version: 1,
                title: "你好".into(),
                blocks_json: "{\"v\":1,\"blocks\":[]}".into(),
                created_at: 1,
            },
        )
        .unwrap();
        let hit = get(&conn, 1, "zh", ENGINE_COMPLETE).unwrap().unwrap();
        assert_eq!(hit.title, "你好");
        assert_eq!(hit.source_hash, "abc");
        assert_eq!(target_lang(&conn).unwrap(), "zh");
        set_backend(&conn, ENGINE_HTTP).unwrap();
        set_http_endpoint(&conn, "http://127.0.0.1:5000", Some("k")).unwrap();
        assert_eq!(backend_id(&conn).unwrap(), ENGINE_HTTP);
        assert_eq!(http_url(&conn).unwrap(), "http://127.0.0.1:5000");
        assert_eq!(http_key(&conn).unwrap().as_deref(), Some("k"));
        set_backend(&conn, ENGINE_DEEPLX).unwrap();
        assert_eq!(backend_id(&conn).unwrap(), ENGINE_DEEPLX);
        assert!(is_enabled(&conn).unwrap());
        set_enabled(&conn, false).unwrap();
        assert!(!is_enabled(&conn).unwrap());
        set_enabled(&conn, true).unwrap();
        assert!(is_enabled(&conn).unwrap());

        set_deepl_config(&conn, Some("https://api-free.deepl.com"), Some("key:fx")).unwrap();
        let (d_url, d_key) = deepl_config(&conn).unwrap();
        assert_eq!(d_url.as_deref(), Some("https://api-free.deepl.com"));
        assert_eq!(d_key.as_deref(), Some("key:fx"));

        set_deeplx_config(&conn, Some("http://127.0.0.1:1188"), Some("token123")).unwrap();
        let (dx_url, dx_tok) = deeplx_config(&conn).unwrap();
        assert_eq!(dx_url, "http://127.0.0.1:1188");
        assert_eq!(dx_tok.as_deref(), Some("token123"));

        set_http_config(&conn, Some("http://127.0.0.1:5000"), Some("key456")).unwrap();
        let (h_url, h_key) = http_config(&conn).unwrap();
        assert_eq!(h_url, "http://127.0.0.1:5000");
        assert_eq!(h_key.as_deref(), Some("key456"));
    }
}
