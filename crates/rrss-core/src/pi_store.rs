//! Pi 工具清单、MCP 配置与对话用量。日志 target：`rrss_core::pi_store`。

use crate::error::settings::PiOverrideProblem;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::db::{self, PI_IDLE_TIMEOUT_SECS_KEY};
use crate::error::{Error, Result};
use crate::feeds::now_ms;

pub const DEFAULT_IDLE_TIMEOUT_SECS: i64 = 1800;
pub const DANGEROUS_TOOL_NAMES: &[&str] = &["bash", "edit", "write", "read"];
pub const PI_SKILLS_ENABLED_KEY: &str = "pi_skills_enabled";
pub const PI_AUTO_DOWNLOAD_KEY: &str = "pi_auto_download";
pub const PI_AUTO_UPDATE_KEY: &str = "pi_auto_update";
pub const PI_MANAGED_VERSION_KEY: &str = "pi_managed_version";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolParam {
    pub name: String,
    pub kind: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

#[derive(Debug, Clone)]
pub struct PiToolDef {
    pub name: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub confirm_required: bool,
    pub parameters: &'static [(&'static str, &'static str, &'static str)],
}

pub const DEFAULT_TOOLS: &[PiToolDef] = &[
    PiToolDef {
        name: "search_articles",
        label: "搜索文章",
        description: "检索本地文章库（FTS）。返回 id、标题、来源、时间、分数、摘录。",
        confirm_required: false,
        parameters: &[("query", "string", "检索词")],
    },
    PiToolDef {
        name: "get_article",
        label: "读文章",
        description: "按 id 取一篇本地文章。元数据加摘要或剥标签正文前缀，不含 HTML。",
        confirm_required: false,
        parameters: &[("id", "number", "article id")],
    },
    PiToolDef {
        name: "search_memory",
        label: "搜索记忆",
        description: "按关键词或向量检索已确认与待确认的本地记忆。",
        confirm_required: false,
        parameters: &[("query", "string", "检索词")],
    },
    PiToolDef {
        name: "get_assistant_config",
        label: "读助手配置",
        description: "只读查看 RRSS 给助手的配置：助手说明、已启用的工具与技能、能否抓取网址、外部工具（MCP）允许的工具名，以及当前对话的模型别名与思考档位（拿得到时）。不含密钥、服务地址、启动命令、系统提示模板、记忆和文章，不修改任何配置。",
        confirm_required: false,
        parameters: &[],
    },
    PiToolDef {
        name: "open_original",
        label: "打开原文",
        description: "用系统浏览器打开文章 URL。",
        confirm_required: false,
        parameters: &[("url", "string", "https URL")],
    },
    PiToolDef {
        name: "fetch_url",
        label: "抓取网址",
        description: "抓取远程 URL 并剥成纯文本。每次都必须先确认。",
        confirm_required: true,
        parameters: &[("url", "string", "https URL")],
    },
    PiToolDef {
        name: "list_focus",
        label: "今日焦点",
        description: "列出今日焦点条目（标题、分数、摘要一句）。",
        confirm_required: false,
        parameters: &[("limit", "number", "条数，默认 20")],
    },
    PiToolDef {
        name: "explain_score",
        label: "解释分数",
        description: "读 reasons_json 与 interest_reasons_json，解释一篇为什么被隐藏或抬高。",
        confirm_required: false,
        parameters: &[("article_id", "number", "article id")],
    },
    PiToolDef {
        name: "list_feeds",
        label: "订阅源",
        description: "列出订阅源及噪声标记。",
        confirm_required: false,
        parameters: &[],
    },
    PiToolDef {
        name: "get_digest",
        label: "读 Digest",
        description: "按 id 精确读某一期简报，或按 period 取最新一期；含条目序号与 article_id。",
        confirm_required: false,
        parameters: &[
            ("id", "number", "简报 id；精确取历史某一期"),
            (
                "period",
                "string",
                "daily 或 weekly；未传 id 时取该类型最新，默认 daily",
            ),
        ],
    },
    PiToolDef {
        name: "add_interest",
        label: "添加兴趣",
        description: "新增一条显式兴趣（只影响可见列表排序，不改隐藏）。需确认。",
        confirm_required: true,
        parameters: &[("label", "string", "兴趣标签")],
    },
    PiToolDef {
        name: "mark_read",
        label: "标已读",
        description: "把一篇标为已读或未读。需确认。",
        confirm_required: true,
        parameters: &[
            ("article_id", "number", "article id"),
            ("read", "number", "1 已读 / 0 未读"),
        ],
    },
    PiToolDef {
        name: "star_article",
        label: "收藏",
        description: "收藏或取消收藏一篇。需确认。",
        confirm_required: true,
        parameters: &[
            ("article_id", "number", "article id"),
            ("starred", "number", "1 收藏 / 0 取消"),
        ],
    },
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiTool {
    pub name: String,
    pub enabled: bool,
    pub label: String,
    pub description: String,
    pub confirm_required: bool,
    pub builtin: bool,
    pub default_description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolManifestEntry {
    pub name: String,
    pub label: String,
    pub description: String,
    pub confirm: bool,
    pub parameters: Vec<ToolParam>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolManifest {
    pub tools: Vec<ToolManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    pub id: i64,
    pub name: String,
    pub transport: String,
    pub command: Option<String>,
    pub args_json: Option<String>,
    pub url: Option<String>,
    pub env_json: Option<String>,
    pub headers_json: Option<String>,
    pub auth: String,
    pub include_tools_json: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PiUsageDay {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost: f64,
}

pub fn seed_default_tools(conn: &Connection) -> Result<()> {
    for tool in DEFAULT_TOOLS {
        conn.execute(
            "INSERT INTO pi_tools (name, enabled, label, description, confirm_required, builtin, default_description)
             VALUES (?1, 1, ?2, ?3, ?4, 1, ?3)
             ON CONFLICT (name) DO UPDATE SET
               label = excluded.label,
               builtin = 1,
               default_description = excluded.default_description,
               confirm_required = excluded.confirm_required,
               description = CASE
                 WHEN pi_tools.description = pi_tools.default_description OR pi_tools.description = ''
                 THEN excluded.description
                 ELSE pi_tools.description
               END",
            params![
                tool.name,
                tool.label,
                tool.description,
                i64::from(tool.confirm_required)
            ],
        )?;
    }
    Ok(())
}

pub fn list_tools(conn: &Connection) -> Result<Vec<PiTool>> {
    seed_default_tools(conn)?;
    read_tools(conn)
}

fn read_tools(conn: &Connection) -> Result<Vec<PiTool>> {
    let mut stmt = conn.prepare(
        "SELECT name, enabled, label, description, confirm_required, builtin, default_description
         FROM pi_tools ORDER BY name",
    )?;
    let rows = stmt.query_map([], map_tool)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn enabled_tool_names(conn: &Connection) -> Result<Vec<String>> {
    Ok(list_tools(conn)?
        .into_iter()
        .filter(|t| t.enabled)
        .map(|t| t.name)
        .collect())
}

pub fn set_tool_enabled(conn: &Connection, name: &str, enabled: bool) -> Result<()> {
    let n = conn.execute(
        "UPDATE pi_tools SET enabled = ?1 WHERE name = ?2",
        params![i64::from(enabled), name],
    )?;
    if n == 0 {
        Err(Error::PiToolNotFound(name.into()))
    } else {
        Ok(())
    }
}

pub fn set_tool_description(conn: &Connection, name: &str, description: &str) -> Result<()> {
    let n = conn.execute(
        "UPDATE pi_tools SET description = ?1 WHERE name = ?2",
        params![description, name],
    )?;
    if n == 0 {
        Err(Error::PiToolNotFound(name.into()))
    } else {
        Ok(())
    }
}

pub fn restore_tool_description(conn: &Connection, name: &str) -> Result<()> {
    let n = conn.execute(
        "UPDATE pi_tools SET description = default_description WHERE name = ?1",
        [name],
    )?;
    if n == 0 {
        Err(Error::PiToolNotFound(name.into()))
    } else {
        Ok(())
    }
}

fn tool_param_optional(tool: &str, name: &str) -> bool {
    matches!(tool, "get_digest") && matches!(name, "id" | "period")
}

fn tool_params_from_def(
    tool: &str,
    params: &'static [(&'static str, &'static str, &'static str)],
) -> Vec<ToolParam> {
    params
        .iter()
        .map(|(n, k, desc)| ToolParam {
            name: (*n).into(),
            kind: (*k).into(),
            description: (*desc).into(),
            optional: tool_param_optional(tool, n),
        })
        .collect()
}

/// Read the manifest without writes; profile loading refreshes defaults via `list_tools`.
pub fn tool_manifest(conn: &Connection) -> Result<ToolManifest> {
    let enabled = read_tools(conn)?
        .into_iter()
        .filter(|t| t.enabled)
        .collect::<Vec<_>>();
    let mut tools = Vec::new();
    for row in enabled {
        let def = DEFAULT_TOOLS.iter().find(|d| d.name == row.name);
        let parameters = def
            .map(|d| tool_params_from_def(d.name, d.parameters))
            .unwrap_or_default();
        tools.push(ToolManifestEntry {
            name: row.name,
            label: row.label,
            description: row.description,
            confirm: row.confirm_required,
            parameters,
        });
    }
    Ok(ToolManifest { tools })
}

pub fn mcp_enabled(conn: &Connection) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM mcp_servers WHERE enabled = 1 AND transport != 'sse')",
        [],
        |r| r.get(0),
    )?)
}

