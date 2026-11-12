//! 代理测试的目标服务。代理客户端始终绕过 `localhost` / `127.0.0.1` / `::1`
//! （`rrss_core::network::DEFAULT_NO_PROXY`），默认的 127.0.0.1 MockServer 作目标时不会经过代理。
//! 127.0.0.2 仍是本机回环（Linux / Windows 整个 127.0.0.0/8 可用），但不在默认绕过集合里；
//! 代理本身照常用 127.0.0.1。

use tokio::io::AsyncReadExt;
use wiremock::MockServer;

pub(crate) fn fake_provider(conn: &rusqlite::Connection, name: &str, url: String) -> i64 {
    rrss_core::ai_store::insert_provider(
        conn,
        &rrss_core::ai_store::NewProvider {
            name: name.into(),
            base_url: url,
            api_key: None,
            protocol: "openai".into(),
        },
    )
    .unwrap()
}

/// 不被默认回环绕过命中的目标：设了代理时必须经代理访问，直连模式下仍可直接连上。
pub(crate) async fn proxied_target() -> MockServer {
    let listener = std::net::TcpListener::bind("127.0.0.2:0").expect("bind 127.0.0.2 loopback");
    MockServer::builder().listener(listener).start().await
}

/// 手写代理 / 服务端读完请求头（到空行为止），返回小写文本；超过 16 KiB 视为测试失控。
pub(crate) async fn read_request_head(stream: &mut tokio::net::TcpStream) -> String {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(stream.read_u8().await.unwrap());
        assert!(head.len() < 16_384);
    }
    String::from_utf8(head).unwrap().to_ascii_lowercase()
}
