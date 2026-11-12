use super::*;
use crate::extract::{Suspicion, extract_article};
use rrss_core::db::site_rules::match_site_rule;

const HTML: &str = include_str!("../../../../../testdata/web/site-rule.html");

fn url() -> Url {
    Url::parse("https://example.test/story").unwrap()
}
fn rule() -> SiteRule {
    SiteRule {
        domain: "example.test".into(),
        ..SiteRule::default()
    }
}

#[test]
fn blank_missing_and_invalid_fields_fall_through_to_the_cascade() {
    let html = include_str!("../../../../../testdata/web/english.html");
    let baseline = extract_article(html, &url(), None).unwrap();
    for selector in [None, Some(""), Some(".missing"), Some("[invalid")] {
        let selector = selector.map(str::to_owned);
        let rule = SiteRule {
            javascript_disabled: true,
            body: selector.clone(),
            title: selector.clone(),
            author: selector.clone(),
            date: selector,
            ..rule()
        };
        let result = extract_article(html, &url(), Some(&rule)).unwrap();
        assert_eq!(result.layer, baseline.layer);
        assert_eq!(result.title, baseline.title);
        assert_eq!(result.byline, baseline.byline);
        assert_eq!(result.published_time, baseline.published_time);
        assert_eq!(result.document, baseline.document);
    }
    let empty_body = SiteRule {
        body: Some(".empty".into()),
        ..rule()
    };
    let result = extract_article(
        &format!("{html}<div class=empty></div>"),
        &url(),
        Some(&empty_body),
    )
    .unwrap();
    assert_ne!(result.layer, Layer::Rule);
}

#[test]
fn body_hit_stops_at_rule_even_when_suspicious_and_deduplicates_nested_matches() {
    let rule = SiteRule {
        body: Some("#rule-body, #rule-body p".into()),
        ..rule()
    };
    let result = extract_article(HTML, &url(), Some(&rule)).unwrap();
    assert_eq!(result.layer, Layer::Rule);
    assert!(result.suspicious.contains(&Suspicion::ShortText));
    let plain = rrss_render::document_to_plain(&result.document);
    assert_eq!(plain.matches("First selected paragraph.").count(), 1);
    assert!(!plain.contains("observatory"));
    assert_eq!(result.image_count, 1);
    assert!(
        matches!(result.document.blocks.last(), Some(rrss_render::Block::Image { src, .. })
        if src == "https://example.test/assets/photo.png")
    );
    let listing = SiteRule {
        body: Some("body".into()),
        ..rule
    };
    let html = include_str!("../../../../../testdata/web/listing.html");
    assert_eq!(
        extract_article(html, &url(), Some(&listing)).unwrap().layer,
        Layer::Rule
    );
}

#[test]
fn strip_applies_to_rule_and_every_fallback_and_counts_unique_selected_elements() {
    let strip = vec![".advert".into(), ".advert, aside".into(), "[invalid".into()];
    let rule = SiteRule {
        body: Some("#rule-body".into()),
        strip: strip.clone(),
        ..rule()
    };
    let result = extract_article(HTML, &url(), Some(&rule)).unwrap();
    assert_eq!(result.stripped_elements, 3); // includes explicitly selected nested element
    assert!(!rrss_render::document_to_plain(&result.document).contains("advert"));
    let rule = SiteRule {
        body: None,
        strip,
        ..rule
    };
    let result = extract_article(HTML, &url(), Some(&rule)).unwrap();
    assert_eq!(result.stripped_elements, 3);
    assert!(!rrss_render::document_to_plain(&result.document).contains("advert"));
    // A long subtree selected by both generic extractors and the container rescue is removed.
    let html = include_str!("../../../../../testdata/web/container.html");
    let baseline = extract_article(html, &url(), None).unwrap();
    let rule = SiteRule {
        strip: vec!["article, main".into()],
        ..self::rule()
    };
    let result = extract_article(html, &url(), Some(&rule));
    assert!(result.is_err() || result.unwrap().text_chars < baseline.text_chars);
}

