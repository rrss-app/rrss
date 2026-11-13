//! 定位 Pi 二进制、按 Profile → Adapter → Workspace 准备托管目录、拉起 `pi --mode rpc` 并转发事件。
//! 日志使用本模块路径；stderr 在 debug 中记录脱敏的单行。

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::Result;
use rrss_ai::pi::PiEvent;
use rrss_core::ai_store::PI_BINARY_KEY;
use rrss_core::db;
use rrss_core::network::{self, ChildProxyEnv, NetworkScope, PROXY_ENV_KEYS, ProxyEnvironment};
use tokio::process::Command;
use tokio::sync::mpsc;

use super::adapter::{self, LaunchSpec, PiCaps, PiVersion};
use super::compat::{VersionGate, compat};
use super::profile::{PiBinary, PiProfile, PiSecrets};
use super::workspace::{self, BUNDLED_SKILLS, WorkspacePaths};
use super::{ChatError, ChatErrorKind};
use crate::views::ModelChoice;

mod process;
pub use process::PiProcess;

/// 命令默认的响应期限。`abort` 要等会话空闲才回应，另给更长的期限。
pub const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// 用户技能优先，其次随应用提供的技能。
pub fn skill_load_path(data_dir: &Path, name: &str) -> PathBuf {
    let paths = WorkspacePaths::new(data_dir);
    let user = paths.user_skill(name);
    if user.join("SKILL.md").is_file() {
        user
    } else {
        paths.bundled_skill(name)
    }
}

/// `(名称, 是否用户技能, 加载目录)`。随应用提供的技能即使还没写到 `runtime/` 也列出。
pub fn list_skill_dirs(data_dir: &Path) -> Vec<(String, bool, PathBuf)> {
    let paths = WorkspacePaths::new(data_dir);
    let mut names = std::collections::BTreeSet::new();
    for (name, _) in BUNDLED_SKILLS {
        names.insert((*name).to_string());
    }
    if let Ok(entries) = std::fs::read_dir(&paths.user_skills) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() && !name.starts_with('_') && path.join("SKILL.md").is_file() {
                names.insert(name);
            }
        }
    }
    names
        .into_iter()
        .map(|name| {
            let user = paths.user_skill(&name).join("SKILL.md").is_file();
            let path = skill_load_path(data_dir, &name);
            (name, user, path)
        })
        .collect()
}

/// 随应用提供的技能正文；`runtime/` 里的副本在首次启动前可能还不存在。
pub fn bundled_skill_body(name: &str) -> Option<&'static str> {
    BUNDLED_SKILLS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, body)| *body)
}

pub fn pi_pkg_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("pi-pkg")
}

/// 本平台 Pi 可执行文件可能的名字（见 `platform::Os::pi_program_names`）。
pub fn pi_bin_names() -> &'static [&'static str] {
    rrss_core::platform::Os::current().pi_program_names()
}

/// GitHub 独立包是 Bun 编译的 ELF / PE，不是 `#!/usr/bin/env node` 或指向 `cli.js` 的 npm 包装。
pub fn is_standalone_binary(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if name.ends_with(".cmd")
        || name.ends_with(".bat")
        || name.ends_with(".ps1")
        || name.ends_with(".js")
    {
        return false;
    }
    if let Ok(real) = path.canonicalize() {
        if real
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("js"))
        {
            return false;
        }
    }
    let Ok(mut f) = File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 4];
    if f.read(&mut magic).unwrap_or(0) < 2 {
        return false;
    }
    if magic[0] == b'#' && magic[1] == b'!' {
        return false;
    }
    magic == *b"\x7fELF" || (magic[0] == b'M' && magic[1] == b'Z') || is_mach_o(&magic)
}

fn is_mach_o(magic: &[u8; 4]) -> bool {
    matches!(
        magic,
        [0xfe, 0xed, 0xfa, 0xce]
            | [0xfe, 0xed, 0xfa, 0xcf]
            | [0xce, 0xfa, 0xed, 0xfe]
            | [0xcf, 0xfa, 0xed, 0xfe]
            | [0xca, 0xfe, 0xba, 0xbe]
    )
}

fn first_existing(dir: &Path, standalone_only: bool) -> Option<PathBuf> {
    for name in pi_bin_names() {
        let cand = dir.join(name);
        if !cand.is_file() {
            continue;
        }
        if standalone_only && !is_standalone_binary(&cand) {
            continue;
        }
        return Some(cand);
    }
    None
}

