//! 规则匹配与 `reasons[]`。日志 target：`rrss_core::score`。

use regex::RegexBuilder;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::rules::{Dimension, Rule, list_enabled_rules};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reason {
    pub rule_id: i64,
    pub dimension: Dimension,
    pub snippet: String,
    pub weight: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScoreResult {
    pub score: i64,
    pub reasons: Vec<Reason>,
}

#[derive(Debug, Clone)]
pub struct MatchInput<'a> {
    pub feed_id: i64,
    pub folder_ids: &'a [i64],
    pub author: Option<&'a str>,
    pub tags: &'a [String],
    pub title: &'a str,
    pub url: Option<&'a str>,
    pub body: &'a str,
}

pub struct CompiledRule {
    pub rule: Rule,
    signed_weight: i64,
    regex: Option<regex::Regex>,
}

pub fn compile_rules(rules: Vec<Rule>) -> Vec<CompiledRule> {
    let mut out = Vec::with_capacity(rules.len());
    for rule in rules {
        let regex = if rule.dimension.is_regex() {
            match RegexBuilder::new(&rule.pattern)
                .case_insensitive(true)
                .build()
            {
                Ok(re) => Some(re),
                Err(_err) => {
                    tracing::warn!(
                        rule_id = rule.id,
                        error_kind = "invalid_regex",
                        "skipping invalid regex rule"
                    );
                    continue;
                }
            }
        } else {
            None
        };
        let signed_weight = rule.polarity.signed_weight(rule.weight);
        out.push(CompiledRule {
            rule,
            signed_weight,
            regex,
        });
    }
    out
}

pub fn load_compiled_rules(conn: &Connection) -> Result<Vec<CompiledRule>> {
    let mut rules = list_enabled_rules(conn)?;
    rules.sort_by_key(|r| r.id);
    Ok(compile_rules(rules))
}

pub fn match_rules(compiled: &[CompiledRule], input: &MatchInput<'_>) -> ScoreResult {
    let mut reasons = Vec::new();
    for item in compiled {
        if !scope_applies(&item.rule, input.feed_id, input.folder_ids) {
            continue;
        }
        if let Some(snippet) = hit(
            &item.rule.dimension,
            &item.rule.pattern,
            item.regex.as_ref(),
            input,
        ) {
            reasons.push(Reason {
                rule_id: item.rule.id,
                dimension: item.rule.dimension,
                snippet,
                weight: item.signed_weight,
            });
        }
    }
    let score = reasons.iter().map(|r| r.weight).sum();
    ScoreResult { score, reasons }
}

pub fn write_score(conn: &Connection, article_id: i64, result: &ScoreResult) -> Result<()> {
    let json = serde_json::to_string(&result.reasons).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "UPDATE articles SET score = ?1, reasons_json = ?2 WHERE id = ?3",
        params![result.score, json, article_id],
    )?;
    Ok(())
}

pub fn parse_reasons(json: Option<&str>) -> Vec<Reason> {
    let Some(json) = json.filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    serde_json::from_str(json).unwrap_or_default()
}

