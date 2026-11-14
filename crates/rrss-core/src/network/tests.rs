use super::*;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn env(values: &[(&str, &str)]) -> ProxyEnvironment {
    ProxyEnvironment::from_lookup(|key| {
        values
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).into())
    })
}

#[test]
fn proxy_events_never_include_credentials_or_environment_values() {
    let capture = crate::test_log::Capture::new();
    capture.run(|| {
        let config = ProxyConfig {
            global: ProxyRule::custom(
                "socks5h://canary-user:canary-password@private-proxy.test:1080",
            ),
            ..Default::default()
        };
        client(&config, NetworkScope::Content, &env(&[]));
        let bad = ProxyConfig {
            global: ProxyRule::custom(
                "canary-invalid://canary-user:canary-password@private-proxy.test",
            ),
            ..Default::default()
        };
        assert!(
            bad.build_client_with_env(NetworkScope::Api, Client::builder(), &env(&[]))
                .is_err()
        );
        let system = ProxyConfig {
            global: ProxyRule {
                mode: ProxyMode::System,
                ..Default::default()
            },
            ..Default::default()
        };
        for _ in 0..2 {
            assert!(
                system
                    .build_client_with_env(
                        NetworkScope::Pi,
                        Client::builder(),
                        &env(&[
                            ("HTTP_PROXY", "http://unused.example:8080"),
                            ("RRSS_HOST_HTTP_PROXY", "canary-invalid-environment"),
                        ])
                    )
                    .is_err()
            );
        }
        let native = crate::platform::SystemProxySnapshot {
            unavailable: true,
            ..Default::default()
        };
        for _ in 0..2 {
            env(&[]).with_system(&native);
        }
    });
    let log = capture.take();
    assert!(
        !log.contains("canary-") && !log.contains("private-proxy"),
        "{log}"
    );
    assert!(
        log.contains("scheme=\"socks5h\"") && log.contains("scope=\"content\""),
        "{log}"
    );
    assert!(
        log.contains("error_kind=\"config\"") && log.contains("error_kind=\"environment\""),
        "{log}"
    );
    // Process-wide once warnings may already have been consumed by a parallel
    // fixture; the second call must never produce a second warning.
    assert!(
        log.lines()
            .filter(|line| line.contains(" WARN ") && line.contains("environment\""))
            .count()
            <= 1
    );
    assert!(log.matches("system proxy read failed").count() <= 1);
}

fn client(config: &ProxyConfig, scope: NetworkScope, env: &ProxyEnvironment) -> Client {
    config
        .build_client_with_env(
            scope,
            Client::builder().timeout(Duration::from_secs(3)),
            env,
        )
        .unwrap()
}

fn database() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT NOT NULL)")
        .unwrap();
    conn
}

#[test]
fn defaults_inheritance_and_overrides() {
    let mut config = ProxyConfig::default();
    assert_eq!(config.global.mode, ProxyMode::Direct);
    for scope in [NetworkScope::Content, NetworkScope::Api, NetworkScope::Pi] {
        assert_eq!(config.effective_for(scope).mode, ProxyMode::Direct);
    }
    config.global = ProxyRule::custom("socks5h://localhost:1080");
    config.api.mode = ProxyMode::Direct;
    config.pi.mode = ProxyMode::System;
    let config = config.normalized().unwrap();
    assert_eq!(config.effective_for(NetworkScope::Content), &config.global);
    assert_eq!(
        config.effective_for(NetworkScope::Api).mode,
        ProxyMode::Direct
    );
    assert_eq!(
        config.effective_for(NetworkScope::Pi).mode,
        ProxyMode::System
    );
    config
        .build_client_with_env(NetworkScope::Content, Client::builder(), &env(&[]))
        .unwrap();
}

#[test]
fn url_validation_normalization_and_redaction() {
    for scheme in ["http", "https", "socks5", "socks5h"] {
        for host in ["127.0.0.1", "proxy.example", "[::1]"] {
            let raw = format!("{scheme}://user:p%40ss@{host}:1234");
            let rule = ProxyRule::custom(&format!("  {raw}  "))
                .normalized()
                .unwrap();
            assert!(rule.has_auth());
            assert!(rule.url.contains("user:p%40ss@"));
            assert!(!rule.redacted_url().contains('@'));
            assert!(!format!("{rule:?}").contains("p%40ss"));
            assert!(!format!("{rule:?}").contains("user"));
            let config = ProxyConfig {
                global: rule,
                ..Default::default()
            };
            config
                .build_client_with_env(NetworkScope::Content, Client::builder(), &env(&[]))
                .unwrap();
        }
    }
    for raw in [
        "",
        "localhost:7890",
        "ftp://user:private@host",
        "socks4://host:1080",
        "http://",
        "http:///host",
        "http://host:65536",
        "http://host:0",
        "http://host/path",
        "http://host/path/..",
        "http://user:private@host?token=secret",
        "http://host#fragment",
        "http://host\n:80",
        "http://host\\path",
        "https://host:bad",
    ] {
        let rule = ProxyRule::custom(raw);
        let error = rule.normalized().unwrap_err();
        let shown = format!("{error:?} {error} {rule:?}");
        assert!(!shown.contains("private"));
        assert!(!shown.contains("secret"));
    }
    let rule = ProxyRule::custom("https://user:pass@Proxy.Example:443/")
        .normalized()
        .unwrap();
    assert_eq!(rule.redacted_url(), "https://proxy.example/");
    assert!(!rule.has_auth() || rule.url.contains('@'));
}