/// 查找顺序：设置里的显式路径 → 与 `rrss` 同目录 → 托管包 `<data_dir>/pi-pkg` → `<data_dir>/bin` → PATH 上的独立二进制 → PATH 上任意 `pi`（npm 包装，不推荐）。
pub fn find_pi(explicit: Option<&str>) -> Option<PathBuf> {
    if let Some(raw) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        let p = PathBuf::from(raw);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if let Some(p) = first_existing(dir, true) {
                return Some(p);
            }
        }
    }
    if let Ok(data) = rrss_core::paths::data_dir() {
        // 托管包里的程序要和旁边的 assets / native 等资源在一起运行，优先于 bin 下的入口。
        for dir in [pi_pkg_dir(&data), data.join("bin")] {
            if let Some(p) = first_existing(&dir, true) {
                return Some(p);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    let mut fallback = None;
    for dir in std::env::split_paths(&path) {
        if let Some(p) = first_existing(&dir, true) {
            return Some(p);
        }
        if fallback.is_none() {
            fallback = first_existing(&dir, false);
        }
    }
    fallback
}

/// 启动前准备好的一切：期望配置、渲染结果与能力表。不含密钥的部分可以展示。
pub struct Prepared {
    pub profile: PiProfile,
    pub spec: LaunchSpec,
    pub caps: PiCaps,
    /// `pi --version` 的结果；读不出时为空。
    pub version: Option<PiVersion>,
    pub gate: VersionGate,
    secrets: PiSecrets,
    /// Pi 用途的代理变量，可能含认证；只进子进程环境（规格 23 · 6.4）。
    proxy: ChildProxyEnv,
}

/// Pi 用途的子进程代理环境：每次启动现读配置与当前进程环境，保存代理不影响已在运行的 Pi。
/// 坏配置报错而不是静默直连。
pub fn pi_proxy_env(conn: &rusqlite::Connection) -> Result<ChildProxyEnv, ChatError> {
    network::load(conn)
        .and_then(|config| config.child_env(NetworkScope::Pi, &ProxyEnvironment::capture()))
        .map_err(|err| ChatError::new(ChatErrorKind::Config, err.to_string()))
}

/// 先清掉继承的全部代理变量，再写入 Pi 用途的值；不改父进程环境。
pub(crate) fn apply_proxy_env<C: ProxyEnvTarget>(cmd: &mut C, proxy: &ChildProxyEnv) {
    for key in PROXY_ENV_KEYS {
        cmd.remove_env(key);
    }
    for (key, value) in proxy.vars() {
        cmd.set_env(key, value);
    }
}

/// `tokio::process::Command` 与 `std::process::Command` 共用的环境写法。
pub(crate) trait ProxyEnvTarget {
    fn remove_env(&mut self, key: &str);
    fn set_env(&mut self, key: &str, value: &str);
}

impl ProxyEnvTarget for Command {
    fn remove_env(&mut self, key: &str) {
        self.env_remove(key);
    }
    fn set_env(&mut self, key: &str, value: &str) {
        self.env(key, value);
    }
}

impl ProxyEnvTarget for std::process::Command {
    fn remove_env(&mut self, key: &str) {
        self.env_remove(key);
    }
    fn set_env(&mut self, key: &str, value: &str) {
        self.env(key, value);
    }
}

/// 读取当前期望配置；不下载、不写文件。找不到 Pi 时报未安装。
pub fn current_profile(
    conn: &rusqlite::Connection,
    data_dir: &Path,
    target: Option<&ModelChoice>,
) -> Result<(PiProfile, PiSecrets), ChatError> {
    let explicit = db::get_setting(conn, PI_BINARY_KEY)
        .map_err(|err| ChatError::new(ChatErrorKind::Internal, err.to_string()))?;
    let binary = find_pi(explicit.as_deref())
        .ok_or_else(|| ChatError::new(ChatErrorKind::NotInstalled, PI_NOT_FOUND_HINT))?;
    PiProfile::load(
        conn,
        &WorkspacePaths::new(data_dir),
        PiBinary::new(binary),
        target,
    )
}

/// 读配置、选适配器并渲染，不写文件。
fn render_current(
    conn: &rusqlite::Connection,
    data_dir: &Path,
    target: Option<&ModelChoice>,
) -> Result<Prepared, ChatError> {
    let (profile, secrets) = current_profile(conn, data_dir, target)?;
    let version = adapter::probe_version(&profile.binary);
    let gate = compat().gate(version.as_ref());
    let adapter = adapter::adapter_for(version.as_ref());
    let caps = adapter.caps(version.as_ref());
    let spec = adapter.render(&profile, &caps, &WorkspacePaths::new(data_dir));
    let proxy = pi_proxy_env(conn)?;
    Ok(Prepared {
        profile,
        spec,
        caps,
        version,
        gate,
        secrets,
        proxy,
    })
}

/// 「查看生成的配置」（19-5）要的数据：启动参数、RRSS 设置的环境变量、生成文件与被忽略的覆盖键。
/// 密钥不在其中：它只在启动时经 `RRSS_PI_KEY_<连接 id>` 传入，`models.json` 里只是变量引用。
#[derive(Debug, Clone, PartialEq, serde::Serialize, ts_rs::TS)]
pub struct GeneratedConfig {
    pub own_config: bool,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    // `(相对 <data>/pi 的路径, 内容)`；扩展与技能正文不列出。
    pub files: Vec<(PathBuf, String)>,
    pub ignored: Vec<String>,
}

pub fn generated_config(
    conn: &rusqlite::Connection,
    data_dir: &Path,
) -> Result<GeneratedConfig, ChatError> {
    let (profile, _) = current_profile(conn, data_dir, None)?;
    let version = adapter::probe_version(&profile.binary);
    let adapter = adapter::adapter_for(version.as_ref());
    let caps = adapter.caps(version.as_ref());
    let spec = adapter.render(&profile, &caps, &WorkspacePaths::new(data_dir));
    // 代理地址去掉认证后展示；坏配置不挡住查看其余配置，但写明 Pi 无法启动。
    let mut env = spec.env.clone();
    match pi_proxy_env(conn) {
        Ok(proxy) => env.extend(proxy.redacted()),
        Err(err) => env.push((
            "网络代理".into(),
            format!("{}（修正前 Pi 无法启动）", err.message),
        )),
    }
    let files = spec
        .files
        .iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "json"))
        .map(|f| (f.path.clone(), redact_config_json(&f.contents)))
        .collect();
    Ok(GeneratedConfig {
        own_config: profile.mode == super::profile::PiMode::Own,
        program: spec.program,
        args: spec.args,
        env,
        files,
        ignored: spec.ignored,
    })
}

