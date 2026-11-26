//! Filter rule, preset and feature write orchestration with atomic rescoring.

use rrss_core::feeds::now_ms;
use rrss_core::rules::{
    self, Dimension, NewRule, Polarity, RuleAction, RuleSource, ScopeKind, validate_pattern,
};
use rrss_core::{presets, rescore_all};
use rusqlite::Connection;

use super::persist_then_rescore;
use crate::views::NewRuleSpec;

fn spec_to_new_rule(spec: &NewRuleSpec) -> rrss_core::Result<NewRule> {
    let polarity = Polarity::parse_str(&spec.polarity)?;
    let dimension = Dimension::parse_str(&spec.dimension)?;
    validate_pattern(dimension, &spec.pattern)?;
    let scope_kind = ScopeKind::parse_str(&spec.scope_kind)?;
    Ok(NewRule {
        polarity,
        dimension,
        pattern: spec.pattern.trim().to_string(),
        scope_kind,
        scope_id: if scope_kind == ScopeKind::Global {
            None
        } else {
            Some(spec.scope_id)
        },
        weight: spec.weight,
        source: RuleSource::Manual,
        enabled: true,
        pack_id: None,
    })
}

pub fn create_rule(conn: &Connection, spec: &NewRuleSpec) -> rrss_core::Result<i64> {
    let spec = spec_to_new_rule(spec)?;
    persist_then_rescore(conn, |c| rules::insert_rule(c, spec.clone()))
}

pub fn release_article(conn: &Connection, id: i64) -> rrss_core::Result<()> {
    set_article_release(conn, id, Some(now_ms()))
}

pub fn unrelease_article(conn: &Connection, id: i64) -> rrss_core::Result<()> {
    set_article_release(conn, id, None)
}

fn set_article_release(conn: &Connection, id: i64, at: Option<i64>) -> rrss_core::Result<()> {
    let changed = conn.execute(
        "UPDATE articles SET released_at = ?1 WHERE id = ?2",
        rusqlite::params![at, id],
    )?;
    if changed == 0 {
        return Err(rrss_core::Error::ArticleNotFound);
    }
    Ok(())
}

pub fn allow_rule_for_feed(
    conn: &Connection,
    rule_id: i64,
    feed_id: i64,
) -> rrss_core::Result<i64> {
    persist_then_rescore(conn, |c| {
        let rule = rules::get_rule(c, rule_id)?;
        rules::insert_rule(
            c,
            NewRule {
                polarity: match rule.polarity {
                    Polarity::Like => Polarity::Dislike,
                    Polarity::Dislike => Polarity::Like,
                },
                dimension: rule.dimension,
                pattern: rule.pattern,
                scope_kind: ScopeKind::Feed,
                scope_id: Some(feed_id),
                weight: rule.weight.abs(),
                source: RuleSource::Manual,
                enabled: true,
                pack_id: None,
            },
        )
    })
}

pub fn update_rule(conn: &Connection, id: i64, spec: &NewRuleSpec) -> rrss_core::Result<()> {
    persist_then_rescore(conn, |c| {
        let mut rule = rules::get_rule(c, id)?;
        let n = spec_to_new_rule(spec)?;
        rule.polarity = n.polarity;
        rule.dimension = n.dimension;
        rule.pattern = n.pattern;
        rule.weight = n.weight;
        rule.scope_kind = n.scope_kind;
        rule.scope_id = n.scope_id;
        rules::update_rule(c, &rule)
    })
}

pub fn set_rule_action(conn: &Connection, id: i64, action: i32) -> rrss_core::Result<()> {
    let mapped = match action {
        0 => RuleAction::Like,
        1 => RuleAction::Dislike,
        _ => RuleAction::Neutral,
    };
    persist_then_rescore(conn, |c| rules::set_rule_action(c, id, mapped))
}

pub fn confirm_suggested_rule(conn: &Connection, id: i64) -> rrss_core::Result<()> {
    persist_then_rescore(conn, |c| rules::confirm_rule(c, id))
}

pub fn discard_rule(conn: &Connection, id: i64) -> rrss_core::Result<()> {
    persist_then_rescore(conn, |c| rules::delete_rule(c, id))
}

pub fn apply_preset(conn: &Connection, id: &str) -> rrss_core::Result<u32> {
    persist_then_rescore(conn, |c| presets::apply_pack(c, id))
}