pub fn idle_timeout_secs(conn: &Connection) -> Result<i64> {
    Ok(db::get_setting(conn, PI_IDLE_TIMEOUT_SECS_KEY)?
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_IDLE_TIMEOUT_SECS)
        .max(0))
}

pub fn set_idle_timeout_secs(conn: &Connection, secs: i64) -> Result<()> {
    db::set_setting(conn, PI_IDLE_TIMEOUT_SECS_KEY, &secs.max(0).to_string())
}

/// Pi 的 `cacheWarming`：默认关闭（D6）；`streaming` / `idle` 可能在没有对话时也产生费用。
pub const PI_CACHE_WARMING_KEY: &str = "pi_cache_warming";
pub const CACHE_WARMING_MODES: &[&str] = &["off", "streaming", "idle"];

/// 库里读不懂的值按关闭处理，不把未知值交给 Pi。
pub fn cache_warming(conn: &Connection) -> Result<String> {
    let value = db::get_setting(conn, PI_CACHE_WARMING_KEY)?.unwrap_or_default();
    let value = value.trim();
    Ok(if CACHE_WARMING_MODES.contains(&value) {
        value
    } else {
        "off"
    }
    .into())
}

pub fn list_mcp_servers(conn: &Connection) -> Result<Vec<McpServer>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, transport, command, args_json, url, env_json, headers_json,
                auth, include_tools_json, enabled
         FROM mcp_servers ORDER BY name",
    )?;
    let rows = stmt.query_map([], map_mcp)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

