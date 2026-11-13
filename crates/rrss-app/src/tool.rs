//! `rrss tool …`：给 Pi 扩展用的 JSON CLI。不开窗。

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use rrss_core::ai_store::{self, FETCH_URL_ENABLED_KEY};
use rrss_core::articles::article_for_tool;
use rrss_core::ingest::{FetchRequest, fetch_http, http_client_with_proxy};
use rrss_core::memories;
use rrss_core::score::strip_html;
use rrss_core::{db, pi_store, rag, search};
use serde_json::{Value, json};

use crate::digest;
use crate::query_embedding::QueryEmbedding;

const QUERY_EMBED_TIMEOUT: Duration = Duration::from_secs(8);

pub fn is_tool_invocation(args: &[String]) -> bool {
    args.get(1).map(String::as_str) == Some("tool")
}

pub fn run(args: &[String]) -> i32 {
    match run_inner(args) {
        Ok(value) => {
            println!("{value}");
            0
        }
        Err(err) => {
            eprintln!("{err:#}");
            1
        }
    }
}

fn run_inner(args: &[String]) -> Result<String> {
    if args.get(2).map(String::as_str) == Some("get_assistant_config") {
        // This read-only command must not create/migrate a database or refresh defaults.
        let conn = rusqlite::Connection::open_with_flags(
            db_path(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        return run_with_connection(args, &conn, QUERY_EMBED_TIMEOUT);
    }
    let conn = db::open_at(&db_path())?;
    run_with_connection(args, &conn, QUERY_EMBED_TIMEOUT)
}

fn run_with_connection(
    args: &[String],
    conn: &rusqlite::Connection,
    embed_timeout: Duration,
) -> Result<String> {
    let cmd = args.get(2).map(String::as_str).unwrap_or("");
    let rest = &args[3.min(args.len())..];
    match cmd {
        "search_articles" => {
            let q = require_flag(rest, "--query")?;
            let limit = flag(rest, "--limit")
                .and_then(|s| s.parse().ok())
                .unwrap_or(10);
            let vector = query_vector(conn, &q, embed_timeout);
            let hits = match vector.as_deref() {
                Some(vector) => rag::hybrid_search(conn, &q, Some(vector), limit)?,
                None => search::search_articles_for_retrieval(conn, &q, limit)?,
            };
            let rows: Vec<Value> = hits
                .iter()
                .map(|h| {
                    json!({
                        "id": h.id,
                        "title": h.title,
                        "feed_title": h.feed_title,
                        "published_at": h.published_at,
                        "score": h.score,
                        "snippet": h.snippet,
                    })
                })
                .collect();
            Ok(serde_json::to_string_pretty(&rows)?)
        }
        "get_article" => {
            let id: i64 = require_flag(rest, "--id")?.parse().context("id")?;
            let view = article_for_tool(&conn, id)?;
            Ok(serde_json::to_string_pretty(&json!({
                "id": view.id,
                "title": view.title,
                "feed_title": view.feed_title,
                "url": view.url,
                "published_at": view.published_at,
                "score": view.score,
                "text": view.text,
            }))?)
        }
        // 扩展每轮写进系统提示「长期记忆」小节的内容（阶段 18 D10）。不在工具清单里，模型不能调用。
        "memory_preview" => Ok(memories::inject_preview(&conn)?.0),
        "get_assistant_config" => Ok(serde_json::to_string_pretty(&assistant_config(
            conn, rest,
        )?)?),
        "search_memory" => {
            let q = require_flag(rest, "--query")?;
            let vector = query_vector(conn, &q, embed_timeout);
            let rows = memories::search_memory_hybrid(conn, &q, vector.as_deref(), 20)?;
            let v: Vec<Value> = rows
                .iter()
                .map(|m| {
                    json!({
                        "id": m.id,
                        "kind": m.kind.as_str(),
                        "content": m.content,
                        "pinned": m.pinned,
                        "pending": m.pending(),
                    })
                })
                .collect();
            Ok(serde_json::to_string_pretty(&v)?)
        }
        "open_original" => {
            let url = require_flag(rest, "--url")?;
            webbrowser::open(&url).map_err(|err| anyhow::anyhow!(err))?;
            Ok(json!({"ok": true, "url": url}).to_string())
        }
        "fetch_url" => {
            let enabled = db::get_setting(&conn, FETCH_URL_ENABLED_KEY)?.as_deref() == Some("1");
            if !enabled {
                anyhow::bail!("fetch_url is disabled");
            }
            let url = require_flag(rest, "--url")?;
            let client = http_client_with_proxy(&rrss_core::network::load(conn)?)?;
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let resp = rt.block_on(fetch_http(
                &client,
                FetchRequest {
                    url: url.clone(),
                    etag: None,
                    last_modified: None,
                },
            ))?;
            let raw = String::from_utf8_lossy(&resp.body);
            let text = ai_store::truncate_chars(&strip_html(&raw), 4000);
            Ok(json!({"url": url, "status": resp.status, "text": text}).to_string())
        }
        "list_focus" => {
            let limit = flag(rest, "--limit")
                .and_then(|s| s.parse().ok())
                .unwrap_or(20);
            let rows = rrss_core::list_focus(&conn, limit)?;
            let v: Vec<Value> = rows
                .iter()
                .map(|c| {
                    json!({
                        "id": c.id,
                        "title": c.title,
                        "feed_title": c.feed_title,
                        "score": c.score,
                        "summary": c.summary,
                        "interest": c.interest_label,
                    })
                })
                .collect();
            Ok(serde_json::to_string_pretty(&v)?)
        }
        "explain_score" => {
            let id: i64 = require_flag(rest, "--article-id")
                .or_else(|_| require_flag(rest, "--id"))?
                .parse()
                .context("article_id")?;
            let article = rrss_core::get_article(&conn, id)?;
            let reasons = rrss_core::parse_reasons(article.reasons_json.as_deref());
            let interests =
                rrss_core::parse_interest_reasons(article.interest_reasons_json.as_deref());
            Ok(serde_json::to_string_pretty(&json!({
                "id": article.id,
                "title": article.title,
                "score": article.score,
                "interest_boost": article.interest_boost,
                "reasons": reasons,
                "interest_reasons": interests,
            }))?)
        }
        "list_feeds" => {
            let feeds = rrss_core::feeds::list_feeds(&conn)?;
            let noisy = rrss_core::feeds::noisy_feed_titles(&conn).unwrap_or_default();
            let v: Vec<Value> = feeds
                .iter()
                .map(|f| {
                    let title = f.title.clone().unwrap_or_else(|| f.url.clone());
                    json!({
                        "id": f.id,
                        "title": title,
                        "url": f.url,
                        "error": f.error,
                        "noisy": noisy.iter().any(|n| n == &title),
                    })
                })
                .collect();
            Ok(serde_json::to_string_pretty(&v)?)
        }
        "get_digest" => Ok(serde_json::to_string_pretty(&digest_tool(&conn, rest)?)?),
        "add_interest" => {
            let label = require_flag(rest, "--label")?;
            let id = rrss_core::insert_interest(&conn, &label, "from_chat")?;
            Ok(json!({"ok": true, "id": id, "label": label}).to_string())
        }
        "mark_read" => {
            let id: i64 = require_flag(rest, "--article-id")
                .or_else(|_| require_flag(rest, "--id"))?
                .parse()
                .context("article_id")?;
            let read = flag(rest, "--read")
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(1)
                != 0;
            rrss_core::set_read(&conn, id, read)?;
            Ok(json!({"ok": true, "id": id, "read": read}).to_string())
        }
        "star_article" => {
            let id: i64 = require_flag(rest, "--article-id")
                .or_else(|_| require_flag(rest, "--id"))?
                .parse()
                .context("article_id")?;
            let starred = flag(rest, "--starred")
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(1)
                != 0;
            rrss_core::set_starred(&conn, id, starred)?;
            Ok(json!({"ok": true, "id": id, "starred": starred}).to_string())
        }
        "" | "help" => anyhow::bail!(
            "usage: rrss tool <search_articles|get_article|search_memory|get_assistant_config|open_original|fetch_url|list_focus|explain_score|list_feeds|get_digest|add_interest|mark_read|star_article>"
        ),
        other => anyhow::bail!("unknown tool {other}"),
    }
}

fn assistant_config(conn: &rusqlite::Connection, args: &[String]) -> Result<Value> {
    let manifest = pi_store::tool_manifest(conn).context("assistant_config_manifest")?;
    let fetch_url_enabled = db::get_setting(conn, FETCH_URL_ENABLED_KEY)?.as_deref() == Some("1")
        && manifest.tools.iter().any(|t| t.name == "fetch_url");
    // Explicit field allowlist: never serialize a profile, provider, or MCP configuration.
    let tools: Vec<_> = manifest
        .tools
        .iter()
        .filter(|t| pi_store::DEFAULT_TOOLS.iter().any(|def| def.name == t.name))
        .filter(|t| t.name != "fetch_url" || fetch_url_enabled)
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "confirm_required": t.confirm,
            })
        })
        .collect();
    let mut stmt = conn.prepare(
        "SELECT name, include_tools_json FROM mcp_servers
         WHERE enabled = 1 AND transport != 'sse' ORDER BY name",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    let mut mcp_servers = Vec::new();
    for row in rows {
        let (name, raw) = row?;
        let mut allowed_tools: Vec<String> = raw
            .map(|raw| serde_json::from_str(&raw))
            .transpose()?
            .unwrap_or_default();
        // An absent/empty allowlist exposes every tool in Pi's MCP configuration.
        if allowed_tools.is_empty() {
            allowed_tools.push("*".into());
        }
        mcp_servers.push(json!({"name": name, "allowed_tools": allowed_tools}));
    }
    let mut result = json!({
        "assistant_note": pi_store::assistant_note(conn)?,
        "tools": tools,
        "skills": pi_store::enabled_skill_names(conn)?,
        "fetch_url_enabled": fetch_url_enabled,
        "mcp_servers": mcp_servers,
    });
    // Supplied by the extension's current context, never guessed from the chat default.
    if let (Some(alias), Some(thinking)) = (flag(args, "--model-alias"), flag(args, "--thinking")) {
        result["model"] = json!({"alias": alias, "thinking": thinking});
    }
    Ok(result)
}

