use std::collections::BTreeMap;

use rrss_core::articles::{self, ListScope};
use rrss_core::feed_scope::{self, FeedScope};
use rrss_core::{hidden, parse_reasons, rules};
use rusqlite::Connection;

use crate::views::*;

pub fn hidden_page(
    conn: &Connection,
    scope: Option<&str>,
    list_scope: Option<&str>,
) -> rrss_core::Result<HiddenPage> {
    let scope = match scope {
        Some(text) => feed_scope::resolve(conn, FeedScope::parse_str(text))?,
        None => FeedScope::All,
    };
    let list_scope = match list_scope {
        Some(text) => ListScope::parse_str(text),
        None => articles::list_scope(conn)?,
    };
    build(conn, scope, list_scope)
}

fn build(
    conn: &Connection,
    scope: FeedScope,
    list_scope: ListScope,
) -> rrss_core::Result<HiddenPage> {
    let articles: Vec<_> = hidden::list_hidden(conn, scope, list_scope)?
        .into_iter()
        .map(|article| HiddenArticleView {
            id: article.id,
            title: article.title,
            feed_id: article.feed_id,
            feed_title: article.feed_title,
            published_at: article.published_at,
            score: article.score,
            spot_liked: article.spot_liked,
            reasons: parse_reasons(article.reasons_json.as_deref())
                .into_iter()
                .filter(|reason| reason.weight < 0)
                .map(ReasonView::from)
                .collect(),
        })
        .collect();
    let mut counts: BTreeMap<_, _> = hidden::rule_counts(conn, scope, list_scope)?
        .into_iter()
        .map(|(rule_id, count)| {
            let message = articles
                .iter()
                .flat_map(|article| &article.reasons)
                .find(|reason| reason.rule_id == rule_id)
                .map(|reason| reason.message.clone())
                .unwrap_or(crate::reasons::ReasonMessage::LegacyText {
                    text: String::new(),
                    prefix: None,
                });
            (
                rule_id,
                HiddenRuleCount {
                    rule_id,
                    message,
                    count,
                },
            )
        })
        .collect();
    for rule in rules::list_rules(conn, &rules::RuleFilter::default())? {
        if let Some(count) = counts.get_mut(&rule.id) {
            count.message = if crate::ui_error::params::reason_text(&rule.pattern) == rule.pattern {
                crate::reasons::ReasonMessage::rule_condition(rule.dimension, &rule.pattern)
            } else {
                crate::reasons::ReasonMessage::LegacyText {
                    text: crate::ui_error::params::reason_legacy_text(
                        &super::rules::rule_condition(rule.dimension, &rule.pattern),
                    ),
                    prefix: None,
                }
            };
        }
    }
    Ok(HiddenPage {
        spot_liked_count: articles.iter().filter(|article| article.spot_liked).count() as i64,
        articles,
        rule_counts: counts.into_values().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::rules::{Dimension, NewRule, Polarity, insert_rule};

    #[test]
    fn hidden_rule_filters_use_condition_codes_and_count_coded_rows() {
        let dir = tempfile::tempdir().unwrap();
        let conn = rrss_core::db::open_at(&dir.path().join("hidden.db")).unwrap();
        let cases = [
            (Dimension::Author, "rule_condition_author"),
            (Dimension::Tag, "rule_condition_tag"),
            (Dimension::TitleKw, "rule_condition_title_kw"),
            (Dimension::TitleRe, "rule_condition_title_re"),
            (Dimension::UrlPhrase, "rule_condition_url_phrase"),
            (Dimension::UrlRe, "rule_condition_url_re"),
            (Dimension::BodyPhrase, "rule_condition_body_phrase"),
            (Dimension::BodyRe, "rule_condition_body_re"),
        ];
        for (dimension, _) in cases {
            insert_rule(
                &conn,
                NewRule::manual_global(Polarity::Dislike, dimension, "Rust"),
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://rules.example/feed',0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO articles(id,feed_id,title,created_at) VALUES(1,1,'Rust',?1)",
            [rrss_core::feeds::now_ms()],
        )
        .unwrap();
        let tags = ["Rust".to_string()];
        let result = rrss_core::score::match_rules(
            &rrss_core::score::load_compiled_rules(&conn).unwrap(),
            &rrss_core::score::MatchInput {
                feed_id: 1,
                folder_ids: &[],
                author: Some("Rust"),
                tags: &tags,
                title: "Rust",
                url: Some("https://rules.example/Rust"),
                body: "Rust",
            },
        );
        rrss_core::score::write_score(&conn, 1, &result).unwrap();
        let page = hidden_page(&conn, None, None).unwrap();
        assert_eq!(page.articles.len(), 1);
        let wire = serde_json::to_value(&page.rule_counts).unwrap();
        for (i, (_, code)) in cases.into_iter().enumerate() {
            assert_eq!(wire[i]["code"], code);
            assert_eq!(wire[i]["params"], serde_json::json!({"pattern":"Rust"}));
            assert_eq!(wire[i]["count"], 1);
        }
    }

    #[test]
    fn url_rule_conditions_keep_the_old_display_instead_of_a_host_only_label() {
        let dir = tempfile::tempdir().unwrap();
        let conn = rrss_core::db::open_at(&dir.path().join("url-rule.db")).unwrap();
        let pattern = "https://stories.example/articles?id=12";
        insert_rule(
            &conn,
            NewRule::manual_global(Polarity::Dislike, Dimension::UrlPhrase, pattern),
        )
        .unwrap();
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(1,'https://stories.example/feed',0)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO articles(id,feed_id,title,created_at) VALUES(1,1,'Story',?1)",
            [rrss_core::feeds::now_ms()],
        )
        .unwrap();
        let result = rrss_core::score::match_rules(
            &rrss_core::score::load_compiled_rules(&conn).unwrap(),
            &rrss_core::score::MatchInput {
                feed_id: 1,
                folder_ids: &[],
                author: None,
                tags: &[],
                title: "Story",
                url: Some(pattern),
                body: "",
            },
        );
        rrss_core::score::write_score(&conn, 1, &result).unwrap();
        let page = hidden_page(&conn, None, None).unwrap();
        let wire = serde_json::to_value(&page.rule_counts[0]).unwrap();
        assert_eq!(wire["code"], "legacy_text");
        let original = super::super::rules::rule_condition(Dimension::UrlPhrase, pattern);
        assert_eq!(
            wire["params"],
            serde_json::json!({"text":original,"prefix":null})
        );
    }
}
