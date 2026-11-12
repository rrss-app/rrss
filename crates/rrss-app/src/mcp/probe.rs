//! 一次性的 MCP 握手；不写库、不打开服务开关，也不调用工具。
use crate::{McpInput, McpProbeView, McpToolView};
use anyhow::{Context, Result, bail};
use rrss_core::{
    network::{self, NetworkScope},
    pi_store, platform,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin},
    sync::mpsc,
};

const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_MESSAGE: usize = 1024 * 1024;
const MAX_OUTPUT: usize = 8192;
const VERSIONS: &[&str] = &["2026-07-28", "2025-06-18", "2025-03-26", "2024-11-05"];
type Diagnostics = Arc<Mutex<DiagnosticOutput>>;

#[derive(Default)]
struct DiagnosticOutput {
    bytes: Vec<u8>,
    omitted: bool,
}

impl DiagnosticOutput {
    fn text(&self) -> String {
        if self.omitted {
            "诊断输出过长，已省略".into()
        } else {
            String::from_utf8_lossy(&self.bytes).into_owned()
        }
    }
}

fn append(diagnostics: &Diagnostics, bytes: &[u8]) {
    let mut diagnostics = diagnostics.lock().unwrap();
    if diagnostics.omitted {
        return;
    }
    if bytes.len() > MAX_OUTPUT - diagnostics.bytes.len() {
        // 不保留截断后的片段：它可能从已知密钥中间开始，之后的清洗无法识别。
        diagnostics.bytes.clear();
        diagnostics.omitted = true;
    } else {
        diagnostics.bytes.extend_from_slice(bytes);
    }
}
async fn stderr_diagnostics(mut reader: impl AsyncRead + Unpin, diagnostics: Diagnostics) {
    let mut buf = [0; 2048];
    while let Ok(n) = reader.read(&mut buf).await {
        if n == 0 {
            break;
        }
        append(&diagnostics, &buf[..n]);
    }
}
async fn stdout_messages(
    mut reader: impl AsyncRead + Unpin,
    tx: mpsc::Sender<Result<Value>>,
    diagnostics: Diagnostics,
) {
    let mut buf = [0; 4096];
    let mut pending = Vec::new();
    while let Ok(n) = reader.read(&mut buf).await {
        if n == 0 {
            break;
        }
        pending.extend_from_slice(&buf[..n]);
        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
            if end > MAX_MESSAGE {
                let _ = tx.send(Err(anyhow::anyhow!("MCP 消息过大"))).await;
                return;
            }
            let line: Vec<_> = pending.drain(..=end).collect();
            match serde_json::from_slice::<Value>(&line) {
                Ok(value) => {
                    if tx.send(Ok(value)).await.is_err() {
                        return;
                    }
                }
                Err(_) => append(&diagnostics, &line),
            }
        }
        if pending.len() > MAX_MESSAGE {
            let _ = tx.send(Err(anyhow::anyhow!("MCP 消息过大"))).await;
            return;
        }
    }
    if !pending.is_empty() {
        append(&diagnostics, &pending);
    }
}

