//! 生命周期与锁在独立进程验证，不污染单测的全局 tracing / panic hook。
use rrss_app::log::{self, Mode};
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn child(dir: &Path, mode: &str) -> Output {
    Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "logging_child", "--nocapture"])
        .env("RRSS_LOG_FIXTURE_DIR", dir)
        .env("RRSS_LOG_FIXTURE_MODE", mode)
        .env(
            "RRSS_LOG",
            match mode {
                "volume" => "warn,rrss=info",
                "chain" => "warn,rrss=debug",
                _ => "off",
            },
        )
        .env_remove("RUST_LOG")
        .output()
        .unwrap()
}

#[test]
#[ignore = "only invoked by the isolated parent"]
fn logging_child() {
    let dir = std::path::PathBuf::from(
        std::env::var_os("RRSS_LOG_FIXTURE_DIR").expect("fixture directory"),
    );
    let mode = std::env::var("RRSS_LOG_FIXTURE_MODE").unwrap();
    log::init(if mode == "tool" {
        Mode::Tool
    } else {
        Mode::Desktop {
            log_dir: dir.clone(),
        }
    });
    match mode.as_str() {
        "panic" => {
            panic!("fixture panic sk-canary-panic\nhttps://u:p@example.test/?token=canary-query")
        }
        "crash" => std::process::exit(0),
        "locked" => assert!(log::current().unwrap().unavailable_reason().is_some()),
        "volume" => {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap()
                .block_on(healthy_feed_round(&dir));
            log::end_session();
        }
        "chain" => {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .unwrap()
                .block_on(feed_chain_round(&dir));
            log::end_session();
        }
        _ => log::end_session(),
    }
}

