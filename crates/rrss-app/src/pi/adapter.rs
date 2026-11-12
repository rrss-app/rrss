//! 版本适配：把 [`PiProfile`] 按探测到的版本与能力渲染成 [`LaunchSpec`]（参数 + 环境变量 + 生成文件）。
//! 版本差异只允许出现在这里；其他层只看 [`PiCaps`]。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rrss_core::pi_store::DANGEROUS_TOOL_NAMES;
use serde_json::{Value, json};

use super::profile::{PROVIDER_PREFIX, PiBinary, PiMode, PiProfile, key_env, provider_name};
use super::workspace::{self, GeneratedFile, WorkspacePaths};

/// 托管 agent 目录设置的基底：固定策略，其余键由生成器补齐。
const AGENT_SETTINGS_BASE: &str = include_str!("../../../../pi/settings.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PiVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl PiVersion {
    /// 从 `pi --version` 的输出里取第一个 `x.y.z`。
    pub fn parse(text: &str) -> Option<Self> {
        text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
            .filter_map(|word| {
                let mut parts = word.split('.').map(|p| p.parse::<u32>().ok());
                let (major, minor, patch) = (parts.next()??, parts.next()??, parts.next()??);
                Some(Self {
                    major,
                    minor,
                    patch,
                })
            })
            .next()
    }
}

impl std::fmt::Display for PiVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// 能力表：代码只判断能力，不判断版本号。握手时再用 RPC 结果补全 `thinking_levels`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiCaps {
    /// `PI_CODING_AGENT_DIR` 可用。
    pub agent_dir_env: bool,
    /// `--no-context-files` 可用。
    pub context_files_flag: bool,
    /// `--no-prompt-templates` 可用。
    pub prompt_templates_flag: bool,
    /// `--thinking <level>` 可用。模型 id 里常有冒号（如 `qwen2.5:7b`），不用 `--model id:<level>` 后缀，避免歧义。
    pub thinking_flag: bool,
    /// 握手时由 `get_available_thinking_levels` 补全；启动前为空。
    pub thinking_levels: Vec<String>,
}

/// 渲染结果。不含密钥，可以原样展示或记日志。
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    pub files: Vec<GeneratedFile>,
    /// 不再需要、启动前删除的生成文件（相对托管根目录）。
    pub stale: Vec<PathBuf>,
    /// 握手时 `get_state.model` 应当报告的 `provider/id`；空表示接受 Pi 的默认模型（自有配置未选模型）。
    pub expected_model: Option<String>,
    /// 覆盖里被忽略的 RRSS 掌控键，如 `settings.defaultProvider`、`models.providers.rrss.baseUrl`。
    pub ignored: Vec<String>,
}

pub trait PiAdapter: Send + Sync {
    fn supports(&self, v: &PiVersion) -> bool;
    fn caps(&self, v: Option<&PiVersion>) -> PiCaps;
    fn render(&self, p: &PiProfile, caps: &PiCaps, ws: &WorkspacePaths) -> LaunchSpec;
}

/// 覆盖 1.5 线（`pi/compat.json`）。以后出现破坏性变化，新增一个实现或给能力表加一项。
/// 0.87 线的适配在 2026-11 升级时移除：版本闸已拒绝低于 `min` 的版本，旧适配器不再会被选中。
pub struct Adapter15;

impl PiAdapter for Adapter15 {
    fn supports(&self, v: &PiVersion) -> bool {
        v.major == 1 && v.minor == 5
    }

    fn caps(&self, _v: Option<&PiVersion>) -> PiCaps {
        PiCaps {
            agent_dir_env: true,
            context_files_flag: true,
            prompt_templates_flag: true,
            thinking_flag: true,
            thinking_levels: Vec::new(),
        }
    }