#[test]
fn rule_metadata_beats_library_then_page_title_for_both_body_paths() {
    for body in [Some("#rule-body".into()), None] {
        let rule = SiteRule {
            body: body.clone(),
            title: Some(".rule-title".into()),
            author: Some(".rule-author".into()),
            date: Some(".rule-date".into()),
            ..rule()
        };
        let result = extract_article(HTML, &url(), Some(&rule)).unwrap();
        assert_eq!(result.title, "Rule title");
        assert_eq!(result.byline.as_deref(), Some("Rule author"));
        assert_eq!(result.published_time.as_deref(), Some("2026-10-20"));
        let blank = SiteRule {
            body,
            ..self::rule()
        };
        let result = extract_article(HTML, &url(), Some(&blank)).unwrap();
        assert_eq!(result.title, "Library metadata title");
        assert_eq!(result.byline.as_deref(), Some("Library author"));
        assert_eq!(result.published_time.as_deref(), Some("2026-10-19"));
    }
    let result = extract_article(
        "<title>Page fallback</title><div id=body><p>Text.</p></div>",
        &url(),
        Some(&SiteRule {
            body: Some("#body".into()),
            ..rule()
        }),
    )
    .unwrap();
    assert_eq!(result.title, "Page fallback");
}

#[test]
fn offline_report_has_three_plain_paragraphs_and_leaves_database_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rrss.db");
    let conn = rrss_core::db::open_at(&path).unwrap();
    let rule = SiteRule {
        body: Some("#rule-body".into()),
        strip: vec![".advert".into()],
        ..rule()
    };
    rrss_core::db::site_rules::upsert(&conn, &rule, true).unwrap();
    conn.execute_batch("PRAGMA query_only=ON;").unwrap();
    let before = (
        conn.total_changes(),
        rrss_core::db::site_rules::list(&conn).unwrap()[0].updated_at,
    );
    let files_before = std::fs::read_dir(dir.path()).unwrap().count();
    let report = test_site_rule(HTML, &url(), &rule).unwrap();
    assert_eq!(report.layer, Layer::Rule);
    assert_eq!(report.title, "Library metadata title");
    assert_eq!(report.author.as_deref(), Some("Library author"));
    assert_eq!(report.date.as_deref(), Some("2026-10-19"));
    assert_eq!(
        report.paragraphs,
        [
            "First selected paragraph.",
            "Second selected paragraph.",
            "Third selected paragraph."
        ]
    );
    assert_eq!(report.image_count, 1);
    assert_eq!(report.stripped_elements, 3);
    assert_eq!(
        report.text_chars,
        [
            "First selected paragraph.",
            "Second selected paragraph.",
            "Third selected paragraph.",
            "Fourth selected paragraph.",
            "Local image",
        ]
        .join("\n\n")
        .chars()
        .count()
    );
    assert_eq!(
        before,
        (
            conn.total_changes(),
            rrss_core::db::site_rules::list(&conn).unwrap()[0].updated_at
        )
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), files_before);
}

#[test]
fn embedded_rules_are_valid_sorted_unique_css_and_loaded_once() {
    let data = builtin_site_rules();
    assert!(std::ptr::eq(data, builtin_site_rules()));
    assert_eq!(data.source, "fivefilters/ftr-site-config");
    assert_eq!(data.commit.len(), 40);
    assert!(data.rules.len() > 1000);
    assert!(
        data.rules
            .windows(2)
            .all(|pair| pair[0].domain < pair[1].domain)
    );
    for rule in &data.rules {
        assert_eq!(
            rule.domain,
            rrss_core::db::site_rules::normalize_domain(&rule.domain)
        );
        for css in [&rule.body, &rule.title, &rule.author, &rule.date]
            .into_iter()
            .flatten()
            .chain(rule.strip.iter())
        {
            assert!(
                dom_query::Matcher::new(css).is_ok(),
                "{}: {css}",
                rule.domain
            );
        }
    }
}

#[test]
fn builtin_index_matches_linear_matching_for_representative_hosts() {
    let data = builtin_site_rules();
    for host in [
        "01net.com",
        "WWW.01NET.COM",    // exact and host normalization
        "blog.mozilla.org", // exact within a wildcard domain
        "news.about.com",
        "deep.news.about.com",
        "about.com",
        "www.about.com",
        "entry.blogs.nytimes.com",
        "news.nytimes.com", // nested wildcard suffixes
        "m.wikihow.com",
        "guide.m.wikihow.com",
        "guide.wikihow.com",
        ".about.com", // an empty leading label is not a subdomain
        "notabout.com",
        "unknown.invalid",
    ] {
        assert_eq!(
            data.matching_rule(host),
            match_site_rule(host, &data.rules),
            "{host}"
        );
    }
}
