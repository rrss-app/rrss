//! L2 应用集成：脚本化假 Pi（`testdata/pi/scenario_rpc.py`）驱动完整的一轮对话，不依赖 Tauri。
//! 每个用例用一次性库和数据目录；场景由提示词里的标记选择。
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rrss_app::pi::{ChatErrorKind, ChatEvent, ChatRuntime, ChatSink, SupervisorConfig};
use rrss_app::views::{ModelChoice, SendMessageArgs};
use rrss_core::ai_store::{self, NewProvider, PI_BINARY_KEY};
use rrss_core::threads::{self, MessageRole};
use rrss_core::{db, pi_store};
use tokio::sync::Notify;

#[derive(Default)]
struct Collect {
    events: Mutex<Vec<ChatEvent>>,
    notify: Notify,
}

impl ChatSink for Collect {
    fn emit(&self, event: ChatEvent) {
        if let ChatEvent::Start(thread_id) | ChatEvent::Done(thread_id) = &event {
            tracing::warn!(
                thread_id = rrss_core::logsafe::unless_in_span("thread_id", *thread_id),
                error_kind = "fixture",
                "turn fixture boundary"
            );
        }
        self.events.lock().unwrap().push(event);
        self.notify.notify_waiters();
    }
}

impl Collect {
    fn snapshot(&self) -> Vec<ChatEvent> {
        self.events.lock().unwrap().clone()
    }

    fn clear(&self) {
        self.events.lock().unwrap().clear();
    }

    async fn until(&self, what: &str, pred: impl Fn(&[ChatEvent]) -> bool) -> Vec<ChatEvent> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let notified = self.notify.notified();
            let events = self.snapshot();
            if pred(&events) {
                return events;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                panic!("timed out waiting for {what}; got {events:#?}");
            }
        }
    }

    async fn done(&self) -> Vec<ChatEvent> {
        self.until("chat done", |e| {
            e.iter().any(|e| matches!(e, ChatEvent::Done(_)))
        })
        .await
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    db: PathBuf,
    data: PathBuf,
    chat: Arc<ChatRuntime>,
    sink: Arc<Collect>,
    provider: i64,
}

/// 对话模型怎么配。
#[derive(Clone, Copy, PartialEq)]
enum Model {
    /// 一个连接与模型 test-model，它是对话默认。
    Bound,
    /// 一个连接与模型 test-model，但还没选对话默认。
    Unbound,
    /// 没有任何连接。
    None,
}

fn setup(idle_secs: i64) -> Harness {
    setup_with(idle_secs, Model::Bound)
}

fn setup_with(idle_secs: i64, model: Model) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().to_path_buf();
    let script = data.join("fake-pi");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pi/scenario_rpc.py"),
        &script,
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let db = data.join("rrss.db");
    let conn = db::open_at(&db).unwrap();
    db::set_setting(&conn, PI_BINARY_KEY, script.to_str().unwrap()).unwrap();
    pi_store::set_pi_auto_download(&conn, false).unwrap();
    pi_store::set_pi_auto_update(&conn, false).unwrap();
    pi_store::set_idle_timeout_secs(&conn, idle_secs).unwrap();
    let provider = if model == Model::None {
        0
    } else {
        ai_store::insert_provider(
            &conn,
            &NewProvider {
                name: "本机测试".into(),
                base_url: "http://127.0.0.1:9/v1".into(),
                api_key: Some("sk-test-key".into()),
                protocol: "chat".into(),
            },
        )
        .unwrap()
    };
    if model != Model::None {
        add_model(&conn, provider, "test-model");
    }
    if model == Model::Bound {
        ai_store::set_task_binding(
            &conn,
            ai_store::TASK_CHAT,
            Some(provider),
            Some("test-model"),
        )
        .unwrap();
    }
    drop(conn);
    let sink = Arc::new(Collect::default());
    let chat = ChatRuntime::start(
        SupervisorConfig {
            db_path: db.clone(),
            data_dir: data.clone(),
            tool_path: PathBuf::from("unused-tool"),
            idle_tick: Duration::from_millis(100),
        },
        rrss_app::network::NetworkClients::new(&db),
        sink.clone(),
        false,
    );
    Harness {
        _dir: dir,
        db,
        data,
        chat,
        sink,
        provider,
    }
}

/// 加一个模型并打开「用于对话」。
fn add_model(conn: &rusqlite::Connection, provider: i64, model: &str) {
    let r = rrss_core::model_pool::ModelRef::new(provider, model);
    rrss_core::model_pool::update_pool(conn, std::slice::from_ref(&r), &[], None).unwrap();
    set_entry(
        conn,
        &r,
        rrss_core::model_pool::EntryPatch {
            chat: Some(true),
            ..Default::default()
        },
    );
}

fn set_entry(
    conn: &rusqlite::Connection,
    r: &rrss_core::model_pool::ModelRef,
    patch: rrss_core::model_pool::EntryPatch,
) {
    rrss_core::model_pool::update_entry(conn, r, &patch).unwrap();
}

fn args(thread_id: i64, text: &str) -> SendMessageArgs {
    SendMessageArgs {
        thread_id,
        text: text.into(),
        citations: vec![],
        digest_id: None,
        model: None,
        thinking: None,
    }
}

impl Harness {
    fn messages(&self, thread: i64) -> Vec<(String, String)> {
        let conn = db::open_at(&self.db).unwrap();
        threads::list_messages(&conn, thread)
            .unwrap()
            .into_iter()
            .map(|m| {
                let role = if matches!(m.role, MessageRole::Assistant) {
                    "assistant"
                } else {
                    "user"
                };
                (role.to_string(), m.content)
            })
            .collect()
    }

    fn pi_log(&self) -> String {
        std::fs::read_to_string(self.data.join("pi/fake-pi.log")).unwrap_or_default()
    }

