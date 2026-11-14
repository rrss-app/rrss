//! 代理配置、平台快照与显式 HTTP / webview 线路映射。
//!
//! `ProxyConfig` 的序列化只用于落库，不能直接用作 IPC 视图；Debug 不含认证信息。
//! 调用方在任务开始时读取一份配置，再按用途构造客户端，已有请求保留原快照。

use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;

use reqwest::{Client, ClientBuilder, NoProxy, Proxy};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{Error, Result, db};

mod error_kind;
pub use error_kind::{ErrorKind, error_kind};

pub const SETTING_KEY: &str = "network_proxy";
pub const CONFIG_VERSION: u32 = 1;
/// 代理客户端始终绕过本机回环地址；用户填写的列表会在运行时与其合并。
/// 局域网和其他保留地址不属于默认集合，避免静默改变用户的代理路由。
pub const DEFAULT_NO_PROXY: &str = "localhost,127.0.0.1,::1";
/// 子进程接入时须先清除全部八个变量，再按有效模式注入；不修改父进程环境。
pub const PROXY_ENV_KEYS: [&str; 8] = [
    "HTTP_PROXY",
    "http_proxy",
    "HTTPS_PROXY",
    "https_proxy",
    "ALL_PROXY",
    "all_proxy",
    "NO_PROXY",
    "no_proxy",
];
/// 原始宿主环境快照，与 PROXY_ENV_KEYS 的大小写对逐一对应。
/// 始终整组传递；未设置或工具不需要系统代理时为空串。
pub const HOST_PROXY_ENV_KEYS: [&str; 4] = [
    "RRSS_HOST_HTTP_PROXY",
    "RRSS_HOST_HTTPS_PROXY",
    "RRSS_HOST_ALL_PROXY",
    "RRSS_HOST_NO_PROXY",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkScope {
    Content,
    Api,
    Pi,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    #[default]
    Direct,
    System,
    Custom,
    Inherit,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProxyRule {
    pub mode: ProxyMode,
    /// 含认证的原值只用于存储和构造客户端，不传给前端或日志。
    pub url: String,
    pub no_proxy: String,
}

impl fmt::Debug for ProxyRule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyRule")
            .field("mode", &self.mode)
            .field("url", &self.redacted_url())
            // 未验证的输入也不能通过 Debug 泄露用户误填的凭据。
            .field("no_proxy_set", &!self.no_proxy.is_empty())
            .finish()
    }
}

impl ProxyRule {
    pub fn inherit() -> Self {
        Self {
            mode: ProxyMode::Inherit,
            ..Self::default()
        }
    }

    /// 自定义代理、不带额外绕过项；地址在保存或构造客户端时才校验。
    pub fn custom(url: impl Into<String>) -> Self {
        Self {
            mode: ProxyMode::Custom,
            url: url.into(),
            ..Self::default()
        }
    }

    fn normalized(&self) -> Result<Self> {
        let url = self.url.trim();
        if self.mode == ProxyMode::Custom && url.is_empty() {
            return Err(invalid("请填写自定义代理地址"));
        }
        Ok(Self {
            mode: self.mode,
            url: if url.is_empty() {
                String::new()
            } else {
                parse_proxy_url(url)?.into()
            },
            no_proxy: normalize_no_proxy(&self.no_proxy)?,
        })
    }

    /// 可用于后续 IPC 投影：彻底删除用户名与密码，不保留部分认证文本。
    pub fn redacted_url(&self) -> String {
        let Ok(mut url) = parse_proxy_url(self.url.trim()) else {
            return String::new();
        };
        let _ = url.set_username("");
        let _ = url.set_password(None);
        url.into()
    }

    pub fn has_auth(&self) -> bool {
        parse_proxy_url(self.url.trim())
            .is_ok_and(|url| !url.username().is_empty() || url.password().is_some())
    }