fn query_vector(conn: &rusqlite::Connection, query: &str, timeout: Duration) -> Option<Vec<f32>> {
    let result = (|| -> Result<Option<Vec<f32>>> {
        let deadline = tokio::time::Instant::now() + timeout;
        let Some(embedding) = QueryEmbedding::load(conn)? else {
            return Ok(None);
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let vector = rt.block_on(async {
            let client = rrss_core::network::load(conn)?.build_client(
                rrss_core::network::NetworkScope::Api,
                reqwest::Client::builder(),
            )?;
            embedding.embed_until(&client, query, deadline).await
        });
        // 这是一次性工具进程；超时后不再等待不可取消的 DNS 阻塞任务。
        rt.shutdown_background();
        Ok(embedding.fit(conn, vector?)?)
    })();
    match result {
        Ok(vector) => vector,
        Err(err) => {
            tracing::warn!(error = %crate::log::error(err.as_ref()), "tool query embed failed; text only");
            None
        }
    }
}

fn digest_tool(conn: &rusqlite::Connection, args: &[String]) -> Result<Value> {
    let optional = |name: &str| -> Result<Option<String>> {
        if args
            .iter()
            .any(|a| a == name || a.starts_with(&format!("{name}=")))
        {
            let value = require_flag(args, name)?;
            if value.is_empty() || value.starts_with("--") {
                anyhow::bail!("missing value for {name}");
            }
            Ok(Some(value))
        } else {
            Ok(None)
        }
    };
    let id = optional("--id")?
        .map(|s| s.parse::<i64>().context("id"))
        .transpose()?;
    let period = optional("--period")?.unwrap_or_else(|| "daily".into());
    match digest::get_digest(conn, id, &period)? {
        Some(context) => Ok(serde_json::to_value(context)?),
        None => Ok(json!({"period": period, "body": null})),
    }
}

fn db_path() -> PathBuf {
    std::env::var("RRSS_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| rrss_core::db_path().unwrap_or_else(|_| PathBuf::from("rrss.db")))
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(v) = a.strip_prefix(&format!("{name}=")) {
            return Some(v.to_string());
        }
        if a == name {
            return it.next().cloned();
        }
    }
    None
}

fn require_flag(args: &[String], name: &str) -> Result<String> {
    flag(args, name).with_context(|| format!("missing {name}"))
}

#[cfg(test)]
mod network_tests;

#[cfg(test)]
mod search_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assistant_config_is_read_only_and_exposes_only_allowed_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rrss.db");
        let conn = db::open_at(&path).unwrap();
        db::set_setting(&conn, "pi_auto_download", "0").unwrap();
        pi_store::set_assistant_note(&conn, "Prefer concise replies.").unwrap();
        pi_store::set_enabled_skill_names(&conn, &["cite-verify".into()]).unwrap();
        pi_store::set_tool_enabled(&conn, "open_original", false).unwrap();
        pi_store::set_tool_description(&conn, "search_memory", "Search confirmed facts.").unwrap();
        db::set_setting(&conn, FETCH_URL_ENABLED_KEY, "1").unwrap();
        ai_store::insert_provider(
            &conn,
            &ai_store::NewProvider {
                name: "Fixture provider".into(),
                base_url: "https://model.example/v1".into(),
                api_key: Some("fake-provider-key".into()),
                protocol: "chat".into(),
            },
        )
        .unwrap();
        db::set_setting(
            &conn,
            rrss_core::network::SETTING_KEY,
            r#"{"global":{"mode":"custom","url":"http://fixture:fake-password@proxy.example:8080"}}"#,
        )
        .unwrap();
        pi_store::set_pi_override(
            &conn,
            pi_store::PI_MODELS_OVERRIDE_KEY,
            r#"{"providers":{"fixture":{"apiKey":"fake-override-key","baseUrl":"https://override.example"}}}"#,
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO mcp_servers(name, command, args_json, url, env_json, headers_json,
                                     include_tools_json, enabled, created_at)
             VALUES ('Fixture tools', 'fake-mcp-command', '[\"fake-mcp-argument\"]',
                     'https://mcp.example', '{\"FIXTURE_TOKEN\":\"fake-env-token\"}',
                     '{\"Authorization\":\"Bearer fake-header-token\"}', '[\"lookup\",\"inspect\"]', 1, 0);
             INSERT INTO mcp_servers(name, transport, include_tools_json, enabled, created_at) VALUES
                 ('All tools', 'http', NULL, 1, 0), ('Empty allowlist', 'stdio', '[]', 1, 0),
                 ('Disabled service', 'stdio', NULL, 0, 0), ('Legacy service', 'sse', NULL, 1, 0);
             INSERT INTO feeds(id, url, title, created_at) VALUES (1, 'https://feed.example', 'Fixture feed', 0);
             INSERT INTO articles(feed_id, title, content_md, created_at) VALUES (1, 'Fixture article', 'private-article-content', 0);
             INSERT INTO memories(kind, content, created_at) VALUES ('semantic', 'private-memory-content', 0);
             UPDATE prompt_templates SET body = 'private-system-template';",
        )
        .unwrap();
        let reader = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let mut args = ["rrss", "tool", "get_assistant_config"]
            .map(str::to_owned)
            .to_vec();
        args.extend(["--model-alias", "Fixture chat", "--thinking", "high"].map(str::to_owned));
        let raw = run_with_connection(&args, &reader, QUERY_EMBED_TIMEOUT).unwrap();
        let result: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(result["assistant_note"], "Prefer concise replies.");
        assert_eq!(result["skills"], json!(["cite-verify"]));
        assert_eq!(result["fetch_url_enabled"], true);
        assert_eq!(
            result["model"],
            json!({"alias": "Fixture chat", "thinking": "high"})
        );
        assert_eq!(
            result["mcp_servers"],
            json!([
                {"name": "All tools", "allowed_tools": ["*"]},
                {"name": "Empty allowlist", "allowed_tools": ["*"]},
                {"name": "Fixture tools", "allowed_tools": ["lookup", "inspect"]},
            ])
        );
        let tools = result["tools"].as_array().unwrap();
        assert!(
            tools
                .iter()
                .any(|t| t["name"] == "get_assistant_config" && t["confirm_required"] == false)
        );
        assert!(
            tools
                .iter()
                .any(|t| t["name"] == "fetch_url" && t["confirm_required"] == true)
        );
        assert!(tools.iter().any(|t| t == &json!({
            "name": "search_memory", "description": "Search confirmed facts.", "confirm_required": false,
        })));
        assert!(!tools.iter().any(|t| t["name"] == "open_original"));
        for forbidden in [
            "fake-provider-key",
            "model.example",
            "fake-password",
            "proxy.example",
            "fake-override-key",
            "override.example",
            "fake-mcp-command",
            "fake-mcp-argument",
            "mcp.example",
            "FIXTURE_TOKEN",
            "fake-env-token",
            "Authorization",
            "fake-header-token",
            "private-article-content",
            "private-memory-content",
            "private-system-template",
        ] {
            assert!(!raw.contains(forbidden), "leaked fixture value");
        }
        let template = serde_json::to_string(crate::pi::workspace::SYSTEM_TEMPLATE.trim()).unwrap();
        assert!(!raw.contains(template.trim_matches('"')));
        // Both gates must be open for fetch_url; no current context means no model field.
        args.truncate(3);
        for (setting, enabled) in [("0", true), ("1", false)] {
            db::set_setting(&conn, FETCH_URL_ENABLED_KEY, setting).unwrap();
            pi_store::set_tool_enabled(&conn, "fetch_url", enabled).unwrap();
            let raw = run_with_connection(&args, &reader, QUERY_EMBED_TIMEOUT).unwrap();
            let result: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(result["fetch_url_enabled"], false);
            assert!(
                !result["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["name"] == "fetch_url")
            );
            assert!(result.get("model").is_none());
        }
    }

    #[test]
    fn digest_cli_explicit_id_never_falls_back_to_latest() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("rrss.db")).unwrap();
        let old = ai_store::insert_digest(&conn, "daily", 1, "- historical", "[]").unwrap();
        let latest = ai_store::insert_digest(&conn, "daily", 2, "- latest", "[]").unwrap();
        assert_eq!(digest_tool(&conn, &[]).unwrap()["id"], latest);
        assert_eq!(
            digest_tool(&conn, &[format!("--id={old}")]).unwrap()["id"],
            old
        );
        for args in [
            vec!["--id"],
            vec!["--id", "--period", "daily"],
            vec!["--id=0"],
            vec!["--id=999"],
            vec!["--period"],
            vec!["--period=monthly"],
        ] {
            let args: Vec<_> = args.into_iter().map(str::to_string).collect();
            assert!(digest_tool(&conn, &args).is_err(), "{args:?}");
        }
    }
}