fn log_stderr(line: &str, secrets: &[String]) -> String {
    let clean = rrss_core::logsafe::scrub_secrets_with(line, secrets.iter().map(String::as_str));
    let line = rrss_core::logsafe::text(&clean, 1000);
    tracing::debug!(%line, "pi stderr");
    line
}

fn redact_config_json(bytes: &[u8]) -> String {
    fn mask(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(obj) => {
                for (key, value) in obj {
                    let key = key.to_ascii_lowercase().replace(['_', '-'], "");
                    if [
                        "apikey",
                        "token",
                        "password",
                        "secret",
                        "authorization",
                        "headers",
                        "env",
                        "args",
                        "command",
                    ]
                    .iter()
                    .any(|s| key.contains(s))
                    {
                        *value = serde_json::json!("***");
                    } else {
                        mask(value);
                    }
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    mask(item);
                }
            }
            serde_json::Value::String(text) => *text = rrss_core::logsafe::scrub_secrets(text),
            _ => {}
        }
    }
    match serde_json::from_slice::<serde_json::Value>(bytes) {
        Ok(mut value) => {
            mask(&mut value);
            value.to_string()
        }
        Err(_) => "（配置无法解析，未显示原文）".into(),
    }
}

/// 读配置、选适配器、渲染并写入托管目录。阻塞操作，调用方放进阻塞线程。
pub fn prepare(
    db_path: &Path,
    data_dir: &Path,
    target: Option<&ModelChoice>,
) -> Result<Prepared, ChatError> {
    let paths = WorkspacePaths::new(data_dir);
    if let Err(err) = workspace::migrate_legacy_skills(data_dir) {
        tracing::warn!(error = %format!("{err:#}"), "migrate legacy pi skills");
    }
    let conn = db::open_at(db_path)
        .map_err(|err| ChatError::new(ChatErrorKind::Internal, err.to_string()))?;
    let prepared = render_current(&conn, data_dir, target)?;
    drop(conn);
    if prepared.gate == VersionGate::TooOld {
        let found = prepared.version.map(|v| v.to_string()).unwrap_or_default();
        return Err(ChatError::new(
            ChatErrorKind::Version,
            format!(
                "对话引擎版本过低（{found}，至少需要 {}）：请在设置里更新托管的 Pi，或改用新版的 pi 路径",
                compat().min
            ),
        ));
    }
    let spec = &prepared.spec;
    for key in &spec.ignored {
        tracing::warn!(
            key,
            error_kind = "controlled_setting",
            "pi override key is controlled by RRSS; ignored"
        );
    }
    workspace::write_files(&paths, &spec.files).map_err(|err| {
        ChatError::new(
            ChatErrorKind::Internal,
            format!("写入对话引擎配置失败：{err:#}"),
        )
    })?;
    for stale in &spec.stale {
        let _ = std::fs::remove_file(paths.root.join(stale));
    }
    tracing::info!(
        model = spec.expected_model.as_deref().unwrap_or("pi default"),
        own_config = prepared.profile.mode == super::profile::PiMode::Own,
        "pi workspace ready"
    );
    Ok(prepared)
}