    /// 将设置页表单应用到规则。仅在 scheme、主机与有效端口相同时沿用旧认证。
    pub fn from_form(
        existing: &Self,
        mode: ProxyMode,
        url: &str,
        no_proxy: &str,
        username: Option<&str>,
        password: Option<&str>,
        clear_auth: bool,
    ) -> Result<Self> {
        let mut next = Self {
            mode,
            url: url.trim().into(),
            no_proxy: no_proxy.into(),
        };
        let old_url = parse_proxy_url(existing.url.trim()).ok();
        let mut next_url = if next.url.is_empty() {
            None
        } else {
            Some(parse_proxy_url(&next.url)?)
        };
        if let Some(url) = next_url.as_mut() {
            if clear_auth {
                let _ = url.set_username("");
                let _ = url.set_password(None);
            } else if username.is_some() || password.is_some() {
                let _ = url.set_username(username.unwrap_or(""));
                let _ = url.set_password(password);
            } else if url.username().is_empty()
                && url.password().is_none()
                && let Some(old) = old_url
                && url.scheme() == old.scheme()
                && url
                    .host_str()
                    .zip(old.host_str())
                    .is_some_and(|(next, old)| next.eq_ignore_ascii_case(old))
                && url.port_or_known_default() == old.port_or_known_default()
                && (!old.username().is_empty() || old.password().is_some())
            {
                let _ = url.set_username(old.username());
                let _ = url.set_password(old.password());
            }
            next.url = url.to_string();
        }
        next.normalized()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProxyConfig {
    pub version: u32,
    pub global: ProxyRule,
    pub content: ProxyRule,
    pub api: ProxyRule,
    pub pi: ProxyRule,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            version: CONFIG_VERSION,
            global: ProxyRule::default(),
            content: ProxyRule::inherit(),
            api: ProxyRule::inherit(),
            pi: ProxyRule::inherit(),
        }
    }
}

impl ProxyConfig {
    pub fn normalized(&self) -> Result<Self> {
        if self.version != CONFIG_VERSION {
            return Err(invalid("不支持此版本的代理配置"));
        }
        if self.global.mode == ProxyMode::Inherit {
            return Err(invalid("全局代理不能选择跟随全局"));
        }
        Ok(Self {
            version: CONFIG_VERSION,
            global: self.global.normalized()?,
            content: self.content.normalized()?,
            api: self.api.normalized()?,
            pi: self.pi.normalized()?,
        })
    }

    pub fn effective_for(&self, scope: NetworkScope) -> &ProxyRule {
        let rule = match scope {
            NetworkScope::Content => &self.content,
            NetworkScope::Api => &self.api,
            NetworkScope::Pi => &self.pi,
        };
        if rule.mode == ProxyMode::Inherit {
            &self.global
        } else {
            rule
        }
    }

    /// 清除整个 userinfo；用于不含密钥的备份，不改变原配置。
    pub fn without_auth(&self) -> Result<Self> {
        let mut config = self.normalized()?;
        for rule in [
            &mut config.global,
            &mut config.content,
            &mut config.api,
            &mut config.pi,
        ] {
            rule.url = rule.redacted_url();
        }
        Ok(config)
    }

    /// 保留调用方的 UA、超时、headers 和 TLS 设置，只统一代理策略。
    pub fn build_client(&self, scope: NetworkScope, builder: ClientBuilder) -> Result<Client> {
        self.build_client_with_env(scope, builder, &ProxyEnvironment::capture())
    }