async fn healthy_feed_round(dir: &Path) {
    use rrss_core::{ai_store, db};
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path_regex},
    };
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path_regex("/unchanged/.*"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    Mock::given(method("GET")).and(path_regex("/updated/.*"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<rss version='2.0'><channel><title>Fixture</title><link>https://example.test</link><description>fixture</description></channel></rss>"))
        .mount(&server).await;
    let db_path = dir.join("fixture.db");
    let conn = db::open_at(&db_path).unwrap();
    db::set_setting(&conn, rrss_core::feed_icon::SETTING_KEY, "0").unwrap();
    ai_store::set_daily_budget_tokens(&conn, 1).unwrap();
    ai_store::add_usage(&conn, &ai_store::local_day(), 1, 0).unwrap();
    let tx = db::write_transaction(&conn).unwrap();
    for id in 0..1200 {
        let route = if id % 2 == 0 { "unchanged" } else { "updated" };
        tx.execute(
            "INSERT INTO feeds(url,created_at) VALUES(?1,0)",
            [format!("{}/{route}/{id}", server.uri())],
        )
        .unwrap();
    }
    tx.commit().unwrap();
    let scheduler = rrss_app::scheduler::Scheduler::start(
        db_path.clone(),
        rrss_app::network::NetworkClients::new(&db_path),
        |_| {},
    );
    let operation = scheduler.start_refresh().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let snapshot = scheduler.snapshot();
            let status = snapshot
                .operations
                .iter()
                .find(|item| item.id == operation.operation_id)
                .unwrap();
            if status.completed == 1200 {
                assert_eq!((status.failed, status.skipped), (0, 0));
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(scheduler.shutdown().await);
    assert_eq!(server.received_requests().await.unwrap().len(), 1200);
}

#[test]
fn scheduler_round_with_1200_healthy_sources_keeps_default_logs_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let result = child(dir.path(), "volume");
    assert!(
        result.status.success(),
        "fixture failed: {}",
        String::from_utf8_lossy(&result.stdout)
    );
    let text = fs::read_to_string(dir.path().join("rrss.log")).unwrap();
    assert_eq!(
        text.matches("manual feed refresh completed").count(),
        1,
        "{text}"
    );
    assert!(
        text.lines().count() <= 10,
        "unexpected log volume: {}",
        text.lines().count()
    );
}

/// 三个源各走一条路径：304、入库、服务端 500。
async fn feed_chain_round(dir: &Path) {
    use rrss_core::{ai_store, db};
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/unchanged"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    Mock::given(method("GET")).and(path("/updated"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<rss version='2.0'><channel><title>Fixture</title><link>https://example.test</link><description>fixture</description><item><guid>chain-1</guid><title>Item</title><link>https://example.test/a</link><description>body</description></item></channel></rss>"))
        .mount(&server).await;
    Mock::given(method("GET"))
        .and(path("/failing"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let db_path = dir.join("fixture.db");
    let conn = db::open_at(&db_path).unwrap();
    db::set_setting(&conn, rrss_core::feed_icon::SETTING_KEY, "0").unwrap();
    ai_store::set_daily_budget_tokens(&conn, 1).unwrap();
    ai_store::add_usage(&conn, &ai_store::local_day(), 1, 0).unwrap();
    for (id, route) in [(1, "unchanged"), (2, "updated"), (3, "failing")] {
        conn.execute(
            "INSERT INTO feeds(id,url,created_at) VALUES(?1,?2,0)",
            rusqlite::params![id, format!("{}/{route}", server.uri())],
        )
        .unwrap();
    }
    let scheduler = rrss_app::scheduler::Scheduler::start(
        db_path.clone(),
        rrss_app::network::NetworkClients::new(&db_path),
        |_| {},
    );
    let operation = scheduler.start_refresh().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let done = scheduler
                .snapshot()
                .operations
                .iter()
                .any(|item| item.id == operation.operation_id && item.done);
            if done {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(scheduler.shutdown().await);
}

#[test]
fn each_feed_round_can_be_followed_by_its_feed_id() {
    let dir = tempfile::tempdir().unwrap();
    let result = child(dir.path(), "chain");
    assert!(
        result.status.success(),
        "fixture failed: {}",
        String::from_utf8_lossy(&result.stdout)
    );
    let text = fs::read_to_string(dir.path().join("rrss.log")).unwrap();
    let ids = |line: &str| {
        line.split_whitespace()
            .filter(|token| token.starts_with("feed_id="))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    for line in text.lines() {
        assert!(ids(line).len() <= 1, "one feed id per line: {line}");
    }
    let chain = |id: u32| {
        text.lines()
            .filter(|line| ids(line) == [format!("feed_id={id}")])
            .collect::<Vec<_>>()
            .join("\n")
    };
    for (id, steps) in [
        (1, &["feed download finished", "not modified"][..]),
        (
            2,
            &["feed download finished", "feed updated", "ingest completed"],
        ),
        (
            3,
            &[
                "feed download finished",
                "feed fetch failing",
                "error_kind=http",
            ],
        ),
    ] {
        let chain = chain(id);
        for step in steps {
            assert!(chain.contains(step), "feed {id} misses {step}:\n{chain}");
        }
    }
    // 整次刷新的汇总不属于某一个源。
    let summary = text
        .lines()
        .find(|line| line.contains("manual feed refresh completed"))
        .unwrap();
    assert!(
        ids(summary).is_empty() && !summary.contains(" feed: "),
        "{summary}"
    );
}

#[test]
fn panic_crash_restart_and_tool_processes() {
    let dir = tempfile::tempdir().unwrap();
    let tool = dir.path().join("tool");
    assert!(child(&tool, "tool").status.success());
    assert!(!tool.exists());
    assert!(child(dir.path(), "crash").status.success());
    assert!(child(dir.path(), "normal").status.success());
    let text = fs::read_to_string(dir.path().join("rrss.log")).unwrap();
    assert!(text.contains("previous session did not exit cleanly"));
    assert!(text.contains("session ended"));
    assert!(!child(dir.path(), "panic").status.success());
    let text = fs::read_to_string(dir.path().join("rrss.log")).unwrap();
    assert!(text.contains("rrss::panic"));
    assert!(!text.contains("sk-canary-panic") && !text.contains("canary-query"));
}

#[test]
fn second_process_cannot_write_or_rotate_owner_files() {
    let dir = tempfile::tempdir().unwrap();
    let (_subscriber, handle) = log::subscriber(
        Mode::Desktop {
            log_dir: dir.path().into(),
        },
        "",
    );
    handle.start();
    let before = fs::read(dir.path().join("rrss.log")).unwrap();
    assert!(child(dir.path(), "locked").status.success());
    assert_eq!(before, fs::read(dir.path().join("rrss.log")).unwrap());
    handle.end();
}