#[test]
fn form_only_reuses_authentication_for_the_same_proxy_endpoint() {
    for scheme in ["http", "https", "socks5", "socks5h"] {
        let existing =
            ProxyRule::custom(format!("{scheme}://old-user:old-pass@proxy.example:8080"));
        let changed_scheme = if scheme == "http" { "https" } else { "http" };
        for (address, username, password, clear, expected) in [
            (
                format!("{scheme}://proxy.example:8080"),
                None,
                None,
                false,
                Some(("old-user", "old-pass")),
            ),
            (
                format!("{scheme}://proxy.example:8080/"),
                None,
                None,
                false,
                Some(("old-user", "old-pass")),
            ),
            (
                format!("{}://PROXY.EXAMPLE:8080", scheme.to_uppercase()),
                None,
                None,
                false,
                Some(("old-user", "old-pass")),
            ),
            (
                format!("{scheme}://proxy.example:8081"),
                None,
                None,
                false,
                None,
            ),
            (
                format!("{scheme}://other.example:8080"),
                None,
                None,
                false,
                None,
            ),
            (
                format!("{changed_scheme}://proxy.example:8080"),
                None,
                None,
                false,
                None,
            ),
            (
                format!("{scheme}://proxy.example:8080"),
                None,
                None,
                true,
                None,
            ),
            (
                format!("{scheme}://other.example:8080"),
                Some("new-user"),
                Some("new-pass"),
                false,
                Some(("new-user", "new-pass")),
            ),
            (
                format!("{scheme}://url-user:url-pass@other.example:8080"),
                None,
                None,
                false,
                Some(("url-user", "url-pass")),
            ),
            (
                format!("{scheme}://url-user:url-pass@other.example:8080"),
                Some("new-user"),
                Some("new-pass"),
                true,
                None,
            ),
        ] {
            let next = ProxyRule::from_form(
                &existing,
                ProxyMode::Custom,
                &address,
                "",
                username,
                password,
                clear,
            )
            .unwrap();
            let url = Url::parse(&next.url).unwrap();
            assert_eq!(next.has_auth(), expected.is_some());
            assert_eq!(
                (url.username(), url.password()),
                expected
                    .map(|(user, pass)| (user, Some(pass)))
                    .unwrap_or(("", None))
            );
        }
    }
    for (scheme, port) in [("http", 80), ("https", 443)] {
        let existing =
            ProxyRule::custom(format!("{scheme}://old-user:old-pass@proxy.example:{port}"));
        let next = ProxyRule::from_form(
            &existing,
            ProxyMode::Custom,
            &format!("{scheme}://proxy.example/"),
            "",
            None,
            None,
            false,
        )
        .unwrap();
        assert!(next.has_auth());
    }
}

#[test]
fn default_no_proxy_is_added_without_being_stored() {
    assert_eq!(merge_no_proxy("").unwrap(), DEFAULT_NO_PROXY);
    assert_eq!(
        merge_no_proxy("10.0.0.0/8, localhost,192.168.0.0/16").unwrap(),
        "localhost,127.0.0.1,::1,10.0.0.0/8,192.168.0.0/16"
    );
    assert_eq!(ProxyConfig::default().global.no_proxy, "");
}

#[test]
fn no_proxy_is_normalized_and_rejects_unsupported_syntax() {
    assert_eq!(
        normalize_no_proxy(
            " LOCALHOST, .Example.COM\n127.0.0.1, ::1, 10.0.0.0/8,::/0,*,localhost "
        )
        .unwrap(),
        "localhost,.example.com,127.0.0.1,::1,10.0.0.0/8,::/0,*"
    );
    for raw in [
        "http://host",
        "user:secret@host",
        "host:8080",
        "10.0.0.0/33",
        "::/129",
        "*.example.com",
        "a..b",
        "-bad.host",
    ] {
        assert!(normalize_no_proxy(raw).is_err());
    }
    assert_eq!(normalize_no_proxy(" , \n\t").unwrap(), "");
}