    fn render(&self, p: &PiProfile, caps: &PiCaps, ws: &WorkspacePaths) -> LaunchSpec {
        let rel = |path: &Path| path.strip_prefix(&ws.root).unwrap_or(path).to_path_buf();
        let managed = (p.mode == PiMode::Managed && !p.endpoints.is_empty()).then_some(());
        let mut ignored = Vec::new();
        // 项目级文件两种模式都写；agent 目录只在托管模式下生成，自有配置不碰用户的 `~/.pi`。
        let mut files = vec![
            GeneratedFile::new(
                rel(&ws.project.join(".pi/settings.json")),
                pretty(&json!({"defaultTools": if p.tools.developer {
                    vec!["bash", "read", "edit", "write"]
                } else { Vec::<&str>::new() }})),
            ),
            GeneratedFile::new(rel(&ws.system_md()), p.system_prompt.as_str()),
            GeneratedFile::new(rel(&ws.tool_manifest()), pretty(&p.manifest)),
            GeneratedFile::new(rel(&ws.extension()), workspace::EXTENSION),
            GeneratedFile::new("runtime/mcp-guard.ts", workspace::MCP_GUARD),
            GeneratedFile::new("runtime/mcp-policy.json", crate::mcp::POLICY),
        ];
        if managed.is_some() {
            files.extend([
                workspace::marker_file(),
                GeneratedFile::new(
                    rel(&ws.agent_settings()),
                    pretty(&agent_settings(p, &mut ignored)),
                ),
                GeneratedFile::new(
                    rel(&ws.agent.join("models.json")),
                    pretty(&models_json(p, &mut ignored)),
                ),
            ]);
        }
        for (name, body) in workspace::BUNDLED_SKILLS {
            files.push(GeneratedFile::new(
                rel(&ws.bundled_skill(name).join("SKILL.md")),
                *body,
            ));
        }
        let mut stale = Vec::new();
        match &p.mcp {
            Some(mcp) => files.push(GeneratedFile::new(
                rel(&ws.mcp_config(managed.is_some())),
                pretty(mcp),
            )),
            None => stale.push(rel(&ws.mcp_config(managed.is_some()))),
        }
        stale.extend(
            [
                "agent/mcp-adapter.json",
                "project/.pi/mcp-adapter.json",
                "agent/.rrss-mcp-packages.json",
            ]
            .map(PathBuf::from),
        );
        if managed.is_some() {
            stale.push(PathBuf::from("project/.pi/mcp.json"));
        }

        let mut args: Vec<String> = vec![
            "--mode".into(),
            "rpc".into(),
            "--approve".into(),
            "--session-dir".into(),
            ws.sessions.display().to_string(),
        ];
        if caps.context_files_flag {
            args.push("--no-context-files".into());
        }
        if caps.prompt_templates_flag {
            args.push("--no-prompt-templates".into());
        }
        args.push("--no-skills".into());
        for skill in &p.skills {
            args.push("--skill".into());
            args.push(skill.path.display().to_string());
        }
        args.push("-e".into());
        args.push(ws.extension().display().to_string());
        // 自有配置始终用排除式，以便使用用户扩展注册的工具；危险内置规则不变。
        let exclusion = managed.is_none();
        let mut enabled = p.tools.enabled.clone();
        if managed.is_some()
            && let Some(config) = &p.mcp
        {
            for (name, server) in config["mcpServers"].as_object().into_iter().flatten() {
                if server["enabled"] == true {
                    enabled.push(format!("mcp__{name}__*"));
                }
            }
        }
        args.extend(tool_args(&enabled, p.tools.developer, exclusion));
        if let Some(model) = p.model_ref() {
            args.push("--model".into());
            args.push(model);
        }
        // 思考强度按模型设置（阶段 19-7）；自有配置交给用户的 Pi。
        if caps.thinking_flag
            && let Some(thinking) = &p.thinking
        {
            args.push("--thinking".into());
            args.push(thinking.clone());
        }

        let mut env = vec![
            ("PI_SKIP_VERSION_CHECK".to_string(), "1".to_string()),
            (
                "RRSS_TOOL_MANIFEST".into(),
                ws.tool_manifest().display().to_string(),
            ),
        ];
        // 自有配置不设 `PI_OFFLINE`，以免阻断用户自己的包更新。
        if managed.is_some() {
            env.push(("PI_OFFLINE".into(), "1".into()));
            if caps.agent_dir_env {
                env.push(("PI_CODING_AGENT_DIR".into(), ws.agent.display().to_string()));
            }
        }

        LaunchSpec {
            program: p.binary.path.clone(),
            args,
            env,
            cwd: ws.project.clone(),
            files,
            stale,
            expected_model: p.model_ref(),
            ignored,
        }
    }
}

