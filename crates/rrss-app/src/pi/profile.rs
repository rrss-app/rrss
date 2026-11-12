//! `PiProfile`：RRSS 对 Pi 的期望配置。纯数据，与 Pi 版本无关，由数据库一次读出，带指纹。
//! 密钥不进 Profile，只以哈希参与指纹；原文放在 [`PiSecrets`]，由启动时经环境变量传入。

use std::path::PathBuf;

use rrss_core::ai_store::{self, DEVELOPER_TOOLS_KEY, FETCH_URL_ENABLED_KEY};
use rrss_core::db;
use rrss_core::pi_store;
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use super::workspace::{self, WorkspacePaths};
use super::{ChatError, ChatErrorKind};
use crate::views::ModelChoice;

/// 生成的 `models.json` 里 RRSS 的 provider 名前缀：每个连接一个 `rrss-<连接 id>`。
pub const PROVIDER_PREFIX: &str = "rrss-";
/// 密钥经环境变量传给 Pi，`models.json` 只写 `$RRSS_PI_KEY_<连接 id>`。
pub const KEY_ENV_PREFIX: &str = "RRSS_PI_KEY_";

pub fn provider_name(provider_id: i64) -> String {
    format!("{PROVIDER_PREFIX}{provider_id}")
}

pub fn key_env(provider_id: i64) -> String {
    format!("{KEY_ENV_PREFIX}{provider_id}")
}

/// 托管模式下的 `provider/id`。
pub fn managed_ref(provider_id: i64, model: &str) -> String {
    format!("{}/{}", provider_name(provider_id), model.trim())
}
/// 最近一次提示过的「未验证版本」版本号。
pub const UNVERIFIED_NOTICE_KEY: &str = "pi_unverified_noticed";
/// 本地端点没有密钥时的占位值：Pi 要求 provider 有认证才会把模型列为可用。
const NO_KEY: &str = "rrss-no-key";

/// 对话模型从哪里来。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    /// 模型页选的对话默认（`chat` 用途）。
    Bound,
    /// 「使用我自己的 Pi 配置」：`pi_own_model` 或 Pi 的默认模型。
    Own,
}

/// 托管（默认，隔离 agent 目录）或使用用户自己的 `~/.pi` 配置（D3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PiMode {
    Managed,
    Own,
}

/// `models.json` 里的一个模型（阶段 19-7）：名称、能力与对话参数来自模型设置。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PiModel {
    pub id: String,
    pub name: Option<String>,
    pub reasoning: bool,
    pub image: bool,
    pub context_window: Option<i64>,
    pub max_tokens: Option<i64>,
    /// 这个模型单独的兼容设置；没有时用连接的。
    pub compat: Option<Value>,
    /// 对话里的思考强度：off / low / medium / high。
    pub thinking: String,
}

impl PiModel {
    /// 没加进模型列表（对话里选过、后来移出）的模型：不思考，不声明额外能力。
    fn bare(id: &str) -> Self {
        Self {
            id: id.trim().to_string(),
            name: None,
            reasoning: false,
            image: false,
            context_window: None,
            max_tokens: None,
            compat: None,
            thinking: "off".into(),
        }
    }

    fn from_entry(e: &rrss_core::model_pool::PoolEntry) -> Self {
        use rrss_core::model_pool::Support;
        let caps = e.effective();
        let thinking = e.chat_thinking().to_string();
        Self {
            id: e.model.clone(),
            name: (!e.display_name.trim().is_empty()).then(|| e.display_name.trim().to_string()),
            // 未知推理能力允许线程临时开启思考；明确不支持仍关闭。
            reasoning: caps.reasoning != Support::Unsupported,
            image: caps.image_input == Support::Supported,
            context_window: e.context_window,
            max_tokens: e.max_output,
            compat: e.compat.clone(),
            thinking,
        }
    }
}