#[test]
fn system_no_proxy_converts_supported_entries_without_rejecting_the_client() {
    let config = ProxyConfig {
        global: ProxyRule {
            mode: ProxyMode::System,
            ..Default::default()
        },
        ..Default::default()
    };
    for (raw, expected) in [
        ("*.service.example", "service.example"),
        (".service.example", ".service.example"),
        ("host.example:8080", "host.example"),
        ("*.service.example:443", "service.example"),
        ("192.0.2.1:8080", "192.0.2.1"),
        ("2001:db8::1", "2001:db8::1"),
        ("[2001:db8::1]", "2001:db8::1"),
        ("[2001:db8::1]:8080", "2001:db8::1"),
        ("2001:db8::8080", "2001:db8::8080"),
        ("*internal*", ""),
        ("https://service.example:8080", ""),
        ("user:fake-pass@service.example", ""),
        ("host.example:65536", ""),
        ("[2001:db8::1]:bad", ""),
        (" , \n\t", ""),
        (
            "*internal*,https://service.example,user:fake-pass@service.example,:8080",
            "",
        ),
        (
            "*,192.0.2.0/24,2001:db8::/32",
            "*,192.0.2.0/24,2001:db8::/32",
        ),
        (
            " *.Service.Example,,service.example\n*internal*,host.example:8080 ",
            "service.example,host.example",
        ),
    ] {
        assert_eq!(normalize_system_no_proxy(raw), expected);
        // Windows and environment input share conversions; its native separator differs.
        assert_eq!(
            crate::platform::normalize_windows_override(
                raw.trim().replace([',', '\n', '\t'], ";").as_str()
            )
            .unwrap_or_default(),
            expected
        );
        for key in ["NO_PROXY", "no_proxy"] {
            let values = env(&[("HTTP_PROXY", "http://proxy.example:8080"), (key, raw)]);
            for scope in [NetworkScope::Content, NetworkScope::Api, NetworkScope::Pi] {
                client(&config, scope, &values);
            }
        }
    }
}

#[test]
fn storage_round_trip_defaults_and_errors_preserve_old_value() {
    let conn = database();
    assert_eq!(load(&conn).unwrap(), ProxyConfig::default());
    db::set_setting(&conn, SETTING_KEY, "{}").unwrap();
    assert_eq!(load(&conn).unwrap(), ProxyConfig::default());
    let config = ProxyConfig {
        global: ProxyRule::custom("http://user:secret@localhost:1234"),
        ..Default::default()
    };
    let saved = save(&conn, &config).unwrap();
    assert_eq!(load(&conn).unwrap(), saved);
    assert_eq!(
        saved.without_auth().unwrap().global.url,
        "http://localhost:1234/"
    );
    let mut bad = config.clone();
    bad.global.mode = ProxyMode::Inherit;
    assert!(save(&conn, &bad).is_err());
    assert_eq!(load(&conn).unwrap(), saved);
    conn.execute_batch("CREATE TRIGGER fail_proxy BEFORE UPDATE ON settings BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
    assert!(save(&conn, &ProxyConfig::default()).is_err());
    assert_eq!(load(&conn).unwrap(), saved);
}

#[test]
fn invalid_stored_config_fails_without_exposing_credentials_or_falling_back_to_direct() {
    let conn = database();
    for raw in [
        r#"{"global":{"mode":"private-secret"}}"#,
        r#"{"version":0}"#,
        r#"{"version":99}"#,
        r#"{"private-secret":true}"#,
        "broken-private-secret",
    ] {
        db::set_setting(&conn, SETTING_KEY, raw).unwrap();
        let error = load(&conn).unwrap_err();
        assert!(!format!("{error:?} {error}").contains("private-secret"));
        assert!(strip_backup_auth(&conn).is_err());
        assert_eq!(
            db::get_setting(&conn, SETTING_KEY).unwrap().as_deref(),
            Some(raw)
        );
    }
}

#[test]
fn auth_is_removed_from_all_rules_including_inactive_values() {
    let conn = database();
    let mut config = ProxyConfig::default();
    for rule in [
        &mut config.global,
        &mut config.content,
        &mut config.api,
        &mut config.pi,
    ] {
        rule.url = "socks5h://proxy-user:proxy-secret@localhost:1080".into();
    }
    save(&conn, &config).unwrap();
    strip_backup_auth(&conn).unwrap();
    let clean = load(&conn).unwrap();
    for rule in [&clean.global, &clean.content, &clean.api, &clean.pi] {
        assert_eq!(rule.url, "socks5h://localhost:1080");
        assert!(!rule.has_auth());
    }
    assert!(config.global.has_auth());
}

#[test]
fn standalone_tool_keeps_standard_environment_priority_and_debug_is_safe() {
    let values = env(&[
        ("HTTP_PROXY", "http://upper:private@host.example:1234"),
        ("http_proxy", "http://lower.example:1234"),
        ("HTTPS_PROXY", " "),
        ("https_proxy", "http://secure.example:1234"),
        ("NO_PROXY", " "),
        ("no_proxy", "*.bypass.example:443"),
        ("UNRELATED", "secret"),
    ]);
    assert_eq!(
        values.value("HTTP_PROXY", "http_proxy"),
        Some("http://upper:private@host.example:1234")
    );
    assert_eq!(
        values.value("HTTPS_PROXY", "https_proxy"),
        Some("http://secure.example:1234")
    );
    assert_eq!(
        values.value("NO_PROXY", "no_proxy"),
        Some("*.bypass.example:443")
    );
    assert!(!format!("{values:?}").contains("private"));
    assert!(!format!("{values:?}").contains("secret"));
}

#[test]
fn empty_or_partial_host_snapshot_never_falls_back_to_pi_variables() {
    let config = ProxyConfig {
        pi: ProxyRule {
            no_proxy: "pi-only.example".into(),
            ..ProxyRule::custom("http://pi-proxy.example:8080")
        },
        ..Default::default()
    };
    let child = config.child_env(NetworkScope::Pi, &env(&[])).unwrap();
    let values = vars(&child);
    for inherited in [values.as_slice(), &[("RRSS_HOST_HTTP_PROXY", "")][..]] {
        let tool = ProxyEnvironment::from_lookup(|key| {
            inherited
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_owned())
                .or_else(|| {
                    PROXY_ENV_KEYS
                        .contains(&key)
                        .then(|| "http://pi-proxy.example:8080".into())
                })
        });
        for pair in PROXY_ENV_KEYS.chunks_exact(2) {
            assert_eq!(tool.value(pair[0], pair[1]), None);
        }
    }
}

