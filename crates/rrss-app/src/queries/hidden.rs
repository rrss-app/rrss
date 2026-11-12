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
            let label = articles
                .iter()
                .flat_map(|article| &article.reasons)
                .find(|reason| reason.rule_id == rule_id)
                .map(|reason| reason.label.clone())
                .unwrap_or_default();
            (
                rule_id,
                HiddenRuleCount {
                    rule_id,
                    label,
                    count,
                },
            )
        })
        .collect();
    for rule in rules::list_rules(conn, &rules::RuleFilter::default())? {
        if let Some(count) = counts.get_mut(&rule.id) {
            count.label = super::rules::rule_condition(rule.dimension, &rule.pattern);
        }
    }
    Ok(HiddenPage {
        spot_liked_count: articles.iter().filter(|article| article.spot_liked).count() as i64,
        articles,
        rule_counts: counts.into_values().collect(),
    })
}
