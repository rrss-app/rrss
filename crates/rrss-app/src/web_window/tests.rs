use super::*;

#[test]
fn title_uses_page_host() {
    assert_eq!(
        title(&Url::parse("https://example.com/a").unwrap()),
        "example.com · 原网页 — RRSS"
    );
}

#[test]
fn session_start_clear_exit_and_route_changes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("web-sessions");
    std::fs::create_dir_all(root.join("abandoned")).unwrap();
    std::fs::write(root.join("abandoned/cookies"), b"synthetic").unwrap();
    std::fs::write(root.join("unrelated-file"), b"keep").unwrap();
    let mut session = WebSession::start(dir.path(), "run-one").unwrap();
    assert!(!root.join("abandoned").exists());
    assert!(root.join("unrelated-file").exists());
    assert_eq!(
        abandoned_directories(&root).unwrap(),
        [session.directory.clone()]
    );
    assert_eq!(session.next_label(&WebviewRoute::Direct).unwrap(), "web-1");
    session.opened(WebviewRoute::Direct);
    assert_eq!(session.next_label(&WebviewRoute::Direct).unwrap(), "web-2");
    let proxy = WebviewRoute::Proxy(Url::parse("http://proxy.example:8080").unwrap());
    assert!(session.next_label(&proxy).is_err());
    let old = session.directory.clone();
    std::fs::write(old.join("cookies"), b"synthetic").unwrap();
    session.clear("run-two").unwrap();
    assert!(!old.exists());
    assert!(session.directory.exists());
    assert_eq!(session.next_label(&proxy).unwrap(), "web-3");
    assert!(fresh_directory(&root, "../escape").is_err());
    session.cleanup().unwrap();
    assert!(!session.directory.exists());
    session.cleanup().unwrap();
}

#[test]
fn prepare_http_only_and_global_site_javascript_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open_at(&dir.path().join("test.db")).unwrap();
    let env = ProxyEnvironment::from_lookup(|_| None);
    let native = SystemProxySnapshot::default();
    for raw in [
        "about:blank",
        "file:///tmp/page",
        "data:text/html,hello",
        "invalid",
    ] {
        assert!(prepare(&conn, raw, &env, &native, Os::Linux).is_err());
    }
    let url = "https://news.example.com/article";
    assert!(
        prepare(&conn, url, &env, &native, Os::Linux)
            .unwrap()
            .javascript
    );
    db::site_rules::upsert(
        &conn,
        &db::site_rules::SiteRule {
            domain: "*.example.com".into(),
            javascript_disabled: true,
            ..Default::default()
        },
        true,
    )
    .unwrap();
    assert!(
        !prepare(&conn, url, &env, &native, Os::Linux)
            .unwrap()
            .javascript
    );
    assert!(
        prepare(&conn, "https://example.com", &env, &native, Os::Linux)
            .unwrap()
            .javascript
    );
    db::site_rules::set_enabled(&conn, "*.example.com", false).unwrap();
    assert!(
        prepare(&conn, url, &env, &native, Os::Linux)
            .unwrap()
            .javascript
    );
    crate::settings_schema::set(&conn, "web_javascript", "0").unwrap();
    assert!(
        !prepare(&conn, url, &env, &native, Os::Linux)
            .unwrap()
            .javascript
    );
    assert!(crate::settings_schema::set(&conn, "web_javascript", "invalid").is_err());
}

#[test]
fn failed_abandoned_cleanup_keeps_startup_usable_and_cleans_other_directories() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("web-sessions");
    std::fs::create_dir_all(root.join("blocked")).unwrap();
    std::fs::create_dir_all(root.join("removable")).unwrap();
    let session = WebSession::start_with_cleanup(dir.path(), "new-run", |path| {
        if path.file_name().unwrap() == "blocked" {
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        } else {
            platform::remove_web_session(path)
        }
    })
    .unwrap();
    assert!(root.join("blocked").exists());
    assert!(!root.join("removable").exists());
    assert!(session.directory.exists());
    assert_ne!(session.directory, root.join("blocked"));
    // Retained directories are retried on the next run.
    let next = WebSession::initialize(dir.path(), "next-run").unwrap();
    assert!(!root.join("blocked").exists());
    assert!(next.directory.exists());
}

#[test]
fn new_session_directory_failure_is_a_displayable_feature_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("web-sessions"), b"synthetic blocker").unwrap();
    let error = WebSession::initialize(dir.path(), "new-run").err().unwrap();
    assert!(error.contains("无法创建原网页数据目录"));
    assert!(error.contains("阅读视图或系统浏览器"));
    assert!(!error.contains(dir.path().to_str().unwrap()));
    // Initialization reports the failure without unwinding or terminating the host.
    std::fs::write(dir.path().join("main-still-usable"), b"synthetic").unwrap();
}