/// 托管模式下一个连接对应的 provider：端点与 Pi 能切换到的模型。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PiEndpoint {
    pub provider_id: i64,
    /// 这个连接上用于对话的模型、对话默认与这条对话选定的模型；按 id 排序去重。
    pub models: Vec<PiModel>,
    pub base_url: String,
    /// `openai-completions` 或 `openai-responses`。
    pub api: String,
    /// `ai_providers.compat_json`，原样写进 provider 的 `compat`。
    pub compat: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolPolicy {
    pub developer: bool,
    pub mcp: bool,
    /// 启用的 RRSS 工具（已按 `fetch_url_enabled` 过滤）。
    pub enabled: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillRef {
    pub name: String,
    /// 实际加载的目录：用户技能优先，其次随应用提供的。
    pub path: PathBuf,
    /// 内容哈希；技能内容变了也要重启。
    pub digest: String,
}

/// 选中的 Pi 二进制。路径、大小与修改时间变化就算换了二进制。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PiBinary {
    pub path: PathBuf,
    pub stamp: String,
}

impl PiBinary {
    pub fn new(path: PathBuf) -> Self {
        let stamp = std::fs::metadata(&path)
            .map(|m| {
                let modified = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                format!("{}:{modified}", m.len())
            })
            .unwrap_or_default();
        Self { path, stamp }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PiProfile {
    pub mode: PiMode,
    /// 托管模式的各连接端点（按连接 id 排序）；自有配置下为空。
    pub endpoints: Vec<PiEndpoint>,
    /// 托管模式的默认模型 `rrss-<id>/<model>`，即对话默认（阶段 19-6）。
    pub default_model: Option<String>,
    /// 自有配置下选用的 `provider/id`；空表示用 Pi 的默认模型。
    pub own_model: Option<String>,
    pub model_source: ModelSource,
    /// 这一轮要用的 `provider/id`：对话中选定的模型，没选时同默认。不参与指纹，换模型在进程内切换。
    #[serde(skip)]
    pub wanted_model: Option<String>,
    /// 默认模型的思考强度（阶段 19-7 起按模型设置）；自有配置下为空，交给用户的 Pi。
    pub thinking: Option<String>,
    /// 这一轮要用的模型的思考强度。不参与指纹，换模型时在进程内设置。
    #[serde(skip)]
    pub wanted_thinking: Option<String>,
    /// `pi_cache_warming`，原样写进 `agent/settings.json`。
    pub cache_warming: String,
    pub tools: ToolPolicy,
    pub skills: Vec<SkillRef>,
    /// `tool-manifest.json` 的内容。
    pub manifest: Value,
    /// MCP 打开时的服务器配置（`{"mcpServers": …}`），写进 `mcp.json`。
    pub mcp: Option<Value>,
    pub binary: PiBinary,
    /// 扩展与随应用提供的技能的内容哈希。
    pub runtime: String,
    /// 固定模板 + 助手说明，写成 `project/.pi/SYSTEM.md`。长期记忆不在这里，由扩展每轮写入小节，不参与指纹。
    pub system_prompt: String,
    /// L2 覆盖：`pi_settings_override` / `pi_models_override`，只在托管模式生效。
    pub settings_override: Value,
    pub models_override: Value,
    key_digest: String,
}

pub struct PiSecrets {
    /// 连接密钥与 MCP 变量 / 请求头，仅经进程环境传递。
    pub api_keys: Vec<(String, String)>,
}

impl std::fmt::Debug for PiSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PiSecrets(***)")
    }
}

