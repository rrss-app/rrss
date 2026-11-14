//! 任务入口使用的网络客户端。每次领取时读取最新配置，未变更则复用连接池。
//! 保存配置只影响下一次领取；已经克隆出去的客户端和进行中的请求不受影响。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use reqwest::Client;
use rrss_core::network::{self, NetworkScope, ProxyConfig, ProxyEnvironment};
use rrss_core::{Error, Result, db};

#[derive(Clone)]
pub struct NetworkClients(Arc<Inner>);

struct Inner {
    db_path: PathBuf,
    cache: Mutex<Option<Cached>>,
    _model_capacity: Arc<crate::scheduler::model_capacity::Capacity>,
    #[cfg(test)]
    feed_builder:
        Option<Arc<dyn Fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder + Send + Sync>>,
}

struct Cached {
    config: ProxyConfig,
    env: ProxyEnvironment,
    clients: [Option<Client>; 4],
}

impl NetworkClients {
    /// 不在启动时读取或校验代理；坏网络配置不应妨碍打开本地阅读和设置页面。
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        let db_path = db_path.into();
        Self(Arc::new(Inner {
            _model_capacity: crate::scheduler::model_capacity::Capacity::for_database(&db_path),
            db_path,
            cache: Mutex::new(None),
            #[cfg(test)]
            feed_builder: None,
        }))
    }

    #[cfg(test)]
    pub(crate) fn with_feed_builder(
        db_path: impl Into<PathBuf>,
        build: impl Fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder + Send + Sync + 'static,
    ) -> Self {
        let mut clients = Self::new(db_path);
        Arc::get_mut(&mut clients.0).unwrap().feed_builder = Some(Arc::new(build));
        clients
    }

    pub async fn client(&self, scope: NetworkScope) -> Result<Client> {
        self.client_with_policy(scope, false).await
    }

    pub async fn feed_client(&self) -> Result<Client> {
        self.client_with_policy(NetworkScope::Content, true).await
    }

    async fn client_with_policy(&self, scope: NetworkScope, feed: bool) -> Result<Client> {
        let inner = self.0.clone();
        tokio::task::spawn_blocking(move || {
            // 读库也在锁内，防止并发领取的旧读取结果覆盖较新的缓存。
            // 不跨 await 持锁，也不让调用方在 UI / Tokio 工作线程做同步 SQLite I/O。
            let mut cache = inner.cache.lock().map_err(|_| unavailable())?;
            let config = {
                let conn = db::open_at(&inner.db_path)?;
                network::load(&conn)?
            };
            let env = ProxyEnvironment::capture();
            if cache
                .as_ref()
                .is_none_or(|old| old.config != config || old.env != env)
            {
                *cache = Some(Cached {
                    config,
                    env,
                    clients: [None, None, None, None],
                });
            }
            let cached = cache.as_mut().ok_or_else(unavailable)?;
            let index = if feed {
                3
            } else {
                match scope {
                    NetworkScope::Content => 0,
                    NetworkScope::Api => 1,
                    NetworkScope::Pi => 2,
                }
            };
            if let Some(client) = &cached.clients[index] {
                return Ok(client.clone());
            }
            let builder = if feed {
                rrss_core::ingest::feed_http::client_builder()
            } else {
                match scope {
                    // 桌面原共享客户端的 Content / API 请求都有浏览器 UA、无全局超时。
                    // 拆分代理时保留这两个特征；业务自身的请求超时仍照常生效。
                    NetworkScope::Content | NetworkScope::Api => {
                        rrss_core::ingest::http_client_builder()
                    }
                    NetworkScope::Pi => Client::builder(),
                }
            };
            #[cfg(test)]
            let builder = if feed && let Some(build) = &inner.feed_builder {
                build(builder)
            } else {
                builder
            };
            let client = cached
                .config
                .build_client_with_env(scope, builder, &cached.env)?;
            cached.clients[index] = Some(client.clone());
            Ok(client)
        })
        .await
        .map_err(|_| unavailable())?
    }
}

fn unavailable() -> Error {
    Error::InvalidNetworkProxy(rrss_core::network::ProxyError::ConfigUnavailable)
}

#[cfg(test)]
mod content_tests;
#[cfg(test)]
mod tests;