pub fn get_mcp_server(conn: &Connection, id: i64) -> Result<McpServer> {
    conn.query_row(
        "SELECT id, name, transport, command, args_json, url, env_json, headers_json,
                auth, include_tools_json, enabled
         FROM mcp_servers WHERE id = ?1",
        [id],
        map_mcp,
    )
    .optional()?
    .ok_or(Error::McpServerNotFound)
}

pub fn upsert_mcp_server(conn: &Connection, server: &McpServer) -> Result<i64> {
    if server.id > 0 {
        let n = conn.execute(
            "UPDATE mcp_servers SET name = ?1, transport = ?2, command = ?3, args_json = ?4,
                url = ?5, env_json = ?6, headers_json = ?7, auth = ?8,
                include_tools_json = ?9, enabled = ?10
             WHERE id = ?11",
            params![
                server.name,
                server.transport,
                server.command,
                server.args_json,
                server.url,
                server.env_json,
                server.headers_json,
                server.auth,
                server.include_tools_json,
                i64::from(server.enabled),
                server.id
            ],
        )?;
        if n == 0 {
            return Err(Error::McpServerNotFound);
        }
        Ok(server.id)
    } else {
        conn.execute(
            "INSERT INTO mcp_servers (name, transport, command, args_json, url, env_json,
                headers_json, auth, include_tools_json, enabled, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                server.name,
                server.transport,
                server.command,
                server.args_json,
                server.url,
                server.env_json,
                server.headers_json,
                server.auth,
                server.include_tools_json,
                i64::from(server.enabled),
                now_ms()
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }
}

pub fn delete_mcp_server(conn: &Connection, id: i64) -> Result<()> {
    let n = conn.execute("DELETE FROM mcp_servers WHERE id = ?1", [id])?;
    if n == 0 {
        Err(Error::McpServerNotFound)
    } else {
        Ok(())
    }
}

pub fn set_mcp_server_enabled(conn: &Connection, id: i64, enabled: bool) -> Result<()> {
    if enabled && get_mcp_server(conn, id)?.transport == "sse" {
        return Err(Error::InvalidSetting(
            "旧式 SSE 不能开启，请改填 Streamable HTTP 地址".into(),
        ));
    }
    let n = conn.execute(
        "UPDATE mcp_servers SET enabled = ?1 WHERE id = ?2",
        params![i64::from(enabled), id],
    )?;
    if n == 0 {
        Err(Error::McpServerNotFound)
    } else {
        Ok(())
    }
}

/// 稳定的内部名，不受用户重命名影响，也满足 Pi 的服务名约束。
pub fn mcp_name(id: i64) -> String {
    format!("s{id}")
}

/// Pi 内置 MCP 配置与仅经进程环境传递的值。配置中不含变量 / 请求头值。
pub fn mcp_config(conn: &Connection) -> Result<(serde_json::Value, Vec<(String, String)>)> {
    mcp_config_for(&list_mcp_servers(conn)?)
}

pub fn mcp_config_for(
    servers_input: &[McpServer],
) -> Result<(serde_json::Value, Vec<(String, String)>)> {
    let mut secrets = Vec::new();
    let mut servers = serde_json::Map::new();
    for s in servers_input.iter().filter(|s| s.transport != "sse") {
        let mut obj = serde_json::Map::new();
        if s.transport == "http" {
            if let Some(url) = &s.url {
                obj.insert("url".into(), serde_json::Value::String(url.clone()));
            }
            obj.insert("type".into(), serde_json::json!("http"));
        } else if let Some(cmd) = &s.command {
            obj.insert("command".into(), serde_json::Value::String(cmd.clone()));
            if let Some(args) = s
                .args_json
                .as_deref()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            {
                obj.insert("args".into(), args);
            }
        }
        obj.insert("description".into(), serde_json::json!(s.name));
        obj.insert("enabled".into(), serde_json::json!(s.enabled));
        obj.insert("exposure".into(), serde_json::json!("direct"));
        obj.insert("autoEnableCodemode".into(), serde_json::json!(false));
        let mut index = 0;
        for (key, raw) in [
            ("env", s.env_json.as_deref()),
            ("headers", s.headers_json.as_deref()),
        ] {
            if let Some(raw) = raw {
                let values: std::collections::BTreeMap<String, String> = serde_json::from_str(raw)
                    .map_err(|_| crate::Error::InvalidSetting("外部工具配置格式无效".into()))?;
                let mut refs = serde_json::Map::new();
                for (name, value) in values {
                    let var = format!("RRSS_MCP_{}_{}", s.id, index);
                    index += 1;
                    refs.insert(name, serde_json::json!(format!("${{{var}}}")));
                    secrets.push((var, value));
                }
                obj.insert(key.into(), serde_json::Value::Object(refs));
            }
        }
        if let Some(raw) = s.include_tools_json.as_deref() {
            let tools: Vec<String> = serde_json::from_str(raw)
                .map_err(|_| crate::Error::InvalidSetting("外部工具配置格式无效".into()))?;
            if !tools.is_empty() {
                obj.insert("exposure".into(), serde_json::json!("hidden"));
                let exposure: serde_json::Map<String, serde_json::Value> = tools
                    .into_iter()
                    .map(|tool| (tool, serde_json::json!("direct")))
                    .collect();
                obj.insert("toolExposure".into(), serde_json::Value::Object(exposure));
            }
        }
        servers.insert(mcp_name(s.id), serde_json::Value::Object(obj));
    }
    Ok((
        serde_json::json!({ "mcpServers": servers, "autoEnableCodemode": false }),
        secrets,
    ))
}

pub fn add_pi_usage(
    conn: &Connection,
    day: &str,
    input_tokens: i64,
    output_tokens: i64,
    cost: f64,
) -> Result<PiUsageDay> {
    conn.execute(
        "INSERT INTO pi_usage (day, input_tokens, output_tokens, cost)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (day) DO UPDATE SET
           input_tokens = input_tokens + excluded.input_tokens,
           output_tokens = output_tokens + excluded.output_tokens,
           cost = cost + excluded.cost",
        params![day, input_tokens, output_tokens, cost],
    )?;
    pi_usage_on(conn, day)
}

pub fn pi_usage_on(conn: &Connection, day: &str) -> Result<PiUsageDay> {
    Ok(conn
        .query_row(
            "SELECT input_tokens, output_tokens, cost FROM pi_usage WHERE day = ?1",
            [day],
            |row| {
                Ok(PiUsageDay {
                    input_tokens: row.get(0)?,
                    output_tokens: row.get(1)?,
                    cost: row.get(2)?,
                })
            },
        )
        .optional()?
        .unwrap_or_default())
}

/// `day` 以 `prefix` 开头的对话用量之和；按月统计时传 `YYYY-MM`。
pub fn pi_usage_with_prefix(conn: &Connection, prefix: &str) -> Result<PiUsageDay> {
    Ok(conn.query_row(
        "SELECT coalesce(sum(input_tokens), 0), coalesce(sum(output_tokens), 0), coalesce(sum(cost), 0)
         FROM pi_usage WHERE substr(day, 1, length(?1)) = ?1",
        [prefix],
        |row| {
            Ok(PiUsageDay {
                input_tokens: row.get(0)?,
                output_tokens: row.get(1)?,
                cost: row.get(2)?,
            })
        },
    )?)
}

pub fn enabled_skill_names(conn: &Connection) -> Result<Vec<String>> {
    Ok(match db::get_setting(conn, PI_SKILLS_ENABLED_KEY)? {
        Some(raw) => serde_json::from_str(&raw).unwrap_or_else(|_| vec!["cite-verify".into()]),
        None => vec!["cite-verify".into()],
    })
}

pub fn set_enabled_skill_names(conn: &Connection, names: &[String]) -> Result<()> {
    let raw = serde_json::to_string(names).unwrap_or_else(|_| "[]".into());
    db::set_setting(conn, PI_SKILLS_ENABLED_KEY, &raw)
}

pub fn pi_auto_download(conn: &Connection) -> Result<bool> {
    db::setting_enabled(conn, PI_AUTO_DOWNLOAD_KEY, true)
}

pub fn set_pi_auto_download(conn: &Connection, enabled: bool) -> Result<()> {
    db::set_setting_enabled(conn, PI_AUTO_DOWNLOAD_KEY, enabled)
}

pub fn pi_auto_update(conn: &Connection) -> Result<bool> {
    db::setting_enabled(conn, PI_AUTO_UPDATE_KEY, false)
}

pub fn set_pi_auto_update(conn: &Connection, enabled: bool) -> Result<()> {
    db::set_setting_enabled(conn, PI_AUTO_UPDATE_KEY, enabled)
}

pub fn pi_managed_version(conn: &Connection) -> Result<Option<String>> {
    db::get_setting(conn, PI_MANAGED_VERSION_KEY)
}

pub fn set_pi_managed_version(conn: &Connection, version: &str) -> Result<()> {
    db::set_setting(conn, PI_MANAGED_VERSION_KEY, version)
}

/// 最近一次握手得到的可用思考档位（JSON 数组）；引擎没在运行时，设置页用它显示可选档位。
pub const PI_THINKING_LEVELS_CACHE_KEY: &str = "pi_thinking_levels_cache";

pub fn cached_thinking_levels(conn: &Connection) -> Result<Vec<String>> {
    Ok(db::get_setting(conn, PI_THINKING_LEVELS_CACHE_KEY)?
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default())
}

pub fn set_cached_thinking_levels(conn: &Connection, levels: &[String]) -> Result<()> {
    let raw = serde_json::to_string(levels).unwrap_or_else(|_| "[]".into());
    db::set_setting(conn, PI_THINKING_LEVELS_CACHE_KEY, &raw)
}

/// 助手说明（D8）：追加在 RRSS 固定系统提示之后，不能替换它。
pub const PI_ASSISTANT_NOTE_KEY: &str = "pi_assistant_note";
pub const ASSISTANT_NOTE_MAX_CHARS: usize = 2000;

pub fn assistant_note(conn: &Connection) -> Result<String> {
    Ok(db::get_setting(conn, PI_ASSISTANT_NOTE_KEY)?
        .map(|s| s.trim().to_string())
        .unwrap_or_default())
}

pub fn set_assistant_note(conn: &Connection, note: &str) -> Result<()> {
    let note = note.trim();
    if note.chars().count() > ASSISTANT_NOTE_MAX_CHARS {
        return Err(Error::InvalidPiOverride(PiOverrideProblem::NoteTooLong { max: ASSISTANT_NOTE_MAX_CHARS }));
    }
    db::set_setting(conn, PI_ASSISTANT_NOTE_KEY, note)
}

/// Pi 设置覆盖：浅合并进生成的 `agent/settings.json`（阶段 18 D7）。
pub const PI_SETTINGS_OVERRIDE_KEY: &str = "pi_settings_override";
/// 模型定义覆盖：合并进生成的 `models.json`。
pub const PI_MODELS_OVERRIDE_KEY: &str = "pi_models_override";
/// 「使用我自己的 Pi 配置」（D3），默认关闭。
pub const PI_USE_OWN_CONFIG_KEY: &str = "pi_use_own_config";
/// 自有配置下选用的模型，`provider/id`；空表示用 Pi 的默认模型。
pub const PI_OWN_MODEL_KEY: &str = "pi_own_model";

/// 读取一项覆盖；未设置或已损坏时是空对象。
pub fn pi_override(conn: &Connection, key: &str) -> Result<serde_json::Value> {
    Ok(db::get_setting(conn, key)?
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({})))
}