impl PiProfile {
    /// 从数据库读出期望配置。托管模式下缺连接或模型时报配置错误；自有配置不需要 RRSS 的连接。
    /// `target` 是这条对话选定的模型：连接 id 为 0 的只在自有配置下生效，其余只在托管模式下生效；
    /// 连接已删除时忽略，回到对话默认。
    pub fn load(
        conn: &Connection,
        paths: &WorkspacePaths,
        binary: PiBinary,
        target: Option<&ModelChoice>,
    ) -> Result<(Self, PiSecrets), ChatError> {
        let db_err =
            |err: rrss_core::Error| ChatError::new(ChatErrorKind::Internal, err.to_string());
        let own = pi_store::use_own_config(conn).map_err(db_err)?;
        let target = target.filter(|t| !t.model.trim().is_empty());
        let (
            mode,
            endpoints,
            default_model,
            own_model,
            model_source,
            wanted_model,
            (thinking, wanted_thinking),
            mut api_keys,
        ) = if own {
            let model = pi_store::own_model(conn).map_err(db_err)?;
            let wanted = target
                .filter(|t| t.provider_id == 0)
                .map(|t| t.model.trim().to_string())
                .or_else(|| model.clone());
            (
                PiMode::Own,
                Vec::new(),
                None,
                model,
                ModelSource::Own,
                wanted,
                (None, None),
                Vec::new(),
            )
        } else {
            let managed = managed_endpoints(conn, target.filter(|t| t.provider_id > 0))?;
            (
                PiMode::Managed,
                managed.endpoints,
                Some(managed.default_model),
                None,
                ModelSource::Bound,
                Some(managed.wanted_model),
                (
                    Some(managed.default_thinking),
                    Some(managed.wanted_thinking),
                ),
                managed.keys,
            )
        };

        let setting_on = |key: &str| -> Result<bool, ChatError> {
            Ok(db::get_setting(conn, key).map_err(db_err)?.as_deref() == Some("1"))
        };
        let developer = setting_on(DEVELOPER_TOOLS_KEY)?;
        let fetch_url = setting_on(FETCH_URL_ENABLED_KEY)?;
        let mcp = pi_store::mcp_enabled(conn).map_err(db_err)?;
        let mut enabled = pi_store::enabled_tool_names(conn).map_err(db_err)?;
        if !fetch_url {
            enabled.retain(|t| t != "fetch_url");
        }
        let manifest = serde_json::to_value(pi_store::tool_manifest(conn).map_err(db_err)?)
            .unwrap_or(Value::Null);
        let (config, secrets) = pi_store::mcp_config(conn).map_err(db_err)?;
        let enabled_ids: Vec<_> = pi_store::list_mcp_servers(conn)
            .map_err(db_err)?
            .into_iter()
            .filter(|s| s.enabled)
            .map(|s| format!("RRSS_MCP_{}_", s.id))
            .collect();
        api_keys.extend(
            secrets
                .into_iter()
                .filter(|(key, _)| enabled_ids.iter().any(|prefix| key.starts_with(prefix))),
        );
        let mcp_json = Some(config);
        let skills = pi_store::enabled_skill_names(conn)
            .map_err(db_err)?
            .into_iter()
            .filter_map(|name| skill_ref(paths, &name))
            .collect();

        let profile = Self {
            mode,
            endpoints,
            default_model,
            own_model,
            model_source,
            wanted_model,
            thinking,
            wanted_thinking,
            cache_warming: pi_store::cache_warming(conn).map_err(db_err)?,
            tools: ToolPolicy {
                developer,
                mcp,
                enabled,
            },
            skills,
            manifest,
            mcp: mcp_json,
            binary,
            runtime: workspace::runtime_digest(),
            system_prompt: workspace::system_prompt(
                &pi_store::assistant_note(conn).map_err(db_err)?,
            ),
            settings_override: pi_store::pi_override(conn, pi_store::PI_SETTINGS_OVERRIDE_KEY)
                .map_err(db_err)?,
            models_override: pi_store::pi_override(conn, pi_store::PI_MODELS_OVERRIDE_KEY)
                .map_err(db_err)?,
            key_digest: if api_keys.is_empty() {
                String::new()
            } else {
                digest(format!("{api_keys:?}").as_bytes())
            },
        };
        Ok((profile, PiSecrets { api_keys }))
    }

    /// 握手时 `get_state` 必须报告的 `provider/id`：托管模式是对话默认 `rrss-<id>/<model>`；
    /// 自有配置是选中的模型，没选时为空（接受 Pi 的默认模型）。
    pub fn model_ref(&self) -> Option<String> {
        match self.mode {
            PiMode::Managed => self.default_model.clone(),
            PiMode::Own => self.own_model.clone(),
        }
    }

