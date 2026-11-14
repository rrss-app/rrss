use rusqlite::Connection;

use rrss_core::articles;
use rrss_core::feeds;
use rrss_core::presets;
use rrss_core::rules::{self, Dimension, Polarity, RuleFilter, RuleSource, ScopeKind};

use crate::views::*;

/// 无查询上下文的调用者沿用默认阈值；规则页使用实际设置和作用域名称。
pub fn rule_to_view(rule: rrss_core::Rule) -> RuleView {
    rule_to_view_with_context(rule, articles::DEFAULT_HIDE_THRESHOLD, None)
}

fn rule_to_view_with_context(
    rule: rrss_core::Rule,
    hide_threshold: i64,
    scope_name: Option<&str>,
) -> RuleView {
    // 与 score::compile_rules 使用同一有符号权重。这里描述单条规则的作用，
    // 文章是否隐藏仍由所有命中规则的总分决定，不能把所有 Dislike 都称为隐藏。
    let weight = rule.polarity.signed_weight(rule.weight);
    let effect = if weight > 0 {
        RuleEffect::Boost
    } else if weight < 0 && weight <= hide_threshold {
        RuleEffect::Hide
    } else if weight < 0 {
        RuleEffect::Lower
    } else {
        RuleEffect::None
    };
    let summary = RuleSummaryView::RuleSummary { effect };
    RuleView {
        id: rule.id,
        polarity: rule.polarity.as_str().into(),
        dimension_id: rule.dimension.as_str().into(),
        pattern: rule.pattern,
        summary,
        scope_name: scope_name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_default()
            .into(),
        scope_kind: rule.scope_kind.as_str().into(),
        scope_id: rule.scope_id.unwrap_or(0),
        weight: rule.weight,
        source: rule.source.as_str().into(),
        enabled: rule.enabled,
        pending: rule.source == RuleSource::Suggested && !rule.enabled,
        pack_id: rule.pack_id.unwrap_or_default(),
    }
}

pub(super) fn rule_condition(dimension: Dimension, pattern: &str) -> String {
    match dimension {
        Dimension::Author => format!("作者是『{pattern}』"),
        Dimension::Tag => format!("带有标签『{pattern}』"),
        Dimension::TitleKw => format!("标题包含『{pattern}』"),
        Dimension::TitleRe => format!("标题匹配正则『{pattern}』"),
        Dimension::UrlPhrase => format!("URL 包含『{pattern}』"),
        Dimension::UrlRe => format!("URL 匹配正则『{pattern}』"),
        Dimension::BodyPhrase => format!("正文包含『{pattern}』"),
        Dimension::BodyRe => format!("正文匹配正则『{pattern}』"),
    }
}

pub fn rules_page(conn: &Connection, filter_key: &str) -> rrss_core::Result<RulesPage> {
    let filter = match filter_key {
        "like" => RuleFilter {
            polarity: Some(Polarity::Like),
            enabled: Some(true),
            ..RuleFilter::default()
        },
        "dislike" => RuleFilter {
            polarity: Some(Polarity::Dislike),
            enabled: Some(true),
            ..RuleFilter::default()
        },
        "pending" => RuleFilter {
            source: Some(RuleSource::Suggested),
            enabled: Some(false),
            ..RuleFilter::default()
        },
        _ => RuleFilter::default(),
    };
    let rules = rules::list_rules(conn, &filter)?;
    let presets = presets::pack_statuses(conn)?
        .into_iter()
        .map(|s| PresetView {
            id: s.pack.id,
            name: s.pack.name,
            description: s.pack.description,
            rule_count: s.pack.rules.len() as i64,
            enabled_count: s.enabled_count,
            installed: s.installed,
        })
        .collect();
    let folders: Vec<ScopeOption> = feeds::list_folders(conn)
        .unwrap_or_default()
        .into_iter()
        .map(|f| ScopeOption {
            id: f.id,
            label: f.name,
        })
        .collect();
    let feeds: Vec<ScopeOption> = feeds::list_feeds(conn)
        .unwrap_or_default()
        .into_iter()
        .map(|f| ScopeOption {
            id: f.id,
            label: f.title.unwrap_or(f.url),
        })
        .collect();
    let hide_threshold = articles::hide_threshold(conn).unwrap_or(articles::DEFAULT_HIDE_THRESHOLD);
    let rules = rules
        .into_iter()
        .map(|rule| {
            let options = match rule.scope_kind {
                ScopeKind::Global => &[][..],
                ScopeKind::Feed => feeds.as_slice(),
                ScopeKind::Folder => folders.as_slice(),
            };
            let scope_name = options
                .iter()
                .find(|option| Some(option.id) == rule.scope_id)
                .map(|option| option.label.as_str());
            rule_to_view_with_context(rule, hide_threshold, scope_name)
        })
        .collect();
    Ok(RulesPage {
        rules,
        presets,
        folders,
        feeds,
        hide_threshold,
    })
}

