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
        let denied = invalid;
        let proxy = |raw: &str| {
            let url = parse_proxy_url(raw)?;
            if !url.username().is_empty() || url.password().is_some() {
                return Err(denied(ProxyError::WebviewAuthUnsupported));
            }
            if !matches!(url.scheme(), "http" | "socks5") {
                return Err(denied(ProxyError::WebviewSchemeUnsupported));
            }
            // tauri-runtime-wry 2.12.2 reads Url::port(), not port_or_known_default().
            // URL strips HTTP :80, which would produce an empty wry proxy port.
            if url.port().is_none() {
                return Err(denied(ProxyError::WebviewPortUnsupported));
            }
            Ok(WebviewRoute::Proxy(url))
        };
        match rule.mode {
            ProxyMode::Direct => Ok(WebviewRoute::Direct),
            ProxyMode::Custom => {
                if !rule.no_proxy.is_empty() {
                    return Err(denied(ProxyError::WebviewBypassUnsupported));
                }
                proxy(&rule.url)
            }
            ProxyMode::System => {
                if native.unavailable {
                    return Err(denied(ProxyError::WebviewSystemUnavailable));
                }
                if os.is_windows() {
                    return Ok(WebviewRoute::System);
                }
                if native.pac.is_some() || native.auto_detect {
                    return Err(denied(ProxyError::WebviewPacUnsupported));
                }
                let env = env.with_system(native);
                let all = env.value("ALL_PROXY", "all_proxy");
                let http = env.value("HTTP_PROXY", "http_proxy").or(all);
                let https = env.value("HTTPS_PROXY", "https_proxy").or(all);
                if http.is_none() && https.is_none() {
                    return Ok(WebviewRoute::Direct);
                }
                if native.bypass_configured || env.value("NO_PROXY", "no_proxy").is_some() {
                    return Err(denied(ProxyError::WebviewBypassUnsupported));
                }
                match (http, https) {
                    (None, None) => Ok(WebviewRoute::Direct),
                    (Some(a), Some(b)) => {
                        let route = proxy(a)?;
                        if route != proxy(b)? {
                            return Err(denied(ProxyError::WebviewRoutesDiffer));
                        }
                        Ok(route)
                    }
                    _ => Err(denied(ProxyError::WebviewRouteIncomplete)),
                }
            }
            ProxyMode::Inherit => Err(denied(ProxyError::WebviewInheritInvalid)),
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
