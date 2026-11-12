//! 外部工具的编辑、脱敏投影、原子导入与用户发起的检测 / 登录。
mod login;
mod probe;
pub use login::{cancel_login, delete, logged_in, login};
pub use probe::run as probe;
use std::collections::BTreeMap;

use rrss_core::{
    Error, Result,
    pi_store::{self, McpServer},
};
use rusqlite::Connection;
use serde_json::Value;

use crate::{McpImportEntry, McpInput, McpSecretInput, McpSecretView, McpView};

fn invalid(message: &str) -> Error {
    Error::InvalidSetting(message.into())
}
fn strings(raw: Option<&str>) -> Vec<String> {
    raw.and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}
fn values(raw: Option<&str>) -> BTreeMap<String, String> {
    raw.and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}
pub fn view(s: McpServer) -> McpView {
    let secrets = |raw: Option<&str>| {
        values(raw)
            .into_iter()
            .map(|(name, value)| McpSecretView {
                name,
                has_value: !value.is_empty(),
            })
            .collect()
    };
    McpView {
        id: s.id,
        name: s.name,
        transport: s.transport,
        command: s.command.unwrap_or_default(),
        args: strings(s.args_json.as_deref()),
        url: s.url.unwrap_or_default(),
        env: secrets(s.env_json.as_deref()),
        headers: secrets(s.headers_json.as_deref()),
        auth: s.auth,
        logged_in: false,
        include_tools: strings(s.include_tools_json.as_deref()),
        enabled: s.enabled,
    }
}
fn merge(input: &[McpSecretInput], old: Option<&str>, headers: bool) -> Result<String> {
    let old = values(old);
    let mut out = BTreeMap::new();
    for item in input {
        let name = item.name.trim();
        let valid = if headers {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
        } else {
            !name.is_empty()
                && name.bytes().enumerate().all(|(i, b)| {
                    b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit())
                })
        };
        if !valid {
            return Err(invalid("变量名或请求头名称无效"));
        }
        let value = if item.value.is_empty() {
            old.get(name).cloned().unwrap_or_default()
        } else {
            item.value.clone()
        };
        if value.contains('\0') || (headers && (value.contains('\r') || value.contains('\n'))) {
            return Err(invalid("变量或请求头值包含无效字符"));
        }
        if out.insert(name.to_string(), value).is_some() {
            return Err(invalid("变量或请求头名称重复"));
        }
    }
    Ok(serde_json::to_string(&out).expect("strings serialize"))
}
fn server(conn: &Connection, input: &McpInput) -> Result<McpServer> {
    let old = if input.id > 0 {
        Some(pi_store::get_mcp_server(conn, input.id)?)
    } else {
        None
    };
    let name = input.name.trim();
    if input.id < 0 || name.is_empty() || name.chars().any(char::is_control) {
        return Err(invalid("请填写有效的服务名称"));
    }
    if !["stdio", "http", "sse"].contains(&input.transport.as_str()) {
        return Err(invalid("不支持的服务类型"));
    }
    if !["none", "token", "oauth"].contains(&input.auth.as_str()) {
        return Err(invalid("不支持的登录方式"));
    }
    if input.auth == "oauth" && input.transport != "http" {
        return Err(invalid("浏览器登录只支持远程 HTTP 服务"));
    }
    if input
        .include_tools
        .iter()
        .any(|tool| tool.is_empty() || tool.contains('*') || tool.chars().any(char::is_control))
    {
        return Err(invalid("开放工具请填写完整名称，不能使用通配符"));
    }
    if input.auth == "oauth"
        && input
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("authorization"))
    {
        return Err(invalid("浏览器登录不能同时填写 Authorization 请求头"));
    }
    let local = input.transport == "stdio";
    if local
        && (input.command.trim().is_empty()
            || input.command.contains('\0')
            || input.args.iter().any(|a| a.contains('\0')))
    {
        return Err(invalid("请填写有效的启动命令和参数"));
    }
    if !local {
        let url = reqwest::Url::parse(input.url.trim())
            .map_err(|_| invalid("请填写 HTTP 或 HTTPS 地址"))?;
        if !["http", "https"].contains(&url.scheme())
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(invalid("请填写 HTTP 或 HTTPS 地址，凭据请放在请求头中"));
        }
    }
    Ok(McpServer {
        id: input.id,
        name: name.into(),
        transport: input.transport.clone(),
        command: local.then(|| input.command.trim().into()),
        args_json: local.then(|| serde_json::to_string(&input.args).expect("strings serialize")),
        url: (!local).then(|| input.url.trim().into()),
        env_json: if local {
            Some(merge(
                &input.env,
                old.as_ref().and_then(|s| s.env_json.as_deref()),
                false,
            )?)
        } else {
            None
        },
        headers_json: if !local {
            Some(merge(
                &input.headers,
                old.as_ref().and_then(|s| s.headers_json.as_deref()),
                true,
            )?)
        } else {
            None
        },
        auth: input.auth.clone(),
        include_tools_json: Some(
            serde_json::to_string(&input.include_tools).expect("strings serialize"),
        ),
        enabled: input.transport != "sse"
            && old.is_some_and(|s| {
                s.enabled
                    && s.auth == input.auth
                    && (local || s.url.as_deref().unwrap_or_default() == input.url.trim())
            }),
    })
}
pub fn save(conn: &Connection, input: &McpInput) -> Result<i64> {
    let tx = rrss_core::db::write_transaction(conn)?;
    if pi_store::list_mcp_servers(&tx)?
        .iter()
        .any(|s| s.name == input.name.trim() && s.id != input.id)
    {
        return Err(invalid("服务名称已存在"));
    }
    let id = pi_store::upsert_mcp_server(&tx, &server(&tx, input)?)?;
    tx.commit()?;
    Ok(id)
}
fn parse(raw: &str) -> Result<Vec<McpInput>> {
    let doc: Value = serde_json::from_str(raw).map_err(|_| invalid("无法解析 JSON，请检查格式"))?;
    let map = doc
        .get("mcpServers")
        .unwrap_or(&doc)
        .as_object()
        .ok_or_else(|| invalid("需要 mcpServers 对象或服务名称到配置的对象"))?;
    if map.is_empty() {
        return Err(invalid("JSON 中没有服务"));
    }
    map.iter()
        .map(|(name, v)| {
            let obj = v.as_object().ok_or_else(|| invalid("服务配置必须是对象"))?;
            let string = |key: &str, default: &str| -> Result<String> {
                match obj.get(key) {
                    None => Ok(default.into()),
                    Some(Value::String(s)) => Ok(s.clone()),
                    _ => Err(invalid("服务文本字段格式错误")),
                }
            };
            let array = |key: &str| -> Result<Vec<String>> {
                obj.get(key)
                    .map(|v| {
                        serde_json::from_value(v.clone())
                            .map_err(|_| invalid("参数与工具白名单必须是字符串数组"))
                    })
                    .unwrap_or_else(|| Ok(vec![]))
            };
            let secrets = |key: &str| -> Result<Vec<McpSecretInput>> {
                let map: BTreeMap<String, String> = obj
                    .get(key)
                    .map(|v| {
                        serde_json::from_value(v.clone())
                            .map_err(|_| invalid("变量与请求头必须是字符串键值对象"))
                    })
                    .unwrap_or_else(|| Ok(BTreeMap::new()))?;
                Ok(map
                    .into_iter()
                    .map(|(name, value)| McpSecretInput { name, value })
                    .collect())
            };
            let transport = string("httpTransport", &string("type", &string("transport", "")?)?)?;
            let url = string("url", "")?;
            let transport = if transport == "sse" {
                "sse"
            } else if !url.is_empty() {
                "http"
            } else {
                "stdio"
            };
            Ok(McpInput {
                id: 0,
                name: name.clone(),
                transport: transport.into(),
                command: string("command", "")?,
                args: array("args")?,
                url,
                env: secrets("env")?,
                headers: secrets("headers")?,
                auth: string(
                    "auth",
                    if secrets("headers")?.is_empty() {
                        "none"
                    } else {
                        "token"
                    },
                )?,
                include_tools: array("includeTools")?,
            })
        })
        .collect()
}
fn prepare(conn: &Connection, raw: &str) -> Result<(Vec<McpServer>, Vec<McpImportEntry>)> {
    let existing = pi_store::list_mcp_servers(conn)?;
    let mut servers = Vec::new();
    let mut entries = Vec::new();
    for mut input in parse(raw)? {
        input.name = input.name.trim().to_string();
        let skipped = existing.iter().any(|s| s.name == input.name)
            || servers.iter().any(|s: &McpServer| s.name == input.name);
        if !skipped {
            servers.push(server(conn, &input).map_err(|e| match e {
                Error::InvalidSetting(m) => invalid(&format!("「{}」：{m}", input.name)),
                e => e,
            })?);
        }
        entries.push(McpImportEntry {
            name: input.name,
            transport: input.transport,
            secret_count: input.env.len() + input.headers.len(),
            skipped,
        });
    }
    Ok((servers, entries))
}
pub fn preview_import(conn: &Connection, raw: &str) -> Result<Vec<McpImportEntry>> {
    Ok(prepare(conn, raw)?.1)
}
pub fn import(conn: &Connection, raw: &str) -> Result<Vec<McpImportEntry>> {
    let tx = rrss_core::db::write_transaction(conn)?;
    let (servers, entries) = prepare(&tx, raw)?;
    for server in servers {
        pi_store::upsert_mcp_server(&tx, &server)?;
    }
    tx.commit()?;
    Ok(entries)
}