    /// 显式环境快照便于测试与运行时复用，测试无需修改进程全局环境。
    pub fn build_client_with_env(
        &self,
        scope: NetworkScope,
        builder: ClientBuilder,
        env: &ProxyEnvironment,
    ) -> Result<Client> {
        let rule = self.effective_for(scope);
        let mode = match rule.mode {
            ProxyMode::Direct => "direct",
            ProxyMode::System => "system",
            ProxyMode::Custom => "custom",
            ProxyMode::Inherit => "inherit",
        };
        let scope_name = match scope {
            NetworkScope::Content => "content",
            NetworkScope::Api => "api",
            NetworkScope::Pi => "pi",
        };
        let scheme = match rule.mode {
            ProxyMode::Custom => proxy_scheme(&rule.url),
            ProxyMode::System => {
                let all = env.value("ALL_PROXY", "all_proxy");
                let http = env
                    .value("HTTP_PROXY", "http_proxy")
                    .or(all)
                    .map(proxy_scheme);
                let https = env
                    .value("HTTPS_PROXY", "https_proxy")
                    .or(all)
                    .map(proxy_scheme);
                match (http, https) {
                    (None, None) => "none",
                    (Some(a), Some(b)) if a != b => "mixed",
                    (Some(a), _) | (_, Some(a)) => a,
                }
            }
            _ => "none",
        };
        let mut category = "config";
        let result = (|| {
            let config = self.normalized()?;
            let rule = config.effective_for(scope);
            // 即使其他依赖通过 feature 合并打开 system-proxy，也不能暗中覆盖本配置。
            let mut builder = builder.no_proxy();
            match rule.mode {
                ProxyMode::Direct => {}
                ProxyMode::Custom => {
                    let proxy = Proxy::all(&rule.url)
                        .map_err(|_| invalid("无法构造自定义代理"))?
                        .no_proxy(reqwest_no_proxy(&rule.no_proxy));
                    builder = builder.proxy(proxy);
                }
                ProxyMode::System => {
                    category = "environment";
                    let no_proxy =
                        normalize_system_no_proxy(env.value("NO_PROXY", "no_proxy").unwrap_or(""));
                    let all = env.value("ALL_PROXY", "all_proxy");
                    for (https, value) in [
                        (false, env.value("HTTP_PROXY", "http_proxy").or(all)),
                        (true, env.value("HTTPS_PROXY", "https_proxy").or(all)),
                    ] {
                        if let Some(value) = value {
                            let url = parse_proxy_url(value)?;
                            let proxy = if https {
                                Proxy::https(url)
                            } else {
                                Proxy::http(url)
                            }
                            .map_err(|_| invalid("无法构造环境代理"))?
                            .no_proxy(reqwest_no_proxy(&no_proxy));
                            builder = builder.proxy(proxy);
                        }
                    }
                }
                ProxyMode::Inherit => return Err(invalid("全局代理不能选择跟随全局")),
            }
            // 不附带 reqwest 的原始错误或 source，避免 Debug / anyhow 错误链泄露地址。
            category = "client";
            builder.build().map_err(|_| invalid("无法创建网络客户端"))
        })();
        let scope = scope_name;
        match &result {
            Ok(_) => tracing::debug!(scope, mode, scheme, category = "ok", "network client built"),
            Err(_) if category == "environment" => {
                // A bad process environment can affect every rebuilt client.
                static WARNED: std::sync::atomic::AtomicBool =
                    std::sync::atomic::AtomicBool::new(false);
                if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    tracing::warn!(
                        scope,
                        mode,
                        scheme,
                        error_kind = category,
                        "network client build failed"
                    );
                } else {
                    tracing::debug!(
                        scope,
                        mode,
                        scheme,
                        error_kind = category,
                        "network client build failed"
                    );
                }
            }
            Err(_) => tracing::warn!(
                scope,
                mode,
                scheme,
                error_kind = category,
                "network client build failed"
            ),
        }
        result
    }

    /// 子进程（Pi）的代理环境。调用方先移除 [`PROXY_ENV_KEYS`] 全部八个变量，再写入这里的值，
    /// 不修改父进程环境。值可能含代理认证，只能进子进程环境，展示用 [`ChildProxyEnv::redacted`]。
    ///
    /// - 无代理：不写任何标准变量（包括启动 RRSS 时继承的）。
    /// - 系统代理：原样转交环境里的地址，大小写两份取同一值；不替 Pi 校验地址。
    /// - 自定义：HTTP / HTTPS / ALL 三组都指向同一地址，SOCKS5 是否被 Pi 接受取决于其运行时。
    ///
    /// 系统与自定义都写入 `NO_PROXY`，并追加默认回环绕过。
    /// 仅当 Api / Content 使用系统代理时另传原始宿主环境，否则整组传空串。
    pub fn child_env(&self, scope: NetworkScope, env: &ProxyEnvironment) -> Result<ChildProxyEnv> {
        let config = self.normalized()?;
        let rule = config.effective_for(scope);
        let mut vars = Vec::new();
        let mut set = |upper: &'static str, lower: &'static str, value: &str| {
            vars.push((upper, value.to_owned()));
            vars.push((lower, value.to_owned()));
        };
        match rule.mode {
            ProxyMode::Direct => {}
            ProxyMode::Custom => {
                set("HTTP_PROXY", "http_proxy", &rule.url);
                set("HTTPS_PROXY", "https_proxy", &rule.url);
                set("ALL_PROXY", "all_proxy", &rule.url);
                set("NO_PROXY", "no_proxy", &merge_no_proxy(&rule.no_proxy)?);
            }
            ProxyMode::System => {
                for (upper, lower) in [
                    ("HTTP_PROXY", "http_proxy"),
                    ("HTTPS_PROXY", "https_proxy"),
                    ("ALL_PROXY", "all_proxy"),
                ] {
                    if let Some(value) = env.value(upper, lower) {
                        set(upper, lower, value);
                    }
                }
                // 环境里的绕过列表由 Pi 自己解释，这里只追加默认回环项，不按 RRSS 的语法校验。
                let mut entries = DEFAULT_NO_PROXY
                    .split(',')
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                for entry in env
                    .value("NO_PROXY", "no_proxy")
                    .unwrap_or("")
                    .split(',')
                    .map(str::trim)
                    .filter(|entry| !entry.is_empty())
                {
                    if !entries
                        .iter()
                        .any(|existing| existing.eq_ignore_ascii_case(entry))
                    {
                        entries.push(entry.to_owned());
                    }
                }
                set("NO_PROXY", "no_proxy", &entries.join(","));
            }
            ProxyMode::Inherit => return Err(invalid("全局代理不能选择跟随全局")),
        }
        let host_values = if [NetworkScope::Api, NetworkScope::Content]
            .into_iter()
            .any(|scope| config.effective_for(scope).mode == ProxyMode::System)
        {
            env.host_values.clone()
        } else {
            Default::default()
        };
        vars.extend(HOST_PROXY_ENV_KEYS.into_iter().zip(host_values));
        Ok(ChildProxyEnv(vars))
    }
}