pub fn training_features(
    conn: &Connection,
    article_id: i64,
    excerpt: Option<&str>,
) -> rrss_core::Result<Vec<FeatureView>> {
    let article = articles::get_article(conn, article_id)?;
    let tags = rrss_core::tags_for_article(conn, article.id).unwrap_or_default();
    Ok(rrss_core::extract_features(
        article.author.as_deref(),
        &tags,
        &article.title,
        article.canonical_url.as_deref(),
        excerpt,
    )
    .into_iter()
    .map(|c| FeatureView {
        dimension: c.dimension.as_str().into(),
        pattern: c.pattern,
    })
    .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::db::{PooledConnection, open_at};
    use rrss_core::feeds::SubscribeRequest;
    use rrss_core::rules::{NewRule, Rule};

    fn effect(view: &RuleView) -> &'static str {
        let RuleSummaryView::RuleSummary { effect } = &view.summary;
        match effect {
            RuleEffect::Boost => "boost",
            RuleEffect::Hide => "hide",
            RuleEffect::Lower => "lower",
            RuleEffect::None => "none",
        }
    }

    fn open_temp() -> (tempfile::TempDir, PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        (dir, conn)
    }

    fn subscribe(conn: &Connection, url: &str, folder_id: Option<i64>) -> i64 {
        feeds::subscribe(
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

    #[test]
    fn training_features_preserve_ids_and_patterns() {
        let (_dir, conn) = open_temp();
        let feed_id = subscribe(&conn, "https://example.test/rss", None);
        let entry = rrss_core::parse::ParsedEntry {
            guid: "training".into(),
            canonical_url: Some("https://example.test/article".into()),
            title: "融资".into(),
            author: Some("某某".into()),
            published_at: Some(1),
            content_html: Some("<p>正文</p>".into()),
            hash: "training".into(),
            tags: vec!["产品".into()],
        };
        rrss_core::ingest::upsert_entries(&conn, feed_id, &[entry], 1).unwrap();
        let article_id: i64 = conn
            .query_row("SELECT id FROM articles", [], |r| r.get(0))
            .unwrap();
        let features = training_features(&conn, article_id, Some("正文")).unwrap();
        let expected = [
            ("author", "某某", "作者是『某某』"),
            ("tag", "产品", "带有标签『产品』"),
            ("url_phrase", "example.test", "URL 包含『example.test』"),
            ("title_kw", "融资", "标题包含『融资』"),
            ("body_phrase", "正文", "正文包含『正文』"),
        ];
        assert_eq!(features.len(), expected.len());
        for (feature, (dimension, pattern, _label)) in features.iter().zip(expected) {
            assert_eq!(feature.dimension, dimension);
            assert_eq!(feature.pattern, pattern);
            assert!(
                serde_json::to_value(feature)
                    .unwrap()
                    .get("label")
                    .is_none()
            );
        }
    }

    #[test]
    fn summaries_cover_all_dimensions_polarities_and_scopes() {
        let (_dir, conn) = open_temp();
        feeds::create_folder(&conn, "其他", None).unwrap();
        let folder_id = feeds::create_folder(&conn, "技术", None).unwrap();
        subscribe(&conn, "https://other.example/rss", None);
        let feed_id = subscribe(&conn, "https://sspai.example/rss", Some(folder_id));
        feeds::rename_feed(&conn, feed_id, "少数派").unwrap();
        let dimensions = [
            (Dimension::Author, "某某", "作者是『某某』"),
            (Dimension::Tag, "科技", "带有标签『科技』"),
            (Dimension::TitleKw, "融资", "标题包含『融资』"),
            (Dimension::TitleRe, "融资.*", "标题匹配正则『融资.*』"),
            (Dimension::UrlPhrase, "/ad/", "URL 包含『/ad/』"),
            (Dimension::UrlRe, "/ad/.*", "URL 匹配正则『/ad/.*』"),
            (Dimension::BodyPhrase, "推广", "正文包含『推广』"),
            (Dimension::BodyRe, "推广.*", "正文匹配正则『推广.*』"),
        ];
        let scopes = [
            (ScopeKind::Global, None, "", "全局"),
            (ScopeKind::Feed, Some(feed_id), "来自『少数派』、", "仅此源"),
            (
                ScopeKind::Folder,
                Some(folder_id),
                "在『技术』文件夹内、",
                "此文件夹及子文件夹",
            ),
        ];
        let effects = [
            (Polarity::Like, 3, "boost"),
            (Polarity::Dislike, 3, "lower"),
            (Polarity::Dislike, 10, "hide"),
        ];
        let mut expected = std::collections::HashMap::new();
        for (dimension, pattern, _condition) in dimensions {
            for (scope_kind, scope_id, _prefix, _suffix) in scopes {
                for (polarity, weight, effect) in effects {
                    let mut spec = NewRule::manual_global(polarity, dimension, pattern);
                    spec.scope_kind = scope_kind;
                    spec.scope_id = scope_id;
                    spec.weight = weight;
                    let id = rules::insert_rule(&conn, spec).unwrap();
                    expected.insert(id, effect);
                }
            }
        }
        let page = rules_page(&conn, "all").unwrap();
        assert_eq!(page.rules.len(), 8 * 3 * 3);
        for view in page.rules {
            assert_eq!(effect(&view), expected[&view.id], "rule {}", view.id);
            assert!(!view.pending);
        }
    }

    #[test]
    fn summaries_follow_live_threshold_and_inclusive_boundary() {
        let (_dir, conn) = open_temp();
        let negative_id = rules::insert_rule(
            &conn,
            NewRule::manual_global(Polarity::Dislike, Dimension::TitleKw, "融资"),
        )
        .unwrap();
        let positive_id = rules::insert_rule(
            &conn,
            NewRule::manual_global(Polarity::Like, Dimension::TitleKw, "融资"),
        )
        .unwrap();
        for (threshold, weight, expected_effect) in [
            (-10, 9, "lower"),
            (-10, 10, "hide"),
            (-10, 11, "hide"),
            (-20, 19, "lower"),
            (-20, 20, "hide"),
            (-20, 21, "hide"),
            (-1, 1, "hide"),
            (0, 1, "hide"),
            (1, 1, "hide"),
            (i64::MIN, i64::MAX, "lower"),
            (i64::MIN + 1, i64::MAX, "hide"),
        ] {
            articles::set_hide_threshold(&conn, threshold).unwrap();
            for id in [negative_id, positive_id] {
                let mut rule = rules::get_rule(&conn, id).unwrap();
                rule.weight = weight;
                rules::update_rule(&conn, &rule).unwrap();
            }
            let page = rules_page(&conn, "all").unwrap();
            assert_eq!(page.hide_threshold, threshold);
            let negative = page.rules.iter().find(|r| r.id == negative_id).unwrap();
            assert_eq!(
                effect(negative),
                expected_effect,
                "threshold={threshold}, weight={weight}"
            );
            let positive = page.rules.iter().find(|r| r.id == positive_id).unwrap();
            assert_eq!(effect(positive), "boost");
        }
    }

    #[test]
    fn pending_mapping_and_rule_fields_remain_unchanged() {
        let (_dir, conn) = open_temp();
        articles::set_hide_threshold(&conn, -20).unwrap();
        let mut expected_pending = None;
        for source in [RuleSource::Manual, RuleSource::Suggested] {
            for enabled in [false, true] {
                let mut spec = NewRule::manual_global(Polarity::Dislike, Dimension::Author, "某某");
                spec.source = source;
                spec.enabled = enabled;
                spec.pack_id = Some("test-pack".into());
                let id = rules::insert_rule(&conn, spec).unwrap();
                if source == RuleSource::Suggested && !enabled {
                    expected_pending = Some(id);
                }
                let rule = rules::get_rule(&conn, id).unwrap();
                let view = rule_to_view(rule.clone());
                assert_eq!(view.id, id);
                assert_eq!(view.polarity, "dislike");
                assert_eq!(view.dimension_id, "author");
                assert_eq!(view.pattern, "某某");
                assert_eq!(view.scope_kind, "global");
                let json = serde_json::to_value(&view).unwrap();
                assert!(json.get("dimension").is_none());
                assert!(json.get("scope_label").is_none());
                assert_eq!(view.scope_id, 0);
                assert_eq!(view.weight, rule.weight);
                assert_eq!(view.source, source.as_str());
                assert_eq!(view.enabled, enabled);
                assert_eq!(view.pending, source == RuleSource::Suggested && !enabled);
                assert_eq!(view.pack_id, "test-pack");
                // 原单参数入口使用默认阈值；待确认只由来源与开关决定。
                assert_eq!(effect(&view), "hide");
            }
        }
        let page = rules_page(&conn, "all").unwrap();
        assert_eq!(page.rules.len(), 4);
        for view in page.rules {
            assert_eq!(view.pending, Some(view.id) == expected_pending);
            assert_eq!(effect(&view), "lower");
        }
        let pending = rules_page(&conn, "pending").unwrap().rules;
        assert_eq!(pending.len(), 1);
        assert_eq!(Some(pending[0].id), expected_pending);
        assert!(pending[0].pending);
        assert!(!pending[0].enabled);
        assert_eq!(effect(&pending[0]), "lower");
    }

    #[test]
    fn scope_names_use_current_feed_title_or_url_and_folder_name() {
        let (_dir, conn) = open_temp();
        let folder_id = feeds::create_folder(&conn, "技术", None).unwrap();
        let child_id = feeds::create_folder(&conn, "工具", Some(folder_id)).unwrap();
        let feed_id = subscribe(&conn, "https://sspai.example/rss", Some(child_id));
        let mut spec = NewRule::manual_global(Polarity::Dislike, Dimension::Author, "某某");
        spec.scope_kind = ScopeKind::Feed;
        spec.scope_id = Some(feed_id);
        let feed_rule_id = rules::insert_rule(&conn, spec.clone()).unwrap();
        spec.scope_kind = ScopeKind::Folder;
        spec.scope_id = Some(folder_id);
        let folder_rule_id = rules::insert_rule(&conn, spec).unwrap();
        let page = rules_page(&conn, "all").unwrap();
        let feed_view = page.rules.iter().find(|r| r.id == feed_rule_id).unwrap();
        assert_eq!(feed_view.scope_name, "https://sspai.example/rss");
        assert_eq!(feed_view.scope_kind, "feed");
        assert_eq!(feed_view.scope_id, feed_id);
        feeds::rename_feed(&conn, feed_id, "少数派").unwrap();
        let page = rules_page(&conn, "all").unwrap();
        assert_eq!(
            page.rules
                .iter()
                .find(|r| r.id == feed_rule_id)
                .unwrap()
                .scope_name,
            feeds::list_feeds(&conn)
                .unwrap()
                .into_iter()
                .find(|feed| feed.id == feed_id)
                .unwrap()
                .title
                .unwrap()
        );
        let folder_view = page.rules.iter().find(|r| r.id == folder_rule_id).unwrap();
        assert_eq!(
            folder_view.scope_name,
            feeds::list_folders(&conn)
                .unwrap()
                .into_iter()
                .find(|folder| folder.id == folder_id)
                .unwrap()
                .name
        );
        assert_eq!(folder_view.scope_kind, "folder");
        assert_eq!(folder_view.scope_id, folder_id);
        let folder_ids = rrss_core::score::folder_chain(&conn, Some(child_id)).unwrap();
        let compiled =
            rrss_core::score::compile_rules(vec![rules::get_rule(&conn, folder_rule_id).unwrap()]);
        let score = rrss_core::score::match_rules(
            &compiled,
            &rrss_core::score::MatchInput {
                feed_id,
                folder_ids: &folder_ids,
                author: Some("某某"),
                tags: &[],
                title: "",
                url: None,
                body: "",
            },
        );
        assert_eq!(score.score, -10);
        assert_eq!(score.reasons.len(), 1);
    }

    #[test]
    fn context_free_scopes_never_become_global_or_invent_conditions() {
        for (scope_kind, _suffix) in [
            (ScopeKind::Global, "全局"),
            (ScopeKind::Feed, "仅此源"),
            (ScopeKind::Folder, "此文件夹及子文件夹"),
        ] {
            let rule = Rule {
                id: 1,
                polarity: Polarity::Like,
                dimension: Dimension::BodyRe,
                pattern: "融资.*".into(),
                scope_kind,
                scope_id: (scope_kind != ScopeKind::Global).then_some(99),
                weight: 3,
                source: RuleSource::Manual,
                enabled: true,
                pack_id: None,
            };
            let view = rule_to_view(rule);
            assert_eq!(effect(&view), "boost");
            assert!(view.scope_name.is_empty());
            assert_eq!(view.scope_kind, scope_kind.as_str());
            assert_eq!(
                view.scope_id,
                if scope_kind == ScopeKind::Global {
                    0
                } else {
                    99
                }
            );
            assert!(view.pack_id.is_empty());
        }
    }
}