/// 按准备好的 [`LaunchSpec`] 启动 `pi --mode rpc`，事件经 `events` 转发。密钥只经环境变量传入。
pub async fn spawn(
    db_path: &Path,
    data_dir: &Path,
    rrss_tool: &Path,
    target: Option<ModelChoice>,
    events: mpsc::UnboundedSender<PiEvent>,
) -> std::result::Result<(PiProcess, Prepared), ChatError> {
    if let Err(err) = super::install::ensure_pi_binary(db_path, data_dir, false).await {
        tracing::warn!(error = %err, "ensure managed pi before spawn");
    }
    let prepared = {
        let (db_path, data_dir) = (db_path.to_path_buf(), data_dir.to_path_buf());
        crate::execution::blocking(move || prepare(&db_path, &data_dir, target.as_ref()))
            .await
            .map_err(|err| ChatError::new(ChatErrorKind::Internal, err.to_string()))??
    };
    let spec = &prepared.spec;

    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        .current_dir(&spec.cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("RRSS_TOOL", rrss_tool)
        .env("RRSS_DB", db_path)
        .kill_on_drop(true);
    rrss_core::platform::hide_console_window_async(&mut cmd);
    for (name, key) in &prepared.secrets.api_keys {
        cmd.env(name, key);
    }
    apply_proxy_env(&mut cmd, &prepared.proxy);
    for (key, value) in &spec.env {
        cmd.env(key, value);
    }

    tracing::info!(file = %rrss_core::logsafe::path(&spec.program), "starting pi --mode rpc");
    let child = cmd.spawn().map_err(|err| {
        ChatError::new(
            ChatErrorKind::ProcessExit,
            format!("无法启动对话引擎：{err}"),
        )
    })?;
    let mut secrets = prepared
        .secrets
        .api_keys
        .iter()
        .map(|(_, value)| value.clone())
        .collect::<Vec<_>>();
    secrets.extend(prepared.proxy.secrets());
    let process = PiProcess::watch(child, secrets, events)?;
    Ok((process, prepared))
}

pub fn validate_skill_name(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() || name.len() > 64 {
        anyhow::bail!("skill 名称须 1–64 字符");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        anyhow::bail!("skill 名称只能是小写字母、数字和连字符");
    }
    Ok(())
}

pub fn validate_skill_description(description: &str) -> Result<()> {
    let description = description.trim();
    if description.is_empty() {
        anyhow::bail!("skill 需要 description");
    }
    if description.chars().count() > 1024 {
        anyhow::bail!("skill description 不能超过 1024 字");
    }
    Ok(())
}

/// 从 SKILL.md 取 frontmatter 的 description；没有则空串。
pub fn skill_description_from_md(raw: &str) -> String {
    let Some(rest) = raw.strip_prefix("---") else {
        return String::new();
    };
    let Some(end) = rest.find("\n---") else {
        return String::new();
    };
    for line in rest[..end].lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("description:") {
            return value.trim().trim_matches('"').to_string();
        }
    }
    String::new()
}