fn vars(env: &ChildProxyEnv) -> Vec<(&str, &str)> {
    env.vars().iter().map(|(k, v)| (*k, v.as_str())).collect()
}

#[test]
fn child_env_only_exposes_host_snapshot_when_tools_need_system_proxy() {
    use ProxyMode::{Custom, Direct, Inherit, System};

    let inherited = env(&[
        (
            "HTTP_PROXY",
            "http://host-user:host-pass@host-proxy.example:8080",
        ),
        ("HTTPS_PROXY", "http://host-https.example:8443"),
        ("ALL_PROXY", "socks5h://host-all.example:1080"),
        ("NO_PROXY", "host-bypass.example"),
    ]);
    let rule = |mode| ProxyRule {
        mode,
        ..ProxyRule::custom("http://configured.example:8080")
    };
    for (global, api, content, needed) in [
        (Direct, Inherit, Inherit, false),
        (Custom, Inherit, Inherit, false),
        (System, Direct, Custom, false),
        (System, Custom, Direct, false),
        (Direct, System, Direct, true),
        (Direct, Custom, System, true),
        (System, Inherit, Direct, true),
        (System, Custom, Inherit, true),
        (System, Inherit, Inherit, true),
    ] {
        for pi in [Direct, Custom, System] {
            let config = ProxyConfig {
                global: rule(global),
                api: rule(api),
                content: rule(content),
                pi: rule(pi),
                ..Default::default()
            };
            let child = config.child_env(NetworkScope::Pi, &inherited).unwrap();
            let values = vars(&child);
            for (key, original) in HOST_PROXY_ENV_KEYS.into_iter().zip(&inherited.host_values) {
                assert_eq!(
                    values
                        .iter()
                        .find(|(name, _)| *name == key)
                        .map(|(_, value)| *value),
                    Some(if needed { original.as_str() } else { "" })
                );
            }
            if pi == Direct {
                assert!(
                    values
                        .iter()
                        .all(|(key, _)| HOST_PROXY_ENV_KEYS.contains(key))
                );
            }
            if !needed && pi != System {
                assert!(values.iter().all(|(_, value)| !value.contains("host-")));
                assert!(child.secrets().iter().all(|value| !value.contains("host-")));
            }
        }
    }
}

#[test]
fn child_env_custom_sets_every_proxy_pair_and_merges_bypass() {
    for url in [
        "http://user:p%40ss@proxy.test:8080",
        "socks5h://proxy.test:1080",
    ] {
        let config = ProxyConfig {
            pi: ProxyRule {
                no_proxy: "Internal.Test, 10.0.0.0/8".into(),
                ..ProxyRule::custom(url)
            },
            ..Default::default()
        };
        let child = config.child_env(NetworkScope::Pi, &env(&[])).unwrap();
        let expected = config.normalized().unwrap().pi.url;
        let got = vars(&child);
        for key in [
            "HTTP_PROXY",
            "http_proxy",
            "HTTPS_PROXY",
            "https_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            assert!(got.contains(&(key, expected.as_str())), "{key}: {got:?}");
        }
        for key in ["NO_PROXY", "no_proxy"] {
            assert!(got.contains(&(key, "localhost,127.0.0.1,::1,internal.test,10.0.0.0/8")));
        }
        assert_eq!(got.len(), 8 + HOST_PROXY_ENV_KEYS.len());
        assert!(!format!("{child:?}").contains("p%40ss"));
    }
}