struct Local {
    child: Child,
    started: Instant,
    tree: Option<platform::ChildTree>,
    input: ChildStdin,
    rx: mpsc::Receiver<Result<Value>>,
    readers: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Local {
    fn drop(&mut self) {
        self.tree.take();
        for task in &self.readers {
            task.abort();
        }
    }
}
impl Local {
    fn start(
        server: &pi_store::McpServer,
        cwd: &Path,
        proxy: &network::ChildProxyEnv,
        diagnostics: Diagnostics,
    ) -> Result<Self> {
        let env = super::values(server.env_json.as_deref());
        let path = env
            .get("PATH")
            .map(std::ffi::OsString::from)
            .or_else(|| std::env::var_os("PATH"));
        let program = platform::expand_mcp_home(server.command.as_deref().unwrap_or_default());
        let mut cmd = platform::mcp_command(
            program.to_str().context("命令路径无效")?,
            cwd,
            path.as_deref(),
        );
        cmd.args(
            super::strings(server.args_json.as_deref())
                .iter()
                .map(|arg| platform::expand_mcp_home(arg)),
        );
        super::apply_environment(&mut cmd, proxy, server)?;
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = cmd
            .spawn()
            .context("无法启动本机程序；请检查命令是否已安装")?;
        let tree = Some(platform::ChildTree::attach(&mut child)?);
        let input = child.stdin.take().unwrap();
        let (tx, rx) = mpsc::channel(32);
        let readers = vec![
            tokio::spawn(stdout_messages(
                child.stdout.take().unwrap(),
                tx,
                diagnostics.clone(),
            )),
            tokio::spawn(stderr_diagnostics(
                child.stderr.take().unwrap(),
                diagnostics,
            )),
        ];
        Ok(Self {
            child,
            started: Instant::now(),
            tree,
            input,
            rx,
            readers,
        })
    }
    async fn exchange(&mut self, body: &Value) -> Result<Value> {
        self.input.write_all(format!("{body}\n").as_bytes()).await?;
        self.input.flush().await?;
        let Some(id) = body.get("id") else {
            return Ok(Value::Null);
        };
        while let Some(message) = self.rx.recv().await {
            let message = message?;
            if message.get("id") == Some(id) {
                return Ok(message);
            }
            if message.get("method").is_some() && message.get("id").is_some() {
                self.input.write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32601,"message":"RRSS probe does not support client requests"}})).as_bytes()).await?;
            }
        }
        let after = self.started.elapsed().as_secs_f64();
        match self.child.try_wait()? {
            Some(status) => match status.code() {
                Some(code) => bail!("程序启动后 {after:.1} 秒就退出了（退出码 {code}）"),
                None => bail!("程序启动后 {after:.1} 秒被系统结束"),
            },
            None => bail!("程序在握手完成前关闭了输出"),
        }
    }
    async fn close(&mut self) {
        let _ = self.input.shutdown().await;
        self.tree.take();
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        for mut reader in self.readers.drain(..) {
            if tokio::time::timeout(Duration::from_millis(100), &mut reader)
                .await
                .is_err()
            {
                reader.abort();
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("远程服务返回 HTTP {0}")]
struct HttpStatus(u16);

struct Remote {
    client: reqwest::Client,
    url: String,
    headers: reqwest::header::HeaderMap,
    session: Option<String>,
    version: Option<String>,
    diagnostics: Diagnostics,
}
impl Remote {
    fn request(&self, method: reqwest::Method) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .request(method, &self.url)
            .headers(self.headers.clone())
            .header("Accept", "application/json, text/event-stream");
        if let Some(session) = &self.session {
            request = request.header("Mcp-Session-Id", session);
        }
        if let Some(version) = &self.version {
            request = request.header("MCP-Protocol-Version", version);
        }
        request
    }
    async fn exchange(&mut self, body: &Value) -> Result<Value> {
        let mut response = self
            .request(reqwest::Method::POST)
            .json(body)
            .send()
            .await
            .context("无法连接远程服务")?;
        if let Some(id) = response.headers().get("Mcp-Session-Id") {
            self.session = Some(id.to_str()?.to_owned());
        }
        let status = response.status();
        let sse = response
            .headers()
            .get("Content-Type")
            .and_then(|s| s.to_str().ok())
            .is_some_and(|s| s.starts_with("text/event-stream"));
        if status.is_success() && body.get("id").is_none() {
            return Ok(Value::Null);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > MAX_MESSAGE {
                bail!("MCP 响应过大");
            }
            if status.is_success() && sse {
                let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
                for event in text.split("\n\n").take(text.matches("\n\n").count()) {
                    let data = event
                        .lines()
                        .filter_map(|l| l.strip_prefix("data:").map(str::trim_start))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if let Ok(value) = serde_json::from_str::<Value>(&data) {
                        if value.get("id") == body.get("id") {
                            return Ok(value);
                        }
                    }
                }
            }
        }
        if !status.is_success() {
            append(&self.diagnostics, &bytes);
            return Err(HttpStatus(status.as_u16()).into());
        }
        if sse {
            bail!("远程流在返回 MCP 响应前关闭");
        }
        serde_json::from_slice(&bytes).map_err(|_| {
            append(&self.diagnostics, &bytes);
            anyhow::anyhow!("远程地址没有返回 MCP JSON 响应")
        })
    }
    async fn close(&self) {
        if self.session.is_some() {
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                self.request(reqwest::Method::DELETE).send(),
            )
            .await;
        }
    }
}
enum Transport {
    Local(Local),
    Remote(Remote),
}
impl Transport {
    async fn exchange(&mut self, body: &Value) -> Result<Value> {
        match self {
            Self::Local(t) => t.exchange(body).await,
            Self::Remote(t) => t.exchange(body).await,
        }
    }
    async fn close(&mut self) {
        match self {
            Self::Local(t) => t.close().await,
            Self::Remote(t) => t.close().await,
        }
    }
}
async fn handshake(transport: &mut Transport) -> Result<Vec<McpToolView>> {
    let mut initialized = false;
    for (i, version) in VERSIONS.iter().enumerate() {
        let response = match transport.exchange(&json!({"jsonrpc":"2.0","id":i+1,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"rrss-probe","version":env!("CARGO_PKG_VERSION")}}})).await {
            Ok(response) => response,
            Err(error) if i + 1 < VERSIONS.len() && error.downcast_ref::<HttpStatus>().is_some_and(|s| matches!(s.0, 400 | 406 | 415)) => {
                if let Transport::Remote(remote) = transport {
                    remote.close().await;
                    remote.session = None;
                }
                continue;
            }
            Err(error) => return Err(error),
        };
        if let Some(error) = response.get("error") {
            if i + 1 < VERSIONS.len() {
                continue;
            }
            bail!(
                "MCP 初始化失败：{}",
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("协议版本不兼容")
            );
        }
        let negotiated = response["result"]["protocolVersion"]
            .as_str()
            .context("MCP 初始化响应缺少协议版本")?;
        if !VERSIONS.contains(&negotiated) {
            continue;
        }
        if let Transport::Remote(t) = transport {
            t.version = Some(negotiated.to_owned());
        }
        initialized = true;
        break;
    }
    if !initialized {
        bail!("MCP 协议版本不兼容");
    }
    transport
        .exchange(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .await?;
    let mut tools = BTreeMap::new();
    let mut cursor = Value::Null;
    for page in 0..100 {
        let params = if cursor.is_null() {
            json!({})
        } else {
            json!({"cursor":cursor})
        };
        let response = transport
            .exchange(&json!({"jsonrpc":"2.0","id":100+page,"method":"tools/list","params":params}))
            .await?;
        if let Some(err) = response.get("error") {
            bail!(
                "读取工具失败：{}",
                err.get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("服务拒绝请求")
            );
        }
        for tool in response["result"]["tools"]
            .as_array()
            .context("服务没有返回工具列表")?
        {
            let name = tool["name"].as_str().context("工具缺少名称")?.to_string();
            tools.insert(
                name.clone(),
                McpToolView {
                    writes: super::writes(&name, tool.get("annotations")),
                    name,
                    description: tool["description"].as_str().unwrap_or_default().into(),
                },
            );
        }
        let next = response["result"]
            .get("nextCursor")
            .cloned()
            .unwrap_or_default();
        if next.is_null() || next == "" {
            return Ok(tools.into_values().collect());
        }
        if next == cursor {
            bail!("工具分页游标没有前进");
        }
        cursor = next;
    }
    bail!("工具列表页数超过上限")
}