pub const POLICY: &str = include_str!("../../../pi/mcp-policy.json");

pub fn writes(name: &str, annotations: Option<&Value>) -> bool {
    static POLICY_VALUE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    let policy = POLICY_VALUE.get_or_init(|| serde_json::from_str(POLICY).expect("MCP policy"));
    for rule in policy["annotations"].as_array().unwrap() {
        if annotations.and_then(|a| a.get(rule["key"].as_str().unwrap())) == Some(&rule["value"]) {
            return rule["writes"].as_bool().unwrap();
        }
    }
    let mut words = String::new();
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            words.push('_');
        }
        words.push(c.to_ascii_lowercase());
    }
    words
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|word| {
            policy["name_words"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| v.as_str() == Some(word))
        })
        || policy["default"].as_bool().unwrap()
}

fn apply_environment(
    command: &mut tokio::process::Command,
    proxy: &rrss_core::network::ChildProxyEnv,
    server: &McpServer,
) -> anyhow::Result<()> {
    crate::pi::launch::apply_proxy_env(command, proxy);
    let mut enabled = server.clone();
    enabled.enabled = true;
    let (_, refs) = pi_store::mcp_config_for(&[enabled])?;
    command.envs(refs).envs(values(server.env_json.as_deref()));
    command
        .env("PI_OFFLINE", "1")
        .env("PI_SKIP_VERSION_CHECK", "1");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn write_policy_annotation_precedence_and_name_fallback() {
        let readonly = serde_json::json!({"readOnlyHint":true,"destructiveHint":true});
        assert!(!writes("delete_file", Some(&readonly)));
        assert!(writes(
            "lookup",
            Some(&serde_json::json!({"destructiveHint":true}))
        ));
        assert!(writes("updateFile", None));
        assert!(writes("write-note", None));
        assert!(!writes("read_file", None));
        assert!(!writes("credit_score", None));
    }

    #[test]
    fn import_edit_and_config_keep_arguments_and_secrets_without_enabling_services() {
        let dir = tempdir().unwrap();
        let conn = rrss_core::db::open_at(&dir.path().join("mcp.db")).unwrap();
        let raw = r#"{"mcpServers":{"local":{"command":"echo","args":["path with spaces",""],"env":{"TOKEN":"!do-not-run"},"includeTools":["read"]},"remote":{"url":"https://example.test/mcp","headers":{"Authorization":"Bearer private-value"}},"events":{"url":"http://127.0.0.1:9000/sse","type":"sse","lifecycle":"keep-alive"}}}"#;
        let preview = preview_import(&conn, raw).unwrap();
        assert_eq!(preview.len(), 3);
        assert!(
            !serde_json::to_string(&preview)
                .unwrap()
                .contains("private-value")
        );
        assert!(pi_store::list_mcp_servers(&conn).unwrap().is_empty());
        import(&conn, raw).unwrap();
        assert!(import(&conn, raw).unwrap().iter().all(|e| e.skipped));
        assert!(!pi_store::mcp_enabled(&conn).unwrap());
        let all = pi_store::list_mcp_servers(&conn).unwrap();
        let local = all.iter().find(|s| s.name == "local").unwrap();
        let remote = all.iter().find(|s| s.name == "remote").unwrap();
        let display = serde_json::to_string(&view(local.clone())).unwrap();
        assert!(!display.contains("do-not-run"));
        let mut edit = parse(raw)
            .unwrap()
            .into_iter()
            .find(|s| s.name == "local")
            .unwrap();
        edit.id = local.id;
        edit.env[0].value.clear();
        pi_store::set_mcp_server_enabled(&conn, local.id, true).unwrap();
        save(&conn, &edit).unwrap();
        let saved = pi_store::get_mcp_server(&conn, local.id).unwrap();
        assert!(saved.enabled);
        assert_eq!(values(saved.env_json.as_deref())["TOKEN"], "!do-not-run");
        for s in all.iter().filter(|s| s.transport != "sse") {
            pi_store::set_mcp_server_enabled(&conn, s.id, true).unwrap();
        }
        let (config, secrets) = pi_store::mcp_config(&conn).unwrap();
        assert_eq!(
            config["mcpServers"][pi_store::mcp_name(local.id)]["args"],
            serde_json::json!(["path with spaces", ""])
        );
        assert_eq!(
            config["mcpServers"][pi_store::mcp_name(local.id)]["toolExposure"],
            serde_json::json!({"read": "direct"})
        );
        assert!(
            config["mcpServers"][pi_store::mcp_name(remote.id)]
                .get("httpTransport")
                .is_none()
        );
        assert_eq!(config["mcpServers"].as_object().unwrap().len(), 2);
        assert_eq!(
            config["mcpServers"][pi_store::mcp_name(remote.id)]["headers"]["Authorization"],
            format!("${{RRSS_MCP_{}_0}}", remote.id)
        );
        assert!(!config.to_string().contains("private-value"));
        assert!(!config.to_string().contains("do-not-run"));
        assert!(secrets.iter().any(|(_, value)| value == "!do-not-run"));
        edit.env.clear();
        save(&conn, &edit).unwrap();
        assert!(
            values(
                pi_store::get_mcp_server(&conn, local.id)
                    .unwrap()
                    .env_json
                    .as_deref()
            )
            .is_empty()
        );
    }

    #[test]
    fn failed_import_or_edit_is_atomic_and_errors_do_not_echo_values() {
        let dir = tempdir().unwrap();
        let conn = rrss_core::db::open_at(&dir.path().join("mcp.db")).unwrap();
        let raw = r#"{"good":{"command":"echo"},"bad":{"url":"secret-invalid-url"}}"#;
        let error = import(&conn, raw).unwrap_err().to_string();
        assert!(!error.contains("secret-invalid-url"));
        assert!(error.contains("「bad」"), "{error}");
        assert!(pi_store::list_mcp_servers(&conn).unwrap().is_empty());
        let raw = r#"{"one":{"command":"echo","args":["a b"]}}"#;
        import(&conn, raw).unwrap();
        let mut input = parse(raw).unwrap().remove(0);
        input.id = pi_store::list_mcp_servers(&conn).unwrap()[0].id;
        input.name = "renamed".into();
        input.env = vec![McpSecretInput {
            name: "BAD NAME".into(),
            value: "never-echo".into(),
        }];
        assert!(
            !save(&conn, &input)
                .unwrap_err()
                .to_string()
                .contains("never-echo")
        );
        assert_eq!(pi_store::list_mcp_servers(&conn).unwrap()[0].name, "one");
        input.id = 999;
        assert!(save(&conn, &input).is_err());
    }
}

pub fn toggle(conn: &Connection, data_dir: &std::path::Path, id: i64, enabled: bool) -> Result<()> {
    let server = pi_store::get_mcp_server(conn, id)?;
    if enabled
        && server.auth == "oauth"
        && (pi_store::use_own_config(conn)? || !logged_in(data_dir, &server))
    {
        return Err(invalid("请先完成浏览器登录，再开启此服务"));
    }
    pi_store::set_mcp_server_enabled(conn, id, enabled)
}