/// 保存一项覆盖。必须是合法的 JSON 对象，否则拒绝，原值不动；空白表示清除。
pub fn set_pi_override(conn: &Connection, key: &str, raw: &str) -> Result<()> {
    if key != PI_SETTINGS_OVERRIDE_KEY && key != PI_MODELS_OVERRIDE_KEY {
        return Err(Error::InvalidPiOverride(PiOverrideProblem::KeyUnknown { key: key.into() }));
    }
    let raw = raw.trim();
    if raw.is_empty() {
        return db::set_setting(conn, key, "{}");
    }
    let value: serde_json::Value = serde_json::from_str(raw)
        .map_err(|err| Error::InvalidPiOverride(format!("不是合法的 JSON：{err}").into()))?;
    if !value.is_object() {
        return Err(Error::InvalidPiOverride(PiOverrideProblem::ObjectRequired));
    }
    db::set_setting(conn, key, &value.to_string())
}

pub fn use_own_config(conn: &Connection) -> Result<bool> {
    db::setting_enabled(conn, PI_USE_OWN_CONFIG_KEY, false)
}

pub fn set_use_own_config(conn: &Connection, enabled: bool) -> Result<()> {
    db::set_setting_enabled(conn, PI_USE_OWN_CONFIG_KEY, enabled)
}