/// 只跟随同源跳转（最多 5 次）：常见服务把 `/mcp` 跳到 `/mcp/`；跨站跳转可能把请求头带到别处，不跟。
fn same_origin_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let same = attempt
            .previous()
            .first()
            .is_some_and(|first| first.origin() == attempt.url().origin());
        if same && attempt.previous().len() <= 5 {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

pub async fn run(db_path: &Path, data_dir: &Path, input: McpInput) -> McpProbeView {
    let start = Instant::now();
    let diagnostics: Diagnostics = Arc::default();
    let prepare_db = db_path.to_path_buf();
    let prepare_data = data_dir.to_path_buf();
    let prepare_input = input.clone();
    let (prepared, secrets, oauth_token) = crate::execution::blocking(move || {
        let mut secrets = Vec::new();
        let mut oauth_token = None;
        let prepared: Result<_> = (|| {
            let conn = rrss_core::db::open_at(&prepare_db)?;
            let server = super::server(&conn, &prepare_input)?;
            if server.transport == "sse" {
                bail!("旧式 SSE 已停止支持");
            }
            if server.auth == "oauth" {
                if pi_store::use_own_config(&conn)? {
                    bail!("自有 Pi 配置模式请在自己的 Pi 中登录和检测 OAuth 服务");
                }
                let token = super::login::probe_token(&prepare_data, &server)?;
                secrets.push(token.clone());
                oauth_token = Some(token);
            }
            secrets.extend(super::values(server.env_json.as_deref()).into_values());
            secrets.extend(super::values(server.headers_json.as_deref()).into_values());
            let proxy =
                crate::pi::launch::pi_proxy_env(&conn).map_err(|e| anyhow::anyhow!(e.message))?;
            secrets.extend(proxy.secrets());
            Ok((server, proxy, network::load(&conn)?))
        })();
        (prepared, secrets, oauth_token)
    })
    .await
    .unwrap_or_else(|error| (Err(error), vec![], None));
    let result = async {
        let (server, proxy, config) = prepared?;
        let paths = crate::pi::workspace::WorkspacePaths::new(data_dir);
        tokio::fs::create_dir_all(&paths.project).await?;
        let mut transport = if server.transport == "stdio" {
            let diagnostics = diagnostics.clone();
            Transport::Local(
                crate::execution::blocking(move || {
                    Local::start(&server, &paths.project, &proxy, diagnostics)
                })
                .await??,
            )
        } else {
            let mut headers: reqwest::header::HeaderMap =
                super::values(server.headers_json.as_deref())
                    .into_iter()
                    .map(|(k, v)| {
                        Ok((reqwest::header::HeaderName::from_bytes(k.as_bytes())?, {
                            let mut header = reqwest::header::HeaderValue::from_str(&v)?;
                            header.set_sensitive(true);
                            header
                        }))
                    })
                    .collect::<Result<_>>()?;
            if let Some(token) = oauth_token {
                let mut header =
                    reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))?;
                header.set_sensitive(true);
                headers.insert(reqwest::header::AUTHORIZATION, header);
            }
            Transport::Remote(Remote {
                client: crate::execution::blocking(move || {
                    config.build_client(
                        NetworkScope::Pi,
                        reqwest::Client::builder()
                            .redirect(same_origin_redirects())
                            .timeout(TIMEOUT),
                    )
                })
                .await??,
                url: server.url.unwrap_or_default(),
                headers,
                session: None,
                version: None,
                diagnostics: diagnostics.clone(),
            })
        };
        let result = tokio::time::timeout(TIMEOUT, handshake(&mut transport))
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("连接检测超过 20 秒")));
        transport.close().await;
        result
    }
    .await;
    let clean = |text: &str| {
        rrss_core::logsafe::scrub_secrets_with(text, secrets.iter().map(String::as_str))
    };
    let mut raw_output = diagnostics.lock().unwrap().text();
    if let Err(error) = &result {
        let causes: Vec<String> = error
            .chain()
            .skip(1)
            .map(|cause| cause.to_string())
            .collect();
        if !causes.is_empty() {
            raw_output = format!("{raw_output}\n{}", causes.join("\n"));
        }
    }
    let output = clean(raw_output.trim())
        .lines()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    let (ok, tools, error, hint) = match result {
        Ok(mut tools) => {
            for tool in &mut tools {
                tool.name = clean(&tool.name);
                tool.description = clean(&tool.description);
            }
            (true, tools, String::new(), String::new())
        }
        Err(error) => {
            let error = clean(&error.to_string());
            let hint =
                if input.transport == "sse" || input.url.trim_end_matches('/').ends_with("/sse") {
                    "地址看起来是旧式 SSE；请向服务提供方获取 Streamable HTTP 的 /mcp 地址"
                } else if input.auth == "oauth" || error.contains("401") || error.contains("403") {
                    "请检查请求头令牌，或先完成浏览器登录"
                } else if input.transport == "stdio" {
                    "请确认命令已安装、参数正确，并补齐所需的环境变量后重试"
                } else {
                    "请检查 Streamable HTTP 地址、请求头与 Pi 用途的代理设置后重试"
                };
            (false, vec![], error, hint.into())
        }
    };
    McpProbeView {
        ok,
        elapsed_ms: start.elapsed().as_millis() as i64,
        tools,
        error,
        hint,
        output,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::McpSecretInput;
    fn draft() -> McpInput {
        McpInput {
            id: 0,
            name: "测试服务".into(),
            transport: "stdio".into(),
            command: "python3".into(),
            args: vec![
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../testdata/pi/mcp_stdio.py")
                    .display()
                    .to_string(),
            ],
            env: vec![McpSecretInput {
                name: "RRSS_FIXTURE_OLD_PROTOCOL".into(),
                value: "1".into(),
            }],
            url: String::new(),
            headers: vec![],
            auth: "none".into(),
            include_tools: vec![],
        }
    }

    #[tokio::test]
    async fn stderr_chunks_are_joined_before_cleaning_and_overflow_exposes_no_fragments() {
        let secret = "prefix-private-suffix-canary";
        let diagnostics: Diagnostics = Arc::default();
        // 密钥横跨底层读取块，仍按完整值清洗。
        let raw = format!("{}{secret}\nnext diagnostic", "x".repeat(2040));
        stderr_diagnostics(raw.as_bytes(), diagnostics.clone()).await;
        let clean =
            rrss_core::logsafe::scrub_secrets_with(&diagnostics.lock().unwrap().text(), [secret]);
        assert!(!clean.contains("private") && !clean.contains("suffix-canary"));
        assert!(clean.ends_with("\nnext diagnostic"));

        // 旧的字节尾窗口会只留下 suffix-canary，已知值匹配无法再覆盖它。
        let diagnostics: Diagnostics = Arc::default();
        let raw = format!("{secret}{}", "x".repeat(MAX_OUTPUT - "suffix-canary".len()));
        stderr_diagnostics(raw.as_bytes(), diagnostics.clone()).await;
        append(&diagnostics, b"more-private-output");
        let shown = diagnostics.lock().unwrap().text();
        assert!(shown.contains("已省略"));
        assert!(!shown.contains("private") && !shown.contains("suffix-canary"));
    }

    #[tokio::test]
    async fn local_probe_falls_back_without_saving_and_masks_saved_secret() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let conn = rrss_core::db::open_at(&db).unwrap();
        let mut input = draft();
        let result = run(&db, dir.path(), input.clone()).await;
        assert!(result.ok, "{} {}", result.error, result.output);
        assert_eq!(result.tools.len(), 3);
        assert!(
            !result
                .tools
                .iter()
                .find(|t| t.name == "read_file")
                .unwrap()
                .writes
        );
        assert!(
            result
                .tools
                .iter()
                .find(|t| t.name == "write-note")
                .unwrap()
                .writes
        );
        assert!(pi_store::list_mcp_servers(&conn).unwrap().is_empty());
        input.env.push(McpSecretInput {
            name: "TEST_SECRET".into(),
            value: "fixture-private-value".into(),
        });
        input.id = super::super::save(&conn, &input).unwrap();
        input.env[1].value.clear();
        input.args.push("--fail".into());
        let result = run(&db, dir.path(), input).await;
        assert!(!result.ok);
        // stdout EOF 与进程退出通知的先后由系统调度决定；两者都必须失败并清洗诊断。
        assert!(!format!("{result:?}").contains("fixture-private-value"));
        assert!(result.output.contains("***"), "{result:?}");
        assert!(!pi_store::list_mcp_servers(&conn).unwrap()[0].enabled);
    }
    #[tokio::test]
    async fn streamable_http_lists_tools_and_closes_session() {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let _conn = rrss_core::db::open_at(&db).unwrap();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pi/mcp_http.py");
        let mut server = tokio::process::Command::new("python3")
            .arg(script)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(server.stdout.take().unwrap())
            .read_line(&mut line)
            .await
            .unwrap();
        let mut input = draft();
        input.transport = "http".into();
        input.url = format!("{}/mcp", line.trim());
        input.env.clear();
        let result = run(&db, dir.path(), input).await;
        let _ = server.kill().await;
        let _ = server.wait().await;
        assert!(result.ok, "{result:?}");
        assert_eq!(result.tools.len(), 3);
    }
    #[tokio::test]
    async fn http_uses_pi_scope_proxy_and_never_returns_credentials() {
        use wiremock::matchers::{body_partial_json, method};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let conn = rrss_core::db::open_at(&db).unwrap();
        let proxy = MockServer::start().await;
        let mut config = network::ProxyConfig::default();
        config.pi = network::ProxyRule::custom(proxy.uri());
        network::save(&conn, &config).unwrap();
        Mock::given(method("POST")).and(body_partial_json(json!({"method":"initialize"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"fixture","version":"1"}}})))
            .mount(&proxy).await;
        Mock::given(method("POST"))
            .and(body_partial_json(
                json!({"method":"notifications/initialized"}),
            ))
            .respond_with(ResponseTemplate::new(202))
            .mount(&proxy)
            .await;
        Mock::given(method("POST"))
            .and(body_partial_json(json!({"method":"tools/list"})))
            .respond_with(ResponseTemplate::new(403).set_body_json(json!({
                "detail": "invalid credential fixture-private-\"header",
                "accessToken": "unsaved-canary",
            })))
            .mount(&proxy)
            .await;
        let mut input = draft();
        input.transport = "http".into();
        input.url = "http://mcp-target.invalid/mcp".into();
        input.env.clear();
        input.headers = vec![McpSecretInput {
            name: "X-Key".into(),
            value: "fixture-private-\"header".into(),
        }];
        let result = run(&db, dir.path(), input).await;
        assert!(!result.ok && result.error.contains("403"), "{result:?}");
        assert!(result.output.contains("***"));
        let shown = serde_json::to_string(&result).unwrap();
        assert!(!shown.contains("fixture-private-") && !shown.contains("unsaved-canary"));
        let requests = proxy.received_requests().await.unwrap();
        assert!(
            requests
                .iter()
                .any(|request| request.headers.get("x-key").is_some())
        );
    }

    #[tokio::test]
    async fn http_follows_a_same_origin_redirect_but_not_a_cross_origin_one() {
        use wiremock::matchers::{body_partial_json, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let _conn = rrss_core::db::open_at(&db).unwrap();
        let server = MockServer::start().await;
        let other = MockServer::start().await;
        // 常见的 Python 服务把 `/mcp` 跳到 `/mcp/`。
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(307).insert_header("Location", "/mcp/"))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/away"))
            .respond_with(
                ResponseTemplate::new(307)
                    .insert_header("Location", format!("{}/mcp/", other.uri()).as_str()),
            )
            .mount(&server)
            .await;
        for target in [&server, &other] {
            Mock::given(method("POST")).and(path("/mcp/")).and(body_partial_json(json!({"method":"initialize"})))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2026-07-28","capabilities":{},"serverInfo":{"name":"fixture","version":"1"}}})))
                .mount(target).await;
            Mock::given(method("POST"))
                .and(path("/mcp/"))
                .and(body_partial_json(
                    json!({"method":"notifications/initialized"}),
                ))
                .respond_with(ResponseTemplate::new(202))
                .mount(target)
                .await;
            Mock::given(method("POST"))
                .and(path("/mcp/"))
                .and(body_partial_json(json!({"method":"tools/list"})))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    json!({"jsonrpc":"2.0","id":100,"result":{"tools":[{"name":"read_file"}]}}),
                ))
                .mount(target)
                .await;
        }
        let mut input = draft();
        input.transport = "http".into();
        input.env.clear();
        input.url = format!("{}/mcp", server.uri());
        let result = run(&db, dir.path(), input.clone()).await;
        assert!(result.ok, "{result:?}");
        assert_eq!(result.tools.len(), 1);
        input.url = format!("{}/away", server.uri());
        let result = run(&db, dir.path(), input).await;
        assert!(!result.ok && result.error.contains("307"), "{result:?}");
        assert!(other.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn probe_timeout_closes_child() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let _conn = rrss_core::db::open_at(&db).unwrap();
        let mut input = draft();
        input.args.push("--hang".into());
        let result = run(&db, dir.path(), input).await;
        assert!(!result.ok && result.error.contains("20 秒"), "{result:?}");
    }
}