    /// 规范化序列化（键排序）后的哈希。密钥只以哈希参与。
    pub fn fingerprint(&self) -> String {
        let value = serde_json::to_value(self).unwrap_or(Value::Null);
        digest(canonical(&value).as_bytes())
    }
}

struct Managed {
    endpoints: Vec<PiEndpoint>,
    default_model: String,
    default_thinking: String,
    wanted_model: String,
    wanted_thinking: String,
    keys: Vec<(String, String)>,
}

/// 托管模式：每个连接一个 provider，列出用于对话的模型与对话默认，Pi 进程内即可切换（阶段 19-6 / 19-7）。
/// 加进来但不用于对话的模型不写，它们变化不引起重启。对话中选定的模型不在其中时（已移出或关了「用于对话」）
/// 也列进去，这时指纹变化、按既有机制重启。密钥没有时用占位值；没填地址的连接跳过，除非它是对话默认。
fn managed_endpoints(
    conn: &Connection,
    target: Option<&ModelChoice>,
) -> Result<Managed, ChatError> {
    let db_err = |err: rrss_core::Error| ChatError::new(ChatErrorKind::Internal, err.to_string());
    let (default, default_id) = resolve_chat_model(conn)?;
    let pool = rrss_core::model_pool::pool(conn).map_err(db_err)?;
    let described = |provider_id: i64, model: &str| {
        pool.iter()
            .find(|e| e.provider_id == provider_id && e.model == model.trim())
            .map(PiModel::from_entry)
            .unwrap_or_else(|| PiModel::bare(model))
    };
    let mut wanted: BTreeMap<i64, BTreeMap<String, PiModel>> = BTreeMap::new();
    let mut add = |provider_id: i64, model: PiModel| {
        if !model.id.is_empty() {
            wanted
                .entry(provider_id)
                .or_default()
                .insert(model.id.clone(), model);
        }
    };
    let default_model = described(default.id, &default_id);
    let default_thinking = default_model.thinking.clone();
    add(default.id, default_model);
    for entry in pool.iter().filter(|e| e.chat) {
        add(entry.provider_id, PiModel::from_entry(entry));
    }
    let mut wanted_model = managed_ref(default.id, &default_id);
    let mut wanted_thinking = default_thinking.clone();
    if let Some(t) = target {
        match ai_store::get_provider(conn, t.provider_id) {
            Ok(p) if !p.base_url.trim().is_empty() => {
                let model = described(p.id, &t.model);
                wanted_thinking = model.thinking.clone();
                add(p.id, model);
                wanted_model = managed_ref(p.id, &t.model);
            }
            Ok(_) | Err(rrss_core::Error::ProviderNotFound) => {}
            Err(err) => return Err(db_err(err)),
        }
    }

    let mut endpoints = Vec::new();
    let mut keys = Vec::new();
    for (provider_id, models) in wanted {
        let provider = if provider_id == default.id {
            default.clone()
        } else {
            match ai_store::get_provider(conn, provider_id) {
                Ok(p) => p,
                Err(rrss_core::Error::ProviderNotFound) => continue,
                Err(err) => return Err(db_err(err)),
            }
        };
        let base_url = provider.base_url.trim().to_string();
        if base_url.is_empty() {
            if provider_id == default.id {
                return Err(ChatError::new(
                    ChatErrorKind::Config,
                    format!(
                        "连接「{}」没有填写接口地址，请在设置 → 模型里补全",
                        provider.name
                    ),
                ));
            }
            continue;
        }
        let key = provider
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .unwrap_or(NO_KEY)
            .to_string();
        keys.push((key_env(provider_id), key));
        endpoints.push(PiEndpoint {
            provider_id,
            models: models.into_values().collect(),
            base_url,
            api: pi_api(&provider.protocol).into(),
            compat: ai_store::provider_compat(conn, provider_id).map_err(db_err)?,
        });
    }
    Ok(Managed {
        endpoints,
        default_model: managed_ref(default.id, &default_id),
        default_thinking,
        wanted_model,
        wanted_thinking,
        keys,
    })
}