#[test]
fn child_env_system_passes_environment_values_and_adds_loopback_bypass() {
    let system = ProxyConfig {
        global: ProxyRule {
            mode: ProxyMode::System,
            ..Default::default()
        },
        ..Default::default()
    };
    // 环境里 RRSS 自己不收的写法（*.example、带端口）原样交给 Pi，不因此拒绝启动。
    let child = system
        .child_env(
            NetworkScope::Pi,
            &env(&[
                ("https_proxy", "http://lower.test:3128"),
                ("ALL_PROXY", "socks5://all.test:1080"),
                ("no_proxy", "*.corp.example, localhost:8080,LOCALHOST"),
            ]),
        )
        .unwrap();
    assert_eq!(
        vars(&child)
            .into_iter()
            .filter(|(key, _)| PROXY_ENV_KEYS.contains(key))
            .collect::<Vec<_>>(),
        [
            ("HTTPS_PROXY", "http://lower.test:3128"),
            ("https_proxy", "http://lower.test:3128"),
            ("ALL_PROXY", "socks5://all.test:1080"),
            ("all_proxy", "socks5://all.test:1080"),
            (
                "NO_PROXY",
                "localhost,127.0.0.1,::1,*.corp.example,localhost:8080"
            ),
            (
                "no_proxy",
                "localhost,127.0.0.1,::1,*.corp.example,localhost:8080"
            ),
        ]
    );
}

#[test]
fn child_env_display_and_secrets_hide_authentication() {
    let config = ProxyConfig {
        global: ProxyRule::custom("http://proxy-user:pa%3Ass@proxy.test:8080"),
        api: ProxyRule {
            mode: ProxyMode::System,
            ..Default::default()
        },
        ..Default::default()
    };
    let child = config
        .child_env(
            NetworkScope::Pi,
            &env(&[
                (
                    "HTTP_PROXY",
                    "http://host-user:host%3Apass@host-proxy.example:8080",
                ),
                ("NO_PROXY", "host-bypass.example"),
            ]),
        )
        .unwrap();
    let shown = format!("{:?}", child.redacted());
    assert!(
        shown.contains("http://proxy.test:8080/（认证已隐藏）"),
        "{shown}"
    );
    for secret in [
        "proxy-user",
        "pa%3Ass",
        "pa:ss",
        "host-user",
        "host%3Apass",
        "host:pass",
    ] {
        assert!(!shown.contains(secret), "{shown}");
        assert!(!format!("{child:?}").contains(secret));
        assert!(child.secrets().iter().any(|s| s == secret), "{secret}");
        assert!(
            !crate::logsafe::scrub_secrets_with(secret, child.secrets().iter().map(String::as_str))
                .contains(secret)
        );
    }
    assert!(!shown.contains("host-bypass.example"));
    // 坏配置报脱敏错误，不退回直连。
    let broken = ProxyConfig {
        version: 999,
        ..Default::default()
    };
    assert!(broken.child_env(NetworkScope::Pi, &env(&[])).is_err());
}

async fn read_headers(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    loop {
        bytes.push(stream.read_u8().await.unwrap());
        if bytes.ends_with(b"\r\n\r\n") {
            break;
        }
        assert!(bytes.len() < 16_384);
    }
    String::from_utf8(bytes).unwrap()
}

async fn respond(stream: &mut TcpStream) {
    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
        .await
        .unwrap();
}

async fn http_endpoint() -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let headers = read_headers(&mut stream).await;
        respond(&mut stream).await;
        headers
    });
    (url, task)
}

