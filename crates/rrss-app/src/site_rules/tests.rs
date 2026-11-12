use super::*;
use rrss_core::db as database;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};
fn rule() -> SiteRuleView {
    db::SiteRule {
        domain: "www.example.com".into(),
        body: Some("article".into()),
        strip: vec![".ad".into()],
        ..Default::default()
    }
    .into()
}
#[test]
fn page_reports_users_and_builtin_version() {
    let dir = tempfile::tempdir().unwrap();
    let conn = database::open_at(&dir.path().join("test.db")).unwrap();
    save_site_rule(&conn, rule(), true).unwrap();
    let page = site_rules_page(&conn).unwrap();
    assert_eq!(page.users[0].rule.domain, "example.com");
    assert!(page.users[0].updated_at > 0);
    assert_eq!(page.builtin_count, builtin_site_rules().rules.len() as i64);
    assert_eq!(page.commit, builtin_site_rules().commit);
    assert_eq!(page.source, builtin_site_rules().source);
}
#[test]
fn save_validates_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let conn = database::open_at(&dir.path().join("test.db")).unwrap();
    for raw in ["", "https://example.com", "a/b", "*example.com", "a b.com"] {
        let mut r = rule();
        r.domain = raw.into();
        assert!(save_site_rule(&conn, r, true).is_err());
    }
    let mut r = rule();
    r.body = Some("[".into());
    assert!(save_site_rule(&conn, r, true).is_err());
    assert!(db::list(&conn).unwrap().is_empty());
    save_site_rule(&conn, rule(), false).unwrap();
    assert!(!db::list(&conn).unwrap()[0].enabled);
}
#[test]
fn delete_normalizes_domain() {
    let dir = tempfile::tempdir().unwrap();
    let conn = database::open_at(&dir.path().join("test.db")).unwrap();
    save_site_rule(&conn, rule(), true).unwrap();
    delete_site_rule(&conn, "WWW.EXAMPLE.COM").unwrap();
    assert!(db::list(&conn).unwrap().is_empty());
    assert!(delete_site_rule(&conn, "https://example.com").is_err());
}
#[test]
fn enable_preserves_rule() {
    let dir = tempfile::tempdir().unwrap();
    let conn = database::open_at(&dir.path().join("test.db")).unwrap();
    save_site_rule(&conn, rule(), true).unwrap();
    set_site_rule_enabled(&conn, "www.example.com", false).unwrap();
    let rows = db::list(&conn).unwrap();
    assert!(!rows[0].enabled);
    assert_eq!(rows[0].rule.body.as_deref(), Some("article"));
    assert!(set_site_rule_enabled(&conn, "a/b", true).is_err());
}
#[tokio::test]
async fn test_fetch_is_read_only_and_errors_are_visible() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.db");
    let conn = database::open_at(&path).unwrap();
    rrss_core::network::save(&conn, &rrss_core::network::ProxyConfig::default()).unwrap();
    save_site_rule(&conn, rule(), true).unwrap();
    let before = conn.total_changes();
    let count = || {
        conn.query_row("SELECT count(*) FROM articles", [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    let rules_before = conn
        .query_row(
            "SELECT json_group_array(json_array(domain, enabled, updated_at)) FROM site_rules",
            [],
            |r| r.get::<_, String>(0),
        )
        .unwrap();
    let articles_before = count();
    let network = crate::network::NetworkClients::new(&path);
    let server = MockServer::start().await;
    let html = format!(
        "<html><head><title>Test title</title></head><body><article><h1>Test title</h1>{}<div class='ad'>Ad</div></article></body></html>",
        "<p>A meaningful article sentence with enough content for extraction.</p>".repeat(12)
    );
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(html))
        .mount(&server)
        .await;
    let report = test_site_rule(&network, &server.uri(), rule())
        .await
        .unwrap();
    assert_eq!(report.layer, WebLinkLayer::UserRule);
    assert_eq!(report.paragraphs.len(), 3);
    assert_eq!(report.stripped_elements, 1);
    assert_eq!(count(), articles_before);
    assert_eq!(conn.total_changes(), before);
    assert_eq!(
        conn.query_row(
            "SELECT json_group_array(json_array(domain, enabled, updated_at)) FROM site_rules",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        rules_before
    );
    let mut invalid = rule();
    invalid.strip = vec!["[".into()];
    assert!(
        test_site_rule(&network, &server.uri(), invalid)
            .await
            .unwrap_err()
            .message
            .contains("CSS")
    );
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    assert_eq!(
        test_site_rule(&network, &server.uri(), rule())
            .await
            .unwrap_err()
            .kind,
        FulltextErrorKind::Waf
    );
}

#[test]
fn script_only_rule_saves_and_legacy_editor_preserves_the_flag() {
    let dir = tempfile::tempdir().unwrap();
    let conn = database::open_at(&dir.path().join("test.db")).unwrap();
    let mut view: SiteRuleView = db::SiteRule {
        domain: "script.example".into(),
        javascript_disabled: true,
        ..Default::default()
    }
    .into();
    save_site_rule(&conn, view.clone(), true).unwrap();
    let page = site_rules_page(&conn).unwrap();
    assert_eq!(page.users[0].rule.javascript_disabled, Some(true));
    assert!(page.users[0].rule.body.is_none());
    view.javascript_disabled = None;
    view.body = Some("article".into());
    save_site_rule(&conn, view.clone(), true).unwrap();
    assert!(db::list(&conn).unwrap()[0].rule.javascript_disabled);
    view.javascript_disabled = Some(false);
    save_site_rule(&conn, view, true).unwrap();
    assert!(!db::list(&conn).unwrap()[0].rule.javascript_disabled);
}

#[test]
fn disabling_site_javascript_preserves_extraction_and_limits_policy_to_host() {
    let dir = tempfile::tempdir().unwrap();
    let conn = database::open_at(&dir.path().join("test.db")).unwrap();
    let mut wildcard = rule();
    wildcard.domain = "*.example.com".into();
    save_site_rule(&conn, wildcard.clone(), true).unwrap();
    disable_site_javascript(&conn, "https://www.news.example.com/story").unwrap();
    let exact = resolve_site_rule(&conn, "news.example.com")
        .unwrap()
        .rule
        .unwrap();
    assert!(exact.javascript_disabled);
    assert_eq!(exact.body, wildcard.body);
    assert_eq!(exact.strip, wildcard.strip);
    assert!(
        !resolve_site_rule(&conn, "other.example.com")
            .unwrap()
            .rule
            .unwrap()
            .javascript_disabled
    );
    // Repeated menu actions preserve the exact extraction rule.
    disable_site_javascript(&conn, "https://news.example.com/other").unwrap();
    assert_eq!(db::list(&conn).unwrap().len(), 2);
    for raw in ["file:///tmp/page", "javascript:alert(1)", "invalid"] {
        assert!(disable_site_javascript(&conn, raw).is_err());
    }
}

#[test]
fn disabling_site_javascript_keeps_builtin_extraction() {
    let dir = tempfile::tempdir().unwrap();
    let conn = database::open_at(&dir.path().join("test.db")).unwrap();
    let builtins = builtin_site_rules();
    let builtin = builtins
        .rules
        .iter()
        .find(|rule| !rule.domain.starts_with("*.") && rule.body.is_some())
        .unwrap();
    disable_site_javascript(&conn, &format!("https://{}/story", builtin.domain)).unwrap();
    let saved = db::list(&conn).unwrap().remove(0).rule;
    assert!(saved.javascript_disabled);
    assert_eq!(saved.body, builtin.body);
    assert_eq!(saved.strip, builtin.strip);
}

#[test]
fn effective_rule_uses_indexed_builtins_and_enabled_user_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let conn = database::open_at(&dir.path().join("rules.db")).unwrap();
    let host = "blog.mozilla.org";
    let builtin = builtin_site_rules().matching_rule(host).unwrap();
    assert_eq!(builtin.domain, host);
    let resolved = resolve_site_rule(&conn, host).unwrap();
    assert_eq!(resolved.rule.as_ref(), Some(builtin));
    assert!(!resolved.user_rule);
    // A wider user rule replaces all fields, even when its selectors are empty.
    let user = db::SiteRule {
        domain: "*.mozilla.org".into(),
        ..Default::default()
    };
    db::upsert(&conn, &user, true).unwrap();
    let resolved = resolve_site_rule(&conn, host).unwrap();
    assert_eq!(resolved.rule, Some(user));
    assert!(resolved.user_rule);
    db::set_enabled(&conn, "*.MOZILLA.ORG", false).unwrap();
    assert!(!db::list(&conn).unwrap()[0].enabled);
    let resolved = resolve_site_rule(&conn, host).unwrap();
    assert_eq!(resolved.rule.as_ref(), Some(builtin));
    assert!(!resolved.user_rule);
    let unknown = resolve_site_rule(&conn, "unknown.invalid").unwrap();
    assert!(unknown.rule.is_none());
    assert!(!unknown.user_rule);
}