pub fn write_user_skill(
    data_dir: &Path,
    name: &str,
    description: &str,
    body: &str,
) -> Result<PathBuf> {
    validate_skill_name(name)?;
    validate_skill_description(description)?;
    let dir = WorkspacePaths::new(data_dir).user_skill(name);
    std::fs::create_dir_all(&dir)?;
    let body = body.trim();
    let md = if body.starts_with("---") {
        format!("{body}\n")
    } else {
        format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}\n")
    };
    let path = dir.join("SKILL.md");
    std::fs::write(&path, md)?;
    Ok(dir)
}

pub fn delete_user_skill(data_dir: &Path, name: &str) -> Result<()> {
    let dir = WorkspacePaths::new(data_dir).user_skill(name);
    if !dir.exists() {
        anyhow::bail!("没有名为 {name} 的用户 skill");
    }
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

fn describe_pi_source(explicit: Option<&str>, path: &Path) -> String {
    let kind = if is_standalone_binary(path) {
        "独立二进制"
    } else {
        "Node 包装（不推荐，会带上 npm 依赖树）"
    };
    if explicit.map(str::trim).filter(|s| !s.is_empty()).is_some() {
        format!("{kind} · 显式路径：{}", path.display())
    } else {
        format!("{kind}：{}", path.display())
    }
}

pub fn probe_pi_version(explicit: Option<&str>) -> (String, String, String) {
    match find_pi(explicit) {
        Some(path) => {
            let source = describe_pi_source(explicit, &path);
            let version = rrss_core::platform::hide_console_window(
                std::process::Command::new(&path).arg("--version"),
            )
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "已找到，未能读取版本".into());
            (source, version, path.display().to_string())
        }
        None => (PI_NOT_FOUND_HINT.into(), String::new(), String::new()),
    }
}

/// 只查文件系统定位 pi，不执行子进程；给页面首屏用。
pub fn locate_pi(explicit: Option<&str>) -> (String, Option<PathBuf>) {
    match find_pi(explicit) {
        Some(path) => (describe_pi_source(explicit, &path), Some(path)),
        None => (PI_NOT_FOUND_HINT.into(), None),
    }
}

const PI_NOT_FOUND_HINT: &str = "未找到 pi 独立二进制。从 GitHub Releases 下载 pi-linux-x64.tar.gz / pi-windows-x64.zip，放到 PATH、与 rrss 同目录，或 ~/.local/share/rrss/bin（Windows：%APPDATA%\\rrss\\bin）。";

#[cfg(test)]
mod tests {
    #[test]
    fn stderr_file_log_redacts_known_secrets_and_query_strings() {
        let dir = tempfile::tempdir().unwrap();
        let (subscriber, handle) = crate::log::subscriber(
            crate::log::Mode::Desktop {
                log_dir: dir.path().into(),
            },
            "rrss=debug",
        );
        tracing::subscriber::with_default(subscriber, || {
            super::log_stderr(
                "error custom-canary-key https://user:proxy-canary@example.test/a?token=query-canary\nnext",
                &["custom-canary-key".into(), "proxy-canary".into()],
            );
        });
        handle.end();
        let text = std::fs::read_to_string(dir.path().join("rrss.log")).unwrap();
        assert!(text.contains("pi stderr"));
        for secret in ["custom-canary-key", "proxy-canary", "query-canary", "user:"] {
            assert!(!text.contains(secret));
        }
        assert_eq!(text.lines().count(), 2);
    }
    use super::*;