async fn headers(task: tokio::task::JoinHandle<String>) -> String {
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn tool_system_proxy_uses_host_environment_for_every_pi_mode_and_scope() {
    for mode in [ProxyMode::Custom, ProxyMode::Direct, ProxyMode::System] {
        for scope in [NetworkScope::Api, NetworkScope::Content] {
            // Exercise Linux, unavailable WinINet, and partial Windows settings
            // with either an absent or a configured native bypass list.
            let windows = crate::platform::parse_windows_proxy_server("https=wininet.example:8443");
            for native in [
                crate::platform::SystemProxySnapshot::default(),
                crate::platform::SystemProxySnapshot {
                    unavailable: true,
                    ..Default::default()
                },
                windows.clone(),
                crate::platform::SystemProxySnapshot {
                    no_proxy: crate::platform::normalize_windows_override(
                        "*.bypass.example;native-only.example",
                    ),
                    ..windows
                },
            ] {
                let (proxy, proxy_task) = http_endpoint().await;
                let (target, bypass_task) = http_endpoint().await;
                let target = Url::parse(&target).unwrap();
                let host = env(&[
                    ("HTTP_PROXY", &proxy),
                    ("http_proxy", "http://unused.example:8080"),
                    ("HTTPS_PROXY", " "),
                    ("https_proxy", "http://host-https.example:8443"),
                    ("ALL_PROXY", "socks5h://host-all.example:1080"),
                    ("all_proxy", "http://unused.example:8080"),
                    ("NO_PROXY", " "),
                    ("no_proxy", "*.bypass.example:443"),
                ])
                .with_system(&native);
                let config = ProxyConfig {
                    global: ProxyRule {
                        mode: ProxyMode::System,
                        ..Default::default()
                    },
                    pi: ProxyRule {
                        mode,
                        no_proxy: "pi-only.example".into(),
                        ..ProxyRule::custom("http://pi-proxy.example:8080")
                    },
                    ..Default::default()
                };
                let child = config.child_env(NetworkScope::Pi, &host).unwrap();
                let tool = env(&vars(&child)).with_system(&native);
                assert_eq!(tool.value("HTTP_PROXY", "http_proxy"), Some(proxy.as_str()));
                assert_eq!(
                    tool.value("HTTPS_PROXY", "https_proxy"),
                    Some(
                        native
                            .https
                            .as_deref()
                            .unwrap_or("http://host-https.example:8443")
                    )
                );
                assert_eq!(
                    tool.value("ALL_PROXY", "all_proxy"),
                    Some("socks5h://host-all.example:1080")
                );
                assert_eq!(
                    tool.value("NO_PROXY", "no_proxy"),
                    Some(native.no_proxy.as_deref().unwrap_or("*.bypass.example:443"))
                );
                // Forward the raw environment, without freezing native settings
                // into the fallback used by the next process.
                assert!(
                    vars(&child)
                        .contains(&("RRSS_HOST_HTTPS_PROXY", "http://host-https.example:8443"))
                );
                assert!(vars(&child).contains(&("RRSS_HOST_NO_PROXY", "*.bypass.example:443")));
                let client = config
                    .build_client_with_env(
                        scope,
                        Client::builder()
                            .timeout(Duration::from_secs(3))
                            .resolve("bypass.example", target.socket_addrs(|| None).unwrap()[0]),
                        &tool,
                    )
                    .unwrap();
                client
                    .get("http://target.example/article")
                    .send()
                    .await
                    .unwrap();
                assert!(
                    headers(proxy_task)
                        .await
                        .starts_with("GET http://target.example/article ")
                );
                client
                    .get(format!("http://bypass.example:{}/", target.port().unwrap()))
                    .send()
                    .await
                    .unwrap();
                assert!(headers(bypass_task).await.starts_with("GET / HTTP/1.1"));
            }
        }
    }
}

#[tokio::test]
async fn custom_http_proxy_preserves_headers_and_does_not_use_environment_bypass() {
    let (url, task) = http_endpoint().await;
    let mut config = ProxyConfig {
        global: ProxyRule::custom(&url),
        ..Default::default()
    };
    config.global.url = url.replace("http://", "http://user:pass@");
    let client = config
        .build_client_with_env(
            NetworkScope::Content,
            Client::builder()
                .timeout(Duration::from_secs(3))
                .user_agent("rrss-proxy-test"),
            &env(&[("NO_PROXY", "*"), ("ALL_PROXY", "http://127.0.0.1:1")]),
        )
        .unwrap();
    assert_eq!(
        client
            .get("http://target.invalid/article")
            .header("Referer", "https://referrer.invalid/")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "ok"
    );
    let request = headers(task).await.to_ascii_lowercase();
    assert!(request.starts_with("get http://target.invalid/article http/1.1"));
    assert!(request.contains("user-agent: rrss-proxy-test"));
    assert!(request.contains("referer: https://referrer.invalid/"));
    assert!(request.contains("proxy-authorization: basic dxnlcjpwyxnz"));
}

#[tokio::test]
async fn default_loopback_addresses_bypass_custom_proxy() {
    let (url, task) = http_endpoint().await;
    let config = ProxyConfig {
        global: ProxyRule::custom("http://127.0.0.1:1"),
        ..Default::default()
    };
    let localhost = url.replace("127.0.0.1", "localhost");
    let response = client(&config, NetworkScope::Content, &env(&[]))
        .get(localhost)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(headers(task).await.starts_with("GET / HTTP/1.1"));
}

#[tokio::test]
async fn bypass_supports_domain_ip_cidr_and_wildcard() {
    for bypass in ["localhost", "127.0.0.1", "127.0.0.0/8", "*"] {
        let (url, task) = http_endpoint().await;
        let host = if bypass == "localhost" {
            url.replace("127.0.0.1", "localhost")
        } else {
            url
        };
        let mut config = ProxyConfig {
            global: ProxyRule::custom("http://127.0.0.1:1"),
            ..Default::default()
        };
        config.global.no_proxy = bypass.into();
        assert_eq!(
            client(&config, NetworkScope::Content, &env(&[]))
                .get(&host)
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        assert!(headers(task).await.starts_with("GET / HTTP/1.1"));
    }
}

#[tokio::test]
async fn scopes_can_use_distinct_routes_at_the_same_time() {
    let (global, global_task) = http_endpoint().await;
    let (content, content_task) = http_endpoint().await;
    let (direct, direct_task) = http_endpoint().await;
    let config = ProxyConfig {
        global: ProxyRule::custom(&global),
        content: ProxyRule::custom(&content),
        pi: ProxyRule::default(),
        ..Default::default()
    };
    let env = env(&[]);
    let api = client(&config, NetworkScope::Api, &env);
    let content = client(&config, NetworkScope::Content, &env);
    let pi = client(&config, NetworkScope::Pi, &env);
    let (a, c, p) = tokio::join!(
        api.get("http://model.invalid/").send(),
        content.get("http://feed.invalid/").send(),
        pi.get(&direct).send()
    );
    assert_eq!(a.unwrap().status(), 200);
    assert_eq!(c.unwrap().status(), 200);
    assert_eq!(p.unwrap().status(), 200);
    assert!(headers(global_task).await.contains("http://model.invalid/"));
    assert!(headers(content_task).await.contains("http://feed.invalid/"));
    assert!(headers(direct_task).await.starts_with("GET / HTTP/1.1"));
}

#[tokio::test]
async fn system_http_precedence_all_fallback_and_no_proxy() {
    for key in ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] {
        let (proxy, task) = http_endpoint().await;
        let mut config = ProxyConfig::default();
        config.global.mode = ProxyMode::System;
        let values = env(&[(key, &proxy), ("HTTPS_PROXY", "http://127.0.0.1:1")]);
        assert_eq!(
            client(&config, NetworkScope::Content, &values)
                .get("http://target.invalid/")
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        assert!(
            headers(task)
                .await
                .starts_with("GET http://target.invalid/")
        );
    }
    let (proxy, task) = http_endpoint().await;
    let mut config = ProxyConfig::default();
    config.global.mode = ProxyMode::System;
    let values = env(&[
        ("HTTP_PROXY", &proxy),
        ("http_proxy", "http://127.0.0.1:1"),
        ("ALL_PROXY", "http://127.0.0.1:1"),
    ]);
    client(&config, NetworkScope::Api, &values)
        .get("http://target.invalid/")
        .send()
        .await
        .unwrap();
    headers(task).await;
    let (target, task) = http_endpoint().await;
    let values = env(&[
        ("ALL_PROXY", "http://127.0.0.1:1"),
        ("NO_PROXY", "127.0.0.1"),
    ]);
    client(&config, NetworkScope::Pi, &values)
        .get(&target)
        .send()
        .await
        .unwrap();
    headers(task).await;
}

#[tokio::test]
async fn direct_ignores_process_proxy_environment() {
    // 单独子进程设置真实环境，避免 unsafe set_var 影响并行测试。
    if std::env::var_os("RRSS_PROXY_ENV_CHILD").is_none() {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "network::tests::direct_ignores_process_proxy_environment",
                "--nocapture",
            ])
            .env("RRSS_PROXY_ENV_CHILD", "1");
        for key in PROXY_ENV_KEYS {
            command.env(
                key,
                if key.eq_ignore_ascii_case("no_proxy") {
                    ""
                } else {
                    "http://127.0.0.1:1"
                },
            );
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let (target, task) = http_endpoint().await;
    let client = ProxyConfig::default()
        .build_client(
            NetworkScope::Content,
            Client::builder().timeout(Duration::from_secs(3)),
        )
        .unwrap();
    assert_eq!(client.get(&target).send().await.unwrap().status(), 200);
    assert!(headers(task).await.starts_with("GET / HTTP/1.1"));
}

#[derive(Debug)]
struct SocksRequest {
    address_type: u8,
    host: String,
    port: u16,
    auth: Option<(String, String)>,
    headers: String,
}

async fn socks_endpoint(auth: bool) -> (String, tokio::task::JoinHandle<SocksRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        assert_eq!(stream.read_u8().await.unwrap(), 5);
        let len = stream.read_u8().await.unwrap();
        let mut methods = vec![0; len as usize];
        stream.read_exact(&mut methods).await.unwrap();
        let method = if auth { 2 } else { 0 };
        assert!(methods.contains(&method));
        stream.write_all(&[5, method]).await.unwrap();
        let auth = if auth {
            assert_eq!(stream.read_u8().await.unwrap(), 1);
            let user = socks_string(&mut stream).await;
            let password = socks_string(&mut stream).await;
            stream.write_all(&[1, 0]).await.unwrap();
            Some((user, password))
        } else {
            None
        };
        let mut header = [0; 4];
        stream.read_exact(&mut header).await.unwrap();
        assert_eq!(&header[..3], &[5, 1, 0]);
        let host = match header[3] {
            1 => {
                let mut ip = [0; 4];
                stream.read_exact(&mut ip).await.unwrap();
                std::net::Ipv4Addr::from(ip).to_string()
            }
            4 => {
                let mut ip = [0; 16];
                stream.read_exact(&mut ip).await.unwrap();
                std::net::Ipv6Addr::from(ip).to_string()
            }
            3 => socks_string(&mut stream).await,
            _ => panic!("unexpected SOCKS address type"),
        };
        let port = stream.read_u16().await.unwrap();
        stream
            .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])
            .await
            .unwrap();
        let headers = read_headers(&mut stream).await;
        respond(&mut stream).await;
        SocksRequest {
            address_type: header[3],
            host,
            port,
            auth,
            headers,
        }
    });
    (address, task)
}