    /// 每次启动记下的参数与环境变量，按启动顺序。
    fn starts(&self) -> Vec<serde_json::Value> {
        self.pi_log()
            .lines()
            .filter_map(|l| l.strip_prefix("START "))
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// 换对话默认模型。
    fn set_model(&self, model: &str) {
        let conn = db::open_at(&self.db).unwrap();
        add_model(&conn, self.provider, model);
        ai_store::set_task_binding(&conn, ai_store::TASK_CHAT, Some(self.provider), Some(model))
            .unwrap();
    }

    fn set_thinking(&self, level: &str) {
        let conn = db::open_at(&self.db).unwrap();
        set_entry(
            &conn,
            &rrss_core::model_pool::ModelRef::new(self.provider, "test-model"),
            rrss_core::model_pool::EntryPatch {
                thinking: Some(level.into()),
                ..Default::default()
            },
        );
    }
}

fn args_of(start: &serde_json::Value) -> Vec<String> {
    start["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_string())
        .collect()
}

fn arg_after(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn errors(events: &[ChatEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            ChatEvent::Error(err) => Some((err.kind.clone(), err.message.clone())),
            _ => None,
        })
        .collect()
}

/// 过程条里的文字状态（压缩、重试、提示）；等待过程的步骤另看 [`steps`]。
fn statuses(events: &[ChatEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            ChatEvent::Status(s) if s.step.is_none() => Some(s.text.clone()),
            _ => None,
        })
        .collect()
}

/// 等待过程的步骤：（文案，是否开始）。
fn steps(events: &[ChatEvent]) -> Vec<(String, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            ChatEvent::Status(s) => s.step.as_ref().map(|step| (step.label.clone(), step.start)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn turn_events_keep_their_thread_from_send_through_settlement() {
    use tracing::instrument::WithSubscriber;

    let _peer = tracing::Dispatch::new(tracing_subscriber::Registry::default());

    let dir = tempfile::tempdir().unwrap();
    let (subscriber, log) = rrss_app::log::subscriber(
        rrss_app::log::Mode::Desktop {
            log_dir: dir.path().into(),
        },
        "rrss=warn",
    );
    let thread_id = async {
        let h = setup(0);
        let thread_id = h.chat.send(args(0, "turn-content-canary")).await.unwrap();
        h.sink.done().await;
        h.sink.clear();
        h.chat
            .send(args(thread_id, "next-turn-content-canary"))
            .await
            .unwrap();
        h.sink.done().await;
        h.chat.stop_engine().await;
        tracing::warn!(error_kind = "fixture", "outside turn fixture");
        thread_id
    }
    .with_subscriber(subscriber)
    .await;
    log.end();
    let text = std::fs::read_to_string(dir.path().join("rrss.log")).unwrap();
    let boundaries: Vec<_> = text
        .lines()
        .filter(|line| line.contains("turn fixture boundary"))
        .collect();
    assert_eq!(boundaries.len(), 4, "{text}");
    for line in boundaries {
        assert!(line.contains(" pi_turn: "), "{line}");
        assert_eq!(
            line.matches(&format!("thread_id={thread_id}")).count(),
            1,
            "{line}"
        );
    }
    let outside = text
        .lines()
        .find(|line| line.contains("outside turn fixture"))
        .unwrap();
    assert!(
        !outside.contains("pi_turn") && !outside.contains("thread_id="),
        "{outside}"
    );
    assert!(!text.contains("turn-content-canary"));
}

#[tokio::test]
async fn deletion_rejects_generation_cascades_and_allows_next_session() {
    let h = setup(0);
    let other = h.chat.send(args(0, "保留的对话")).await.unwrap();
    h.sink.done().await;
    h.sink.clear();
    let id = h.chat.send(args(0, "[[slow]]")).await.unwrap();
    let err = h.chat.lock_thread_deletion(id).await.unwrap_err();
    assert_eq!(err.message, "这条对话正在生成，先停止再删除");
    let conn = db::open_at(&h.db).unwrap();
    let file = threads::get_thread(&conn, id)
        .unwrap()
        .pi_session_file
        .unwrap();
    assert!(Path::new(&file).exists());
    h.chat.abort().await.unwrap();
    h.sink.done().await;
    {
        let _guard = h.chat.lock_thread_deletion(id).await.unwrap();
        rrss_app::actions::delete_thread(&conn, id, &h.data).unwrap();
    }
    assert!(threads::get_thread(&conn, id).is_err());
    assert!(!Path::new(&file).exists());
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE thread_id=?1",
            [id],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    h.sink.clear();
    h.chat.send(args(other, "接着聊")).await.unwrap();
    assert!(errors(&h.sink.done().await).is_empty());
    assert_eq!(h.messages(other).len(), 4);
    h.sink.clear();
    let fresh = h.chat.send(args(0, "新话题")).await.unwrap();
    assert!(errors(&h.sink.done().await).is_empty());
    assert_eq!(h.messages(fresh).len(), 2);
    h.chat.stop_engine().await;
}

#[tokio::test]
async fn plain_turn_streams_persists_and_reports_tools() {
    let h = setup(0);
    let thread = h.chat.send(args(0, "[[tools]] 有什么新闻")).await.unwrap();
    let events = h.sink.done().await;
    // 冷启动：线程归属之前先有「启动对话引擎」这一步，线程建好后结束；之后才是增量。
    let first = events
        .iter()
        .find(|e| !matches!(e, ChatEvent::Status(s) if s.step.is_some()));
    assert!(matches!(first, Some(ChatEvent::Start(t)) if *t == thread));
    assert_eq!(
        steps(&events)[..2],
        [
            ("启动对话引擎".to_string(), true),
            ("启动对话引擎".to_string(), false)
        ]
    );
    assert!(errors(&events).is_empty(), "{events:#?}");
    assert!(steps(&events).contains(&("查阅本地文章".to_string(), true)));
    assert!(
        !statuses(&events).iter().any(|s| s.contains("查阅")),
        "tool progress stays out of the activity line"
    );
    assert_eq!(
        statuses(&events).last().map(String::as_str),
        Some(""),
        "status is cleared on settle"
    );
    assert_eq!(
        h.messages(thread),
        vec![
            ("user".into(), "[[tools]] 有什么新闻".into()),
            ("assistant".into(), "查到了两篇。".into())
        ]
    );
    let conn = db::open_at(&h.db).unwrap();
    let file = threads::get_thread(&conn, thread)
        .unwrap()
        .pi_session_file
        .unwrap();
    assert!(Path::new(&file).is_file());

    // 同一线程第二轮沿用当前会话，不再切换。
    h.sink.clear();
    h.chat.send(args(thread, "继续")).await.unwrap();
    h.sink.done().await;
    assert!(!h.pi_log().contains("switch_session"), "{}", h.pi_log());
}

#[tokio::test]
async fn model_errors_carry_kind_and_are_redacted() {
    let h = setup(0);
    let thread = h.chat.send(args(0, "[[model-error]]")).await.unwrap();
    let errs = errors(&h.sink.done().await);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].0, "model");
    // 消息就是脱敏后的上游原因，不再叠「模型返回错误」前缀（类别已说明）。
    assert!(
        errs[0].1.starts_with("401") && errs[0].1.contains("***") && !errs[0].1.contains("abcdef"),
        "{errs:?}"
    );
    assert_eq!(h.messages(thread).len(), 1, "no empty assistant message");

    h.sink.clear();
    h.chat.send(args(thread, "[[retry-fail]]")).await.unwrap();
    let events = h.sink.done().await;
    assert!(
        statuses(&events)
            .iter()
            .any(|s| s.contains("正在重试（1/2）"))
    );
    let errs = errors(&events);
    assert_eq!(errs[0].0, "model");
    assert!(errs[0].1.contains("529 overloaded_error"), "{errs:?}");
}

#[tokio::test]
async fn process_exit_keeps_partial_text_and_next_send_restarts() {
    let h = setup(0);
    let thread = h.chat.send(args(0, "[[exit]]")).await.unwrap();
    let errs = errors(&h.sink.done().await);
    assert_eq!(
        errs,
        vec![(
            "process_exit".into(),
            "对话引擎意外退出，已收到的内容已保留；下次发送会自动重启".into()
        )]
    );
    assert_eq!(
        h.messages(thread).last().unwrap(),
        &("assistant".into(), "半句".into())
    );
    let first_generation = h.chat.supervisor().generation();

    h.sink.clear();
    h.chat.send(args(thread, "再来")).await.unwrap();
    let events = h.sink.done().await;
    assert!(errors(&events).is_empty(), "{events:#?}");
    assert_eq!(h.chat.supervisor().generation(), first_generation + 1);
    assert_eq!(h.messages(thread).last().unwrap().1, "收到，这是回答。");
}

#[tokio::test]
async fn abort_keeps_partial_text_without_error_and_busy_gate_holds() {
    let h = setup(0);
    let thread = h.chat.send(args(0, "[[slow]]")).await.unwrap();
    h.sink
        .until("some streamed text", |e| {
            e.iter()
                .any(|e| matches!(e, ChatEvent::Delta(d) if d.text.chars().count() >= 3))
        })
        .await;
    let busy = h.chat.send(args(thread, "插队")).await.unwrap_err();
    assert_eq!(busy.kind, ChatErrorKind::Busy);
    h.chat.abort().await.unwrap();
    let events = h.sink.done().await;
    assert!(errors(&events).is_empty(), "{events:#?}");
    let last = h.messages(thread).last().unwrap().clone();
    assert_eq!(last.0, "assistant");
    assert!(last.1.starts_with("字"));
    assert!(!h.chat.supervisor().is_busy());
}

#[tokio::test]
async fn confirm_requests_can_be_allowed_denied_or_time_out() {
    let h = setup(0);
    let thread = h.chat.send(args(0, "[[confirm:5000]]")).await.unwrap();
    let events = h
        .sink
        .until("confirm", |e| {
            e.iter().any(|e| matches!(e, ChatEvent::Confirm(_)))
        })
        .await;
    let ChatEvent::Confirm(request) = events
        .iter()
        .find(|e| matches!(e, ChatEvent::Confirm(_)))
        .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(
        (
            request.thread_id,
            request.title.as_str(),
            request.timeout_ms
        ),
        (thread, "读取网页", Some(5000))
    );
    assert!(steps(&events).contains(&("读取网页".to_string(), true)));
    h.chat.confirm(&request.id, true, false).await.unwrap();
    h.sink.done().await;
    assert_eq!(h.messages(thread).last().unwrap().1, "已读取网页。");

    h.sink.clear();
    h.chat.send(args(thread, "[[confirm:5000]]")).await.unwrap();
    let events = h
        .sink
        .until("confirm", |e| {
            e.iter().any(|e| matches!(e, ChatEvent::Confirm(_)))
        })
        .await;
    let ChatEvent::Confirm(request) = events
        .iter()
        .find(|e| matches!(e, ChatEvent::Confirm(_)))
        .unwrap()
    else {
        unreachable!()
    };
    h.chat.confirm(&request.id, false, false).await.unwrap();
    h.sink.done().await;
    assert_eq!(h.messages(thread).last().unwrap().1, "用户取消了读取。");

    // 没人回应：对话引擎按期限自动拒绝，本轮照常结束。
    h.sink.clear();
    h.chat.send(args(thread, "[[confirm:200]]")).await.unwrap();
    let events = h.sink.done().await;
    assert!(errors(&events).is_empty());
    assert_eq!(h.messages(thread).last().unwrap().1, "用户取消了读取。");
}

#[tokio::test]
async fn unsupported_dialogs_are_cancelled_immediately() {
    let h = setup(0);
    let thread = h.chat.send(args(0, "[[select]]")).await.unwrap();
    let events = h.sink.done().await;
    assert!(!events.iter().any(|e| matches!(e, ChatEvent::Confirm(_))));
    assert_eq!(h.messages(thread).last().unwrap().1, "select cancelled");
    assert!(
        h.pi_log().contains(r#""cancelled": true"#),
        "{}",
        h.pi_log()
    );
}

#[tokio::test]
async fn rejected_prompt_discards_the_user_message_and_releases_the_gate() {
    let h = setup(0);
    let err = h.chat.send(args(0, "[[reject]]")).await.unwrap_err();
    assert_eq!(err.kind, ChatErrorKind::Model);
    assert!(
        err.message.contains("No model configured")
            && err.message.contains("***")
            && !err.message.contains("zzzz"),
        "{}",
        err.message
    );
    let conn = db::open_at(&h.db).unwrap();
    let thread = threads::list_threads(&conn, 10).unwrap()[0].id;
    assert!(h.messages(thread).is_empty(), "user message rolled back");
    assert!(!h.chat.supervisor().is_busy());
    // 失败的这一轮也发过 Done，界面据此解除「生成中」。
    assert!(
        h.sink
            .snapshot()
            .iter()
            .any(|e| matches!(e, ChatEvent::Done(t) if *t == thread))
    );
    h.sink.clear();
    h.chat.send(args(thread, "好了吗")).await.unwrap();
    h.sink.done().await;
    assert_eq!(h.messages(thread).len(), 2);
}

#[tokio::test]
async fn missing_session_file_starts_a_new_session_with_a_notice() {
    let h = setup(0);
    let conn = db::open_at(&h.db).unwrap();
    let thread = threads::create_thread(&conn, Some("旧对话")).unwrap().id;
    let gone = h.data.join("pi/sessions/gone.jsonl");
    threads::set_pi_session_file(&conn, thread, Some(gone.to_str().unwrap())).unwrap();
    h.chat.send(args(thread, "还记得吗")).await.unwrap();
    let events = h.sink.done().await;
    assert!(errors(&events).is_empty(), "{events:#?}");
    assert!(statuses(&events).contains(&"之前的对话上下文无法恢复，已从新会话开始".to_string()));
    let file = threads::get_thread(&conn, thread)
        .unwrap()
        .pi_session_file
        .unwrap();
    assert_ne!(file, gone.to_str().unwrap());
    assert!(Path::new(&file).is_file());
}

#[tokio::test]
async fn idle_process_is_released_and_restarted_on_demand() {
    let h = setup(1);
    let thread = h.chat.send(args(0, "你好")).await.unwrap();
    h.sink.done().await;
    assert!(h.chat.supervisor().is_running().await);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while h.chat.supervisor().is_running().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "pi was not released after idling"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        h.pi_log().trim_end().ends_with("EOF"),
        "stdin closed for an orderly exit: {}",
        h.pi_log()
    );

    h.sink.clear();
    h.chat.send(args(thread, "又来了")).await.unwrap();
    let events = h.sink.done().await;
    assert!(errors(&events).is_empty(), "{events:#?}");
    assert_eq!(h.messages(thread).len(), 4);
}

#[tokio::test]
async fn managed_launch_is_isolated_and_keeps_the_key_off_disk() {
    let h = setup(0);
    h.chat.send(args(0, "你好")).await.unwrap();
    let events = h.sink.done().await;
    assert!(errors(&events).is_empty(), "{events:#?}");

    let starts = h.starts();
    assert_eq!(starts.len(), 1);
    let argv = args_of(&starts[0]);
    for flag in [
        "--no-context-files",
        "--no-prompt-templates",
        "--no-skills",
        "--approve",
    ] {
        assert!(argv.iter().any(|a| a == flag), "{flag} missing: {argv:?}");
    }
    assert!(!argv.iter().any(|a| a == "--provider"), "{argv:?}");
    assert_eq!(
        arg_after(&argv, "--model").as_deref(),
        Some("rrss-1/test-model")
    );
    let env = &starts[0]["env"];
    let agent = h.data.join("pi/agent");
    assert_eq!(env["PI_CODING_AGENT_DIR"], agent.to_str().unwrap());
    assert_eq!(env["PI_OFFLINE"], "1");
    assert_eq!(env["RRSS_PI_KEY_1"], "sk-test-key");
    // RRSS 不再自己设置 OpenAI 的环境变量；有值只可能是测试进程继承下来的。
    for key in ["OPENAI_BASE_URL", "OPENAI_API_KEY"] {
        assert_eq!(
            env[key].as_str(),
            std::env::var(key).ok().as_deref(),
            "{key}"
        );
    }

    let models = std::fs::read_to_string(agent.join("models.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&models).unwrap();
    let rrss = &parsed["providers"]["rrss-1"];
    assert_eq!(rrss["baseUrl"], "http://127.0.0.1:9/v1");
    assert_eq!(rrss["api"], "openai-completions");
    assert_eq!(rrss["apiKey"], "$RRSS_PI_KEY_1");
    assert_eq!(rrss["models"][0]["id"], "test-model");
    assert!(agent.join(".rrss-managed").is_file());
    assert!(h.data.join("pi/runtime/rrss-tools.ts").is_file());
    assert!(h.data.join("pi/project/.pi/tool-manifest.json").is_file());
    for file in [
        "agent/settings.json",
        "agent/models.json",
        "project/.pi/settings.json",
    ] {
        let body = std::fs::read_to_string(h.data.join("pi").join(file)).unwrap();
        assert!(!body.contains("sk-test-key"), "key written to {file}");
    }
}

#[tokio::test]
async fn proxy_changes_keep_pi_running_and_next_turn_uses_the_new_api_route() {
    use rrss_core::network::{self, ProxyConfig, ProxyRule};
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

    let h = setup(0);
    let first_proxy = MockServer::start().await;
    let next_proxy = MockServer::start().await;
    for proxy in [&first_proxy, &next_proxy] {
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{ "embedding": [0.1, 0.2] }]
            })))
            .expect(1)
            .mount(proxy)
            .await;
    }
    let conn = db::open_at(&h.db).unwrap();
    // 代理客户端始终绕过 localhost / 127.0.0.1 / ::1；嵌入端点用不解析的域名，
    // 只有经过代理才能到达，直连会失败而不是悄悄打到本机。
    let embed_provider = ai_store::insert_provider(
        &conn,
        &NewProvider {
            name: "代理后的嵌入".into(),
            base_url: "http://embed.invalid/v1".into(),
            api_key: Some("sk-test-key".into()),
            protocol: "chat".into(),
        },
    )
    .unwrap();
    add_model(&conn, embed_provider, "embed-model");
    ai_store::set_task_binding(
        &conn,
        ai_store::TASK_EMBED,
        Some(embed_provider),
        Some("embed-model"),
    )
    .unwrap();
    // 没有生效向量空间时检索不嵌入查询；给绑定配一个生效空间，让这次请求走 API 代理。
    conn.execute(
        "INSERT INTO vector_spaces(provider_id, base_url, model, dims, prefix_version, normalized, state, created_at)
         VALUES(?1, 'http://embed.invalid/v1', 'embed-model', 3, 1, 0, 'active', 0)",
        [embed_provider],
    )
    .unwrap();
    let mut config = ProxyConfig {
        api: ProxyRule::custom(first_proxy.uri()),
        ..Default::default()
    };
    network::save(&conn, &config).unwrap();
    let thread = h.chat.send(args(0, "[[slow]]")).await.unwrap();
    h.sink
        .until("some streamed text", |events| {
            events
                .iter()
                .any(|e| matches!(e, ChatEvent::Delta(d) if !d.text.is_empty()))
        })
        .await;
    let generation = h.chat.supervisor().generation();
    config.api.url = next_proxy.uri();
    config.pi = config.api.clone();
    network::save(&conn, &config).unwrap();
    assert!(h.chat.supervisor().is_running().await);
    assert_eq!(h.chat.supervisor().generation(), generation);
    assert!(
        h.chat.supervisor().is_busy(),
        "saving proxy must not abort the turn"
    );
    // slow 场景会持续输出直到显式中止；保存配置本身不能结束它。
    h.chat.abort().await.unwrap();
    assert!(errors(&h.sink.done().await).is_empty());
    h.sink.clear();
    h.chat.send(args(thread, "下一句")).await.unwrap();
    assert!(errors(&h.sink.done().await).is_empty());
    assert_eq!(h.chat.supervisor().generation(), generation);
    assert_eq!(first_proxy.received_requests().await.unwrap().len(), 1);
    assert_eq!(next_proxy.received_requests().await.unwrap().len(), 1);
    h.chat.stop_engine().await;
}

