use super::*;
use std::fs;

#[test]
fn filters_reload_without_losing_session_markers() {
    let dir = tempfile::tempdir().unwrap();
    let (subscriber, handle) = super::subscriber(
        Mode::Desktop {
            log_dir: dir.path().into(),
        },
        "",
    );
    tracing::subscriber::with_default(subscriber, || {
        handle.start();
        tracing::debug!("hidden detail");
        tracing::info!("visible summary");
        tracing::warn!(
            error = r#"{"Cookie":"canary-cookie", "api_key":"canary-json-key"}"#,
            "fixture credentials"
        );
        handle.set_level(Level::Detailed).unwrap();
        tracing::debug!("enabled detail");
        handle.set_level(Level::Default).unwrap();
        tracing::debug!("hidden again");
        handle.end();
    });
    let text = fs::read_to_string(dir.path().join("rrss.log")).unwrap();
    assert!(!text.contains("hidden detail") && !text.contains("hidden again"));
    assert!(text.contains("visible summary") && text.contains("enabled detail"));
    assert!(text.contains("session started") && text.contains("session ended"));
    assert!(!text.contains('\x1b'));
    assert!(!text.contains("canary-cookie") && !text.contains("canary-json-key"));
    drop(handle);
    let (subscriber, handle) = super::subscriber(
        Mode::Desktop {
            log_dir: dir.path().into(),
        },
        "off",
    );
    tracing::subscriber::with_default(subscriber, || {
        handle.start();
        handle.end();
    });
    assert!(handle.status().unwrap().previous_unclean.is_none());
}

#[test]
fn unknown_error_chains_keep_their_causes() {
    let failure =
        anyhow::Error::new(std::io::Error::other("disk is read-only")).context("open database");
    let text = error(failure.as_ref());
    assert_eq!(text, "open database: disk is read-only");
}

#[test]
fn unavailable_directory_and_tool_mode_are_usable() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("not-a-directory");
    fs::write(&file, "fixture").unwrap();
    let (subscriber, handle) = super::subscriber(Mode::Desktop { log_dir: file }, "");
    tracing::subscriber::with_default(subscriber, || {
        handle.start();
        tracing::warn!(error_kind = "fixture", "still usable");
    });
    assert_eq!(
        handle.unavailable_reason(),
        Some(crate::views::LogUnavailableReason::NotWritable)
    );
    assert!(handle.set_level(Level::Detailed).is_err());
    let (_, tool) = super::subscriber(Mode::Tool, "");
    assert!(tool.status().is_none());
    assert_eq!(
        extra_filter(Some(" ".into()), Some("debug".into())),
        "debug"
    );
    assert_eq!(
        extra_filter(Some("warn".into()), Some("debug".into())),
        "warn"
    );
}

#[tokio::test]
async fn failure_paths_do_not_write_credentials_queries_or_content() {
    use tracing::instrument::WithSubscriber;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_string("CANARY_MODEL_BODY sk-canary-api Bearer canary-token"),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string("CANARY_MODEL_BODY"))
        .mount(&server)
        .await;
    let (subscriber, handle) = super::subscriber(
        Mode::Desktop {
            log_dir: dir.path().into(),
        },
        "rrss=debug",
    );
    handle.start();
    async {
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let feed = rrss_core::ingest::FetchRequest {
            url: "http://private-user:private-pass@127.0.0.1:0/feed?token=canary-query".into(),
            etag: None,
            last_modified: None,
        };
        assert!(rrss_core::ingest::fetch_http(&client, feed).await.is_err());
        let failure = rrss_ai::embed(
            &client,
            &rrss_ai::EmbedRequest {
                base_url: server.uri(),
                api_key: Some("sk-canary-api".into()),
                model: "fixture".into(),
                input: "CANARY_INPUT".into(),
            },
        )
        .await
        .unwrap_err();
        tracing::warn!(error = %error(&failure), "model command failed");
        let failure = rrss_ai::complete(
            &client,
            &rrss_ai::CompleteRequest {
                base_url: server.uri(),
                api_key: Some("sk-canary-api".into()),
                model: "fixture".into(),
                system: "CANARY_SYSTEM".into(),
                user: "CANARY_INPUT".into(),
                json_schema: None,
                protocol: rrss_ai::Protocol::Chat,
            },
        )
        .await
        .unwrap_err();
        let failure = anyhow::Error::from(failure).context("CANARY_MODEL_BODY");
        tracing::warn!(error = %error(failure.as_ref()), "complete command failed");
        let shared = crate::scheduler::digest_jobs::Failure {
            reason: "model_error",
            message: "CANARY_MODEL_BODY".into(),
        };
        tracing::warn!(error = %error(&shared), "digest command failed");
    }
    .with_subscriber(subscriber)
    .await;
    handle.end();
    let text = fs::read_to_string(dir.path().join("rrss.log")).unwrap();
    assert!(text.contains("embedding http response error"));
    assert!(text.contains("complete command failed"));
    for secret in [
        "private-user",
        "private-pass",
        "canary-query",
        "sk-canary-api",
        "canary-token",
        "CANARY_MODEL_BODY",
        "CANARY_INPUT",
        "CANARY_SYSTEM",
    ] {
        assert!(!text.contains(secret), "leaked {secret}");
    }
}

#[test]
fn status_level_and_export_follow_the_lock_owner() {
    use crate::views::LogUnavailableReason;
    use std::io::Read;
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let (subscriber, handle) = super::subscriber(
        Mode::Desktop {
            log_dir: logs.clone(),
        },
        "",
    );
    // 级别切换依赖订阅者仍在，整个用例都持有它。
    let dispatch = tracing::Dispatch::new(subscriber);
    tracing::dispatcher::with_default(&dispatch, || handle.start());
    // 旧版本留下的历史文件：导出时要再脱敏一遍。
    fs::write(
        logs.join("rrss.1.log"),
        "2026-11-05T00:00:00.000+00:00  INFO rrss: old token=canary-old /home/someone/private\n",
    )
    .unwrap();
    let view = handle.status_view().unwrap();
    assert!(view.unavailable.is_none() && view.can_clear && !view.detailed);
    assert!(view.total_bytes > 0 && view.file.ends_with("rrss.log"));
    handle.set_level(Level::Detailed).unwrap();
    assert!(handle.status_view().unwrap().detailed);

    let dest = dir.path().join("diagnostics.zip");
    handle
        .export(&dest, &serde_json::json!({ "schema": 1 }))
        .unwrap();
    let mut archive = zip::ZipArchive::new(fs::File::open(&dest).unwrap()).unwrap();
    let mut names: Vec<_> = archive.file_names().map(str::to_owned).collect();
    names.sort();
    assert_eq!(names, ["diagnostics.json", "rrss.1.log", "rrss.log"]);
    let mut old = String::new();
    archive
        .by_name("rrss.1.log")
        .unwrap()
        .read_to_string(&mut old)
        .unwrap();
    assert!(
        !old.contains("canary-old") && old.contains("~/private"),
        "{old}"
    );

    let (_, other) = super::subscriber(Mode::Desktop { log_dir: logs }, "");
    let view = other.status_view().unwrap();
    assert_eq!(view.unavailable, Some(LogUnavailableReason::OtherInstance));
    assert!(!view.can_clear && view.total_bytes > 0);
    assert!(other.clear().is_err() && other.set_level(Level::Detailed).is_err());
    handle.end();
}