pub fn own_model(conn: &Connection) -> Result<Option<String>> {
    Ok(db::get_setting(conn, PI_OWN_MODEL_KEY)?
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty()))
}

/// `provider/id`，或空（用 Pi 的默认模型）。
pub fn set_own_model(conn: &Connection, model: &str) -> Result<()> {
    let model = model.trim();
    if !model.is_empty() {
        let (provider, id) = model.split_once('/').unwrap_or(("", ""));
        if provider.trim().is_empty() || id.trim().is_empty() {
            return Err(Error::InvalidPiOverride(PiOverrideProblem::ModelInvalid { model: model.into() }));
        }
    }
    db::set_setting(conn, PI_OWN_MODEL_KEY, model)
}

fn map_tool(row: &rusqlite::Row<'_>) -> rusqlite::Result<PiTool> {
    Ok(PiTool {
        name: row.get(0)?,
        enabled: row.get::<_, i64>(1)? != 0,
        label: row.get(2)?,
        description: row.get(3)?,
        confirm_required: row.get::<_, i64>(4)? != 0,
        builtin: row.get::<_, i64>(5)? != 0,
        default_description: row.get(6)?,
    })
}

fn map_mcp(row: &rusqlite::Row<'_>) -> rusqlite::Result<McpServer> {
    Ok(McpServer {
        id: row.get(0)?,
        name: row.get(1)?,
        transport: row.get(2)?,
        command: row.get(3)?,
        args_json: row.get(4)?,
        url: row.get(5)?,
        env_json: row.get(6)?,
        headers_json: row.get(7)?,
        auth: row.get(8)?,
        include_tools_json: row.get(9)?,
        enabled: row.get::<_, i64>(10)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_at;
    use tempfile::tempdir;

    #[test]
    fn overrides_accept_only_json_objects_and_keep_the_old_value() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        assert_eq!(
            pi_override(&conn, PI_SETTINGS_OVERRIDE_KEY).unwrap(),
            serde_json::json!({})
        );
        set_pi_override(
            &conn,
            PI_SETTINGS_OVERRIDE_KEY,
            r#"{"quietStartup": false}"#,
        )
        .unwrap();
        for bad in ["{not json", "[1, 2]", "\"text\""] {
            let err = set_pi_override(&conn, PI_SETTINGS_OVERRIDE_KEY, bad).unwrap_err();
            assert!(matches!(err, Error::InvalidPiOverride(_)), "{bad}: {err}");
        }
        assert_eq!(
            pi_override(&conn, PI_SETTINGS_OVERRIDE_KEY).unwrap(),
            serde_json::json!({"quietStartup": false})
        );
        set_pi_override(&conn, PI_SETTINGS_OVERRIDE_KEY, "  ").unwrap();
        assert_eq!(
            pi_override(&conn, PI_SETTINGS_OVERRIDE_KEY).unwrap(),
            serde_json::json!({})
        );
        assert!(set_pi_override(&conn, "theme", "{}").is_err());

        assert!(!use_own_config(&conn).unwrap());
        set_use_own_config(&conn, true).unwrap();
        assert!(use_own_config(&conn).unwrap());
        assert!(set_own_model(&conn, "no-slash").is_err());
        assert!(set_own_model(&conn, "/id").is_err());
        set_own_model(&conn, "anthropic/claude-x").unwrap();
        assert_eq!(
            own_model(&conn).unwrap().as_deref(),
            Some("anthropic/claude-x")
        );
        set_own_model(&conn, "").unwrap();
        assert_eq!(own_model(&conn).unwrap(), None);
    }

    #[test]
    fn seeds_and_manifest_skips_disabled() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let tools = list_tools(&conn).unwrap();
        assert!(tools.iter().any(|t| t.name == "explain_score"));
        set_tool_enabled(&conn, "fetch_url", false).unwrap();
        let manifest = tool_manifest(&conn).unwrap();
        assert!(manifest.tools.iter().any(|t| t.name == "search_articles"));
        assert!(!manifest.tools.iter().any(|t| t.name == "fetch_url"));
        let config = manifest
            .tools
            .iter()
            .find(|t| t.name == "get_assistant_config")
            .unwrap();
        assert!(!config.confirm);
        assert!(config.parameters.is_empty());
    }

    #[test]
    fn chat_usage_sums_by_day_prefix() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        add_pi_usage(&conn, "2026-09-28", 100, 20, 0.1).unwrap();
        add_pi_usage(&conn, "2026-09-29", 10, 2, 0.01).unwrap();
        add_pi_usage(&conn, "2026-10-01", 1, 1, 0.0).unwrap();
        let month = pi_usage_with_prefix(&conn, "2026-09").unwrap();
        assert_eq!((month.input_tokens, month.output_tokens), (110, 22));
        assert_eq!(
            pi_usage_with_prefix(&conn, "2026-09-29")
                .unwrap()
                .input_tokens,
            10
        );
        assert_eq!(pi_usage_with_prefix(&conn, "2025").unwrap().input_tokens, 0);
    }

    #[test]
    fn mcp_json_retains_disabled_servers_for_login() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        upsert_mcp_server(
            &conn,
            &McpServer {
                id: 0,
                name: "filesystem".into(),
                transport: "stdio".into(),
                command: Some("npx".into()),
                args_json: Some(r#"["-y","@modelcontextprotocol/server-filesystem","."]"#.into()),
                url: None,
                env_json: None,
                headers_json: None,
                auth: "none".into(),
                include_tools_json: None,
                enabled: true,
            },
        )
        .unwrap();
        upsert_mcp_server(
            &conn,
            &McpServer {
                id: 0,
                name: "off".into(),
                transport: "stdio".into(),
                command: Some("echo".into()),
                args_json: None,
                url: None,
                env_json: None,
                headers_json: None,
                auth: "none".into(),
                include_tools_json: None,
                enabled: false,
            },
        )
        .unwrap();
        let (json, _) = mcp_config(&conn).unwrap();
        assert_eq!(json["mcpServers"]["s1"]["enabled"], true);
        assert_eq!(json["mcpServers"]["s2"]["enabled"], false);
    }

    #[test]
    fn get_digest_manifest_params_optional_get_article_id_required() {
        let dir = tempdir().unwrap();
        let conn = open_at(&dir.path().join("rrss.db")).unwrap();
        let manifest = tool_manifest(&conn).unwrap();
        let digest = manifest
            .tools
            .iter()
            .find(|t| t.name == "get_digest")
            .expect("get_digest");
        assert_eq!(digest.parameters.len(), 2);
        let id = digest
            .parameters
            .iter()
            .find(|p| p.name == "id")
            .expect("id param");
        let period = digest
            .parameters
            .iter()
            .find(|p| p.name == "period")
            .expect("period param");
        assert!(id.optional);
        assert!(period.optional);
        assert_eq!(id.kind, "number");
        assert_eq!(period.kind, "string");

        let article = manifest
            .tools
            .iter()
            .find(|t| t.name == "get_article")
            .expect("get_article");
        let article_id = article
            .parameters
            .iter()
            .find(|p| p.name == "id")
            .expect("article id");
        assert!(!article_id.optional);
    }
}