/// 子进程代理变量。Debug 只列变量名；值可能含代理认证。
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ChildProxyEnv(Vec<(&'static str, String)>);

impl fmt::Debug for ChildProxyEnv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChildProxyEnv")
            .field(
                "keys",
                &self.0.iter().map(|(key, _)| key).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl ChildProxyEnv {
    /// 写入子进程的变量（含认证）。调用方须先移除全部 [`PROXY_ENV_KEYS`]。
    pub fn vars(&self) -> &[(&'static str, String)] {
        &self.0
    }

    /// 展示副本：代理地址去掉用户名和密码，无法解析的值整体隐藏。
    pub fn redacted(&self) -> Vec<(String, String)> {
        self.0
            .iter()
            .map(|(key, value)| {
                let shown = if HOST_PROXY_ENV_KEYS.contains(key) {
                    crate::logsafe::Secret(value).to_string()
                } else if key.eq_ignore_ascii_case("NO_PROXY") {
                    value.clone()
                } else {
                    match Url::parse(value) {
                        Ok(mut url) => {
                            let had_auth = !url.username().is_empty() || url.password().is_some();
                            let _ = url.set_username("");
                            let _ = url.set_password(None);
                            if had_auth {
                                format!("{url}（认证已隐藏）")
                            } else {
                                url.into()
                            }
                        }
                        Err(_) => "***".into(),
                    }
                };
                ((*key).to_owned(), shown)
            })
            .collect()
    }

    /// 需要从子进程输出里遮盖的片段：完整地址、用户名和密码（解码前后两种写法）。
    pub fn secrets(&self) -> Vec<String> {
        let mut secrets = Vec::new();
        for (key, value) in &self.0 {
            if value.is_empty() || key.eq_ignore_ascii_case("NO_PROXY") {
                continue;
            }
            let Ok(url) = Url::parse(value) else {
                secrets.push(value.clone());
                continue;
            };
            if url.username().is_empty() && url.password().is_none() {
                continue;
            }
            secrets.push(value.clone());
            for part in [Some(url.username()), url.password()].into_iter().flatten() {
                if part.is_empty() {
                    continue;
                }
                secrets.push(part.to_owned());
                if let Ok(decoded) = percent_encoding::percent_decode_str(part).decode_utf8() {
                    secrets.push(decoded.into_owned());
                }
            }
        }
        secrets.sort();
        secrets.dedup();
        secrets
    }
}

/// 保存有效代理与合并原生系统设置前的宿主环境；不修改父进程环境。
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ProxyEnvironment {
    values: BTreeMap<String, String>,
    host_values: [String; 4],
}

impl fmt::Debug for ProxyEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyEnvironment")
            .field("keys", &self.values.keys())
            .finish()
    }
}

impl ProxyEnvironment {
    pub fn capture() -> Self {
        Self::from_lookup(|key| std::env::var(key).ok()).with_system(&Self::system_snapshot())
    }

    /// Merge a captured native snapshot without consulting the host (fixture friendly).
    pub fn with_system(&self, native: &crate::platform::SystemProxySnapshot) -> Self {
        warn_native_proxy(native);
        let mut result = self.clone();
        // Windows 原生代理优先于启动环境变量；没有原生值的协议继续回退环境变量。
        for (key, value) in [
            ("HTTP_PROXY", &native.http),
            ("HTTPS_PROXY", &native.https),
            ("ALL_PROXY", &native.all),
            ("NO_PROXY", &native.no_proxy),
        ] {
            if let Some(value) = value {
                result.values.insert(key.to_owned(), value.clone());
                result
                    .values
                    .insert(key.to_ascii_lowercase(), value.clone());
            }
        }
        result
    }

    pub fn system_snapshot() -> crate::platform::SystemProxySnapshot {
        let native = crate::platform::system_proxy();
        warn_native_proxy(&native);
        native
    }

    pub fn from_lookup(mut lookup: impl FnMut(&str) -> Option<String>) -> Self {
        let host = HOST_PROXY_ENV_KEYS.map(&mut lookup);
        // Presence, including an empty value, selects the whole host snapshot.
        // Never fill gaps from Pi's standard variables.
        let restored = host.iter().any(Option::is_some);
        let mut env = Self::default();
        for (pair, saved) in PROXY_ENV_KEYS.chunks_exact(2).zip(host) {
            if restored {
                if let Some(value) = saved {
                    env.values.insert(pair[0].to_owned(), value);
                }
            } else {
                for key in pair {
                    if let Some(value) = lookup(key) {
                        env.values.insert((*key).to_owned(), value);
                    }
                }
            }
        }
        env.host_values = std::array::from_fn(|index| {
            let pair = &PROXY_ENV_KEYS[index * 2..index * 2 + 2];
            env.value(pair[0], pair[1]).unwrap_or("").to_owned()
        });
        env
    }

    /// 大写非空值优先，小写次之；目标协议未配置时才回退 ALL_PROXY。
    fn value(&self, upper: &str, lower: &str) -> Option<&str> {
        [upper, lower]
            .into_iter()
            .filter_map(|key| self.values.get(key))
            .map(|value| value.trim())
            .find(|value| !value.is_empty())
    }

    /// 给设置页展示当前进程看到的代理地址，认证信息只显示为提示，不回传凭据。
    pub fn display_value(&self, upper: &str, lower: &str) -> Option<String> {
        let value = self.value(upper, lower)?;
        Some(match Url::parse(value) {
            Ok(mut url) => {
                let had_auth = !url.username().is_empty() || url.password().is_some();
                let _ = url.set_username("");
                let _ = url.set_password(None);
                if had_auth {
                    format!("{url}（认证已隐藏）")
                } else {
                    url.into()
                }
            }
            Err(_) => "已设置（地址格式无法解析）".into(),
        })
    }

    /// NO_PROXY 不含凭据，展示时保留当前进程的原始绕过列表。
    pub fn display_no_proxy(&self) -> Option<String> {
        self.value("NO_PROXY", "no_proxy").map(str::to_owned)
    }
}

fn warn_native_proxy(native: &crate::platform::SystemProxySnapshot) {
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if native.unavailable && !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        tracing::warn!(
            mode = "system",
            error_kind = "native_unavailable",
            "system proxy read failed; using environment"
        );
    }
}

fn proxy_scheme(value: &str) -> &'static str {
    // Allowlisted labels only: even a malformed scheme may contain a secret.
    match value.trim().split_once("://").map(|(scheme, _)| scheme) {
        Some("http") => "http",
        Some("https") => "https",
        Some("socks5") => "socks5",
        Some("socks5h") => "socks5h",
        _ => "invalid",
    }
}

