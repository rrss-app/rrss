use super::*;

fn rule(domain: &str) -> SiteRule {
    SiteRule {
        domain: domain.into(),
        ..SiteRule::default()
    }
}

#[test]
fn exact_beats_wildcards_and_longest_suffix_wins() {
    let rules = [
        rule("*.example.com"),
        rule("news.eu.example.com"),
        rule("*.eu.example.com"),
    ];
    assert_eq!(
        match_site_rule("news.eu.example.com", &rules),
        Some(&rules[1])
    );
    assert_eq!(
        match_site_rule("other.eu.example.com", &rules),
        Some(&rules[2])
    );
    assert_eq!(
        match_site_rule("other.example.com", &rules),
        Some(&rules[0])
    );
    assert_eq!(match_site_rule("unrelated.test", &rules), None);
}

#[test]
fn wildcard_excludes_bare_domain_and_requires_label_boundary() {
    let rules = [rule("*.example.com")];
    for host in ["example.com", "www.example.com", "evilexample.com"] {
        assert_eq!(match_site_rule(host, &rules), None, "{host}");
    }
    assert_eq!(match_site_rule("a.b.example.com", &rules), Some(&rules[0]));
}

#[test]
fn normalizes_www_case_and_database_writes() {
    let dir = tempfile::tempdir().unwrap();
    let conn = crate::db::open_at(&dir.path().join("rrss.db")).unwrap();
    let mut input = rule("WWW.Example.COM");
    input.strip = vec![".ad".into()];
    upsert(&conn, &input, true).unwrap();
    let first = list(&conn).unwrap().remove(0);
    assert_eq!(first.rule.domain, "example.com");
    assert_eq!(first.rule.strip, [".ad"]);
    assert!(first.updated_at > 1_000_000_000_000);
    assert_eq!(
        match_site_rule("www.EXAMPLE.com", [&first.rule]),
        Some(&first.rule)
    );
    input.domain = "example.com".into();
    input.strip.clear();
    input.body = Some("article".into());
    upsert(&conn, &input, true).unwrap();
    let rows = list(&conn).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, first.id);
    assert_eq!(rows[0].rule, input);
    let strip: Option<String> = conn
        .query_row("SELECT strip FROM site_rules", [], |r| r.get(0))
        .unwrap();
    assert!(strip.is_none());
    delete(&conn, "WWW.EXAMPLE.COM").unwrap();
    assert!(list(&conn).unwrap().is_empty());
}
