use super::*;
use crate::platform::{Os, SystemProxySnapshot};

/// A single browser environment cannot express per-protocol routes or bypasses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebviewRoute {
    Direct,
    /// Let WebView2 resolve WinINet policy, including PAC and bypass lists.
    System,
    Proxy(Url),
}

impl ProxyConfig {
    /// Pure mapping of the current scope, including native/variable precedence.
    /// Errors are fixed display text and never contain proxy credentials.
    pub fn webview_route(
        &self,
        scope: NetworkScope,
        env: &ProxyEnvironment,
        native: &SystemProxySnapshot,
        os: Os,
    ) -> Result<WebviewRoute> {
        let mut config = self.clone();
        // Webview-specific compatibility aliases must be handled before the
        // general HTTP client's stricter no_proxy syntax validation.
        for rule in [
            &mut config.global,
            &mut config.content,
            &mut config.api,
            &mut config.pi,
        ] {
            if loopback_bypasses_only(&rule.no_proxy) {
                rule.no_proxy.clear();
            }
        }
        let config = config.normalized()?;
        let rule = config.effective_for(scope);
        let denied = |reason| {
            invalid(&format!(
                "原网页无法使用此代理：{reason}。请改用阅读视图或系统浏览器，或调整网络代理设置。"
            ))
        };
        let proxy = |raw: &str| {
            let url = parse_proxy_url(raw)?;
            if !url.username().is_empty() || url.password().is_some() {
                return Err(denied("不支持代理认证"));
            }
            if !matches!(url.scheme(), "http" | "socks5") {
                return Err(denied("仅支持 HTTP 或 SOCKS5 代理"));
            }
            // tauri-runtime-wry 2.12.2 reads Url::port(), not port_or_known_default().
            // URL strips HTTP :80, which would produce an empty wry proxy port.
            if url.port().is_none() {
                return Err(denied(
                    "当前 webview 无法表达此代理端口，请使用显式的非默认端口",
                ));
            }
            Ok(WebviewRoute::Proxy(url))
        };
        match rule.mode {
            ProxyMode::Direct => Ok(WebviewRoute::Direct),
            ProxyMode::Custom => {
                if !rule.no_proxy.is_empty() {
                    return Err(denied("不支持不代理列表"));
                }
                proxy(&rule.url)
            }
            ProxyMode::System => {
                if native.unavailable {
                    return Err(denied("无法读取系统代理"));
                }
                if os.is_windows() {
                    return Ok(WebviewRoute::System);
                }
                if native.pac.is_some() || native.auto_detect {
                    return Err(denied("不支持 PAC 或自动代理配置"));
                }
                let env = env.with_system(native);
                let all = env.value("ALL_PROXY", "all_proxy");
                let http = env.value("HTTP_PROXY", "http_proxy").or(all);
                let https = env.value("HTTPS_PROXY", "https_proxy").or(all);
                if http.is_none() && https.is_none() {
                    return Ok(WebviewRoute::Direct);
                }
                if native.bypass_configured || env.value("NO_PROXY", "no_proxy").is_some() {
                    return Err(denied("不支持不代理列表"));
                }
                match (http, https) {
                    (None, None) => Ok(WebviewRoute::Direct),
                    (Some(a), Some(b)) => {
                        let route = proxy(a)?;
                        if route != proxy(b)? {
                            return Err(denied("HTTP 与 HTTPS 线路不同"));
                        }
                        Ok(route)
                    }
                    _ => Err(denied("无法统一 HTTP 与 HTTPS 线路")),
                }
            }
            ProxyMode::Inherit => Err(denied("无效的代理继承配置")),
        }
    }
}

fn loopback_bypasses_only(raw: &str) -> bool {
    raw.split(|c: char| c == ',' || c == ';' || c.is_ascii_whitespace())
        .filter(|part| !part.is_empty())
        .all(|part| {
            let part = part.to_ascii_lowercase();
            matches!(part.as_str(), "localhost" | "::1" | "127.*" | "<local>")
                || part.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
                || part.split_once('/').is_some_and(|(ip, bits)| {
                    match (ip.parse::<IpAddr>(), bits.parse::<u8>()) {
                        (Ok(IpAddr::V4(ip)), Ok(bits)) => {
                            ip.is_loopback() && (8..=32).contains(&bits)
                        }
                        (Ok(IpAddr::V6(ip)), Ok(128)) => ip.is_loopback(),
                        _ => false,
                    }
                })
        })
}

#[cfg(test)]
mod tests;