fn invalid(message: &str) -> Error {
    Error::InvalidNetworkProxy(message.into())
}

fn parse_proxy_url(raw: &str) -> Result<Url> {
    // Url 会修复某些缺少斜线 / 含换行的输入，这里明确拒绝，避免误判代理地址。
    if !raw.contains("://")
        || raw.chars().any(|c| c.is_whitespace() || c.is_control())
        || raw.contains('\\')
    {
        return Err(invalid("代理地址格式无效，请填写完整 URL"));
    }
    let url = Url::parse(raw).map_err(|_| invalid("代理地址格式无效"))?;
    if !matches!(url.scheme(), "http" | "https" | "socks5" | "socks5h") {
        return Err(invalid("代理只支持 HTTP、HTTPS、SOCKS5 或 SOCKS5H"));
    }
    // 禁止 URL 解析器将 http:///host 修复成另一个有效地址。
    let authority = raw.split_once("://").map(|(_, rest)| rest).unwrap_or("");
    if authority.starts_with('/')
        || url.host_str().is_none_or(str::is_empty)
        || url.port() == Some(0)
    {
        return Err(invalid("代理地址需要有效主机和端口"));
    }
    if authority
        .split_once('/')
        .is_some_and(|(_, path)| !path.is_empty())
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid("代理地址不能包含路径、查询参数或片段"));
    }
    Ok(url)
}