/// 对话默认模型：模型页「对话」选的那个（阶段 19-7 起没有主模型可以继承）。
fn resolve_chat_model(conn: &Connection) -> Result<(ai_store::AiProvider, String), ChatError> {
    let db_err = |err: rrss_core::Error| ChatError::new(ChatErrorKind::Internal, err.to_string());
    let binding = ai_store::get_task_binding(conn, ai_store::TASK_CHAT).map_err(db_err)?;
    ai_store::resolve_binding_model(conn, &binding)
        .map_err(db_err)?
        .ok_or_else(|| {
            ChatError::new(
                ChatErrorKind::Config,
                "还没选对话模型，请在设置 → 模型的「对话」里选一个",
            )
        })
}

/// `ai_providers.protocol` → Pi 的 `api`。`auto` 在测试连接后会被改写成探测到的协议；未探测时按 Chat Completions。
pub fn pi_api(protocol: &str) -> &'static str {
    match ai_store::normalize_protocol(protocol) {
        "responses" => "openai-responses",
        _ => "openai-completions",
    }
}

fn skill_ref(paths: &WorkspacePaths, name: &str) -> Option<SkillRef> {
    let user = paths.user_skill(name);
    let (path, body) = if let Ok(body) = std::fs::read(user.join("SKILL.md")) {
        (user, body)
    } else {
        let (_, body) = workspace::BUNDLED_SKILLS.iter().find(|(n, _)| *n == name)?;
        (paths.bundled_skill(name), body.as_bytes().to_vec())
    };
    Some(SkillRef {
        name: name.to_string(),
        path,
        digest: digest(&body),
    })
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    hash.iter().map(|b| format!("{b:02x}")).collect()
}

/// 键排序的紧凑 JSON；不依赖 `serde_json` 是否启用 `preserve_order`。
pub(crate) fn canonical(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let body = keys
                .into_iter()
                .map(|k| format!("{}:{}", Value::String(k.clone()), canonical(&map[k])))
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        other => other.to_string(),
    }
}

/// 单测共用：一次性库 + 一个对话连接 + 一个假二进制。
#[cfg(test)]
pub(crate) mod testkit {
    use super::*;
    use rrss_core::ai_store::NewProvider;

    pub struct Kit {
        pub dir: tempfile::TempDir,
        pub conn: rrss_core::db::PooledConnection,
        pub provider: i64,
    }

    impl Kit {
        pub fn paths(&self) -> WorkspacePaths {
            WorkspacePaths::new(self.dir.path())
        }

        pub fn load(&self) -> Result<(PiProfile, PiSecrets), ChatError> {
            let bin = self.dir.path().join("pi-bin");
            PiProfile::load(&self.conn, &self.paths(), PiBinary::new(bin), None)
        }

        pub fn profile(&self) -> PiProfile {
            self.load().unwrap().0
        }

        pub fn update(&self, f: impl FnOnce(&mut ai_store::AiProvider)) {
            let mut p = ai_store::get_provider(&self.conn, self.provider).unwrap();
            f(&mut p);
            ai_store::update_provider(&self.conn, &p).unwrap();
        }
    }

    pub fn provider(name: &str, key: Option<&str>) -> NewProvider {
        NewProvider {
            name: name.into(),
            base_url: format!("http://{name}.test/v1"),
            api_key: key.map(Into::into),
            protocol: "chat".into(),
        }
    }

    /// 加一个模型；对话列表为空时它自动用于对话。
    pub fn add_model(conn: &Connection, provider_id: i64, model: &str) {
        rrss_core::model_pool::update_pool(
            conn,
            &[rrss_core::model_pool::ModelRef::new(provider_id, model)],
            &[],
            None,
        )
        .unwrap();
    }