#[tokio::test]
async fn pi_receives_its_proxy_environment_on_the_next_start_only() {
    use rrss_core::network::{self, ProxyConfig, ProxyRule};

    let h = setup(0);
    let conn = db::open_at(&h.db).unwrap();
    let url = "http://pi-user:pi-pass@proxy.invalid:3128";
    network::save(
        &conn,
        &ProxyConfig {
            pi: ProxyRule {
                no_proxy: "internal.test".into(),
                ..ProxyRule::custom(url)
            },
            ..Default::default()
        },
    )
    .unwrap();
    let thread = h.chat.send(args(0, "第一句")).await.unwrap();
    assert!(errors(&h.sink.done().await).is_empty());
    let env = &h.starts()[0]["env"];
    for key in ["HTTP_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
        assert_eq!(env[key], format!("{url}/"), "{key}");
    }
    assert_eq!(env["NO_PROXY"], "localhost,127.0.0.1,::1,internal.test");

    // 改成无代理：运行中的 Pi 不重启；下次启动时八个变量都不存在。
    let generation = h.chat.supervisor().generation();
    network::save(&conn, &ProxyConfig::default()).unwrap();
    h.sink.clear();
    h.chat.send(args(thread, "第二句")).await.unwrap();
    assert!(errors(&h.sink.done().await).is_empty());
    assert_eq!(h.chat.supervisor().generation(), generation);
    assert_eq!(h.starts().len(), 1);
    h.chat.stop_engine().await;
    h.sink.clear();
    h.chat.send(args(thread, "第三句")).await.unwrap();
    assert!(errors(&h.sink.done().await).is_empty());
    let starts = h.starts();
    assert_eq!(starts.len(), 2);
    for key in rrss_core::network::PROXY_ENV_KEYS {
        assert!(starts[1]["env"][key].is_null(), "{key}");
    }

    // 坏配置不能静默直连：启动报配置错误，且不回显存储的原文。
    h.chat.stop_engine().await;
    db::set_setting(&conn, network::SETTING_KEY, "broken-private-secret").unwrap();
    h.sink.clear();
    let err = h.chat.send(args(thread, "第四句")).await.unwrap_err();
    assert_eq!(err.kind, ChatErrorKind::Config, "{}", err.message);
    assert!(!err.message.contains("private-secret"));
    assert_eq!(h.starts().len(), 2);
    h.chat.stop_engine().await;
}

