//! 保存网络代理设置。返回的视图只含脱敏投影（`queries::network_proxy`）；
//! 认证仅在显式填写或清除时改变。保存只写库，已领取的客户端与运行中的 Pi 不受影响。

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

use rrss_core::network::{self, NetworkScope, ProxyConfig, ProxyMode, ProxyRule};

use crate::queries::network_proxy;
use crate::views::{
    NetworkProxyMode, NetworkProxyScope, NetworkProxyTestArgs, NetworkProxyTestView,
    NetworkProxyView, SaveNetworkProxyArgs, SaveNetworkProxyRuleArgs,
};

fn mode(mode: NetworkProxyMode) -> ProxyMode {
    match mode {
        NetworkProxyMode::Direct => ProxyMode::Direct,
        NetworkProxyMode::System => ProxyMode::System,
        NetworkProxyMode::Custom => ProxyMode::Custom,
        NetworkProxyMode::Inherit => ProxyMode::Inherit,
    }
}

fn apply_rule(
    existing: &ProxyRule,
    input: &SaveNetworkProxyRuleArgs,
) -> rrss_core::Result<ProxyRule> {
    ProxyRule::from_form(
        existing,
        mode(input.mode),
        &input.url,
        &input.no_proxy,
        input.username.as_deref(),
        input.password.as_deref(),
        input.clear_auth,
    )
}

/// 草稿 → 完整配置：认证缺省时沿用已保存的值。保存与测活共用，测活结果才与保存后一致。
fn config_from_args(
    existing: &ProxyConfig,
    args: &SaveNetworkProxyArgs,
) -> rrss_core::Result<ProxyConfig> {
    Ok(ProxyConfig {
        version: existing.version,
        global: apply_rule(&existing.global, &args.global)?,
        content: apply_rule(&existing.content, &args.content)?,
        api: apply_rule(&existing.api, &args.api)?,
        pi: apply_rule(&existing.pi, &args.pi)?,
    })
}

pub fn save_network_proxy(
    conn: &Connection,
    args: &SaveNetworkProxyArgs,
) -> rrss_core::Result<NetworkProxyView> {
    let config = config_from_args(&network::load(conn)?, args)?;
    network::save(conn, &config)?;
    network_proxy(conn)
}

/// 测活目标：Pi 下载与更新都来自 GitHub；`rate_limit` 不需要密钥，也不计入未登录配额。
const TEST_TARGET: &str = "https://api.github.com/rate_limit";

/// 用未保存的草稿测一个用途的实际线路（跟随全局时走全局），不写库。
pub async fn test_network_proxy(
    db_path: &Path,
    args: NetworkProxyTestArgs,
) -> rrss_core::Result<NetworkProxyTestView> {
    test_network_proxy_at(db_path, &args, TEST_TARGET).await
}