    #[test]
    fn proxy_credentials_are_masked_in_engine_output_and_generated_config() {
        let dir = tempfile::tempdir().unwrap();
        let conn = db::open_at(&dir.path().join("rrss.db")).unwrap();
        let fake = dir.path().join("fake-pi");
        std::fs::write(&fake, b"not executed").unwrap();
        db::set_setting(&conn, PI_BINARY_KEY, fake.to_str().unwrap()).unwrap();
        // 自有配置不要求选对话模型；代理环境与托管模式同一条路径。
        rrss_core::pi_store::set_use_own_config(&conn, true).unwrap();
        network::save(
            &conn,
            &network::ProxyConfig {
                pi: network::ProxyRule::custom("http://pi-user:pi%2Fpass@proxy.test:3128"),
                ..Default::default()
            },
        )
        .unwrap();

        let proxy = pi_proxy_env(&conn).unwrap();
        let line = "proxy http://pi-user:pi%2Fpass@proxy.test:3128 refused (pi/pass)";
        let masked = log_stderr(line, &proxy.secrets());
        for secret in ["pi-user", "pi%2Fpass", "pi/pass"] {
            assert!(!masked.contains(secret), "{masked}");
        }

        let shown = generated_config(&conn, dir.path()).unwrap();
        let rendered = format!("{:?}", shown.env);
        assert!(
            rendered.contains("http://proxy.test:3128/（认证已隐藏）"),
            "{rendered}"
        );
        assert!(!rendered.contains("pi-user") && !rendered.contains("pi%2Fpass"));

        // 坏配置不挡住查看其余配置，但明确写出 Pi 无法启动，不回显原文。
        db::set_setting(&conn, network::SETTING_KEY, "broken-private-secret").unwrap();
        let shown = generated_config(&conn, dir.path()).unwrap();
        let rendered = format!("{:?}", shown.env);
        assert!(rendered.contains("Pi 无法启动"), "{rendered}");
        assert!(!rendered.contains("private-secret"));
    }

    #[test]
    fn displayed_configuration_masks_credentials_without_changing_the_source() {
        let raw = br#"{"providers":{"custom":{"apiKey":"plain-credential","baseUrl":"https://example.test"}},"mcp":{"env":{"CUSTOM_AUTH":"env-secret"},"args":["--secret","arg-secret"]},"nested":[{"password":"password-secret"}]}"#;
        let display = redact_config_json(raw);
        for secret in [
            "plain-credential",
            "env-secret",
            "arg-secret",
            "password-secret",
        ] {
            assert!(!display.contains(secret));
        }
        assert!(display.contains("https://example.test"));
        assert!(String::from_utf8_lossy(raw).contains("plain-credential"));
    }

    #[test]
    fn user_skills_shadow_bundled_ones_and_live_in_the_managed_dir() {
        let dir = tempfile::tempdir().unwrap();
        let listed = list_skill_dirs(dir.path());
        assert_eq!(
            listed,
            vec![(
                "cite-verify".to_string(),
                false,
                dir.path().join("pi/runtime/skills/cite-verify")
            )]
        );
        let written = write_user_skill(dir.path(), "cite-verify", "mine", "body").unwrap();
        assert_eq!(written, dir.path().join("pi/user-skills/cite-verify"));
        write_user_skill(dir.path(), "notes", "my notes", "body").unwrap();
        let listed = list_skill_dirs(dir.path());
        assert_eq!(listed.len(), 2);
        assert!(listed.iter().all(|(_, user, _)| *user));
        assert_eq!(skill_load_path(dir.path(), "cite-verify"), written);
        delete_user_skill(dir.path(), "cite-verify").unwrap();
        assert_eq!(
            skill_load_path(dir.path(), "cite-verify"),
            dir.path().join("pi/runtime/skills/cite-verify")
        );
        assert!(bundled_skill_body("cite-verify").is_some());
    }

    #[test]
    fn skill_name_rules() {
        assert!(validate_skill_name("cite-verify").is_ok());
        assert!(validate_skill_name("Bad Name").is_err());
        assert!(validate_skill_description("ok").is_ok());
        assert!(validate_skill_description("").is_err());
        let desc = skill_description_from_md("---\nname: x\ndescription: hello\n---\nbody\n");
        assert_eq!(desc, "hello");
    }

    #[test]
    fn standalone_binary_rejects_node_wrappers() {
        let dir = std::env::temp_dir().join(format!("rrss-pi-magic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let js = dir.join("cli.js");
        std::fs::write(&js, b"console.log('pi')\n").unwrap();
        let shim = dir.join("pi-shim");
        std::fs::write(&shim, b"#!/usr/bin/env node\nconsole.log(1)\n").unwrap();
        let elf = dir.join("pi-elf");
        std::fs::write(&elf, b"\x7fELF\x02\x01\x01\x00rest").unwrap();
        assert!(!is_standalone_binary(&js));
        assert!(!is_standalone_binary(&shim));
        assert!(is_standalone_binary(&elf));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