static ADAPTER_15: Adapter15 = Adapter15;

/// 选出适配器。版本无法解析或不在任何实现范围内时用最新的实现（P3 的版本闸再决定是否拒绝）。
pub fn adapter_for(v: Option<&PiVersion>) -> &'static dyn PiAdapter {
    match v {
        Some(v) if ADAPTER_15.supports(v) => &ADAPTER_15,
        _ => &ADAPTER_15,
    }
}

type VersionCache = HashMap<(PathBuf, String), Option<PiVersion>>;

/// `pi --version`，按二进制路径与大小 / 修改时间缓存。
pub fn probe_version(binary: &PiBinary) -> Option<PiVersion> {
    static CACHE: Mutex<Option<VersionCache>> = Mutex::new(None);
    let key = (binary.path.clone(), binary.stamp.clone());
    if let Ok(cache) = CACHE.lock()
        && let Some(hit) = cache.as_ref().and_then(|c| c.get(&key))
    {
        return *hit;
    }
    let version = rrss_core::platform::hide_console_window(
        std::process::Command::new(&binary.path).arg("--version"),
    )
    .output()
    .ok()
    .and_then(|o| {
        PiVersion::parse(&format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ))
    });
    if let Ok(mut cache) = CACHE.lock() {
        cache.get_or_insert_with(HashMap::new).insert(key, version);
    }
    version
}

/// `agent/settings.json` 里 RRSS 掌控、覆盖不能改的键。`cacheWarming` 由「高级」里的单独控件管（D6，默认关）。
pub const CONTROLLED_SETTINGS: &[&str] = &[
    "defaultProvider",
    "defaultModel",
    "enabledModels",
    "defaultProjectTrust",
    "sessionDir",
    "packages",
    "extensions",
    "skills",
    "cacheWarming",
    "enableInstallTelemetry",
];
/// `models.json` 中 RRSS 的 provider（`rrss-<连接 id>`）里 RRSS 掌控的键。
pub const CONTROLLED_RRSS_PROVIDER: &[&str] = &["baseUrl", "api", "apiKey", "models"];

/// 托管模式保留内置 MCP，关闭不属于阅读助手的其他内置扩展。
pub const BUILTIN_EXTENSIONS_OFF: &[&str] = &[
    "-builtin:codemode",
    "-builtin:tool-search",
    "-builtin:llama.cpp",
];

/// 固定策略 → 默认值 → 设置覆盖（浅合并，跳过掌控键）→ RRSS 掌控的键；不载入第三方包。
fn agent_settings(p: &PiProfile, ignored: &mut Vec<String>) -> Value {
    let mut settings: Value =
        serde_json::from_str(AGENT_SETTINGS_BASE).unwrap_or_else(|_| json!({}));
    if !settings.is_object() {
        settings = json!({});
    }
    let obj = settings.as_object_mut().expect("object");
    if let Some(thinking) = &p.thinking {
        obj.insert("defaultThinkingLevel".into(), json!(thinking));
    }
    if let Some(over) = p.settings_override.as_object() {
        for (key, value) in over {
            if CONTROLLED_SETTINGS.contains(&key.as_str()) {
                ignored.push(format!("settings.{key}"));
            } else {
                obj.insert(key.clone(), value.clone());
            }
        }
    }
    let (provider, model) = p
        .default_model
        .as_deref()
        .and_then(|m| m.split_once('/'))
        .unwrap_or_default();
    obj.insert("defaultProvider".into(), json!(provider));
    obj.insert("defaultModel".into(), json!(model));
    obj.insert("packages".into(), json!([]));
    obj.insert("extensions".into(), json!(BUILTIN_EXTENSIONS_OFF));
    obj.insert("skills".into(), json!([]));
    obj.insert("enableInstallTelemetry".into(), json!(false));
    // 0.87 起 Pi 默认在流式中预热提示缓存，会额外计费；RRSS 默认关，用户可在「高级」里改（D6）。
    obj.insert("cacheWarming".into(), json!(p.cache_warming));
    settings
}

