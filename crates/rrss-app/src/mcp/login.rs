//! Pi owns OAuth credentials and refreshes. Views observe presence only; probes may borrow an access token.
use crate::{
    McpLoginResult,
    pi::{
        launch,
        workspace::{self, WorkspacePaths},
    },
};
use anyhow::{Context, Result, bail};
use rrss_core::{
    db,
    pi_store::{self, McpServer},
    platform,
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{io::AsyncReadExt, sync::watch};

type Key = (PathBuf, i64);
struct ActiveLogin {
    cancel: watch::Sender<bool>,
    done: watch::Receiver<bool>,
    deleting: bool,
}
static ACTIVE: OnceLock<Mutex<HashMap<Key, ActiveLogin>>> = OnceLock::new();
fn active() -> &'static Mutex<HashMap<Key, ActiveLogin>> {
    ACTIVE.get_or_init(Mutex::default)
}
struct LoginGuard {
    key: Key,
    dir: PathBuf,
    done: watch::Sender<bool>,
}
impl Drop for LoginGuard {
    fn drop(&mut self) {
        let _ = self.done.send(true);
        let mut jobs = active().lock().unwrap();
        if jobs.get(&self.key).is_some_and(|job| !job.deleting) {
            jobs.remove(&self.key);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
#[derive(Deserialize)]
struct TokenPresence {
    access_token: Option<serde::de::IgnoredAny>,
    refresh_token: Option<serde::de::IgnoredAny>,
}
#[derive(Deserialize)]
struct AuthPresence {
    tokens: Option<TokenPresence>,
}

/// Streaming deserialization skips token values; no credential is returned or logged.
pub fn logged_in(data_dir: &Path, server: &McpServer) -> bool {
    if server.auth != "oauth" || server.transport != "http" {
        return false;
    }
    let Some(url) = server
        .url
        .as_deref()
        .and_then(|v| reqwest::Url::parse(v).ok())
    else {
        return false;
    };
    let path = WorkspacePaths::new(data_dir).agent.join("mcp-auth.json");
    let states: HashMap<String, AuthPresence> = match std::fs::File::open(path)
        .ok()
        .and_then(|file| serde_json::from_reader(std::io::BufReader::new(file)).ok())
    {
        Some(value) => value,
        None => return false,
    };
    let key = format!("mcp__{}|{url}", pi_store::mcp_name(server.id));
    states
        .get(&key)
        .or_else(|| states.get(url.as_str()))
        .and_then(|state| state.tokens.as_ref())
        .is_some_and(|tokens| tokens.access_token.is_some() || tokens.refresh_token.is_some())
}

pub fn cancel_login(data_dir: &Path, id: i64) {
    if let Some(job) = active().lock().unwrap().get(&(data_dir.to_path_buf(), id)) {
        let _ = job.cancel.send(true);
    }
}
async fn drain(mut reader: impl tokio::io::AsyncRead + Unpin) {
    // Authorization URLs and CLI output are never returned or logged.
    let mut buf = [0; 4096];
    while let Ok(n) = reader.read(&mut buf).await {
        if n == 0 {
            break;
        }
    }
}

async fn command(
    db_path: &Path,
    data_dir: &Path,
    server: McpServer,
    operation: &str,
    cancel: &mut watch::Receiver<bool>,
    activity: impl Send + 'static,
) -> Result<bool> {
    let db_path = db_path.to_path_buf();
    let data_dir = data_dir.to_path_buf();
    let project = WorkspacePaths::new(&data_dir)
        .root
        .join(format!("mcp-{operation}-{}", server.id));
    let prepare_project = project.clone();
    let (paths, binary, proxy) = crate::execution::blocking(move || -> Result<_> {
        // A dropped caller cannot release login/deletion ownership while preparation
        // is still writing the shared Pi configuration or this operation's directory.
        let _activity = activity;
        let project = prepare_project;
        let paths = WorkspacePaths::new(&data_dir);
        let (binary, proxy, config) = {
            let conn = db::open_at(&db_path)?;
            let explicit = db::get_setting(&conn, rrss_core::ai_store::PI_BINARY_KEY)?;
            (
                launch::find_pi(explicit.as_deref()).context("未找到对话引擎，请先安装 Pi")?,
                launch::pi_proxy_env(&conn).map_err(|e| anyhow::anyhow!(e.message))?,
                pi_store::mcp_config(&conn)?.0,
            )
        };
        std::fs::create_dir_all(&project)?;
        workspace::write_files(
            &paths,
            &[workspace::GeneratedFile::new(
                "agent/mcp.json",
                serde_json::to_vec(&config)?,
            )],
        )?;
        Ok((paths, binary, proxy))
    })
    .await??;
    let mut cmd = platform::mcp_command(binary.to_str().context("Pi 路径无效")?, &project, None);
    super::apply_environment(&mut cmd, &proxy, &server)?;
    cmd.env("PI_CODING_AGENT_DIR", &paths.agent).args([
        "mcp",
        operation,
        &pi_store::mcp_name(server.id),
    ]);
    if operation == "login" {
        cmd.args(["--timeout", "300"]);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if *cancel.borrow() {
        bail!("已取消登录");
    }
    let mut child = cmd.spawn().context("无法启动 Pi 登录")?;
    let tree = platform::ChildTree::attach(&mut child)?;
    let out = tokio::spawn(drain(child.stdout.take().unwrap()));
    let err = tokio::spawn(drain(child.stderr.take().unwrap()));
    let result = tokio::select! {
        status = child.wait() => status.map(|s| s.success()).context("登录进程退出异常"),
        _ = cancel.changed() => Err(anyhow::anyhow!("已取消登录")),
        _ = tokio::time::sleep(Duration::from_secs(if operation == "login" { 300 } else { 15 })) => Err(anyhow::anyhow!("登录超时，请重试")),
    };
    drop(tree);
    let _ = child.kill().await;
    let _ = child.wait().await;
    out.abort();
    err.abort();
    result
}

pub async fn login(db_path: &Path, data_dir: &Path, id: i64) -> McpLoginResult {
    let (cancel, mut rx) = watch::channel(false);
    let (done, done_rx) = watch::channel(false);
    let key = (data_dir.to_path_buf(), id);
    {
        let mut jobs = active().lock().unwrap();
        if jobs.contains_key(&key) {
            return McpLoginResult {
                ok: false,
                message: "此服务正在登录或删除".into(),
            };
        }
        jobs.insert(
            key.clone(),
            ActiveLogin {
                cancel,
                done: done_rx,
                deleting: false,
            },
        );
    }
    let guard = Arc::new(LoginGuard {
        key,
        done,
        dir: WorkspacePaths::new(data_dir)
            .root
            .join(format!("mcp-login-{id}")),
    });
    let result = async {
        let server = crate::execution::with_db(db_path.to_path_buf(), move |conn| {
            if pi_store::use_own_config(conn)? {
                bail!("自有 Pi 配置模式请在自己的 Pi 中登录；RRSS 浏览器登录只用于托管模式");
            }
            Ok(pi_store::get_mcp_server(conn, id)?)
        })
        .await?;
        if server.transport != "http" || server.auth != "oauth" {
            bail!("浏览器登录只支持使用 OAuth 的 HTTP 服务");
        }
        if !command(
            db_path,
            data_dir,
            server.clone(),
            "login",
            &mut rx,
            guard.clone(),
        )
        .await?
        {
            bail!("登录失败，请检查服务地址、Pi 代理设置与浏览器授权结果");
        }
        let current = crate::execution::core_db(db_path.to_path_buf(), move |conn| {
            pi_store::get_mcp_server(conn, id)
        })
        .await?;
        if current.url != server.url || current.auth != server.auth {
            bail!("登录期间配置已改变，请重新登录");
        }
        let data_dir = data_dir.to_path_buf();
        if !crate::execution::blocking(move || logged_in(&data_dir, &server)).await? {
            bail!("服务未保存登录凭据，请检查是否需要浏览器登录");
        }
        Ok(())
    }
    .await;
    let _ = crate::execution::blocking(move || drop(guard)).await;
    match result {
        Ok(()) => McpLoginResult {
            ok: true,
            message: "已登录".into(),
        },
        Err(error) => McpLoginResult {
            ok: false,
            message: format!("{error}"),
        },
    }
}

pub async fn delete(db_path: &Path, data_dir: &Path, id: i64) -> rrss_core::Result<()> {
    let key = (data_dir.to_path_buf(), id);
    let mut pending = {
        let mut jobs = active().lock().unwrap();
        if let Some(job) = jobs.get_mut(&key) {
            if job.deleting {
                return Err(super::invalid("此服务正在删除"));
            }
            job.deleting = true;
            let _ = job.cancel.send(true);
            Some(job.done.clone())
        } else {
            let (cancel, _) = watch::channel(false);
            let (_, done) = watch::channel(true);
            jobs.insert(
                key.clone(),
                ActiveLogin {
                    cancel,
                    done,
                    deleting: true,
                },
            );
            None
        }
    };
    struct Deleting(Key);
    impl Drop for Deleting {
        fn drop(&mut self) {
            active().lock().unwrap().remove(&self.0);
        }
    }
    let deleting = Arc::new(Deleting(key));
    if let Some(done) = &mut pending {
        if !*done.borrow() {
            let _ = done.changed().await;
        }
    }
    let server = crate::execution::core_db(db_path.to_path_buf(), move |conn| {
        pi_store::get_mcp_server(conn, id)
    })
    .await?;
    if server.auth == "oauth" {
        let dir = WorkspacePaths::new(data_dir)
            .root
            .join(format!("mcp-logout-{id}"));
        let (_tx, mut rx) = watch::channel(false);
        let _ = command(
            db_path,
            data_dir,
            server,
            "logout",
            &mut rx,
            deleting.clone(),
        )
        .await;
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
    crate::execution::core_db(db_path.to_path_buf(), move |conn| {
        let _deleting = deleting;
        pi_store::delete_mcp_server(conn, id)
    })
    .await
}

/// Explicitly authorized for one-shot probing only; never part of a view or a log.
pub(super) fn probe_token(data_dir: &Path, server: &McpServer) -> Result<String> {
    #[derive(Deserialize)]
    struct Tokens {
        access_token: Option<String>,
    }
    #[derive(Deserialize)]
    struct State {
        tokens: Option<Tokens>,
        #[serde(rename = "tokensExpireAt")]
        expires: Option<i64>,
    }
    let url = reqwest::Url::parse(server.url.as_deref().unwrap_or_default())?;
    let file = std::fs::File::open(WorkspacePaths::new(data_dir).agent.join("mcp-auth.json"))
        .map_err(|_| anyhow::anyhow!("请先完成浏览器登录"))?;
    let mut states: HashMap<String, State> = serde_json::from_reader(std::io::BufReader::new(file))
        .map_err(|_| anyhow::anyhow!("登录凭据不可用，请重新登录"))?;
    let key = format!("mcp__{}|{url}", pi_store::mcp_name(server.id));
    let state = states
        .remove(&key)
        .or_else(|| states.remove(url.as_str()))
        .context("请先完成浏览器登录")?;
    if state
        .expires
        .is_some_and(|at| at <= rrss_core::feeds::now_ms())
    {
        bail!("登录已过期，请重新登录后检测");
    }
    state
        .tokens
        .and_then(|tokens| tokens.access_token)
        .filter(|value| !value.is_empty())
        .context("登录凭据不可用，请重新登录")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn oauth(conn: &rusqlite::Connection) -> McpServer {
        crate::mcp::import(
            conn,
            r#"{"test":{"url":"https://example.test/mcp","auth":"oauth"}}"#,
        )
        .unwrap();
        pi_store::list_mcp_servers(conn).unwrap().remove(0)
    }
    #[tokio::test]
    async fn credentials_are_scoped_to_server_url_and_own_mode_cannot_login() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db");
        let conn = db::open_at(&db).unwrap();
        let mut server = oauth(&conn);
        assert!(crate::mcp::toggle(&conn, dir.path(), server.id, true).is_err());
        let agent = WorkspacePaths::new(dir.path()).agent;
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(
            agent.join("mcp-auth.json"),
            r#"{"mcp__s1|https://example.test/mcp":{"tokens":{"access_token":"fixture"}}}"#,
        )
        .unwrap();
        assert!(logged_in(dir.path(), &server));
        crate::mcp::toggle(&conn, dir.path(), server.id, true).unwrap();
        server.url = Some("https://other.test/mcp".into());
        assert!(!logged_in(dir.path(), &server));
        pi_store::set_use_own_config(&conn, true).unwrap();
        let result = login(&db, dir.path(), server.id).await;
        assert!(!result.ok && result.message.contains("自有 Pi"));
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn cancellation_stops_login_and_deletion_waits_for_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("db");
        let conn = db::open_at(&db).unwrap();
        let server = oauth(&conn);
        let fake = dir.path().join("fake-pi");
        std::fs::write(&fake, "#!/usr/bin/python3\nimport pathlib,os,time,sys\np=pathlib.Path(os.environ['PI_CODING_AGENT_DIR'])\nif sys.argv[2]=='login':\n (p/'started').write_text(str(os.getpid()))\n time.sleep(120)\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        db::set_setting(
            &conn,
            rrss_core::ai_store::PI_BINARY_KEY,
            fake.to_str().unwrap(),
        )
        .unwrap();
        let data = dir.path().join("data");
        let db2 = db.clone();
        let data2 = data.clone();
        let id = server.id;
        let run = tokio::spawn(async move { login(&db2, &data2, id).await });
        let started = WorkspacePaths::new(&data).agent.join("started");
        // 假 Pi 先建文件再写入；文件存在不代表已写完，读到完整 pid 为止。
        let pid = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(pid) = std::fs::read_to_string(&started)
                    .ok()
                    .and_then(|text| text.trim().parse::<u32>().ok())
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(!login(&db, &data, id).await.ok);
        delete(&db, &data, id).await.unwrap();
        let result = run.await.unwrap();
        assert!(!result.ok && result.message.contains("取消"));
        assert_eq!(platform::process_alive(pid), Some(false));
        assert!(pi_store::list_mcp_servers(&conn).unwrap().is_empty());
        assert!(active().lock().unwrap().get(&(data, id)).is_none());
    }
}