pub fn remove_preset(conn: &Connection, id: &str) -> rrss_core::Result<u32> {
    persist_then_rescore(conn, |c| presets::remove_pack(c, id))
}

pub fn confirm_feature(
    conn: &Connection,
    _article_id: i64,
    feed_id: i64,
    dislike: bool,
    feed_scope: bool,
    dimension: &str,
    pattern: &str,
) -> rrss_core::Result<i64> {
    let polarity = if dislike {
        Polarity::Dislike
    } else {
        Polarity::Like
    };
    let dimension = Dimension::parse_str(dimension)?;
    let (scope_kind, scope_id) = if feed_scope {
        (ScopeKind::Feed, Some(feed_id))
    } else {
        (ScopeKind::Global, None)
    };
    persist_then_rescore(conn, |c| {
        rules::insert_rule(
            c,
            NewRule {
                polarity,
                dimension,
                pattern: pattern.to_string(),
                scope_kind,
                scope_id,
                weight: rules::DEFAULT_WEIGHT,
                source: RuleSource::Manual,
                enabled: true,
                pack_id: None,
            },
        )
    })
}

pub fn rescore(conn: &Connection) -> rrss_core::Result<u32> {
    rescore_all(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::open_temp;

    #[test]
    fn create_rule_rescores_existing_articles() {
        use rrss_core::articles::{self, FocusFilter};
        use rrss_core::feeds::{SubscribeRequest, now_ms, subscribe};
        use rrss_core::parse::ParsedEntry;

        let (_dir, conn) = open_temp();
        // Focus only admits recent articles; the rescoring under test is date-independent.
        let now = now_ms();
        let feed = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://news.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        rrss_core::ingest::upsert_entries(
            &conn,
            feed,
            &[ParsedEntry {
                language: None,
                guid: "r1".into(),
                canonical_url: Some("https://news.example.com/rust".into()),
                title: "Rust 1.80 shipped".into(),
                author: None,
                published_at: Some(now),
                content_html: Some("<p>release</p>".into()),
                hash: "h".into(),
                tags: vec![],
            }],
            now,
        )
        .unwrap();
        articles::set_focus_filter(&conn, FocusFilter::FocusOnly).unwrap();
        assert!(rrss_focus::list_focus(&conn, 20).unwrap().is_empty());

        create_rule(
            &conn,
            &NewRuleSpec {
                polarity: "like".into(),
                dimension: "title_kw".into(),
                pattern: "Rust".into(),
                weight: 10,
                scope_kind: "global".into(),
                scope_id: 0,
            },
        )
        .unwrap();

        let focus = rrss_focus::list_focus(&conn, 20).unwrap();
        assert_eq!(focus.len(), 1);
        assert_eq!(focus[0].title, "Rust 1.80 shipped");
        assert_eq!(focus[0].score, 10);
    }

    #[test]
    fn create_rule_rolls_back_when_rescore_fails() {
        use rrss_core::feeds::{SubscribeRequest, subscribe};
        use rrss_core::parse::ParsedEntry;
        use rrss_core::rules::{self, RuleFilter};

        let (_dir, conn) = open_temp();
        let feed = subscribe(
            &conn,
            SubscribeRequest {
                url: "https://news.example.com/rss.xml".into(),
                folder_id: None,
                as_rsshub: false,
                rsshub_instance_id: None,
            },
        )
        .unwrap();
        rrss_core::ingest::upsert_entries(
            &conn,
            feed,
            &[ParsedEntry {
                language: None,
                guid: "r1".into(),
                canonical_url: Some("https://news.example.com/rust".into()),
                title: "Rust 1.80 shipped".into(),
                author: None,
                published_at: Some(1),
                content_html: Some("<p>release</p>".into()),
                hash: "h".into(),
                tags: vec![],
            }],
            1,
        )
        .unwrap();
        conn.execute(
            "CREATE TRIGGER fail_score BEFORE UPDATE OF score ON articles
             BEGIN SELECT RAISE(ABORT, 'injected'); END;",
            [],
        )
        .unwrap();

        let err = create_rule(
            &conn,
            &NewRuleSpec {
                polarity: "like".into(),
                dimension: "title_kw".into(),
                pattern: "Rust".into(),
                weight: 10,
                scope_kind: "global".into(),
                scope_id: 0,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("injected"), "{err}");
        let n = rules::list_rules(&conn, &RuleFilter::default())
            .unwrap()
            .len();
        assert_eq!(n, 0);
    }
}