/// 一个模型的定义；字段按 Pi 1.5.0 的 `docs/models-configuration.md` 核对。
fn model_json(m: &super::profile::PiModel) -> Value {
    let mut v = json!({ "id": m.id, "reasoning": m.reasoning });
    if let Some(name) = &m.name {
        v["name"] = json!(name);
    }
    if m.image {
        v["input"] = json!(["text", "image"]);
    }
    if let Some(n) = m.context_window {
        v["contextWindow"] = json!(n);
    }
    if let Some(n) = m.max_tokens {
        v["maxTokens"] = json!(n);
    }
    if let Some(compat) = m
        .compat
        .as_ref()
        .filter(|c| c.as_object().is_some_and(|o| !o.is_empty()))
    {
        v["compat"] = compat.clone();
    }
    v
}

/// 每个连接一个 `rrss-<id>` provider，再加模型定义覆盖。覆盖可以新增 provider，也可以给 RRSS 的 provider
/// 加 `modelOverrides`、`compat` 等，但地址、协议、密钥和模型列表由 RRSS 决定；以 `rrss` 开头却不是现有连接的
/// provider 整个忽略，免得冒充。密钥写成环境变量引用，不落盘。
fn models_json(p: &PiProfile, ignored: &mut Vec<String>) -> Value {
    let mut ours = serde_json::Map::new();
    for endpoint in &p.endpoints {
        let models: Vec<Value> = endpoint.models.iter().map(model_json).collect();
        let mut provider = json!({
            "baseUrl": endpoint.base_url,
            "api": endpoint.api,
            "apiKey": format!("${}", key_env(endpoint.provider_id)),
            "models": models,
        });
        if endpoint.compat.as_object().is_some_and(|c| !c.is_empty()) {
            provider["compat"] = endpoint.compat.clone();
        }
        ours.insert(provider_name(endpoint.provider_id), provider);
    }
    let mut out = json!({ "providers": {} });
    if let Some(over) = p.models_override.as_object() {
        for (key, value) in over {
            if key != "providers" {
                out[key] = value.clone();
            }
        }
        if let Some(providers) = over.get("providers").and_then(Value::as_object) {
            for (name, provider) in providers {
                let reserved = name == "rrss" || name.starts_with(PROVIDER_PREFIX);
                match ours.get_mut(name) {
                    Some(own) => {
                        for (key, value) in provider.as_object().into_iter().flatten() {
                            if CONTROLLED_RRSS_PROVIDER.contains(&key.as_str()) {
                                ignored.push(format!("models.providers.{name}.{key}"));
                            } else {
                                own[key] = value.clone();
                            }
                        }
                    }
                    None if reserved => ignored.push(format!("models.providers.{name}")),
                    None => out["providers"][name] = provider.clone(),
                }
            }
        }
    }
    for (name, provider) in ours {
        out["providers"][name] = provider;
    }
    out
}

/// 工具参数。MCP 关：严格白名单；MCP 开：排除式，只挡危险内置。开发者工具打开时放开 `bash/read/edit/write`。
pub fn tool_args(enabled: &[String], developer: bool, mcp: bool) -> Vec<String> {
    let mut args = Vec::new();
    if mcp {
        if developer {
            // 其余内置工具仍禁用；扩展工具继续按排除式开放。
            // 尤其 Windows 的 powershell 不能随开发者开关一起意外放开。
            args.push("--exclude-tools".into());
            args.push("powershell,grep,find,ls".into());
        } else {
            args.push("--no-builtin-tools".into());
            args.push("--exclude-tools".into());
            args.push(DANGEROUS_TOOL_NAMES.join(","));
        }
        return args;
    }
    if !developer {
        args.push("--no-builtin-tools".into());
    }
    let mut tools = enabled.to_vec();
    tools.push("rrss_mcp_guard".into());
    if developer {
        for name in ["bash", "read", "edit", "write"] {
            if !tools.iter().any(|t| t == name) {
                tools.push(name.into());
            }
        }
    }
    if !tools.is_empty() {
        args.push("--tools".into());
        args.push(tools.join(","));
    }
    args
}