#[tokio::test]
async fn profile_changes_restart_pi_before_the_next_send() {
    let h = setup(0);
    let thread = h.chat.send(args(0, "第一句")).await.unwrap();
    h.sink.done().await;
    let first = h.chat.supervisor().generation();

    // 配置不变：沿用同一进程。
    h.sink.clear();
    h.chat.send(args(thread, "第二句")).await.unwrap();
    h.sink.done().await;
    assert_eq!(h.chat.supervisor().generation(), first);

    h.set_model("other-model");
    h.sink.clear();
    h.chat.send(args(thread, "第三句")).await.unwrap();
    let events = h.sink.done().await;
    assert!(errors(&events).is_empty(), "{events:#?}");
    assert_eq!(h.chat.supervisor().generation(), first + 1);
    let starts = h.starts();
    assert_eq!(starts.len(), 2);
    assert_eq!(
        arg_after(&args_of(&starts[1]), "--model").as_deref(),
        Some("rrss-1/other-model")
    );
    // 旧进程被有序关闭，新进程接着用线程原来的会话。
    assert!(h.pi_log().contains("EOF"), "{}", h.pi_log());
    assert!(h.pi_log().contains("switch_session"), "{}", h.pi_log());
    assert_eq!(h.messages(thread).last().unwrap().1, "收到，这是回答。");
}

