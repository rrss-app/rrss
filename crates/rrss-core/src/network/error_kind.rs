//! Classify the source chain before request_error deliberately discards it.
//! Never retain an error's Display/Debug text or its URL in a diagnostic.
use std::{error::Error, fmt, io};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ErrorKind {
    Timeout,
    Dns,
    Refused,
    Reset,
    TlsCert,
    Tls,
    ProxyConnect,
    ProxyAuth,
    Redirect,
    Body,
    Decode,
    Http,
    Parse,
    #[default]
    Other,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Dns => "dns",
            Self::Refused => "refused",
            Self::Reset => "reset",
            Self::TlsCert => "tls_cert",
            Self::Tls => "tls",
            Self::ProxyConnect => "proxy_connect",
            Self::ProxyAuth => "proxy_auth",
            Self::Redirect => "redirect",
            Self::Body => "body",
            Self::Decode => "decode",
            Self::Http => "http",
            Self::Parse => "parse",
            Self::Other => "other",
        }
    }

    pub fn http(status: u16) -> Self {
        if status == 407 {
            Self::ProxyAuth
        } else {
            Self::Http
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub fn error_kind(error: &reqwest::Error) -> ErrorKind {
    if error.is_timeout() {
        ErrorKind::Timeout
    } else if let Some(status) = error.status() {
        ErrorKind::http(status.as_u16())
    } else if error.is_redirect() {
        ErrorKind::Redirect
    } else if let Some(kind) = source_kind(error.source()) {
        kind
    } else if error.is_decode() {
        ErrorKind::Decode
    } else if error.is_body() {
        ErrorKind::Body
    } else {
        ErrorKind::Other
    }
}

fn source_kind(mut source: Option<&(dyn Error + 'static)>) -> Option<ErrorKind> {
    // A malformed dependency error chain must not stall failure handling.
    for _ in 0..32 {
        let error = source?;
        if let Some(tls) = error.downcast_ref::<rustls::Error>() {
            return Some(if matches!(tls, rustls::Error::InvalidCertificate(_)) {
                ErrorKind::TlsCert
            } else {
                ErrorKind::Tls
            });
        }
        if let Some(error) = error.downcast_ref::<io::Error>() {
            match error.kind() {
                io::ErrorKind::ConnectionRefused => return Some(ErrorKind::Refused),
                io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted => {
                    return Some(ErrorKind::Reset);
                }
                io::ErrorKind::TimedOut => return Some(ErrorKind::Timeout),
                _ => {}
            }
            // io::Error::source skips the contained error itself. rustls puts
            // its typed error in get_ref(), so inspect that before advancing.
            if let Some(inner) = error.get_ref() {
                source = Some(inner);
                continue;
            }
        }
        // hyper-util hides these concrete types. Match only its fixed messages
        // (locked dependency), not arbitrary OS text or substrings of URLs.
        match error.to_string().as_str() {
            "dns error" => return Some(ErrorKind::Dns),
            "tunnel error: proxy authorization required" => return Some(ErrorKind::ProxyAuth),
            "tunnel error: unsuccessful"
            | "tunnel error: unexpected end of file"
            | "tunnel error: missing destination host"
            | "tunnel error: io error establishing tunnel"
            | "tunnel error: proxy response headers too long" => {
                return Some(ErrorKind::ProxyConnect);
            }
            _ => {}
        }
        source = error.source();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("dns error")]
    struct Dns(#[source] io::Error);

    struct FailedDns;
    impl reqwest::dns::Resolve for FailedDns {
        fn resolve(&self, _: reqwest::dns::Name) -> reqwest::dns::Resolving {
            Box::pin(async { Err(io::Error::other("fixture DNS failure").into()) })
        }
    }

    #[tokio::test]
    async fn real_reqwest_dns_and_connect_tunnel_categories() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };
        let client = reqwest::Client::builder()
            .no_proxy()
            .dns_resolver(std::sync::Arc::new(FailedDns))
            .build()
            .unwrap();
        let error = client
            .get("https://user:password@fixture.invalid/?token=canary")
            .send()
            .await
            .unwrap_err();
        assert_eq!(error_kind(&error), ErrorKind::Dns);
        for (status, expected) in [(407, ErrorKind::ProxyAuth), (502, ErrorKind::ProxyConnect)] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    head.push(stream.read_u8().await.unwrap());
                }
                stream
                    .write_all(
                        format!("HTTP/1.1 {status} Fixture\r\nContent-Length: 0\r\n\r\n")
                            .as_bytes(),
                    )
                    .await
                    .unwrap();
            });
            let client = super::super::ProxyConfig {
                global: super::super::ProxyRule::custom(format!("http://user:password@{address}")),
                ..Default::default()
            }
            .build_client_with_env(
                super::super::NetworkScope::Content,
                reqwest::Client::builder().timeout(std::time::Duration::from_secs(3)),
                &super::super::ProxyEnvironment::default(),
            )
            .unwrap();
            let error = client
                .get("https://fixture.invalid/private?token=canary")
                .send()
                .await
                .unwrap_err();
            assert_eq!(error_kind(&error), expected);
            server.await.unwrap();
        }
    }

    #[test]
    fn platform_source_fixtures_and_typed_tls_errors() {
        // Resolver messages differ by OS; hyper's DNS wrapper is stable.
        for message in [
            "failed to lookup address information: Name or service not known",
            "No such host is known. (os error 11001)",
        ] {
            assert_eq!(
                source_kind(Some(&Dns(io::Error::other(message)))),
                Some(ErrorKind::Dns)
            );
        }
        for (kind, expected) in [
            (io::ErrorKind::ConnectionRefused, ErrorKind::Refused),
            (io::ErrorKind::ConnectionReset, ErrorKind::Reset),
            (io::ErrorKind::TimedOut, ErrorKind::Timeout),
        ] {
            assert_eq!(source_kind(Some(&io::Error::from(kind))), Some(expected));
        }
        let cert = io::Error::other(rustls::Error::InvalidCertificate(
            rustls::CertificateError::UnknownIssuer,
        ));
        assert_eq!(source_kind(Some(&cert)), Some(ErrorKind::TlsCert));
        let tls = io::Error::other(rustls::Error::NoCertificatesPresented);
        assert_eq!(source_kind(Some(&tls)), Some(ErrorKind::Tls));
        // A URL which happens to contain a category name is not evidence.
        assert_eq!(
            source_kind(Some(&io::Error::other(
                "https://dns error:pass@x/?tls_cert"
            ))),
            None
        );
    }
}