fn pretty(v: &Value) -> Vec<u8> {
    let mut out = serde_json::to_vec_pretty(v).unwrap_or_default();
    out.push(b'\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pi::profile::testkit::kit;
    use rrss_core::{ai_store, pi_store};

    #[test]
    fn exclusion_mode_developer_only_opens_the_four_allowed_builtins() {
        let args = tool_args(&["search_articles".into()], true, true);
        assert_eq!(args, vec!["--exclude-tools", "powershell,grep,find,ls"]);
        let k = kit();
        let mut p = k.profile();
        p.tools.developer = true;
        for own in [false, true] {
            p.mode = if own { PiMode::Own } else { PiMode::Managed };
            p.tools.mcp = !own;
            let spec = Adapter15.render(&p, &Adapter15.caps(None), &k.paths());
            if own {
                assert!(
                    spec.args
                        .windows(2)
                        .any(|a| a == ["--exclude-tools", "powershell,grep,find,ls"])
                );
                assert!(!spec.args.iter().any(|a| a == "--tools"));
            } else {
                assert!(spec.args.iter().any(|a| a.contains("bash,read,edit,write")));
            }
            let settings = spec
                .files
                .iter()
                .find(|f| f.path == Path::new("project/.pi/settings.json"))
                .unwrap();
            let settings: Value = serde_json::from_slice(&settings.contents).unwrap();
            assert_eq!(
                settings["defaultTools"],
                json!(["bash", "read", "edit", "write"])
            );
        }
    }

    fn render(k: &crate::pi::profile::testkit::Kit) -> LaunchSpec {
        let adapter = adapter_for(None);
        let caps = adapter.caps(None);
        adapter.render(&k.profile(), &caps, &k.paths())
    }

    fn file(spec: &LaunchSpec, rel: &str) -> Value {
        let f = spec
            .files
            .iter()
            .find(|f| f.path == Path::new(rel))
            .unwrap_or_else(|| panic!("{rel} not generated"));
        serde_json::from_slice(&f.contents).unwrap()
    }

    fn after<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        let i = args.iter().position(|a| a == flag)?;
        args.get(i + 1).map(String::as_str)
    }

    #[test]
    fn versions_parse_from_cli_output() {
        let v = PiVersion::parse("pi 0.84.4\n").unwrap();
        assert_eq!(v.to_string(), "0.84.4");
        assert_eq!(PiVersion::parse("v0.87.1-beta").unwrap().minor, 87);
        assert!(PiVersion::parse("unknown").is_none());
        assert!(!Adapter15.supports(&v));
        assert!(Adapter15.supports(&PiVersion::parse("pi 1.5.0").unwrap()));
        assert!(!Adapter15.supports(&PiVersion::parse("1.4.0").unwrap()));
    }

    #[test]
    fn launch_is_isolated_and_selects_the_rrss_model() {
        let k = kit();
        let spec = render(&k);
        let ws = k.paths();
        let args = &spec.args;
        for flag in [
            "--no-context-files",
            "--no-prompt-templates",
            "--no-skills",
            "--approve",
        ] {
            assert!(args.iter().any(|a| a == flag), "{flag}: {args:?}");
        }
        assert!(!args.iter().any(|a| a == "--provider"));
        assert_eq!(after(args, "--mode"), Some("rpc"));
        assert_eq!(after(args, "--model"), Some("rrss-1/m1"));
        assert_eq!(after(args, "--thinking"), Some("off"));
        assert_eq!(after(args, "-e"), Some(ws.extension().to_str().unwrap()));
        assert_eq!(
            after(args, "--session-dir"),
            Some(ws.sessions.to_str().unwrap())
        );
        assert_eq!(
            after(args, "--skill"),
            Some(ws.bundled_skill("cite-verify").to_str().unwrap())
        );
        assert_eq!(spec.cwd, ws.project);
        let system = spec
            .files
            .iter()
            .find(|f| f.path == Path::new("project/.pi/SYSTEM.md"))
            .expect("SYSTEM.md generated");
        let system = String::from_utf8_lossy(&system.contents);
        assert!(system.starts_with("<!-- rrss-system v") && !system.contains("用户补充说明"));
        pi_store::set_assistant_note(&k.conn, "说话简短").unwrap();
        let noted = render(&k);
        let noted = noted
            .files
            .iter()
            .find(|f| f.path == Path::new("project/.pi/SYSTEM.md"))
            .unwrap();
        assert!(String::from_utf8_lossy(&noted.contents).ends_with("## 用户补充说明\n\n以下是用户写给你的说明，与上面的规则冲突时以上面的规则为准。\n\n说话简短\n"));
        assert_eq!(spec.expected_model.as_deref(), Some("rrss-1/m1"));
        let env: HashMap<_, _> = spec.env.iter().cloned().collect();
        assert_eq!(env["PI_CODING_AGENT_DIR"], ws.agent.display().to_string());
        assert_eq!(env["PI_OFFLINE"], "1");
        assert!(!env.contains_key("PI_MCP_CONFIG_MODE"));
        assert!(
            !env.contains_key(&key_env(1)),
            "the key is added only at spawn"
        );
        assert!(!format!("{spec:?}").contains("sk-secret-1"));
    }

    #[test]
    fn models_json_holds_a_provider_per_connection_with_key_references() {
        use crate::pi::profile::testkit::{add_model, provider};
        use rrss_core::model_pool::{CapsOverride, EntryPatch, ModelRef, update_entry};
        let k = kit();
        let other = ai_store::insert_provider(&k.conn, &provider("other", None)).unwrap();
        add_model(&k.conn, other, "o-fast");
        add_model(&k.conn, k.provider, "m-big");
        for (id, model) in [(other, "o-fast"), (k.provider, "m-big")] {
            update_entry(
                &k.conn,
                &ModelRef::new(id, model),
                &EntryPatch {
                    chat: Some(true),
                    ..EntryPatch::default()
                },
            )
            .unwrap();
        }
        let models = file(&render(&k), "agent/models.json");
        let rrss = &models["providers"]["rrss-1"];
        assert_eq!(models["providers"].as_object().unwrap().len(), 2);
        assert_eq!(rrss["baseUrl"], "http://main.test/v1");
        assert_eq!(rrss["api"], "openai-completions");
        assert_eq!(rrss["apiKey"], "$RRSS_PI_KEY_1");
        assert_eq!(
            rrss["models"],
            json!([{"id": "m-big", "reasoning": true}, {"id": "m1", "reasoning": true}])
        );
        assert_eq!(
            models["providers"][format!("rrss-{other}")]["models"][0]["id"],
            "o-fast"
        );
        assert_eq!(
            models["providers"][format!("rrss-{other}")]["apiKey"],
            format!("$RRSS_PI_KEY_{other}")
        );
        assert!(rrss.get("compat").is_none(), "empty compat is omitted");

        ai_store::set_provider_compat(&k.conn, k.provider, &json!({"supportsStore": false}))
            .unwrap();
        update_entry(
            &k.conn,
            &ModelRef::new(k.provider, "m1"),
            &EntryPatch {
                display_name: Some("主力".into()),
                thinking: Some("high".into()),
                context_window: Some(Some(128_000)),
                max_output: Some(Some(8192)),
                compat: Some(Some(json!({"maxTokensField": "max_tokens"}))),
                overrides: Some(CapsOverride {
                    image_input: Some(true),
                    ..CapsOverride::default()
                }),
                ..EntryPatch::default()
            },
        )
        .unwrap();
        let spec = render(&k);
        let rrss = file(&spec, "agent/models.json")["providers"]["rrss-1"].clone();
        assert_eq!(rrss["compat"], json!({"supportsStore": false}));
        assert_eq!(
            rrss["models"][1],
            json!({
                "id": "m1", "name": "主力", "reasoning": true, "input": ["text", "image"],
                "contextWindow": 128000, "maxTokens": 8192, "compat": {"maxTokensField": "max_tokens"}
            })
        );
        assert_eq!(after(&spec.args, "--thinking"), Some("high"));
        assert_eq!(
            file(&spec, "agent/settings.json")["defaultThinkingLevel"],
            "high"
        );
    }

    #[test]
    fn agent_settings_remove_adapter_and_native_mcp_is_enabled() {
        let k = kit();
        let ws = k.paths();
        std::fs::create_dir_all(&ws.agent).unwrap();
        std::fs::write(
            ws.agent_settings(),
            r#"{"packages": ["npm:pi-mcp-adapter", "npm:other-ext"]}"#,
        )
        .unwrap();
        let spec = render(&k);
        let settings = file(&spec, "agent/settings.json");
        assert_eq!(settings["defaultProvider"], "rrss-1");
        assert_eq!(settings["defaultModel"], "m1");
        assert_eq!(settings["enableInstallTelemetry"], false);
        assert_eq!(settings["cacheWarming"], "off", "默认关闭（D6）");
        rrss_core::db::set_setting(&k.conn, pi_store::PI_CACHE_WARMING_KEY, "idle").unwrap();
        assert_eq!(
            file(&render(&k), "agent/settings.json")["cacheWarming"],
            "idle"
        );
        assert_eq!(settings["packages"], json!([]), "MCP off: no packages");
        assert_eq!(
            settings["extensions"],
            json!([
                "-builtin:codemode",
                "-builtin:tool-search",
                "-builtin:llama.cpp"
            ]),
            "built-in extensions stay off"
        );
        assert!(
            spec.stale
                .contains(&PathBuf::from("agent/mcp-adapter.json"))
        );
        assert!(spec.stale.contains(&PathBuf::from("project/.pi/mcp.json")));

        // 老 adapter 声明在重写后消失；启用的原生 MCP 工具进入白名单。
        workspace::write_files(&ws, &spec.files).unwrap();
        k.conn.execute("INSERT INTO mcp_servers (name, transport, command, enabled, created_at) VALUES ('test', 'stdio', 'echo', 1, 0)", []).unwrap();
        let spec = render(&k);
        assert_eq!(file(&spec, "agent/settings.json")["packages"], json!([]));
        assert!(
            spec.files
                .iter()
                .any(|f| f.path == Path::new("agent/mcp.json"))
        );
        assert!(spec.args.iter().any(|a| a == "--tools"));
        assert!(spec.args.iter().any(|a| a.contains("mcp__s1__*")));
    }

    #[test]
    fn overrides_merge_but_rrss_controlled_keys_are_ignored_and_listed() {
        let k = kit();
        pi_store::set_pi_override(
            &k.conn,
            pi_store::PI_SETTINGS_OVERRIDE_KEY,
            r#"{"quietStartup": false, "retry": {"maxRetries": 1}, "defaultProvider": "evil", "packages": ["npm:x"]}"#,
        )
        .unwrap();
        pi_store::set_pi_override(
            &k.conn,
            pi_store::PI_MODELS_OVERRIDE_KEY,
            r#"{"providers": {
                "ollama": {"baseUrl": "http://localhost:11434/v1", "api": "openai-completions", "models": [{"id": "q"}]},
                "rrss-1": {"baseUrl": "http://evil/v1", "apiKey": "sk-leak", "modelOverrides": {"m1": {"contextWindow": 32000}}},
                "rrss-9": {"baseUrl": "http://evil/v1", "models": [{"id": "x"}]},
                "rrss": {"baseUrl": "http://evil/v1"}
            }}"#,
        )
        .unwrap();
        let spec = render(&k);
        let settings = file(&spec, "agent/settings.json");
        assert_eq!(settings["quietStartup"], false);
        assert_eq!(settings["retry"], json!({"maxRetries": 1}));
        assert_eq!(settings["defaultProvider"], "rrss-1");
        assert_eq!(settings["packages"], json!([]));
        let models = file(&spec, "agent/models.json");
        assert_eq!(models["providers"]["ollama"]["models"][0]["id"], "q");
        let rrss = &models["providers"]["rrss-1"];
        assert_eq!(rrss["baseUrl"], "http://main.test/v1");
        assert_eq!(rrss["apiKey"], "$RRSS_PI_KEY_1");
        assert_eq!(rrss["modelOverrides"]["m1"]["contextWindow"], 32000);
        assert!(
            models["providers"].get("rrss-9").is_none()
                && models["providers"].get("rrss").is_none()
        );
        let mut ignored = spec.ignored.clone();
        ignored.sort();
        assert_eq!(
            ignored,
            vec![
                "models.providers.rrss",
                "models.providers.rrss-1.apiKey",
                "models.providers.rrss-1.baseUrl",
                "models.providers.rrss-9",
                "settings.defaultProvider",
                "settings.packages",
            ]
        );
    }

    #[test]
    fn own_config_uses_the_users_pi_setup() {
        let k = kit();
        pi_store::set_use_own_config(&k.conn, true).unwrap();
        pi_store::set_pi_override(&k.conn, pi_store::PI_SETTINGS_OVERRIDE_KEY, r#"{"a": 1}"#)
            .unwrap();
        let spec = render(&k);
        let env: HashMap<_, _> = spec.env.iter().cloned().collect();
        assert!(!env.contains_key("PI_CODING_AGENT_DIR"));
        assert!(!env.contains_key("PI_OFFLINE"));
        assert!(!env.contains_key("PI_MCP_CONFIG_MODE"));
        assert!(
            !spec.files.iter().any(|f| f.path.starts_with("agent")),
            "the user's agent dir is never written"
        );
        assert!(
            !spec.args.iter().any(|a| a == "--model"),
            "Pi's default model"
        );
        assert_eq!(spec.expected_model, None);
        // 排除式工具参数，保留用户扩展的工具。
        assert!(spec.args.iter().any(|a| a == "--exclude-tools"));
        for flag in [
            "--no-context-files",
            "--no-prompt-templates",
            "--no-skills",
            "-e",
        ] {
            assert!(spec.args.iter().any(|a| a == flag), "{flag}");
        }

        pi_store::set_own_model(&k.conn, "anthropic/claude-x").unwrap();
        let spec = render(&k);
        assert_eq!(after(&spec.args, "--model"), Some("anthropic/claude-x"));
        assert_eq!(spec.expected_model.as_deref(), Some("anthropic/claude-x"));
        // 自有配置不需要 RRSS 的连接。
        ai_store::delete_provider(&k.conn, k.provider).unwrap();
        assert!(k.load().is_ok());
    }

    #[test]
    fn default_tools_are_a_strict_whitelist_without_bash() {
        let args = tool_args(
            &["search_articles".into(), "get_article".into()],
            false,
            false,
        );
        assert_eq!(
            args,
            vec![
                "--no-builtin-tools",
                "--tools",
                "search_articles,get_article,rrss_mcp_guard"
            ]
        );
    }

    #[test]
    fn developer_tools_allow_bash() {
        let args = tool_args(&["search_articles".into()], true, false);
        assert!(!args.iter().any(|a| a == "--no-builtin-tools"));
        assert_eq!(
            args.last().unwrap(),
            "search_articles,rrss_mcp_guard,bash,read,edit,write"
        );
    }

    #[test]
    fn own_config_preserves_user_extension_tools() {
        let args = tool_args(&["search_articles".into()], false, true);
        assert_eq!(
            args,
            vec![
                "--no-builtin-tools",
                "--exclude-tools",
                "bash,edit,write,read"
            ]
        );
        assert_eq!(
            tool_args(&[], true, true),
            vec!["--exclude-tools", "powershell,grep,find,ls"]
        );
    }
}