async fn test_network_proxy_at(
    db_path: &Path,
    args: &NetworkProxyTestArgs,
    target: &str,
) -> rrss_core::Result<NetworkProxyTestView> {
    let draft = args.draft.clone();
    let scope = match args.scope {
        NetworkProxyScope::Content => NetworkScope::Content,
        NetworkProxyScope::Api => NetworkScope::Api,
        NetworkProxyScope::Pi => NetworkScope::Pi,
    };
    let client = crate::execution::core_db(db_path.to_path_buf(), move |conn| {
        let config = config_from_args(&network::load(conn)?, &draft)?;
        config.build_client(
            scope,
            reqwest::Client::builder()
                .user_agent("RRSS proxy health check")
                .timeout(Duration::from_secs(8)),
        )
    })
    .await?;
    let started = std::time::Instant::now();
    let response = client
        .get(target)
        .send()
        .await
        .map_err(|error| network::request_error(&error))?;
    let latency_ms = started.elapsed().as_millis() as u64;
    let status = response.status().as_u16();
    let ok = response.status().is_success();
    let message = if ok {
        format!("GitHub 可访问 · HTTP {status} · {latency_ms} ms")
    } else {
        format!("已连接到 GitHub，但返回 HTTP {status}")
    };
    Ok(NetworkProxyTestView {
        ok,
        latency_ms,
        status,
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::{NetworkProxyMode, SaveNetworkProxyArgs, SaveNetworkProxyRuleArgs};
    use rrss_core::db;
    use tempfile::tempdir;

    fn rule(mode: NetworkProxyMode, url: &str) -> SaveNetworkProxyRuleArgs {
        SaveNetworkProxyRuleArgs {
            mode,
            url: url.into(),
            no_proxy: "example.test".into(),
            username: None,
            password: None,
            clear_auth: false,
        }
    }

    fn args(global: SaveNetworkProxyRuleArgs) -> SaveNetworkProxyArgs {
        let inherited = rule(NetworkProxyMode::Inherit, "");
        SaveNetworkProxyArgs {
            global,
            content: inherited.clone(),
            api: inherited.clone(),
            pi: inherited,
        }
    }

    #[test]
    fn save_projects_a_redacted_view_and_preserves_existing_authentication() {
        let dir = tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("rrss.db")).unwrap();
        let initial = ProxyConfig {
            global: ProxyRule {
                mode: ProxyMode::Custom,
                url: "http://old-user:old-pass@proxy.example:8080".into(),
                no_proxy: "localhost".into(),
            },
            ..Default::default()
        };
        network::save(&conn, &initial).unwrap();

        let view = save_network_proxy(
            &conn,
            &args(rule(NetworkProxyMode::Custom, "http://proxy.example:8080")),
        )
        .unwrap();
        // 只改地址（认证字段缺省）时保留旧认证；返回视图永不带凭据。
        assert_eq!(view.global.url, "http://proxy.example:8080/");
        assert!(view.global.has_auth);
        assert!(!view.global.url.contains("old-user"));
        assert_eq!(
            network::load(&conn).unwrap().global.url,
            "http://old-user:old-pass@proxy.example:8080/"
        );
    }

    #[test]
    fn save_can_replace_or_clear_authentication_without_returning_it() {
        let dir = tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("rrss.db")).unwrap();
        network::save(
            &conn,
            &ProxyConfig {
                global: ProxyRule::custom("socks5h://old-user:old-pass@proxy.example:1080"),
                ..Default::default()
            },
        )
        .unwrap();

        let mut replace = rule(NetworkProxyMode::Custom, "socks5h://proxy.example:1080");
        replace.username = Some("new-user".into());
        replace.password = Some("new-pass".into());
        let view = save_network_proxy(&conn, &args(replace)).unwrap();
        assert!(view.global.has_auth);
        assert!(!view.global.url.contains("new-user"));
        assert_eq!(
            network::load(&conn).unwrap().global.url,
            "socks5h://new-user:new-pass@proxy.example:1080"
        );

        let mut clear = rule(NetworkProxyMode::Custom, "socks5h://proxy.example:1080");
        clear.clear_auth = true;
        let view = save_network_proxy(&conn, &args(clear)).unwrap();
        assert!(!view.global.has_auth);
        assert_eq!(
            network::load(&conn).unwrap().global.url,
            "socks5h://proxy.example:1080"
        );
    }

    #[test]
    fn invalid_proxy_save_does_not_replace_the_old_configuration() {
        let dir = tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("rrss.db")).unwrap();
        let old = ProxyConfig {
            global: ProxyRule::custom("http://proxy.example:8080"),
            ..Default::default()
        };
        network::save(&conn, &old).unwrap();
        let error = save_network_proxy(
            &conn,
            &args(rule(NetworkProxyMode::Custom, "127.0.0.1:7890")),
        )
        .unwrap_err();
        let shown = format!("{error}");
        assert!(shown.contains("代理地址"), "{shown}");
        assert_eq!(network::load(&conn).unwrap(), old.normalized().unwrap());
    }

    #[tokio::test]
    async fn test_uses_the_unsaved_draft_and_keeps_saved_authentication_for_inherited_uses() {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let proxy = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let head = crate::test_net::read_request_head(&mut stream).await;
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            head
        });
        let dir = tempdir().unwrap();
        let path = dir.path().join("rrss.db");
        let conn = db::open_at(&path).unwrap();
        // 已保存：全局无代理、Pi 单独走带认证的代理。
        network::save(
            &conn,
            &ProxyConfig {
                pi: ProxyRule::custom(format!("http://proxy-user:proxy-pass@{addr}")),
                ..Default::default()
            },
        )
        .unwrap();
        // 草稿：全局改走同一代理（不重填认证），Pi 改回跟随全局，未保存。
        let mut draft = args(rule(NetworkProxyMode::Custom, &format!("http://{addr}")));
        draft.pi = rule(NetworkProxyMode::Inherit, "");
        let view = test_network_proxy_at(
            &path,
            &NetworkProxyTestArgs {
                scope: NetworkProxyScope::Content,
                draft,
            },
            "http://proxy-check.invalid/rate_limit",
        )
        .await
        .unwrap();
        assert!(view.ok, "{}", view.message);
        assert_eq!(view.status, 200);
        let head = proxy.await.unwrap();
        assert!(
            head.starts_with("get http://proxy-check.invalid/rate_limit"),
            "{head}"
        );
        // 全局草稿没有认证、旧全局也没有：不会把 Pi 的认证挪给全局。
        assert!(!head.contains("proxy-authorization"), "{head}");
        // 测活不写库。
        assert_eq!(network::load(&conn).unwrap().global.mode, ProxyMode::Direct);
    }

    #[tokio::test]
    async fn test_an_inherited_use_sends_the_saved_global_authentication() {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let proxy = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let head = crate::test_net::read_request_head(&mut stream).await;
            stream
                .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            head
        });
        let dir = tempdir().unwrap();
        let path = dir.path().join("rrss.db");
        let conn = db::open_at(&path).unwrap();
        network::save(
            &conn,
            &ProxyConfig {
                global: ProxyRule::custom(format!("http://proxy-user:proxy-pass@{addr}")),
                ..Default::default()
            },
        )
        .unwrap();
        // 前端草稿只有脱敏地址；跟随全局的 Pi 测活仍应带上已保存的全局认证。
        let view = test_network_proxy_at(
            &path,
            &NetworkProxyTestArgs {
                scope: NetworkProxyScope::Pi,
                draft: args(rule(NetworkProxyMode::Custom, &format!("http://{addr}/"))),
            },
            "http://proxy-check.invalid/rate_limit",
        )
        .await
        .unwrap();
        assert!(view.ok);
        let head = proxy.await.unwrap();
        // base64("proxy-user:proxy-pass")，请求头已转小写。
        assert!(
            head.contains("proxy-authorization: basic chjvehktdxnlcjpwcm94es1wyxnz"),
            "{head}"
        );
    }
}