pub fn folder_chain(conn: &Connection, mut folder_id: Option<i64>) -> Result<Vec<i64>> {
    let mut ids = Vec::new();
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = folder_id {
        if !seen.insert(id) {
            break;
        }
        ids.push(id);
        folder_id = conn
            .query_row("SELECT parent_id FROM folders WHERE id = ?1", [id], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .optional()?
            .flatten();
    }
    Ok(ids)
}

pub fn tags_for_article(conn: &Connection, article_id: i64) -> Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT tag FROM article_tags WHERE article_id = ?1 ORDER BY tag")?;
    let rows = stmt.query_map([article_id], |row| row.get(0))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn rescore_all(conn: &Connection) -> Result<u32> {
    let compiled = load_compiled_rules(conn)?;
    let articles: Vec<ArticleScoreRow> = {
        let mut stmt = conn.prepare(
            "SELECT a.id, a.feed_id, f.folder_id, a.author, a.title, a.canonical_url, a.content_html
             FROM articles a
             JOIN feeds f ON f.id = a.feed_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ArticleScoreRow {
                id: row.get(0)?,
                feed_id: row.get(1)?,
                folder_id: row.get(2)?,
                author: row.get(3)?,
                title: row.get(4)?,
                url: row.get(5)?,
                html: row.get(6)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut tag_map: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
    {
        let mut tag_stmt = conn.prepare("SELECT article_id, tag FROM article_tags")?;
        let tag_rows = tag_stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in tag_rows {
            let (id, tag) = row?;
            tag_map.entry(id).or_default().push(tag);
        }
    }
    let mut folder_cache: std::collections::HashMap<Option<i64>, Vec<i64>> =
        std::collections::HashMap::new();
    let mut write_all = |c: &Connection| -> Result<u32> {
        let mut n = 0u32;
        for article in &articles {
            let folder_ids = if let Some(cached) = folder_cache.get(&article.folder_id) {
                cached.clone()
            } else {
                let chain = folder_chain(c, article.folder_id)?;
                folder_cache.insert(article.folder_id, chain.clone());
                chain
            };
            let empty = Vec::new();
            let tags = tag_map.get(&article.id).unwrap_or(&empty);
            let body = strip_html(article.html.as_deref().unwrap_or(""));
            let result = match_rules(
                &compiled,
                &MatchInput {
                    feed_id: article.feed_id,
                    folder_ids: &folder_ids,
                    author: article.author.as_deref(),
                    tags,
                    title: &article.title,
                    url: article.url.as_deref(),
                    body: &body,
                },
            );
            write_score(c, article.id, &result)?;
            n += 1;
        }
        Ok(n)
    };
    // 已在调用方事务内时不能再 BEGIN（这里不是 SAVEPOINT）。独立调用走写事务。
    let n = if conn.is_autocommit() {
        let tx = crate::db::write_transaction(conn)?;
        let n = write_all(&tx)?;
        tx.commit()?;
        n
    } else {
        write_all(conn)?
    };
    tracing::info!(articles = n, rules = compiled.len(), "rescored");
    Ok(n)
}

pub fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn scope_applies(rule: &Rule, feed_id: i64, folder_ids: &[i64]) -> bool {
    match rule.scope_kind {
        crate::rules::ScopeKind::Global => true,
        crate::rules::ScopeKind::Feed => rule.scope_id == Some(feed_id),
        crate::rules::ScopeKind::Folder => rule
            .scope_id
            .map(|id| folder_ids.contains(&id))
            .unwrap_or(false),
    }
}

fn hit(
    dimension: &Dimension,
    pattern: &str,
    regex: Option<&regex::Regex>,
    input: &MatchInput<'_>,
) -> Option<String> {
    match dimension {
        Dimension::Author => {
            let author = input.author?;
            if eq_ignore_case(author, pattern) {
                Some(author.to_string())
            } else {
                None
            }
        }
        Dimension::Tag => input
            .tags
            .iter()
            .find(|t| eq_ignore_case(t, pattern))
            .cloned(),
        Dimension::TitleKw => {
            contains_ignore_case(input.title, pattern).then(|| pattern.to_string())
        }
        Dimension::UrlPhrase => input
            .url
            .and_then(|u| contains_ignore_case(u, pattern).then(|| pattern.to_string())),
        Dimension::BodyPhrase => {
            contains_ignore_case(input.body, pattern).then(|| pattern.to_string())
        }
        Dimension::TitleRe => regex.and_then(|re| first_match(re, input.title)),
        Dimension::UrlRe => input
            .url
            .and_then(|u| regex.and_then(|re| first_match(re, u))),
        Dimension::BodyRe => regex.and_then(|re| first_match(re, input.body)),
    }
}

fn first_match(re: &regex::Regex, hay: &str) -> Option<String> {
    re.find(hay).map(|m| m.as_str().to_string())
}

fn eq_ignore_case(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}

fn contains_ignore_case(hay: &str, needle: &str) -> bool {
    !needle.is_empty() && hay.to_lowercase().contains(&needle.to_lowercase())
}

struct ArticleScoreRow {
    id: i64,
    feed_id: i64,
    folder_id: Option<i64>,
    author: Option<String>,
    title: String,
    url: Option<String>,
    html: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{PooledConnection, open_at};
    use crate::feeds::{SubscribeRequest, create_folder, subscribe};
    use crate::parse::ParsedEntry;
    use crate::rules::{
        Dimension, NewRule, Polarity, RuleSource, ScopeKind, insert_rule, set_rule_enabled,
    };
    use tempfile::tempdir;

    fn open_temp() -> (tempfile::TempDir, PooledConnection) {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        (dir, conn)
    }

    fn subscribe_url(conn: &Connection, url: &str, folder_id: Option<i64>) -> i64 {
        subscribe(
            conn,
            SubscribeRequest {
                url: url.into(),
                folder_id,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap()
    }

    fn entry(
        title: &str,
        author: Option<&str>,
        url: &str,
        html: &str,
        tags: &[&str],
    ) -> ParsedEntry {
        ParsedEntry {
            guid: format!("guid:{title}"),
            canonical_url: Some(url.into()),
            title: title.into(),
            author: author.map(str::to_string),
            published_at: Some(1_700_000_000_000),
            content_html: Some(html.into()),
            hash: crate::parse::entry_hash(title, Some(url), Some(html)),
            tags: tags.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn reasons_snapshot_author_title_url_body_tag() {
        let (_dir, conn) = open_temp();
        let feed = subscribe_url(&conn, "https://news.example.com/rss.xml", None);
        insert_rule(
            &conn,
            NewRule::manual_global(Polarity::Dislike, Dimension::Author, "Ada Example"),
        )
        .unwrap();
        insert_rule(
            &conn,
            NewRule::manual_global(Polarity::Like, Dimension::TitleKw, "library"),
        )
        .unwrap();
        insert_rule(
            &conn,
            NewRule::manual_global(Polarity::Dislike, Dimension::UrlPhrase, "/sponsored/"),
        )
        .unwrap();
        insert_rule(
            &conn,
            NewRule::manual_global(Polarity::Like, Dimension::BodyPhrase, "downtown"),
        )
        .unwrap();
        insert_rule(
            &conn,
            NewRule::manual_global(Polarity::Like, Dimension::Tag, "City"),
        )
        .unwrap();
        crate::ingest::upsert_entries(
            &conn,
            feed,
            &[entry(
                "City opens new library",
                Some("Ada Example"),
                "https://news.example.com/sponsored/library",
                "<p>The new library opened <b>downtown</b>.</p>",
                &["City", "Library"],
            )],
            1,
        )
        .unwrap();
        let article = crate::articles::get_article(&conn, 1).unwrap();
        let reasons = parse_reasons(article.reasons_json.as_deref());
        let json = serde_json::to_value(&reasons).unwrap();
        assert_eq!(
            json,
            serde_json::json!([
                {"rule_id": 1, "dimension": "author", "snippet": "Ada Example", "weight": -10},
                {"rule_id": 2, "dimension": "title_kw", "snippet": "library", "weight": 10},
                {"rule_id": 3, "dimension": "url_phrase", "snippet": "/sponsored/", "weight": -10},
                {"rule_id": 4, "dimension": "body_phrase", "snippet": "downtown", "weight": 10},
                {"rule_id": 5, "dimension": "tag", "snippet": "City", "weight": 10},
            ])
        );
        assert_eq!(article.score, 10);
        assert!(!article.reasons_json.as_ref().unwrap().contains("<b>"));
    }

    #[test]
    fn regex_and_invalid_regex_skipped() {
        let (_dir, conn) = open_temp();
        let feed = subscribe_url(&conn, "https://lab.example.org/atom.xml", None);
        insert_rule(
            &conn,
            NewRule::manual_global(Polarity::Like, Dimension::TitleRe, r"rust\s+async"),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO rules (polarity, dimension, pattern, scope_kind, weight, source, enabled, created_at)
             VALUES ('dislike', 'url_re', '(', 'global', 10, 'manual', 1, 1)",
            [],
        )
        .unwrap();
        insert_rule(
            &conn,
            NewRule::manual_global(Polarity::Like, Dimension::BodyRe, r"tokio"),
        )
        .unwrap();
        crate::ingest::upsert_entries(
            &conn,
            feed,
            &[entry(
                "Notes on Rust async",
                None,
                "https://lab.example.org/posts/1",
                "<p>Using tokio runtime.</p>",
                &[],
            )],
            1,
        )
        .unwrap();
        let article = crate::articles::get_article(&conn, 1).unwrap();
        assert_eq!(article.score, 20);
        let reasons = parse_reasons(article.reasons_json.as_deref());
        assert_eq!(reasons.len(), 2);
        assert!(reasons.iter().all(|r| r.weight == 10));
    }

    #[test]
    fn scopes_stack_not_override() {
        let (_dir, conn) = open_temp();
        let parent = create_folder(&conn, "News", None).unwrap();
        let child = create_folder(&conn, "Local", Some(parent)).unwrap();
        let feed = subscribe_url(&conn, "https://news.example.com/local.xml", Some(child));
        insert_rule(
            &conn,
            NewRule {
                polarity: Polarity::Like,
                dimension: Dimension::TitleKw,
                pattern: "budget".into(),
                scope_kind: ScopeKind::Global,
                scope_id: None,
                weight: 10,
                source: RuleSource::Manual,
                enabled: true,
                pack_id: None,
            },
        )
        .unwrap();
        insert_rule(
            &conn,
            NewRule {
                polarity: Polarity::Dislike,
                dimension: Dimension::Author,
                pattern: "Noise Bot".into(),
                scope_kind: ScopeKind::Folder,
                scope_id: Some(parent),
                weight: 10,
                source: RuleSource::Manual,
                enabled: true,
                pack_id: None,
            },
        )
        .unwrap();
        insert_rule(
            &conn,
            NewRule {
                polarity: Polarity::Like,
                dimension: Dimension::Tag,
                pattern: "civic".into(),
                scope_kind: ScopeKind::Feed,
                scope_id: Some(feed),
                weight: 5,
                source: RuleSource::Manual,
                enabled: true,
                pack_id: None,
            },
        )
        .unwrap();
        crate::ingest::upsert_entries(
            &conn,
            feed,
            &[entry(
                "City budget hearing",
                Some("Noise Bot"),
                "https://news.example.com/budget",
                "<p>ok</p>",
                &["civic"],
            )],
            1,
        )
        .unwrap();
        let article = crate::articles::get_article(&conn, 1).unwrap();
        assert_eq!(article.score, 5);
        assert_eq!(parse_reasons(article.reasons_json.as_deref()).len(), 3);
    }

    #[test]
    fn disabled_rule_does_not_score() {
        let (_dir, conn) = open_temp();
        let feed = subscribe_url(&conn, "https://news.example.com/rss.xml", None);
        let id = insert_rule(
            &conn,
            NewRule::manual_global(Polarity::Dislike, Dimension::Author, "Ada Example"),
        )
        .unwrap();
        set_rule_enabled(&conn, id, false).unwrap();
        crate::ingest::upsert_entries(
            &conn,
            feed,
            &[entry(
                "Hello",
                Some("Ada Example"),
                "https://news.example.com/a",
                "<p>hi</p>",
                &[],
            )],
            1,
        )
        .unwrap();
        let article = crate::articles::get_article(&conn, 1).unwrap();
        assert_eq!(article.score, 0);
        assert!(parse_reasons(article.reasons_json.as_deref()).is_empty());
    }

    #[test]
    fn rescore_all_updates_old_rows_skip_does_not_clobber() {
        let (_dir, conn) = open_temp();
        let feed = subscribe_url(&conn, "https://news.example.com/rss.xml", None);
        let entries = [entry(
            "Hello",
            Some("Ada Example"),
            "https://news.example.com/a",
            "<p>hi</p>",
            &[],
        )];
        crate::ingest::upsert_entries(&conn, feed, &entries, 1).unwrap();
        assert_eq!(crate::articles::get_article(&conn, 1).unwrap().score, 0);
        insert_rule(
            &conn,
            NewRule::manual_global(Polarity::Dislike, Dimension::Author, "Ada Example"),
        )
        .unwrap();
        assert_eq!(rescore_all(&conn).unwrap(), 1);
        assert_eq!(crate::articles::get_article(&conn, 1).unwrap().score, -10);
        let (ins, skip) = crate::ingest::upsert_entries(&conn, feed, &entries, 2).unwrap();
        assert_eq!(ins, 0);
        assert_eq!(skip, 1);
        assert_eq!(crate::articles::get_article(&conn, 1).unwrap().score, -10);
    }

    #[test]
    fn strip_html_does_not_match_tags() {
        assert_eq!(strip_html("<p>Hello <b>world</b></p>"), "Hello world");
        let input = MatchInput {
            feed_id: 1,
            folder_ids: &[],
            author: None,
            tags: &[],
            title: "",
            url: None,
            body: &strip_html("<script>evil</script><p>hello</p>"),
        };
        let rule = Rule {
            id: 1,
            polarity: Polarity::Like,
            dimension: Dimension::BodyPhrase,
            pattern: "script".into(),
            scope_kind: ScopeKind::Global,
            scope_id: None,
            weight: 10,
            source: RuleSource::Manual,
            enabled: true,
            pack_id: None,
        };
        let compiled = compile_rules(vec![rule]);
        assert!(match_rules(&compiled, &input).reasons.is_empty());
    }
}
