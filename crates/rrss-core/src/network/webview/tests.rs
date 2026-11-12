use super::*;

fn env(pairs: &[(&str, &str)]) -> ProxyEnvironment {
    ProxyEnvironment::from_lookup(|key| {
        pairs
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| (*v).into())
    })
}

fn system() -> ProxyConfig {
    ProxyConfig {
        global: ProxyRule {
            mode: ProxyMode::System,
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn webview_custom_mapping_and_redacted_rejections() {
    for (url, bypass, accepted) in [
        ("http://proxy.example:8080", "", true),
        ("socks5://proxy.example:1080", "", true),
        ("https://proxy.example:8443", "", false),
        ("http://proxy.example:80", "", false),
        ("socks5://proxy.example", "", false),
        ("socks5h://proxy.example:1080", "", false),
        (
            "http://synthetic-user:synthetic-pass@proxy.example:8080",
            "",
            false,
        ),
        ("http://proxy.example:8080", "example.com", false),
    ] {
        let config = ProxyConfig {
            content: ProxyRule {
                no_proxy: bypass.into(),
                ..ProxyRule::custom(url)
            },
            ..Default::default()
        };
        let result = config.webview_route(
            NetworkScope::Content,
            &env(&[]),
            &Default::default(),
            Os::Linux,
        );
        assert_eq!(result.is_ok(), accepted);
        if let Err(error) = result {
            let text = error.to_string();
            for secret in ["synthetic-user", "synthetic-pass", "proxy.example"] {
                assert!(!text.contains(secret));
            }
        }
    }
    let route = ProxyConfig::default()
        .webview_route(
            NetworkScope::Content,
            &env(&[("ALL_PROXY", "http://proxy.example:8080")]),
            &Default::default(),
            Os::Linux,
        )
        .unwrap();
    assert_eq!(route, WebviewRoute::Direct);
}

#[test]
fn webview_linux_variable_precedence_and_no_proxy() {
    for pairs in [
        vec![("ALL_PROXY", "socks5://proxy.example:1080")],
        vec![("all_proxy", "http://proxy.example:8080")],
        vec![
            ("HTTP_PROXY", "http://proxy.example:8080"),
            ("HTTPS_PROXY", "http://proxy.example:8080"),
        ],
        vec![
            ("http_proxy", "http://proxy.example:8080"),
            ("https_proxy", "http://proxy.example:8080"),
        ],
        vec![
            ("HTTP_PROXY", ""),
            ("http_proxy", "http://proxy.example:8080"),
            ("ALL_PROXY", "http://proxy.example:8080"),
        ],
    ] {
        assert!(matches!(
            system()
                .webview_route(
                    NetworkScope::Content,
                    &env(&pairs),
                    &Default::default(),
                    Os::Linux
                )
                .unwrap(),
            WebviewRoute::Proxy(_)
        ));
    }
    for pairs in [
        vec![("HTTP_PROXY", "http://a.example:8080")],
        vec![
            ("HTTP_PROXY", "http://a.example:8080"),
            ("HTTPS_PROXY", "http://b.example:8080"),
        ],
        vec![
            ("ALL_PROXY", "http://proxy.example:8080"),
            ("NO_PROXY", "localhost"),
        ],
        vec![
            ("all_proxy", "http://proxy.example:8080"),
            ("no_proxy", "example.com"),
        ],
        vec![
            ("HTTP_PROXY", "invalid"),
            ("ALL_PROXY", "http://proxy.example:8080"),
        ],
    ] {
        assert!(
            system()
                .webview_route(
                    NetworkScope::Content,
                    &env(&pairs),
                    &Default::default(),
                    Os::Linux
                )
                .is_err()
        );
    }
    assert_eq!(
        system()
            .webview_route(
                NetworkScope::Content,
                &env(&[]),
                &Default::default(),
                Os::Linux
            )
            .unwrap(),
        WebviewRoute::Direct
    );
}

#[test]
fn windows_system_uses_webview2_policy_and_still_rejects_unavailable_settings() {
    let split = crate::platform::parse_windows_proxy_server("http=a.example:80;https=b.example:80");
    for native in [
        SystemProxySnapshot::default(),
        split,
        SystemProxySnapshot {
            bypass_configured: true,
            no_proxy: Some("example.com".into()),
            ..Default::default()
        },
        SystemProxySnapshot {
            pac: Some("https://synthetic.example/proxy.pac".into()),
            ..Default::default()
        },
        SystemProxySnapshot {
            auto_detect: true,
            ..Default::default()
        },
    ] {
        // Native Windows policy also takes precedence over incompatible inherited
        // environment variables; do not try to flatten these into proxy_url.
        assert_eq!(
            system()
                .webview_route(
                    NetworkScope::Content,
                    &env(&[
                        ("ALL_PROXY", "socks5h://fallback.example:1080"),
                        ("NO_PROXY", "example.com")
                    ]),
                    &native,
                    Os::Windows
                )
                .unwrap(),
            WebviewRoute::System
        );
    }
    let native = SystemProxySnapshot {
        unavailable: true,
        ..Default::default()
    };
    for os in [Os::Windows, Os::Linux] {
        assert!(
            system()
                .webview_route(NetworkScope::Content, &env(&[]), &native, os)
                .is_err()
        );
    }
}

#[test]
fn linux_system_still_rejects_unsupported_native_policy_when_proxying() {
    for native in [
        SystemProxySnapshot {
            bypass_configured: true,
            ..Default::default()
        },
        SystemProxySnapshot {
            no_proxy: Some("example.com".into()),
            ..Default::default()
        },
        SystemProxySnapshot {
            pac: Some("https://synthetic.example/proxy.pac".into()),
            ..Default::default()
        },
        SystemProxySnapshot {
            auto_detect: true,
            ..Default::default()
        },
    ] {
        assert!(
            system()
                .webview_route(
                    NetworkScope::Content,
                    &env(&[("ALL_PROXY", "http://proxy.example:8080")]),
                    &native,
                    Os::Linux
                )
                .is_err()
        );
    }
}

#[test]
fn no_proxy_without_any_proxy_does_not_block_direct_routes() {
    for pairs in [
        vec![],
        vec![("NO_PROXY", "example.com")],
        vec![("no_proxy", "localhost")],
        vec![
            ("HTTP_PROXY", ""),
            ("HTTPS_PROXY", ""),
            ("ALL_PROXY", ""),
            ("NO_PROXY", "*"),
        ],
    ] {
        let native = SystemProxySnapshot {
            bypass_configured: true,
            no_proxy: Some("example.com".into()),
            ..Default::default()
        };
        assert_eq!(
            system()
                .webview_route(NetworkScope::Content, &env(&pairs), &native, Os::Linux)
                .unwrap(),
            WebviewRoute::Direct
        );
    }
}

#[test]
fn custom_loopback_only_bypasses_are_equivalent_to_empty_bypass() {
    for os in [Os::Windows, Os::Linux] {
        for bypass in [
            "",
            "localhost",
            "127.0.0.1",
            "::1",
            "127.*",
            "<local>",
            "LOCALHOST,127.0.0.1,::1",
            "127.0.0.0/8",
            "::1/128",
            "<LOCAL>;127.*",
        ] {
            let config = ProxyConfig {
                content: ProxyRule {
                    no_proxy: bypass.into(),
                    ..ProxyRule::custom("http://proxy.example:8080")
                },
                ..Default::default()
            };
            assert_eq!(
                config
                    .webview_route(NetworkScope::Content, &env(&[]), &Default::default(), os)
                    .unwrap(),
                WebviewRoute::Proxy(Url::parse("http://proxy.example:8080").unwrap())
            );
        }
        for bypass in [
            "localhost,example.com",
            "127.*;example.com",
            "127.0.0.0/7",
            "::/0",
            "10.0.0.0/8",
            "*",
        ] {
            let config = ProxyConfig {
                content: ProxyRule {
                    no_proxy: bypass.into(),
                    ..ProxyRule::custom("http://proxy.example:8080")
                },
                ..Default::default()
            };
            assert!(
                config
                    .webview_route(NetworkScope::Content, &env(&[]), &Default::default(), os)
                    .is_err()
            );
        }
    }
}

#[test]
fn direct_webview_platform_support_is_explicit() {
    use crate::platform::{Os, webview_direct_args};
    assert!(
        webview_direct_args(Os::Windows)
            .unwrap()
            .contains("--no-proxy-server")
    );
    // Linux adds no Chromium argument: webview_native sets WebKit NoProxy.
    assert_eq!(webview_direct_args(Os::Linux).unwrap(), "");
    assert!(webview_direct_args(Os::Other).is_err());
}

#[test]
fn windows_connection_flags_detect_auto_proxy_and_reject_incomplete_snapshots() {
    for (flags, automatic) in [(1u32, false), (3, false), (5, true), (9, true), (15, true)] {
        let mut data = vec![0u8; 12];
        data[8..12].copy_from_slice(&flags.to_le_bytes());
        assert_eq!(crate::platform::windows_auto_proxy(&data), Some(automatic));
    }
    assert_eq!(crate::platform::windows_auto_proxy(&[0; 8]), None);
    let invalid = crate::platform::parse_windows_proxy_server("unsupported=proxy.example:8080");
    assert!(
        system()
            .webview_route(NetworkScope::Content, &env(&[]), &invalid, Os::Linux)
            .is_err()
    );
}
