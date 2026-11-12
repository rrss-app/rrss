//! `rrss tool …`：给 Pi 扩展用的 JSON CLI。不开窗。

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use rrss_core::ai_store::{self, FETCH_URL_ENABLED_KEY};
use rrss_core::articles::article_for_tool;
use rrss_core::ingest::{FetchRequest, fetch_http, http_client_with_proxy};
use rrss_core::memories;
use rrss_core::score::strip_html;
use rrss_core::{db, rag, search};
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
            "usage: rrss tool <search_articles|get_article|search_memory|open_original|fetch_url|list_focus|explain_score|list_feeds|get_digest|add_interest|mark_read|star_article>"
        ),
        other => anyhow::bail!("unknown tool {other}"),
    }
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