async fn socks_string(stream: &mut TcpStream) -> String {
    let len = stream.read_u8().await.unwrap();
    let mut bytes = vec![0; len as usize];
    stream.read_exact(&mut bytes).await.unwrap();
    String::from_utf8(bytes).unwrap()
}

#[tokio::test]
async fn socks5_resolves_locally_and_socks5h_sends_domain_with_auth() {
    for (scheme, host, auth) in [
        ("socks5", "127.0.0.2", false),
        ("socks5h", "unresolvable.invalid", false),
        ("socks5h", "unresolvable.invalid", true),
    ] {
        let (address, task) = socks_endpoint(auth).await;
        let credentials = if auth { "user:p%40ss@" } else { "" };
        let config = ProxyConfig {
            global: ProxyRule::custom(&format!("{scheme}://{credentials}{address}")),
            ..Default::default()
        };
        let response = client(&config, NetworkScope::Pi, &env(&[]))
            .get(format!("http://{host}:8080/feed"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.text().await.unwrap(), "ok");
        let request = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(request.port, 8080);
        if scheme == "socks5h" {
            assert_eq!(request.address_type, 3);
            assert_eq!(request.host, host);
        } else {
            assert!([1, 4].contains(&request.address_type));
            assert!(request.host.parse::<IpAddr>().unwrap().is_loopback());
        }
        assert_eq!(request.auth, auth.then(|| ("user".into(), "p@ss".into())));
        assert!(request.headers.starts_with("GET /feed HTTP/1.1"));
        assert!(
            !request
                .headers
                .to_ascii_lowercase()
                .contains("proxy-authorization")
        );
    }
}

#[tokio::test]
async fn subscription_client_keeps_ua_and_conditional_headers() {
    let (proxy, task) = http_endpoint().await;
    let config = ProxyConfig {
        global: ProxyRule::custom(&proxy),
        ..Default::default()
    };
    let client = crate::ingest::http_client_with_proxy(&config).unwrap();
    let response = crate::ingest::fetch_http(
        &client,
        crate::ingest::FetchRequest {
            url: "http://feed.invalid/rss".into(),
            etag: Some("\"revision-1\"".into()),
            last_modified: Some("Mon, 05 Oct 2026 00:00:00 GMT".into()),
        },
    )
    .await
    .unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"ok");
    let request = headers(task).await.to_ascii_lowercase();
    assert!(request.starts_with("get http://feed.invalid/rss http/1.1"));
    assert!(request.contains("user-agent: mozilla/5.0"));
    assert!(request.contains("if-none-match: \"revision-1\""));
    assert!(request.contains("if-modified-since: mon, 05 oct 2026 00:00:00 gmt"));
}

#[tokio::test]
async fn timeout_and_connection_errors_are_safe() {
    let config = ProxyConfig {
        global: ProxyRule::custom("http://private-user:private-password@127.0.0.1:1"),
        ..Default::default()
    };
    let error = client(&config, NetworkScope::Api, &env(&[]))
        .get("https://target.invalid/")
        .send()
        .await
        .unwrap_err();
    let safe = request_error(&error);
    assert_eq!(error_kind(&error), ErrorKind::Refused);
    assert!(matches!(
        &safe,
        Error::Network {
            kind: ErrorKind::Refused,
            ..
        }
    ));
    assert!(!format!("{safe} {safe:?}").contains("private-"));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = ProxyConfig::default()
        .build_client_with_env(
            NetworkScope::Content,
            Client::builder().timeout(Duration::from_millis(30)),
            &env(&[]),
        )
        .unwrap();
    let error = client
        .get(format!("http://{}", listener.local_addr().unwrap()))
        .send()
        .await
        .unwrap_err();
    assert!(error.is_timeout());
    assert_eq!(error_kind(&error), ErrorKind::Timeout);
    assert!(matches!(
        request_error(&error),
        Error::Network {
            kind: ErrorKind::Timeout,
            ..
        }
    ));
}
