//! 任务入口使用的网络客户端。每次领取时读取最新配置，未变更则复用连接池。
//! 保存配置只影响下一次领取；已经克隆出去的客户端和进行中的请求不受影响。

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use reqwest::Client;
use rrss_core::network::{self, NetworkScope, ProxyConfig, ProxyEnvironment};
use rrss_core::{Error, Result, db};

#[derive(Clone)]
pub struct NetworkClients(Arc<Inner>);

struct Inner {
    db_path: PathBuf,
    cache: Mutex<VecDeque<Cached>>,
    #[cfg(test)]
    feed_builder:
        Option<Arc<dyn Fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder + Send + Sync>>,
}

struct Cached {
    raw: Option<String>,
    config: Arc<ProxyConfig>,
    env: ProxyEnvironment,
    clients: [Option<Client>; 4],
}

impl NetworkClients {
    /// 不在启动时读取或校验代理；坏网络配置不应妨碍打开本地阅读和设置页面。
    pub fn new(db_path: impl Into<PathBuf>) -> Self {
        let db_path = db_path.into();
        Self(Arc::new(Inner {
            db_path,
            cache: Mutex::new(VecDeque::new()),
            #[cfg(test)]
            feed_builder: None,
        }))
    }

    pub(crate) fn db_path(&self) -> &std::path::Path {
        &self.0.db_path
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
        crate::execution::blocking(move || {
            // SQLite/system reads and client construction never hold the cache
            // mutex. Entries are keyed by immutable snapshots, so a late old
            // acquisition cannot replace the current configuration's clients.
            let raw = {
                let conn = db::open_at(&inner.db_path)?;
                db::get_setting(&conn, network::SETTING_KEY)?
            };
            let config = inner
                .cache
                .lock()
                .map_err(|_| unavailable())?
                .iter()
                .find(|entry| entry.raw == raw)
                .map(|entry| entry.config.clone());
            let config = match config {
                Some(config) => config,
                None => Arc::new(match &raw {
                    Some(raw) => network::decode(raw)?,
                    None => ProxyConfig::default(),
                }),
            };
            let env = if config.effective_for(scope).mode == network::ProxyMode::System {
                db::latency::measure("proxy_environment", ProxyEnvironment::capture)
            } else {
                ProxyEnvironment::default()
            };
            let index = if feed {
                3
            } else {
                match scope {
                    NetworkScope::Content => 0,
                    NetworkScope::Api => 1,
                    NetworkScope::Pi => 2,
                }
            };
            let hit = {
                let mut cache = inner.cache.lock().map_err(|_| unavailable())?;
                cache
                    .iter()
                    .position(|entry| entry.raw == raw && entry.env == env)
                    .and_then(|pos| {
                        let entry = cache.remove(pos).expect("cached snapshot");
                        let client = entry.clients[index].clone();
                        cache.push_back(entry);
                        client
                    })
            };
            if let Some(client) = hit {
                return Ok(client);
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
            let client = db::latency::measure("network_client_build", || {
                config.build_client_with_env(scope, builder, &env)
            })?;
            let mut cache = inner.cache.lock().map_err(|_| unavailable())?;
            if let Some(entry) = cache
                .iter_mut()
                .find(|entry| entry.raw == raw && entry.env == env)
            {
                return Ok(entry.clients[index].get_or_insert(client).clone());
            }
            if cache.len() == 4 {
                cache.pop_front();
            }
            let mut clients = [None, None, None, None];
            clients[index] = Some(client.clone());
            cache.push_back(Cached {
                raw,
                config,
                env,
                clients,
            });
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