/// Reqwest 绕过语法：域名、IP、CIDR 或 *；不接受 URL、端口、凭据或局部通配符。
pub fn normalize_no_proxy(raw: &str) -> Result<String> {
    let mut entries = Vec::new();
    for part in raw
        .split(|c: char| c == ',' || c.is_ascii_whitespace())
        .filter(|s| !s.is_empty())
    {
        let entry = part.to_ascii_lowercase();
        let valid = if entry == "*" || entry.parse::<IpAddr>().is_ok() {
            true
        } else if let Some((ip, bits)) = entry.split_once('/') {
            match (ip.parse::<IpAddr>(), bits.parse::<u8>()) {
                (Ok(IpAddr::V4(_)), Ok(bits)) => bits <= 32,
                (Ok(IpAddr::V6(_)), Ok(bits)) => bits <= 128,
                _ => false,
            }
        } else {
            let host = entry.trim_start_matches('.').trim_end_matches('.');
            !host.is_empty()
                && host.len() <= 253
                && host.split('.').all(|label| {
                    !label.is_empty()
                        && label.len() <= 63
                        && !label.starts_with('-')
                        && !label.ends_with('-')
                        && label
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
                })
        };
        if !valid {
            return Err(invalid("不代理列表只支持域名、IP、网段或 *"));
        }
        if !entries.contains(&entry) {
            entries.push(entry);
        }
    }
    Ok(entries.join(","))
}

/// 系统绕过列表的一项：去掉通配域名前缀与端口，保留 IPv6 字面量。
/// 返回待校验的候选项；Windows 可先将其专有 IPv4 通配转成 CIDR，
/// 再共用严格校验，无法表达的项由调用方跳过。
pub(crate) fn normalize_system_no_proxy_entry(raw: &str) -> &str {
    let item = raw.trim().strip_prefix("*.").unwrap_or(raw.trim());
    if let Some((host, suffix)) = item.strip_prefix('[').and_then(|v| v.split_once(']'))
        && matches!(host.parse::<IpAddr>(), Ok(IpAddr::V6(_)))
        && (suffix.is_empty()
            || suffix
                .strip_prefix(':')
                .is_some_and(|port| port.parse::<u16>().is_ok()))
    {
        return host;
    }
    match item.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') && port.parse::<u16>().is_ok() => host,
        _ => item,
    }
}

fn normalize_system_no_proxy(raw: &str) -> String {
    let mut entries = Vec::new();
    for item in raw
        .split(|c: char| c == ',' || c.is_ascii_whitespace())
        .filter(|s| !s.is_empty())
    {
        if let Ok(entry) = normalize_no_proxy(normalize_system_no_proxy_entry(item))
            && !entry.is_empty()
            && !entries.contains(&entry)
        {
            entries.push(entry);
        }
    }
    entries.join(",")
}