    /// 一个连接与它的模型 m1，m1 是对话默认。
    pub fn kit() -> Kit {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pi-bin"), b"\x7fELF").unwrap();
        let conn = rrss_core::db::open_at(&dir.path().join("rrss.db")).unwrap();
        let provider =
            ai_store::insert_provider(&conn, &provider("main", Some("sk-secret-1"))).unwrap();
        add_model(&conn, provider, "m1");
        ai_store::set_task_binding(&conn, ai_store::TASK_CHAT, Some(provider), Some("m1")).unwrap();
        Kit {
            dir,
            conn,
            provider,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::*;
    use super::*;
    use rrss_core::memories;
    use rrss_core::model_pool::{self, CapsOverride, EntryPatch, ModelRef};

    fn endpoint(p: &PiProfile, id: i64) -> &PiEndpoint {
        p.endpoints.iter().find(|e| e.provider_id == id).unwrap()
    }

    fn ids(e: &PiEndpoint) -> Vec<&str> {
        e.models.iter().map(|m| m.id.as_str()).collect()
    }

    #[test]
    fn mcp_values_only_enter_secrets_and_changes_invalidate_the_profile() {
        let k = kit();
        crate::mcp::import(&k.conn, r#"{"remote":{"url":"https://example.test/mcp","headers":{"X-Key":"private-mcp-value"}}}"#).unwrap();
        let id = pi_store::list_mcp_servers(&k.conn).unwrap()[0].id;
        pi_store::set_mcp_server_enabled(&k.conn, id, true).unwrap();
        let (before, secrets) = k.load().unwrap();
        assert!(
            secrets
                .api_keys
                .contains(&(format!("RRSS_MCP_{id}_0"), "private-mcp-value".into()))
        );
        assert!(!format!("{before:?} {secrets:?}").contains("private-mcp-value"));
        assert!(
            !serde_json::to_string(&before)
                .unwrap()
                .contains("private-mcp-value")
        );
        k.conn
            .execute(
                "UPDATE mcp_servers SET headers_json = '{\"X-Key\":\"replacement\"}' WHERE id = ?1",
                [id],
            )
            .unwrap();
        assert_ne!(before.fingerprint(), k.profile().fingerprint());
        pi_store::set_mcp_server_enabled(&k.conn, id, false).unwrap();
        let (disabled, secrets) = k.load().unwrap();
        assert!(!disabled.tools.mcp);
        assert!(
            disabled.mcp.as_ref().unwrap()["mcpServers"]
                .as_object()
                .unwrap()
                .values()
                .all(|s| s["enabled"] == false)
        );
        assert!(
            !secrets
                .api_keys
                .iter()
                .any(|(name, _)| name.starts_with("RRSS_MCP_"))
        );
    }

    #[test]
    fn the_chat_default_and_chat_models_are_listed_with_their_settings() {
        let k = kit();
        let other = ai_store::insert_provider(&k.conn, &provider("other", None)).unwrap();
        ai_store::set_provider_protocol(&k.conn, other, "responses").unwrap();
        add_model(&k.conn, other, "o-chat");
        add_model(&k.conn, other, "o-summary");
        model_pool::update_entry(
            &k.conn,
            &ModelRef::new(other, "o-chat"),
            &EntryPatch {
                chat: Some(true),
                display_name: Some("快".into()),
                thinking: Some("high".into()),
                max_output: Some(Some(4096)),
                overrides: Some(CapsOverride {
                    image_input: Some(true),
                    reasoning: Some(true),
                    ..CapsOverride::default()
                }),
                ..EntryPatch::default()
            },
        )
        .unwrap();
        ai_store::set_task_binding(&k.conn, ai_store::TASK_CHAT, Some(other), Some("o-chat"))
            .unwrap();
        let (p, secrets) = k.load().unwrap();
        assert_eq!(p.model_source, ModelSource::Bound);
        assert_eq!(p.model_ref(), Some(format!("rrss-{other}/o-chat")));
        assert_eq!(p.thinking.as_deref(), Some("high"));
        let e = endpoint(&p, other);
        assert_eq!(
            (e.base_url.as_str(), e.api.as_str()),
            ("http://other.test/v1", "openai-responses")
        );
        assert_eq!(ids(e), ["o-chat"], "models not used for chat stay out");
        let m = &e.models[0];
        assert!(
            m.reasoning && m.image && m.name.as_deref() == Some("快") && m.max_tokens == Some(4096)
        );
        // 原来的对话默认 m1 仍用于对话，对话里可以切过去。
        assert_eq!(ids(endpoint(&p, k.provider)), ["m1"]);
        // 本地端点没有密钥：用占位值，照样能启动；每个连接一个环境变量。
        assert!(secrets.api_keys.contains(&(key_env(other), NO_KEY.into())));
        assert!(
            secrets
                .api_keys
                .contains(&(key_env(k.provider), "sk-secret-1".into()))
        );
    }

    #[test]
    fn chat_models_switch_in_process_and_only_an_unlisted_choice_restarts() {
        let k = kit();
        let other = ai_store::insert_provider(&k.conn, &provider("other", None)).unwrap();
        add_model(&k.conn, other, "o-fast");
        let base = k.profile();
        assert!(
            base.endpoints.iter().all(|e| e.provider_id != other),
            "not used for chat"
        );
        model_pool::update_entry(
            &k.conn,
            &ModelRef::new(other, "o-fast"),
            &EntryPatch {
                chat: Some(true),
                thinking: Some("low".into()),
                ..EntryPatch::default()
            },
        )
        .unwrap();
        let base = k.profile();
        assert_eq!(ids(endpoint(&base, other)), ["o-fast"]);
        let load = |provider_id: i64, model: &str| {
            let bin = k.dir.path().join("pi-bin");
            let target = ModelChoice {
                provider_id,
                model: model.into(),
            };
            PiProfile::load(&k.conn, &k.paths(), PiBinary::new(bin), Some(&target))
                .unwrap()
                .0
        };
        let chosen = load(other, "o-fast");
        assert_eq!(chosen.wanted_model, Some(format!("rrss-{other}/o-fast")));
        assert_eq!(chosen.wanted_thinking.as_deref(), Some("low"));
        assert_eq!(chosen.model_ref(), base.model_ref(), "the default stays");
        assert_eq!(
            chosen.fingerprint(),
            base.fingerprint(),
            "no restart for a chat model"
        );
        let unlisted = load(other, "o-removed");
        assert_eq!(ids(endpoint(&unlisted, other)), ["o-fast", "o-removed"]);
        assert_eq!(unlisted.wanted_thinking.as_deref(), Some("off"));
        assert_ne!(unlisted.fingerprint(), base.fingerprint());
        // 加进来但不用于对话的模型变化不重启。
        add_model(&k.conn, other, "o-summary");
        assert_eq!(k.profile().fingerprint(), base.fingerprint());
        // 连接已删除或是自有配置的选择：回到对话默认。
        for p in [load(999, "x"), load(0, "anthropic/claude")] {
            assert_eq!(p.wanted_model.as_deref(), Some("rrss-1/m1"));
            assert_eq!(p.fingerprint(), base.fingerprint());
        }
        pi_store::set_use_own_config(&k.conn, true).unwrap();
        let own = load(0, "anthropic/claude");
        assert_eq!(own.mode, PiMode::Own);
        assert_eq!(own.wanted_model.as_deref(), Some("anthropic/claude"));
        assert_eq!(
            (own.thinking.as_deref(), own.wanted_thinking.as_deref()),
            (None, None),
            "Pi keeps its own thinking"
        );
        assert_eq!(own.model_ref(), None, "Pi keeps its own default");
        assert_eq!(load(other, "o-fast").wanted_model, None);
    }

    #[test]
    fn no_chat_model_or_address_is_a_config_error() {
        let k = kit();
        ai_store::set_task_binding(&k.conn, ai_store::TASK_CHAT, None, None).unwrap();
        let err = k.load().unwrap_err();
        assert_eq!(err.kind, ChatErrorKind::Config);
        assert!(err.message.contains("还没选对话模型"), "{}", err.message);

        ai_store::set_task_binding(&k.conn, ai_store::TASK_CHAT, Some(k.provider), Some("m1"))
            .unwrap();
        k.update(|p| p.base_url = "  ".into());
        let err = k.load().unwrap_err();
        assert_eq!(err.kind, ChatErrorKind::Config);
        assert!(err.message.contains("main"), "{}", err.message);

        ai_store::delete_provider(&k.conn, k.provider).unwrap();
        assert!(k.load().unwrap_err().message.contains("还没选对话模型"));
    }

    #[test]
    fn auto_protocol_maps_to_chat_completions_until_detected() {
        assert_eq!(pi_api("auto"), "openai-completions");
        assert_eq!(pi_api("chat"), "openai-completions");
        assert_eq!(pi_api("responses"), "openai-responses");
    }

    #[test]
    fn fingerprint_is_stable_and_follows_every_input_but_never_holds_the_key() {
        let k = kit();
        let base = k.profile().fingerprint();
        assert_eq!(k.profile().fingerprint(), base, "stable across loads");
        let json = serde_json::to_string(&k.profile()).unwrap();
        assert!(!json.contains("sk-secret-1"), "{json}");

        let mut seen = vec![base];
        let changed = |seen: &mut Vec<String>, what: &str| {
            let fp = k.profile().fingerprint();
            assert!(!seen.contains(&fp), "{what} did not change the fingerprint");
            seen.push(fp);
        };
        model_pool::update_entry(
            &k.conn,
            &ModelRef::new(k.provider, "m1"),
            &EntryPatch {
                context_window: Some(Some(8000)),
                ..EntryPatch::default()
            },
        )
        .unwrap();
        changed(&mut seen, "model settings");
        k.update(|p| p.api_key = Some("sk-secret-2".into()));
        changed(&mut seen, "key");
        ai_store::set_provider_compat(
            &k.conn,
            k.provider,
            &serde_json::json!({"supportsDeveloperRole": false}),
        )
        .unwrap();
        changed(&mut seen, "compat");
        model_pool::update_entry(
            &k.conn,
            &ModelRef::new(k.provider, "m1"),
            &EntryPatch {
                thinking: Some("medium".into()),
                ..EntryPatch::default()
            },
        )
        .unwrap();
        changed(&mut seen, "thinking");
        db::set_setting(&k.conn, pi_store::PI_CACHE_WARMING_KEY, "idle").unwrap();
        changed(&mut seen, "cache warming");
        pi_store::set_tool_enabled(&k.conn, "search_articles", false).unwrap();
        changed(&mut seen, "tools");
        db::set_setting(&k.conn, DEVELOPER_TOOLS_KEY, "1").unwrap();
        changed(&mut seen, "developer tools");
        k.conn.execute("INSERT INTO mcp_servers (name, transport, command, enabled, created_at) VALUES ('test', 'stdio', 'echo', 1, 0)", []).unwrap();
        changed(&mut seen, "mcp");
        pi_store::set_enabled_skill_names(&k.conn, &[]).unwrap();
        changed(&mut seen, "skills");
        pi_store::set_enabled_skill_names(&k.conn, &["cite-verify".into()]).unwrap();
        assert_eq!(
            k.profile().fingerprint(),
            seen[seen.len() - 2],
            "back to bundled"
        );
        let user = k.paths().user_skill("cite-verify");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(user.join("SKILL.md"), "---\nname: cite-verify\n---\nmine").unwrap();
        changed(&mut seen, "user skill content");
        pi_store::set_assistant_note(&k.conn, "说话简短").unwrap();
        changed(&mut seen, "assistant note");
        memories::insert_memory(&k.conn, memories::MemoryKind::Semantic, "记忆", "user", 1.0)
            .unwrap();
        assert_eq!(
            k.profile().fingerprint(),
            *seen.last().unwrap(),
            "memory never restarts pi"
        );
    }

    #[test]
    fn canonical_json_sorts_keys_at_every_level() {
        let v = serde_json::json!({"b": 1, "a": {"d": [1, {"z": 0, "y": null}], "c": "x"}});
        assert_eq!(
            canonical(&v),
            r#"{"a":{"c":"x","d":[1,{"y":null,"z":0}]},"b":1}"#
        );
    }
}
