//! Read-only, bounded language metadata for list pages and feed priors.
use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension};

use crate::Result;

pub struct ArticleLanguage {
    pub id: i64,
    pub feed_id: i64,
    pub title: String,
    pub lang: Option<String>,
    pub fulltext_preview: Option<String>,
    pub html_preview: Option<String>,
}

/// Known languages never load body text. Legacy rows load only bounded snippets.
pub fn article_languages(conn: &Connection, ids: &[i64]) -> Result<Vec<ArticleLanguage>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, feed_id, title, lang,
            CASE WHEN lang IS NULL THEN substr(fulltext_md,1,4096) END,
            CASE WHEN lang IS NULL AND COALESCE(fulltext_md,'')='' THEN substr(content_html,1,8192) END
         FROM articles WHERE id=?1",
    )?;
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for &id in ids {
        if !seen.insert(id) {
            continue;
        }
        if let Some(row) = stmt
            .query_row([id], |r| {
                Ok(ArticleLanguage {
                    id: r.get(0)?,
                    feed_id: r.get(1)?,
                    title: r.get(2)?,
                    lang: r.get(3)?,
                    fulltext_preview: r.get(4)?,
                    html_preview: r.get(5)?,
                })
            })
            .optional()?
        {
            rows.push(row);
        }
    }
    Ok(rows)
}

#[derive(Default)]
pub struct FeedLanguageEvidence {
    pub declared: Option<String>,
    pub observed: Vec<String>,
}

impl FeedLanguageEvidence {
    pub fn measured(&self) -> Option<&str> {
        if self.observed.len() < 3 {
            return None;
        }
        let mut counts = HashMap::<&str, usize>::new();
        for lang in &self.observed {
            *counts.entry(lang).or_default() += 1;
        }
        let (lang, count) = counts.into_iter().max_by_key(|(_, count)| *count)?;
        (count * 10 >= self.observed.len() * 7).then_some(lang)
    }

    pub fn prior(&self) -> Option<&str> {
        self.measured().or(self.declared.as_deref())
    }
}

/// Only recent articles with independently useful prose may vote. Manual articles
/// have no shared source language, even if their synthetic feed has metadata.
pub fn feed_language_evidence(
    conn: &Connection,
    feed_ids: &[i64],
) -> Result<HashMap<i64, FeedLanguageEvidence>> {
    let mut feed =
        conn.prepare_cached("SELECT language FROM feeds WHERE id=?1 AND kind!='manual'")?;
    let mut recent = conn.prepare_cached(
        "SELECT lang, substr(fulltext_md,1,4096),
                CASE WHEN COALESCE(fulltext_md,'')='' THEN substr(content_html,1,8192) END
         FROM articles WHERE id IN (SELECT id FROM articles WHERE feed_id=?1 ORDER BY id DESC LIMIT 30)
           AND lang IS NOT NULL AND (fulltext_md IS NOT NULL OR length(COALESCE(content_html,''))>=200)",
    )?;
    let mut result = HashMap::new();
    for &id in feed_ids {
        if result.contains_key(&id) {
            continue;
        }
        let Some(declared) = feed
            .query_row([id], |r| r.get::<_, Option<String>>(0))
            .optional()?
        else {
            continue;
        };
        let mut evidence = FeedLanguageEvidence {
            declared: declared
                .as_deref()
                .and_then(rrss_render::normalize_lang_code),
            observed: Vec::new(),
        };
        let rows = recent.query_map([id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?;
        for row in rows {
            let (lang, fulltext, html) = row?;
            let prose = if let Some(text) = fulltext.filter(|s| !s.is_empty()) {
                rrss_render::markdown_prose_preview(&text, 2000)
            } else {
                rrss_render::html_prose_preview(html.as_deref().unwrap_or_default(), 2000)
            };
            if rrss_render::has_language_evidence(&prose)
                && let Some(lang) = rrss_render::normalize_lang_code(&lang)
            {
                evidence.observed.push(lang);
            }
        }
        result.insert(id, evidence);
    }
    Ok(result)
}

pub fn feed_language_priors(conn: &Connection, ids: &[i64]) -> Result<HashMap<i64, String>> {
    Ok(feed_language_evidence(conn, ids)?
        .into_iter()
        .filter_map(|(id, evidence)| Some((id, evidence.prior()?.to_owned())))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_requested_legacy_snippets_without_writes() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("languages.db")).unwrap();
        conn.execute_batch("INSERT INTO feeds(id,url,created_at) VALUES(1,'https://feed.example/',0);
            INSERT INTO articles(id,feed_id,title,lang,created_at) VALUES(1,1,'Stored','en',0),(2,1,'Legacy',NULL,0),(3,1,'Markdown',NULL,0);").unwrap();
        let huge = "文字".repeat(10000);
        conn.execute("UPDATE articles SET content_html=?1", [&huge])
            .unwrap();
        conn.execute("UPDATE articles SET fulltext_md=?1 WHERE id=3", [&huge])
            .unwrap();
        conn.pragma_update(None, "query_only", true).unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        let rows = article_languages(&tx, &[1, 2, 3, 2, 999]).unwrap();
        assert_eq!(rows.len(), 3);
        assert!(rows[0].fulltext_preview.is_none() && rows[0].html_preview.is_none());
        assert_eq!(rows[1].html_preview.as_ref().unwrap().chars().count(), 8192);
        assert_eq!(
            rows[2].fulltext_preview.as_ref().unwrap().chars().count(),
            4096
        );
        assert!(rows[2].html_preview.is_none());
        assert_eq!(crate::articles::get_article(&tx, 2).unwrap().lang, None);
        tx.commit().unwrap();
    }

    #[test]
    fn measured_prior_requires_recent_independent_prose_and_seventy_percent() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open_at(&dir.path().join("priors.db")).unwrap();
        conn.execute_batch("INSERT INTO feeds(id,url,language,kind,created_at) VALUES(1,'https://feed.example/','en','rss',0),(2,'manual:','zh','manual',0);").unwrap();
        let prose = "This is a synthetic report about the public library and how the reading rooms will be improved. ".repeat(4);
        let prior = || feed_language_priors(&conn, &[1, 1, 2, 999]).unwrap();
        for id in 1..=10 {
            conn.execute("INSERT INTO articles(id,feed_id,title,lang,content_html,created_at) VALUES(?1,1,'Fixture',?2,?3,0)", rusqlite::params![id,if id<=7 {"fr"} else {"de"},format!("<p>{prose}</p>")]).unwrap();
            if id == 2 {
                assert_eq!(prior().get(&1).map(String::as_str), Some("en"));
            }
        }
        assert_eq!(prior().get(&1).map(String::as_str), Some("fr"));
        assert_eq!(prior().len(), 1, "manual and missing feeds have no prior");
        conn.execute("UPDATE articles SET lang='de' WHERE id=7", [])
            .unwrap();
        assert_eq!(prior().get(&1).map(String::as_str), Some("en"));
        // Long markup, code and title-only rows must not reinforce their stored prior.
        for id in 11..=40 {
            conn.execute("INSERT INTO articles(id,feed_id,title,lang,content_html,created_at) VALUES(?1,1,'Product','ja',?2,0)", rusqlite::params![id,if id%2==0 {format!("<pre>{prose}</pre>")} else {String::new()}]).unwrap();
        }
        assert_eq!(prior().get(&1).map(String::as_str), Some("en"));
        assert!(
            feed_language_evidence(&conn, &[1]).unwrap()[&1]
                .observed
                .is_empty()
        );
    }
}
