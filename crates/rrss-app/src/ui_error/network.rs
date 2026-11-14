use super::UiErrorKind;
use rrss_core::{
    Error,
    network::{ErrorKind, RequestErrorMessage},
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
        Error::Http {
            status: 407,
            message,
        } if message == rrss_core::ingest::feed_http::PROXY_AUTH_MESSAGE => {
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
