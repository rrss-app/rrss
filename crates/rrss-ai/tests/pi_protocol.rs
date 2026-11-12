//! L1 协议契约：逐个样本目录解码 `testdata/pi/fixtures/*/`，断言对话需要的事件与字段。
//! 手写样本（`docs-*`）与真实录制（`<版本>/`，由 `scripts/pi-compat.mjs` 生成）跑同一组断言；
//! 断言只看 RRSS 依赖的结构，不看 Pi 自己拼的错误文案。某个目录没有录的场景跳过。

use std::path::{Path, PathBuf};

use rrss_ai::pi::{JsonlDecoder, PiEvent, interpret};

fn fixture_dirs() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pi/fixtures");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    assert!(!dirs.is_empty(), "no fixtures under {}", root.display());
    dirs
}

/// 这个目录里有哪些样本目录录了该场景。
fn dirs_with(name: &str) -> Vec<PathBuf> {
    fixture_dirs()
        .into_iter()
        .filter(|d| d.join(name).is_file())
        .collect()
}

/// 按 4 KiB 分块喂给解码器，同时覆盖跨块拼行。
fn events(dir: &Path, name: &str) -> Vec<PiEvent> {
    let bytes = std::fs::read(dir.join(name)).unwrap();
    let mut decoder = JsonlDecoder::default();
    bytes
        .chunks(4096)
        .flat_map(|chunk| decoder.push(chunk))
        .map(|v| interpret(&v))
        .collect()
}

fn assistant_ends(events: &[PiEvent]) -> Vec<(String, Option<String>, Option<String>)> {
    events
        .iter()
        .filter_map(|e| match e {
            PiEvent::MessageEnd {
                role,
                text,
                stop_reason,
                error_message,
                ..
            } if role == "assistant" => {
                Some((text.clone(), stop_reason.clone(), error_message.clone()))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn text_run_streams_deltas_and_settles() {
    for dir in fixture_dirs() {
        let ev = events(&dir, "text.jsonl");
        assert!(
            matches!(&ev[0], PiEvent::Response { id: Some(id), success: true, .. } if id == "rrss-1")
        );
        let deltas: String = ev
            .iter()
            .filter_map(|e| match e {
                PiEvent::TextDelta(d) => Some(d.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, "你好，有什么可以帮你？", "{}", dir.display());
        assert_eq!(
            assistant_ends(&ev),
            vec![("你好，有什么可以帮你？".into(), Some("stop".into()), None)]
        );
        assert!(ev.contains(&PiEvent::AgentEnd { will_retry: false }));
        assert_eq!(ev.last(), Some(&PiEvent::AgentSettled));
    }
}

#[test]
fn tool_loop_reports_tools_and_keeps_both_assistant_messages() {
    for dir in fixture_dirs() {
        let ev = events(&dir, "tool_loop.jsonl");
        assert!(
            ev.iter()
                .any(|e| matches!(e, PiEvent::ToolStart { call_id, name, args }
            if call_id == "call_1" && name == "search_articles" && args.get("query").is_some()))
        );
        assert!(ev.contains(&PiEvent::ToolEnd {
            call_id: "call_1".into(),
            name: "search_articles".into(),
            is_error: false
        }));
        let ends = assistant_ends(&ev);
        assert_eq!(ends.len(), 2);
        assert_eq!(ends[0].1.as_deref(), Some("toolUse"));
        assert_eq!(
            ends[1],
            ("Rust 1.90 发布了。".into(), Some("stop".into()), None)
        );
        assert_eq!(ev.last(), Some(&PiEvent::AgentSettled));
    }
}

#[test]
fn model_error_carries_stop_reason_and_message() {
    for dir in fixture_dirs() {
        let ev = events(&dir, "model_error.jsonl");
        let ends = assistant_ends(&ev);
        assert_eq!(ends[0].1.as_deref(), Some("error"));
        assert!(ends[0].2.as_deref().unwrap().starts_with("401"));
        assert_eq!(ev.last(), Some(&PiEvent::AgentSettled));
    }
}

#[test]
fn auto_retry_final_failure_is_visible() {
    for dir in fixture_dirs() {
        let ev = events(&dir, "auto_retry.jsonl");
        assert!(ev.contains(&PiEvent::AgentEnd { will_retry: true }));
        assert!(ev.iter().any(|e| matches!(e,
            PiEvent::AutoRetryStart { attempt: 1, max_attempts: 3, error_message } if error_message.starts_with("529"))));
        assert!(
            ev.iter().any(|e| matches!(e,
                PiEvent::AutoRetryEnd { success: false, final_error: Some(err) } if err.starts_with("529"))),
            "{}",
            dir.display()
        );
        assert_eq!(ev.last(), Some(&PiEvent::AgentSettled));
    }
}

#[test]
fn compaction_start_and_end_are_named_events() {
    for dir in dirs_with("compaction.jsonl") {
        let ev = events(&dir, "compaction.jsonl");
        assert_eq!(
            ev[0],
            PiEvent::CompactionStart {
                reason: "threshold".into()
            }
        );
        assert!(matches!(ev[1], PiEvent::Other(ref t) if t == "summarization_retry_scheduled"));
        assert_eq!(
            ev[2],
            PiEvent::CompactionEnd {
                aborted: false,
                error_message: None
            }
        );
        assert_eq!(
            ev[4],
            PiEvent::CompactionEnd {
                aborted: false,
                error_message: Some("summary model failed".into())
            }
        );
    }
}

#[test]
fn extension_ui_requests_keep_method_and_timeout() {
    for dir in fixture_dirs() {
        let ev = events(&dir, "confirm.jsonl");
        let confirm = ev
            .iter()
            .find(
                |e| matches!(e, PiEvent::ExtensionUiRequest { method, .. } if method == "confirm"),
            )
            .unwrap_or_else(|| panic!("no confirm in {}", dir.display()));
        assert!(
            matches!(confirm, PiEvent::ExtensionUiRequest { id, title, message, timeout_ms: Some(120_000), .. }
                if !id.is_empty() && title == "读取网页" && message == "执行 fetch_url？"),
            "{confirm:?}"
        );
        if !dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("docs-")
        {
            // 真实录制：确认允许后工具照常执行，本轮结算。
            assert!(ev.iter().any(|e| matches!(e, PiEvent::ToolEnd { name, is_error: false, .. } if name == "fetch_url")));
            assert_eq!(ev.last(), Some(&PiEvent::AgentSettled));
            continue;
        }
        // 手写样本另外覆盖其他对话框、通知与扩展错误。
        assert!(
            matches!(&ev[2], PiEvent::ExtensionUiRequest { method, timeout_ms: None, .. } if method == "select")
        );
        assert!(matches!(&ev[3], PiEvent::ExtensionUiRequest { method, .. } if method == "notify"));
        assert_eq!(
            ev[4],
            PiEvent::ExtensionError {
                message: "Error message".into()
            }
        );
    }
}

#[test]
fn abort_keeps_partial_text_with_aborted_stop_reason() {
    for dir in fixture_dirs() {
        let ev = events(&dir, "abort.jsonl");
        let (text, stop, error) = assistant_ends(&ev)[0].clone();
        assert!(!text.is_empty(), "partial text kept: {}", dir.display());
        assert_eq!(stop.as_deref(), Some("aborted"));
        assert_eq!(error.as_deref(), Some("Request was aborted"));
        // abort 的响应可能早于也可能晚于 agent_settled。
        assert!(ev.iter().any(
            |e| matches!(e, PiEvent::Response { command, success: true, .. } if command == "abort")
        ));
    }
}
