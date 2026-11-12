use rusqlite::Connection;

use rrss_core::network::{self, ProxyEnvironment, ProxyMode, ProxyRule};

use crate::views::{NetworkProxyMode, NetworkProxyRuleView, NetworkProxyView, SystemProxyView};

fn rule_view(rule: &ProxyRule) -> NetworkProxyRuleView {
    NetworkProxyRuleView {
        mode: match rule.mode {
            ProxyMode::Direct => NetworkProxyMode::Direct,
            ProxyMode::System => NetworkProxyMode::System,
            ProxyMode::Custom => NetworkProxyMode::Custom,
            ProxyMode::Inherit => NetworkProxyMode::Inherit,
        },
        url: rule.redacted_url(),
        no_proxy: rule.no_proxy.clone(),
        has_auth: rule.has_auth(),
    }
}

pub fn network_proxy(conn: &Connection) -> rrss_core::Result<NetworkProxyView> {
    let config = network::load(conn)?;
    let env = ProxyEnvironment::capture();
    let native = ProxyEnvironment::system_snapshot();
    Ok(NetworkProxyView {
        global: rule_view(&config.global),
        content: rule_view(&config.content),
        api: rule_view(&config.api),
        pi: rule_view(&config.pi),
        system: SystemProxyView {
            http: env.display_value("HTTP_PROXY", "http_proxy"),
            https: env.display_value("HTTPS_PROXY", "https_proxy"),
            all: env.display_value("ALL_PROXY", "all_proxy"),
            no_proxy: env.display_no_proxy(),
            source: native.source.into(),
            pac: native.pac.map(|_| "已配置（未解析）".into()),
        },
    })
}
