use super::*;
use rrss_core::{
    db,
    network::{self, ProxyConfig, ProxyRule},
};
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn failed_streams_keep_errors_safe_and_remove_partial_media() {
    for oversized in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!(
            "http://proxy-user:proxy-password@{}",
            listener.local_addr().unwrap()
        );
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            crate::test_net::read_request_head(&mut stream).await;
            let len = if oversized {
                media::MAX_DIRECT_BYTES + 1
            } else {
                4096
            };
            stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {len}\r\nConnection: close\r\n\r\npartial").as_bytes()).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let conn = db::open_at(&path).unwrap();
        conn.execute(
            "INSERT INTO feeds (id, url, created_at) VALUES (1, 'http://content.invalid/rss', 1)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO articles (id, feed_id, title, hash, created_at, content_html) VALUES (1, 1, 'Title', 'h', 1, '<video src=\"http://content.invalid/movie.mp4\"></video>')", []).unwrap();
        network::save(
            &conn,
            &ProxyConfig {
                content: ProxyRule::custom(proxy),
                ..Default::default()
            },
        )
        .unwrap();
        let clients = crate::network::NetworkClients::new(&path);
        let error = download_direct(
            &clients,
            &path,
            dir.path(),
            1,
            "http://content.invalid/movie.mp4",
            "",
        )
        .await
        .unwrap_err();
        if oversized {
            assert!(matches!(error, DirectFail::TooLarge));
        } else {
            assert!(matches!(error, DirectFail::Network(_)));
        }
        let text = format!("{error} {error:?}");
        assert!(!text.contains("proxy-user"));
        assert!(!text.contains("proxy-password"));
        let dest = media::cache_path(dir.path(), "http://content.invalid/movie.mp4");
        assert!(!dest.exists());
        assert!(!dest.with_extension("part").exists());
        assert_eq!(
            conn.query_row("SELECT count(*) FROM media", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        // 代理不会把正文以外的任意媒体地址变为可下载资源。
        assert!(matches!(
            download_direct(
                &clients,
                &path,
                dir.path(),
                1,
                "file:///private/movie.mp4",
                ""
            )
            .await,
            Err(DirectFail::NotDirect)
        ));
        server.await.unwrap();
    }
}
