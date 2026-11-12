//! Configuration is the IPC security boundary; keep it independent of GUI tests.
use std::{fs, path::Path};

#[test]
fn all_capabilities_exclude_remote_and_web_windows() {
    let desktop = Path::new(env!("CARGO_MANIFEST_DIR")).join("../rrss-desktop");
    let mut count = 0;
    for entry in fs::read_dir(desktop.join("capabilities")).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        count += 1;
        let capability: serde_json::Value =
            serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert!(capability.get("remote").is_none(), "no remote IPC grants");
        assert!(
            capability.get("windows").is_none(),
            "parent-window grants would be inherited by child webviews"
        );
        let patterns = capability["webviews"].as_array().expect("webviews");
        assert!(!patterns.is_empty(), "webviews must name the main webview");
        for pattern in patterns {
            let pattern = pattern.as_str().expect("webview label");
            // Require literal labels. This deliberately rejects broad globs
            // (including *, ?, classes and escaped forms) without sampling
            // a few web labels and accidentally missing another match.
            assert!(
                !pattern.contains(['*', '?', '[', ']', '\\']),
                "capability labels must be literal: {pattern}"
            );
            assert!(!pattern.starts_with("web-"));
        }
    }
    assert!(count > 0);
    let host = include_str!("../../rrss-desktop/src/commands/web_window.rs");
    assert!(host.contains("invoke.message.webview_ref().label() != \"main\""));
    assert!(host.contains("invoke.resolver.reject("));
    // Every command reaches Tauri through the single gated dispatcher built in commands/mod.rs.
    let lib = include_str!("../../rrss-desktop/src/lib.rs");
    assert!(lib.contains(".invoke_handler(main_only(commands::invoke_handler))"));
    assert_eq!(lib.matches(".invoke_handler(").count(), 1);
    let config: serde_json::Value =
        serde_json::from_slice(&fs::read(desktop.join("tauri.conf.json")).unwrap()).unwrap();
    assert!(!config["app"]["withGlobalTauri"].as_bool().unwrap_or(false));
}

#[test]
fn host_callbacks_keep_unconditional_denials_and_main_only_events() {
    // Lock the stable API wiring without compiling/running a desktop test binary.
    // Native engine behavior is separately verified on real hosts.
    let source = include_str!("../../rrss-desktop/src/commands/web_window.rs");
    assert!(source.contains(".on_permission_request(|_, _| PermissionResponse::Deny)"));
    let popup = source
        .split(".on_new_window(")
        .nth(1)
        .unwrap()
        .split(".on_permission_request(")
        .next()
        .unwrap();
    assert!(popup.contains("web_navigation_allowed(&url)"));
    assert!(popup.contains("window.navigate(url)"));
    assert!(popup.contains("NewWindowResponse::Deny"));
    // W4a-2 routes downloads through the native adapter: wry outside Linux,
    // WebKitGTK's context callback on Linux. Both paths must deny and target main.
    assert!(source.contains("crate::webview_native::download_handler(builder)"));
    assert!(source.contains("app.emit_to(\"main\", event, payload)"));
    assert_eq!(source.matches(".emit_to(").count(), 1);
    let native = include_str!("../../rrss-desktop/src/webview_native/mod.rs");
    let download = native
        .split("builder.on_download(")
        .nth(1)
        .unwrap()
        .split("#[cfg(test)]")
        .next()
        .unwrap();
    assert!(download.contains("false"));
    assert!(download.contains("emit_main("));
    assert!(download.contains("\"web:download-blocked\""));
    let linux = include_str!("../../rrss-desktop/src/webview_native/linux.rs");
    let download = linux
        .split("context.connect_download_started(")
        .nth(1)
        .unwrap();
    assert!(download.contains("download.cancel()"));
    assert!(download.contains("emit_main("));
    assert!(download.contains("\"web:download-blocked\""));
    for adapter in [source, native, linux] {
        assert!(!adapter.contains(".emit("));
    }
    for adapter in [
        native,
        linux,
        include_str!("../../rrss-desktop/src/commands/web_verify.rs"),
    ] {
        assert!(!adapter.contains(".emit_to("));
        assert!(adapter.contains("emit_main("));
    }
    assert!(source.contains(".devtools(false)"));
    assert!(!source.contains(".incognito("));
    let windows = include_str!("../../rrss-desktop/src/webview_native/windows.rs");
    for file in [source, native, linux, windows] {
        assert!(
            !file.contains("get_webview_window"),
            "main is no longer a WebviewWindow once it has a child"
        );
    }
    let escape = native.split("\"web:escape\"").next().unwrap();
    assert!(
        escape.contains("emit_main("),
        "web:escape must be sent through emit_main"
    );
    assert!(linux.contains("emit_escape("));
    assert!(windows.contains("emit_escape("));
}