fn reqwest_no_proxy(normalized: &str) -> Option<NoProxy> {
    let merged = merge_no_proxy(normalized).ok()?;
    // 锁定的 hyper-util 将 * 只匹配到域名，IP 走另一条分支。补全两个地址族，
    // 保证用户填写 * 时所有目标都直连，而非 IP 目标仍意外经过代理。
    if merged.split(',').any(|entry| entry == "*") {
        NoProxy::from_string("*,0.0.0.0/0,::/0")
    } else {
        NoProxy::from_string(&merged)
    }
}

/// 将用户列表与 RRSS 的最小默认回环集合合并，并保持规范化顺序。
/// 默认项不写入数据库，因此清空用户输入也不会关闭本机直连保护。
pub fn merge_no_proxy(user: &str) -> Result<String> {
    let user = normalize_no_proxy(user)?;
    let mut entries = DEFAULT_NO_PROXY
        .split(',')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    for entry in user.split(',').filter(|entry| !entry.is_empty()) {
        if !entries.iter().any(|existing| existing == entry) {
            entries.push(entry.to_owned());
        }
    }
    Ok(entries.join(","))
}

fn decode(raw: &str) -> Result<ProxyConfig> {
    // serde 的错误可能包含原始字段值，因此只回传固定文案。
    let config: ProxyConfig = serde_json::from_str(raw).map_err(|_| invalid("代理配置格式无效"))?;
    config.normalized()
}

pub fn load(conn: &Connection) -> Result<ProxyConfig> {
    match db::get_setting(conn, SETTING_KEY)? {
        None => Ok(ProxyConfig::default()),
        Some(raw) => decode(&raw),
    }
}

/// 完整校验后只写一条记录；校验 / SQLite 写入失败均保留旧值。
pub fn save(conn: &Connection, config: &ProxyConfig) -> Result<ProxyConfig> {
    let config = config.normalized()?;
    let raw = serde_json::to_string(&config).map_err(|_| invalid("无法保存代理配置"))?;
    db::set_setting(conn, SETTING_KEY, &raw)?;
    Ok(config)
}

/// 备份副本内执行。坏配置或未知版本宁可终止无密钥备份，也不原样带出秘密。
pub(crate) fn strip_backup_auth(conn: &Connection) -> Result<()> {
    if let Some(raw) = db::get_setting(conn, SETTING_KEY)? {
        let config = decode(&raw)?.without_auth()?;
        save(conn, &config)?;
    }
    Ok(())
}

/// `reqwest` 结果的安全写法：`.redacted()` 等价于 `.map_err(|e| request_error(&e))`。
pub trait Redacted<T> {
    fn redacted(self) -> Result<T>;
}

impl<T> Redacted<T> for std::result::Result<T, reqwest::Error> {
    fn redacted(self) -> Result<T> {
        self.map_err(|error| request_error(&error))
    }
}

/// 网络接入方可共用的安全错误投影：保留类别，不传播可能含认证的 URL / source。
#[derive(Debug, Clone, Copy)]
pub enum RequestErrorMessage {
    Timeout,
    Connect,
    Redirect,
    Decode,
    Other,
}

// Preserve the existing display projection while giving IPC a typed value.
impl std::fmt::Display for RequestErrorMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Timeout => "网络请求超时",
            Self::Connect => "网络连接失败，请检查目标服务和代理设置",
            Self::Redirect => "网络重定向失败",
            Self::Decode => "网络响应解码失败",
            Self::Other => "网络请求失败",
        })
    }
}

pub fn request_error(error: &reqwest::Error) -> Error {
    let message = if error.is_timeout() {
        RequestErrorMessage::Timeout
    } else if error.is_connect() {
        RequestErrorMessage::Connect
    } else if error.is_redirect() {
        RequestErrorMessage::Redirect
    } else if error.is_decode() {
        RequestErrorMessage::Decode
    } else {
        RequestErrorMessage::Other
    };
    Error::Network {
        kind: error_kind(error),
        status: error.status().map_or(0, |s| s.as_u16()),
        message,
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tls_tests;

mod webview;
pub use webview::WebviewRoute;
