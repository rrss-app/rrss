use super::UiErrorKind;
use rrss_core::{
    Error,
    network::{ErrorKind, ProxyError, RequestErrorMessage},
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[serde(rename_all = "snake_case")]
pub enum NetworkMessage {
    Timeout,
    Connect,
    Redirect,
    Decode,
    Other,
    Http,
    ProxyAuth,
}

error_codes!(NetworkError {
    ProxyVersionUnsupported { found: u32, supported: u32 } => "proxy_version_unsupported",
    ProxyCustomUrlMissing => "proxy_custom_url_missing",
    ProxyGlobalInherit => "proxy_global_inherit",
    ProxyUrlIncomplete => "proxy_url_incomplete",
    ProxyUrlInvalid => "proxy_url_invalid",
    ProxySchemeUnsupported => "proxy_scheme_unsupported",
    ProxyEndpointInvalid => "proxy_endpoint_invalid",
    ProxyUrlComponentsUnsupported => "proxy_url_components_unsupported",
    ProxyNoProxyInvalid => "proxy_no_proxy_invalid",
    ProxyConfigInvalid => "proxy_config_invalid",
    ProxySaveFailed => "proxy_save_failed",
    ProxyCustomBuildFailed => "proxy_custom_build_failed",
    ProxyEnvironmentBuildFailed => "proxy_environment_build_failed",
    ProxyClientBuildFailed => "proxy_client_build_failed",
    ProxyConfigUnavailable => "proxy_config_unavailable",
    ProxyWebviewRouteChanged => "proxy_webview_route_changed",
    ProxyWebviewDirectUnsupported => "proxy_webview_direct_unsupported",
    ProxyWebviewAuthUnsupported => "proxy_webview_auth_unsupported",
    ProxyWebviewSchemeUnsupported => "proxy_webview_scheme_unsupported",
    ProxyWebviewPortUnsupported => "proxy_webview_port_unsupported",
    ProxyWebviewBypassUnsupported => "proxy_webview_bypass_unsupported",
    ProxyWebviewSystemUnavailable => "proxy_webview_system_unavailable",
    ProxyWebviewPacUnsupported => "proxy_webview_pac_unsupported",
    ProxyWebviewRoutesDiffer => "proxy_webview_routes_differ",
    ProxyWebviewRouteIncomplete => "proxy_webview_route_incomplete",
    ProxyWebviewInheritInvalid => "proxy_webview_inherit_invalid",
    NetworkTimeout { status: u16, message: NetworkMessage } => "network_timeout",
    NetworkDns { status: u16, message: NetworkMessage } => "network_dns",
    NetworkRefused { status: u16, message: NetworkMessage } => "network_refused",
    NetworkReset { status: u16, message: NetworkMessage } => "network_reset",
    NetworkTlsCert { status: u16, message: NetworkMessage } => "network_tls_cert",
    NetworkTls { status: u16, message: NetworkMessage } => "network_tls",
    NetworkProxyConnect { status: u16, message: NetworkMessage } => "network_proxy_connect",
    NetworkProxyAuth { status: u16, message: NetworkMessage } => "network_proxy_auth",
    NetworkRedirect { status: u16, message: NetworkMessage } => "network_redirect",
    NetworkBody { status: u16, message: NetworkMessage } => "network_body",
    NetworkDecode { status: u16, message: NetworkMessage } => "network_decode",
    NetworkHttp { status: u16, message: NetworkMessage } => "network_http",
    NetworkParse { status: u16, message: NetworkMessage } => "network_parse",
    NetworkOther { status: u16, message: NetworkMessage } => "network_other",
});

pub(super) fn classify(error: &Error) -> Option<UiErrorKind> {
    if let Error::InvalidNetworkProxy(reason) = error {
        return Some(UiErrorKind::Network(match reason {
            ProxyError::VersionUnsupported { found, supported } => {
                NetworkError::ProxyVersionUnsupported {
                    found: *found,
                    supported: *supported,
                }
            }
            ProxyError::CustomUrlMissing => NetworkError::ProxyCustomUrlMissing,
            ProxyError::GlobalInherit => NetworkError::ProxyGlobalInherit,
            ProxyError::UrlIncomplete => NetworkError::ProxyUrlIncomplete,
            ProxyError::UrlInvalid => NetworkError::ProxyUrlInvalid,
            ProxyError::SchemeUnsupported => NetworkError::ProxySchemeUnsupported,
            ProxyError::EndpointInvalid => NetworkError::ProxyEndpointInvalid,
            ProxyError::UrlComponentsUnsupported => NetworkError::ProxyUrlComponentsUnsupported,
            ProxyError::NoProxyInvalid => NetworkError::ProxyNoProxyInvalid,
            ProxyError::ConfigInvalid => NetworkError::ProxyConfigInvalid,
            ProxyError::SaveFailed => NetworkError::ProxySaveFailed,
            ProxyError::CustomBuildFailed => NetworkError::ProxyCustomBuildFailed,
            ProxyError::EnvironmentBuildFailed => NetworkError::ProxyEnvironmentBuildFailed,
            ProxyError::ClientBuildFailed => NetworkError::ProxyClientBuildFailed,
            ProxyError::ConfigUnavailable => NetworkError::ProxyConfigUnavailable,
            ProxyError::WebviewRouteChanged => NetworkError::ProxyWebviewRouteChanged,
            ProxyError::WebviewDirectUnsupported => NetworkError::ProxyWebviewDirectUnsupported,
            ProxyError::WebviewAuthUnsupported => NetworkError::ProxyWebviewAuthUnsupported,
            ProxyError::WebviewSchemeUnsupported => NetworkError::ProxyWebviewSchemeUnsupported,
            ProxyError::WebviewPortUnsupported => NetworkError::ProxyWebviewPortUnsupported,
            ProxyError::WebviewBypassUnsupported => NetworkError::ProxyWebviewBypassUnsupported,
            ProxyError::WebviewSystemUnavailable => NetworkError::ProxyWebviewSystemUnavailable,
            ProxyError::WebviewPacUnsupported => NetworkError::ProxyWebviewPacUnsupported,
            ProxyError::WebviewRoutesDiffer => NetworkError::ProxyWebviewRoutesDiffer,
            ProxyError::WebviewRouteIncomplete => NetworkError::ProxyWebviewRouteIncomplete,
            ProxyError::WebviewInheritInvalid => NetworkError::ProxyWebviewInheritInvalid,
        }));
    }
    let (kind, status, message) = match error {
        Error::Network {
            kind,
            status,
            message,
        } => (
            *kind,
            *status,
            match message {
                RequestErrorMessage::Timeout => NetworkMessage::Timeout,
                RequestErrorMessage::Connect => NetworkMessage::Connect,
                RequestErrorMessage::Redirect => NetworkMessage::Redirect,
                RequestErrorMessage::Decode => NetworkMessage::Decode,
                RequestErrorMessage::Other => NetworkMessage::Other,
            },
        ),
        Error::FeedRequest(rrss_core::ingest::feed_http::FeedRequestError::ProxyAuth) => {
            (ErrorKind::ProxyAuth, 407, NetworkMessage::ProxyAuth)
        }
        _ => return None,
    };
    Some(UiErrorKind::Network(match kind {
        ErrorKind::Timeout => NetworkError::NetworkTimeout { status, message },
        ErrorKind::Dns => NetworkError::NetworkDns { status, message },
        ErrorKind::Refused => NetworkError::NetworkRefused { status, message },
        ErrorKind::Reset => NetworkError::NetworkReset { status, message },
        ErrorKind::TlsCert => NetworkError::NetworkTlsCert { status, message },
        ErrorKind::Tls => NetworkError::NetworkTls { status, message },
        ErrorKind::ProxyConnect => NetworkError::NetworkProxyConnect { status, message },
        ErrorKind::ProxyAuth => NetworkError::NetworkProxyAuth { status, message },
        ErrorKind::Redirect => NetworkError::NetworkRedirect { status, message },
        ErrorKind::Body => NetworkError::NetworkBody { status, message },
        ErrorKind::Decode => NetworkError::NetworkDecode { status, message },
        ErrorKind::Http => NetworkError::NetworkHttp { status, message },
        ErrorKind::Parse => NetworkError::NetworkParse { status, message },
        ErrorKind::Other => NetworkError::NetworkOther { status, message },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::UiError;
    use serde_json::json;

    #[test]
    fn proxy_reasons_have_codes_and_safe_typed_parameters() {
        let cases = [
            (ProxyError::CustomUrlMissing, "proxy_custom_url_missing"),
            (ProxyError::GlobalInherit, "proxy_global_inherit"),
            (ProxyError::UrlIncomplete, "proxy_url_incomplete"),
            (ProxyError::UrlInvalid, "proxy_url_invalid"),
            (ProxyError::SchemeUnsupported, "proxy_scheme_unsupported"),
            (ProxyError::EndpointInvalid, "proxy_endpoint_invalid"),
            (
                ProxyError::UrlComponentsUnsupported,
                "proxy_url_components_unsupported",
            ),
            (ProxyError::NoProxyInvalid, "proxy_no_proxy_invalid"),
            (ProxyError::ConfigInvalid, "proxy_config_invalid"),
            (ProxyError::SaveFailed, "proxy_save_failed"),
            (ProxyError::CustomBuildFailed, "proxy_custom_build_failed"),
            (
                ProxyError::EnvironmentBuildFailed,
                "proxy_environment_build_failed",
            ),
            (ProxyError::ClientBuildFailed, "proxy_client_build_failed"),
            (ProxyError::ConfigUnavailable, "proxy_config_unavailable"),
            (
                ProxyError::WebviewRouteChanged,
                "proxy_webview_route_changed",
            ),
            (
                ProxyError::WebviewDirectUnsupported,
                "proxy_webview_direct_unsupported",
            ),
            (
                ProxyError::WebviewAuthUnsupported,
                "proxy_webview_auth_unsupported",
            ),
            (
                ProxyError::WebviewSchemeUnsupported,
                "proxy_webview_scheme_unsupported",
            ),
            (
                ProxyError::WebviewPortUnsupported,
                "proxy_webview_port_unsupported",
            ),
            (
                ProxyError::WebviewBypassUnsupported,
                "proxy_webview_bypass_unsupported",
            ),
            (
                ProxyError::WebviewSystemUnavailable,
                "proxy_webview_system_unavailable",
            ),
            (
                ProxyError::WebviewPacUnsupported,
                "proxy_webview_pac_unsupported",
            ),
            (
                ProxyError::WebviewRoutesDiffer,
                "proxy_webview_routes_differ",
            ),
            (
                ProxyError::WebviewRouteIncomplete,
                "proxy_webview_route_incomplete",
            ),
            (
                ProxyError::WebviewInheritInvalid,
                "proxy_webview_inherit_invalid",
            ),
        ];
        for (reason, code) in cases {
            let error = UiError::from(Error::InvalidNetworkProxy(reason));
            let wire = serde_json::to_value(&error).unwrap();
            assert_eq!(wire["code"], code);
            assert_eq!(wire.get("params").cloned().unwrap_or(json!({})), json!({}));
            assert!(!error.log_detail().contains("proxy.example"));
        }
        let error = UiError::from(Error::InvalidNetworkProxy(ProxyError::VersionUnsupported {
            found: 99,
            supported: rrss_core::network::CONFIG_VERSION,
        }));
        let wire = serde_json::to_value(error).unwrap();
        assert_eq!(wire["code"], "proxy_version_unsupported");
        assert_eq!(
            wire["params"],
            json!({ "found": 99, "supported": rrss_core::network::CONFIG_VERSION })
        );
    }
}