#[tokio::test]
async fn pool_models_switch_in_process_between_threads_without_a_restart() {
    let h = setup(0);
    let conn = db::open_at(&h.db).unwrap();
    let other = ai_store::insert_provider(
        &conn,
        &NewProvider {
            name: "云端".into(),
            base_url: "http://127.0.0.1:9/v2".into(),
            api_key: Some("sk-other-key".into()),
            protocol: "chat".into(),
        },
    )
    .unwrap();
    add_model(&conn, other, "fast");
    set_entry(
        &conn,
        &rrss_core::model_pool::ModelRef::new(other, "fast"),
        rrss_core::model_pool::EntryPatch {
            thinking: Some("low".into()),
            ..Default::default()
        },
    );
    drop(conn);
    let first = h.chat.send(args(0, "第一句")).await.unwrap();
    h.sink.done().await;
    let generation = h.chat.supervisor().generation();
    assert!(
        !h.pi_log().contains("\"set_model\""),
        "default needs no switch"
    );

    // 另一条对话选了池里的模型：同一进程里切换。
    h.sink.clear();
    let mut chosen = args(0, "换个模型");
    chosen.model = Some(ModelChoice {
        provider_id: other,
        model: "fast".into(),
    });
    let second = h.chat.send(chosen).await.unwrap();
    let events = h.sink.done().await;
    assert!(errors(&events).is_empty(), "{events:#?}");
    assert_ne!(second, first);
    assert_eq!(h.chat.supervisor().generation(), generation);
    assert!(
        h.pi_log().contains(&format!(
            "\"modelId\": \"fast\", \"provider\": \"rrss-{other}\""
        )),
        "{}",
        h.pi_log()
    );
    // 思考强度跟着模型走：fast 设的是「低」。
    let log = h.pi_log();
    let switched = log.rfind("\"modelId\": \"fast\"").unwrap();
    assert!(
        log[switched..].contains(r#""type": "set_thinking_level""#)
            && log[switched..].contains(r#""level": "low""#),
        "{log}"
    );

    // 回到第一条对话：切回对话默认，仍不重启。
    h.sink.clear();
    h.chat.send(args(first, "回来")).await.unwrap();
    h.sink.done().await;
    assert_eq!(h.chat.supervisor().generation(), generation);
    assert!(
        h.pi_log()
            .contains("\"modelId\": \"test-model\", \"provider\": \"rrss-1\""),
        "{}",
        h.pi_log()
    );
    let starts = h.starts();
    assert_eq!(starts.len(), 1);
    assert_eq!(
        starts[0]["env"][format!("RRSS_PI_KEY_{other}")],
        "sk-other-key"
    );
}

#[tokio::test]
async fn handshake_rejects_a_model_pi_did_not_load() {
    let h = setup(0);
    h.set_model("wrong-model");
    let err = h.chat.send(args(0, "你好")).await.unwrap_err();
    assert_eq!(err.kind, ChatErrorKind::Config);
    assert!(
        err.message.contains("rrss-1/wrong-model"),
        "{}",
        err.message
    );
    assert!(!h.chat.supervisor().is_busy());
    assert!(!h.chat.supervisor().is_running().await);
    let conn = db::open_at(&h.db).unwrap();
    for thread in threads::list_threads(&conn, 10).unwrap() {
        assert!(h.messages(thread.id).is_empty(), "no message kept");
    }
}

#[tokio::test]
async fn unsupported_thinking_level_is_downgraded_at_startup() {
    let h = setup(0);
    h.set_thinking("high");
    h.chat.send(args(0, "想一想")).await.unwrap();
    let events = h.sink.done().await;
    assert!(errors(&events).is_empty(), "{events:#?}");
    let argv = args_of(&h.starts()[0]);
    assert_eq!(arg_after(&argv, "--thinking").as_deref(), Some("high"));
    let log = h.pi_log();
    assert!(
        log.contains(r#""type": "set_thinking_level""#) && log.contains(r#""level": "medium""#),
        "{log}"
    );
    assert_eq!(
        h.chat.supervisor().thinking_levels().await,
        vec!["off", "minimal", "low", "medium"]
    );
    // 设置页在引擎停下后仍按这份缓存显示档位。
    h.chat.stop_engine().await;
    let conn = db::open_at(&h.db).unwrap();
    assert_eq!(
        pi_store::cached_thinking_levels(&conn).unwrap(),
        vec!["off", "minimal", "low", "medium"]
    );
    drop(conn);

    // 明确不支持推理时只剩 off。
    let conn = db::open_at(&h.db).unwrap();
    set_entry(
        &conn,
        &rrss_core::model_pool::ModelRef::new(h.provider, "test-model"),
        rrss_core::model_pool::EntryPatch {
            overrides: Some(rrss_core::model_pool::CapsOverride {
                reasoning: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    drop(conn);
    h.set_thinking("off");
    h.sink.clear();
    h.chat.send(args(0, "直接说")).await.unwrap();
    h.sink.done().await;
    let starts = h.starts();
    assert_eq!(starts.len(), 2, "thinking change restarts pi");
    assert_eq!(
        arg_after(&args_of(&starts[1]), "--thinking").as_deref(),
        Some("off")
    );
    assert_eq!(h.chat.supervisor().thinking_levels().await, vec!["off"]);
}

#[tokio::test]
async fn compaction_context_is_emitted_only_when_new_usage_is_known() {
    let h = setup(0);
    for (marker, expected) in [
        ("[[compact-ok]]", vec![77, 77]),
        ("[[compact-unknown]]", vec![190]),
    ] {
        h.sink.clear();
        let id = h.chat.send(args(0, marker)).await.unwrap();
        let events = h.sink.done().await;
        let contexts: Vec<_> = events
            .iter()
            .filter_map(|e| {
                if let ChatEvent::Context(c) = e {
                    assert_eq!(c.thread_id, id);
                    Some(c.context.used)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(contexts, expected);
    }
}

#[tokio::test]
async fn context_is_current_usage_and_persisted_for_only_its_thread() {
    let h = setup(0);
    let conn = db::open_at(&h.db).unwrap();
    let untouched = threads::create_thread(&conn, Some("其他线程")).unwrap();
    assert!(
        threads::get_thread(&conn, untouched.id)
            .unwrap()
            .context_used
            .is_none()
    );
    let id = h.chat.send(args(0, "你好")).await.unwrap();
    let events = h.sink.done().await;
    let context_events: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let ChatEvent::Context(c) = e {
                Some(c)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(context_events.len(), 1);
    assert_eq!(context_events[0].thread_id, id);
    assert_eq!(context_events[0].context.used, 250);
    assert_eq!(context_events[0].context.window, Some(32000));
    let context_at = events
        .iter()
        .position(|e| matches!(e, ChatEvent::Context(_)))
        .unwrap();
    let done_at = events
        .iter()
        .position(|e| matches!(e, ChatEvent::Done(_)))
        .unwrap();
    assert!(context_at < done_at);
    assert_eq!(
        threads::get_thread(&conn, id).unwrap().context_used,
        Some(250)
    );
    assert!(
        threads::get_thread(&conn, untouched.id)
            .unwrap()
            .context_used
            .is_none()
    );
    assert_eq!(
        rrss_app::queries::threads(&conn).unwrap()[0]
            .context
            .as_ref()
            .unwrap()
            .used,
        250
    );
    h.sink.clear();
    h.chat.send(args(id, "[[usage-fallback]]")).await.unwrap();
    h.sink.done().await;
    assert_eq!(
        threads::get_thread(&conn, id).unwrap().context_used,
        Some(190)
    );
    // 下轮启动换模型窗，旧 used / at 必须保留，直到结算。
    set_entry(
        &conn,
        &rrss_core::model_pool::ModelRef::new(h.provider, "test-model"),
        rrss_core::model_pool::EntryPatch {
            context_window: Some(Some(64000)),
            ..Default::default()
        },
    );
    h.sink.clear();
    h.chat.send(args(id, "[[slow]]")).await.unwrap();
    let stored = threads::get_thread(&conn, id).unwrap();
    assert_eq!(
        (stored.context_used, stored.context_window),
        (Some(190), Some(64000))
    );
    h.chat.abort().await.unwrap();
    h.sink.done().await;
}

#[tokio::test]
async fn thread_thinking_records_keeps_clears_and_downgrades_without_changing_model() {
    let h = setup(0);
    h.set_thinking("low");
    let mut send = args(0, "你好");
    send.thinking = Some("high".into());
    let id = h.chat.send(send).await.unwrap();
    assert!(errors(&h.sink.done().await).is_empty());
    let conn = db::open_at(&h.db).unwrap();
    assert_eq!(
        threads::get_thread(&conn, id)
            .unwrap()
            .chat_thinking
            .as_deref(),
        Some("high")
    );
    assert_eq!(
        rrss_app::queries::threads(&conn).unwrap()[0]
            .chat_thinking
            .as_deref(),
        Some("high")
    );
    assert!(h.pi_log().contains(r#""level": "medium""#));
    h.sink.clear();
    h.chat.send(args(id, "继续")).await.unwrap();
    h.sink.done().await;
    assert_eq!(
        threads::get_thread(&conn, id)
            .unwrap()
            .chat_thinking
            .as_deref(),
        Some("high")
    );
    h.sink.clear();
    let mut send = args(id, "跟随");
    send.thinking = Some(String::new());
    h.chat.send(send).await.unwrap();
    h.sink.done().await;
    assert_eq!(threads::get_thread(&conn, id).unwrap().chat_thinking, None);
    assert!(h.pi_log().contains(r#""level": "low""#));
    assert_eq!(h.starts().len(), 1);
    let mut invalid = args(0, "非法");
    invalid.thinking = Some("extreme".into());
    let err = h.chat.send(invalid).await.unwrap_err();
    assert_eq!(err.kind, ChatErrorKind::Config);
    assert!(err.message.contains("思考强度"));
    assert_eq!(threads::list_threads(&conn, 80).unwrap().len(), 1);
    set_entry(
        &conn,
        &rrss_core::model_pool::ModelRef::new(h.provider, "test-model"),
        rrss_core::model_pool::EntryPatch {
            overrides: Some(rrss_core::model_pool::CapsOverride {
                reasoning: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    h.sink.clear();
    let mut send = args(id, "不支持推理");
    send.thinking = Some("high".into());
    h.chat.send(send).await.unwrap();
    h.sink.done().await;
    let log = h.pi_log();
    assert!(log[log.rfind("START ").unwrap()..].contains(r#""level": "off""#));
}

#[tokio::test]
async fn an_unset_chat_model_is_a_config_error_even_with_models_added() {
    let h = setup_with(0, Model::Unbound);
    let err = h.chat.send(args(0, "你好")).await.unwrap_err();
    assert_eq!(err.kind, ChatErrorKind::Config);
    assert!(err.message.contains("还没选对话模型"), "{}", err.message);
    let conn = db::open_at(&h.db).unwrap();
    assert!(threads::list_threads(&conn, 10).unwrap().is_empty());
    assert!(h.starts().is_empty(), "pi never started");
}

#[tokio::test]
async fn compaction_failure_is_a_redacted_model_error_but_abort_is_not() {
    let h = setup(0);
    let thread = h.chat.send(args(0, "[[compact-fail]]")).await.unwrap();
    let events = h.sink.done().await;
    let errs = errors(&events);
    assert_eq!(errs.len(), 1);
    assert_eq!(errs[0].0, "model");
    assert!(errs[0].1.contains("压缩失败") && errs[0].1.contains("***"));
    assert!(!errs[0].1.contains("compactsecret"));
    assert_eq!(h.messages(thread).last().unwrap().1, "已收到的正文。");
    h.sink.clear();
    h.chat
        .send(args(thread, "[[compact-abort]]"))
        .await
        .unwrap();
    assert!(errors(&h.sink.done().await).is_empty());
}

#[tokio::test]
async fn prompt_timeout_retires_process_before_next_turn_and_drops_late_events() {
    let h = setup(0);
    let err = h.chat.send(args(0, "[[timeout]]")).await.unwrap_err();
    assert_eq!(err.kind, ChatErrorKind::ProcessExit);
    assert!(!h.chat.supervisor().is_running().await);
    assert!(!h.chat.supervisor().is_busy());
    let conn = db::open_at(&h.db).unwrap();
    let thread = threads::list_threads(&conn, 10).unwrap()[0].id;
    assert!(h.messages(thread).is_empty());
    assert!(
        !h.sink
            .snapshot()
            .iter()
            .any(|e| matches!(e, ChatEvent::Delta(d) if d.text.contains("污染")))
    );
    h.sink.clear();
    h.chat.send(args(thread, "重试")).await.unwrap();
    assert!(errors(&h.sink.done().await).is_empty());
    assert_eq!(h.starts().len(), 2);
    assert_eq!(h.messages(thread).last().unwrap().1, "收到，这是回答。");
}

#[tokio::test]
async fn version_notices_ignore_old_markers_and_remember_each_acknowledged_version() {
    let h = setup(0);
    let script = h.data.join("fake-pi");
    let original = std::fs::read_to_string(&script).unwrap();
    let conn = db::open_at(&h.db).unwrap();
    db::set_setting(&conn, rrss_app::pi::profile::UNVERIFIED_NOTICE_KEY, "1.6.1").unwrap();
    for (version, should_notice, acknowledge) in [
        ("1.6.1", true, false),
        ("1.6.1", true, true),
        ("1.7.1", true, true),
        ("1.6.1", false, false),
    ] {
        h.chat.stop_engine().await;
        std::fs::write(
            &script,
            original.replace("print(\"1.5.0\")", &format!("print(\"{version}\")")),
        )
        .unwrap();
        h.sink.clear();
        h.chat.send(args(0, "你好")).await.unwrap();
        let events = h.sink.done().await;
        assert!(errors(&events).is_empty());
        assert_eq!(
            statuses(&events)
                .iter()
                .any(|s| s.contains(version) && s.contains("不在已验证")),
            should_notice
        );
        if acknowledge {
            h.chat.acknowledge_version_notice(version).await.unwrap();
        }
    }
}

#[tokio::test]
async fn no_model_is_a_config_error_before_any_thread_exists() {
    let h = setup_with(0, Model::None);
    let err = h.chat.send(args(0, "你好")).await.unwrap_err();
    assert_eq!(err.kind, ChatErrorKind::Config);
    assert!(err.message.contains("设置"), "{}", err.message);
    let conn = db::open_at(&h.db).unwrap();
    assert!(threads::list_threads(&conn, 10).unwrap().is_empty());
    assert!(h.starts().is_empty(), "pi never started");
    assert!(!h.chat.supervisor().is_busy());
}

#[tokio::test]
async fn own_pi_config_switches_isolation_off_and_back() {
    let h = setup(0);
    let conn = db::open_at(&h.db).unwrap();
    pi_store::set_use_own_config(&conn, true).unwrap();
    drop(conn);
    assert_eq!(
        h.chat.available_models().await.unwrap(),
        vec!["user/default-model", "user/other-model"]
    );
    let conn = db::open_at(&h.db).unwrap();
    assert!(
        threads::list_threads(&conn, 10).unwrap().is_empty(),
        "model lookup does not create a conversation"
    );
    drop(conn);
    assert!(h.chat.supervisor().try_begin());
    assert_eq!(
        h.chat.available_models().await.unwrap_err().kind,
        ChatErrorKind::Busy
    );
    assert!(
        h.chat.supervisor().is_busy(),
        "lookup must not release another operation's gate"
    );
    h.chat.supervisor().end();
    let thread = h.chat.send(args(0, "用我自己的配置")).await.unwrap();
    let events = h.sink.done().await;
    assert!(errors(&events).is_empty(), "{events:#?}");
    let own = &h.starts()[0];
    assert!(own["env"]["PI_CODING_AGENT_DIR"].is_null(), "{own}");
    assert!(
        !own["env"]
            .as_object()
            .unwrap()
            .keys()
            .any(|k| k.starts_with("RRSS_PI_KEY")),
        "no RRSS key in own mode"
    );
    assert!(own["env"]["PI_OFFLINE"].is_null());
    assert_eq!(arg_after(&args_of(own), "--model"), None);
    assert_eq!(
        h.chat.available_models().await.unwrap(),
        vec!["user/default-model", "user/other-model"]
    );

    // 关掉后回到托管模式：重启，并重新隔离。
    let conn = db::open_at(&h.db).unwrap();
    pi_store::set_use_own_config(&conn, false).unwrap();
    drop(conn);
    h.sink.clear();
    h.chat.send(args(thread, "回到托管")).await.unwrap();
    h.sink.done().await;
    let starts = h.starts();
    assert_eq!(starts.len(), 2);
    assert_eq!(
        starts[1]["env"]["PI_CODING_AGENT_DIR"],
        h.data.join("pi/agent").to_str().unwrap()
    );
    assert_eq!(
        arg_after(&args_of(&starts[1]), "--model").as_deref(),
        Some("rrss-1/test-model")
    );
}
