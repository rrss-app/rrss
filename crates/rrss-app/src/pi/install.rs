//! 下载并维护 GitHub 发布的 Pi 独立二进制。安装 `pi/compat.json` 的 `tested` 精确版本；
//! 打开自动更新时只在兼容线内升级 patch。

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use rrss_core::ai_store::PI_BINARY_KEY;
use rrss_core::db;
use rrss_core::network::{self, NetworkScope, Redacted};
use rrss_core::pi_store;
use rrss_core::platform::{self, Arch, Os};
use sha2::{Digest, Sha256};
use tar::Archive;

use super::adapter::PiVersion;
use super::compat::compat;
use super::launch::{is_standalone_binary, pi_bin_names, pi_pkg_dir};
use crate::ui_error::pi::{PiDownloadStage, PiUiError};
use crate::views::{PiInstallView, UiError};
use rrss_core::{Error, error::PiFailure};

const REPO: &str = "earendil-works/pi";

/// 生产固定使用 GitHub；本地测试替换源地址，不访问真实发布服务。
struct ReleaseSource<'a> {
    api: &'a str,
    downloads: &'a str,
}

const GITHUB: ReleaseSource<'static> = ReleaseSource {
    api: "https://api.github.com",
    downloads: "https://github.com",
};

/// 同一时刻只有一个安装在跑：启动预载、对话前补装和设置页手动下载共用暂存目录，不能交叠。
static INSTALL_LOCK: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
static STATUS: LazyLock<Mutex<PiInstallView>> = LazyLock::new(|| {
    Mutex::new(PiInstallView {
        state: "idle".into(),
        ..Default::default()
    })
});
static NOTIFY: OnceLock<Box<dyn Fn(&PiInstallView) + Send + Sync>> = OnceLock::new();
/// 下载进度至少隔这么多字节才推一次，避免每个分块都发事件。
const PROGRESS_STEP: u64 = 256 * 1024;

/// 宿主注册一次：进度变化时推给前端。
pub fn set_install_notifier(notify: impl Fn(&PiInstallView) + Send + Sync + 'static) {
    let _ = NOTIFY.set(Box::new(notify));
}

/// 当前（或最近一次）安装进度，附上托管目录。
pub fn install_status(data_dir: &Path) -> PiInstallView {
    let mut view = STATUS.lock().map(|s| s.clone()).unwrap_or_default();
    view.dir = pi_pkg_dir(data_dir).display().to_string();
    view
}

/// 手动下载：先把状态置为下载中，界面立刻有反馈；已有安装在跑时返回 false。
pub fn begin_install(data_dir: &Path) -> bool {
    if install_running() {
        return false;
    }
    report(data_dir, |s| {
        *s = PiInstallView {
            state: "downloading".into(),
            ..Default::default()
        };
    });
    true
}

/// 正在下载或安装时不再起第二个；返回是否已有安装在跑。
pub fn install_running() -> bool {
    STATUS
        .lock()
        .is_ok_and(|s| matches!(s.state.as_str(), "downloading" | "installing"))
}

fn report(data_dir: &Path, update: impl FnOnce(&mut PiInstallView)) {
    let view = {
        let Ok(mut status) = STATUS.lock() else {
            return;
        };
        update(&mut status);
        status.dir = pi_pkg_dir(data_dir).display().to_string();
        status.clone()
    };
    if let Some(notify) = NOTIFY.get() {
        notify(&view);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureReport {
    UpToDate { version: String },
    Downloaded { version: String },
    Updated { from: String, to: String },
    SkippedCustomPath,
    MissingDisabled,
}

pub fn managed_bin_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("bin")
}

pub fn managed_executable(data_dir: &Path) -> PathBuf {
    let name = pi_bin_names()[0];
    // 不建链接的平台（Windows）直接用包目录里的程序，见 `platform::Os::links_managed_binary`。
    if Os::current().links_managed_binary() {
        managed_bin_dir(data_dir).join(name)
    } else {
        pi_pkg_dir(data_dir).join(name)
    }
}

pub fn uses_custom_pi_binary(conn: &rusqlite::Connection, data_dir: &Path) -> bool {
    let Ok(Some(raw)) = db::get_setting(conn, PI_BINARY_KEY) else {
        return false;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return false;
    }
    let path = PathBuf::from(trimmed);
    if !path.exists() {
        return false;
    }
    !is_managed_binary_path(data_dir, &path)
}

pub fn is_managed_binary_path(data_dir: &Path, path: &Path) -> bool {
    let Ok(canonical) = path.canonicalize() else {
        return false;
    };
    for name in pi_bin_names() {
        let link = managed_bin_dir(data_dir).join(name);
        if link.exists() {
            if let Ok(c) = link.canonicalize()
                && c == canonical
            {
                return true;
            }
        }
        let exe = pi_pkg_dir(data_dir).join(name);
        if exe.exists() {
            if let Ok(c) = exe.canonicalize()
                && c == canonical
            {
                return true;
            }
        }
    }
    if let Ok(pkg) = pi_pkg_dir(data_dir).canonicalize() {
        return canonical.parent().is_some_and(|p| p == pkg);
    }
    false
}

pub async fn ensure_pi_binary(
    db_path: &Path,
    data_dir: &Path,
    force: bool,
) -> Result<EnsureReport> {
    ensure_from(db_path, data_dir, force, &GITHUB).await
}

async fn ensure_from(
    db_path: &Path,
    data_dir: &Path,
    force: bool,
    source: &ReleaseSource<'_>,
) -> Result<EnsureReport> {
    let _guard = INSTALL_LOCK.lock().await;
    let result = ensure_locked(db_path, data_dir, force, source)
        .await
        .map_err(|error| {
            let ui = PiUiError::project_ref(&error);
            error.context(ui)
        });
    // 手动下载总要给出结论；自动检查只在真的下载过时才更新进度。
    match &result {
        Ok(r)
            if force
                || matches!(
                    r,
                    EnsureReport::Downloaded { .. } | EnsureReport::Updated { .. }
                ) =>
        {
            report_done(data_dir, format_report(r));
        }
        Ok(_) => {}
        Err(err) if force || install_running() => {
            let error = err
                .downcast_ref::<UiError>()
                .cloned()
                .expect("install error projected above");
            report(data_dir, |s| {
                s.state = "failed".into();
                s.message.clear();
                s.notice = None;
                s.error = Some(error);
            });
        }
        Err(_) => {}
    }
    result
}

fn report_done(data_dir: &Path, notice: UiError) {
    report(data_dir, |s| {
        s.state = "done".into();
        s.message.clear();
        s.notice = Some(notice);
        s.error = None;
    });
}

async fn ensure_locked(
    db_path: &Path,
    data_dir: &Path,
    force: bool,
    source: &ReleaseSource<'_>,
) -> Result<EnsureReport> {
    let conn = db::open_at(db_path)?;
    if uses_custom_pi_binary(&conn, data_dir) {
        return Ok(EnsureReport::SkippedCustomPath);
    }

    let auto_download = pi_store::pi_auto_download(&conn)?;
    let auto_update = pi_store::pi_auto_update(&conn)?;

    let managed = managed_executable(data_dir);
    let has_managed = managed.is_file() && is_standalone_binary(&managed);
    let tested = compat().tested;

    if !has_managed {
        if !auto_download && !force {
            return Ok(EnsureReport::MissingDisabled);
        }
        let client = install_client(&conn)?;
        let version = install_version(data_dir, &client, tested, source).await?;
        pi_store::set_pi_managed_version(&conn, &version)?;
        return Ok(EnsureReport::Downloaded { version });
    }

    let current = read_pi_version(&managed)
        .ok()
        .and_then(|raw| PiVersion::parse(&raw));
    let current_label = current.map(|v| v.to_string()).unwrap_or_default();

    // 比 tested 旧（例如兼容线切换前装的版本）：补装 tested，否则版本闸会拒绝启动。
    if current.is_none_or(|v| v < tested) {
        if !auto_download && !force {
            return Ok(EnsureReport::UpToDate {
                version: current_label,
            });
        }
        let client = install_client(&conn)?;
        let version = install_version(data_dir, &client, tested, source).await?;
        pi_store::set_pi_managed_version(&conn, &version)?;
        return Ok(EnsureReport::Updated {
            from: current_label,
            to: version,
        });
    }

    if !auto_update && !force {
        return Ok(EnsureReport::UpToDate {
            version: current_label,
        });
    }
    let client = install_client(&conn)?;
    let latest = match fetch_latest_in_line(&client, source).await {
        Ok(latest) => latest,
        Err(err) => {
            tracing::warn!(error = %format!("{err:#}"), "check pi patch releases");
            return Ok(EnsureReport::UpToDate {
                version: current_label,
            });
        }
    };
    if current.is_some_and(|c| latest <= c) {
        return Ok(EnsureReport::UpToDate {
            version: current_label,
        });
    }
    let version = install_version(data_dir, &client, latest, source).await?;
    pi_store::set_pi_managed_version(&conn, &version)?;
    Ok(EnsureReport::Updated {
        from: current_label,
        to: version,
    })
}

/// 只在确实需要联网时读取配置，每次安装 / 更新任务独立持有一份客户端快照。
fn install_client(conn: &rusqlite::Connection) -> Result<reqwest::Client> {
    network::load(conn)?
        .build_client(
            NetworkScope::Pi,
            reqwest::Client::builder().user_agent("rrss-pi-install"),
        )
        .context("http client")
}

/// 安装指定版本并返回装好后读出的版本号。
async fn install_version(
    data_dir: &Path,
    client: &reqwest::Client,
    version: PiVersion,
    source: &ReleaseSource<'_>,
) -> Result<String> {
    install_release(data_dir, client, &format!("v{version}"), source).await?;
    read_pi_version(&managed_executable(data_dir))
}

pub fn format_report(report: &EnsureReport) -> UiError {
    use crate::ui_error::params;
    let kind = match report {
        EnsureReport::UpToDate { version } if version.is_empty() => PiUiError::Ready,
        EnsureReport::UpToDate { version } => PiUiError::UpToDate {
            version: params::name(version),
        },
        EnsureReport::Downloaded { version } => PiUiError::Downloaded {
            version: params::name(version),
        },
        EnsureReport::Updated { from, to } => PiUiError::Updated {
            from: params::name(from),
            to: params::name(to),
        },
        EnsureReport::SkippedCustomPath => PiUiError::CustomUnmanaged,
        EnsureReport::MissingDisabled => PiUiError::MissingDisabled,
    };
    kind.failure("pi installation completed")
}

/// 兼容线内最新的正式版本（只在自动更新打开时查询 GitHub API）。
async fn fetch_latest_in_line(
    client: &reqwest::Client,
    source: &ReleaseSource<'_>,
) -> Result<PiVersion> {
    let url = format!("{}/repos/{REPO}/releases?per_page=30", source.api);
    let releases: Vec<serde_json::Value> = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .redacted()
        .map_err(|error| PiUiError::download(PiDownloadStage::ReleasesFetch, error))?
        .error_for_status()
        .redacted()
        .map_err(|error| PiUiError::download(PiDownloadStage::ReleasesStatus, error))?
        .json()
        .await
        .redacted()
        .map_err(|error| PiUiError::download(PiDownloadStage::ReleasesParse, error))?;
    latest_in_line(
        releases
            .iter()
            .filter_map(|rel| rel.get("tag_name").and_then(|v| v.as_str())),
    )
    .ok_or_else(|| {
        Error::Pi(PiFailure::ReleaseMissing {
            line: compat().line_label(),
        })
        .into()
    })
}

/// 从 tag 列表里挑兼容线内的最高版本；预发布 tag（带 `-`）不算。
fn latest_in_line<'a>(tags: impl Iterator<Item = &'a str>) -> Option<PiVersion> {
    tags.filter(|tag| !tag.contains('-'))
        .filter_map(|tag| PiVersion::parse(tag.trim_start_matches('v')))
        .filter(|v| compat().in_line(v))
        .max()
}

async fn install_release(
    data_dir: &Path,
    client: &reqwest::Client,
    tag: &str,
    source: &ReleaseSource<'_>,
) -> Result<()> {
    let (asset, exe_name) = platform_asset()?;
    let base = format!(
        "{}/{REPO}/releases/download/{tag}/{asset}",
        source.downloads
    );
    tracing::info!(tag, asset, "downloading managed pi");

    report(data_dir, |s| {
        *s = PiInstallView {
            state: "downloading".into(),
            ..Default::default()
        };
    });
    let mut response = client
        .get(&base)
        .send()
        .await
        .redacted()
        .map_err(|error| PiUiError::download(PiDownloadStage::AssetFetch, error))?
        .error_for_status()
        .redacted()
        .map_err(|error| PiUiError::download(PiDownloadStage::AssetStatus, error))?;
    let total = response.content_length();
    let mut bytes = Vec::with_capacity(total.unwrap_or(0) as usize);
    let mut reported = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .redacted()
        .map_err(|error| PiUiError::download(PiDownloadStage::AssetRead, error))?
    {
        bytes.extend_from_slice(&chunk);
        let received = bytes.len() as u64;
        if received - reported >= PROGRESS_STEP || Some(received) == total {
            reported = received;
            report(data_dir, |s| {
                s.received = received as i64;
                s.total = total.map(|t| t as i64);
            });
        }
    }
    report(data_dir, |s| {
        s.state = "installing".into();
        s.received = bytes.len() as i64;
    });

    verify_sha256(client, tag, asset, &bytes, source).await?;

    let staging = data_dir.join("pi-pkg.staging");
    if staging.exists() {
        fs::remove_dir_all(&staging)
            .map_err(|error| PiUiError::StagingCleanup.failure(error.to_string()))?;
    }
    fs::create_dir_all(&staging)
        .map_err(|error| PiUiError::StagingCreate.failure(error.to_string()))?;
    // 失败时不把上百 MB 的半成品留在数据目录里。
    let installed = unpack_and_promote(&bytes, asset, exe_name, &staging, data_dir);
    if installed.is_err() {
        fs::remove_dir_all(&staging).ok();
    }
    installed?;
    link_managed_executable(data_dir)?;
    Ok(())
}

fn unpack_and_promote(
    bytes: &[u8],
    asset: &str,
    exe_name: &str,
    staging: &Path,
    data_dir: &Path,
) -> Result<()> {
    if asset.ends_with(".tar.gz") {
        extract_tar_gz(bytes, staging)?;
    } else if asset.ends_with(".zip") {
        extract_zip(bytes, staging)?;
    } else {
        bail!(Error::Pi(PiFailure::PackageUnsupported {
            file: asset.to_string()
        }));
    }
    let root = package_root(staging, exe_name).ok_or_else(|| {
        Error::Pi(PiFailure::PackageBinaryMissing {
            file: exe_name.to_string(),
        })
    })?;
    if !is_standalone_binary(&root.join(exe_name)) {
        bail!(Error::Pi(PiFailure::PackageNotStandalone {
            file: exe_name.to_string()
        }));
    }
    promote(&root, staging, data_dir)
}

/// 发布包里程序所在的目录：Linux / macOS 的 tar.gz 外层套一层 `pi/`，Windows 的 zip 直接平铺在根上。两种都认。
fn package_root(staging: &Path, exe_name: &str) -> Option<PathBuf> {
    [staging.join("pi"), staging.to_path_buf()]
        .into_iter()
        .find(|dir| dir.join(exe_name).is_file())
}

/// 把解压好的目录换成 `pi-pkg/`：旧版先挪到 `pi-pkg.old`，换上失败就挪回去，不留下没有程序的空档。
fn promote(root: &Path, staging: &Path, data_dir: &Path) -> Result<()> {
    let dest = pi_pkg_dir(data_dir);
    let backup = data_dir.join("pi-pkg.old");
    if backup.exists() {
        fs::remove_dir_all(&backup).ok();
    }
    if dest.exists() {
        fs::rename(&dest, &backup)
            .map_err(|error| PiUiError::VersionBackup.failure(error.to_string()))?;
    }
    if let Err(err) = fs::rename(root, &dest) {
        if backup.exists() {
            fs::rename(&backup, &dest).ok();
        }
        return Err(PiUiError::VersionPromote.failure(err.to_string()).into());
    }
    if root != staging {
        fs::remove_dir_all(staging).ok();
    }
    fs::remove_dir_all(&backup).ok();
    Ok(())
}

async fn verify_sha256(
    client: &reqwest::Client,
    tag: &str,
    asset: &str,
    bytes: &[u8],
    source: &ReleaseSource<'_>,
) -> Result<()> {
    let sums_url = format!(
        "{}/{REPO}/releases/download/{tag}/SHA256SUMS",
        source.downloads
    );
    let text = client
        .get(&sums_url)
        .send()
        .await
        .redacted()
        .map_err(|error| PiUiError::download(PiDownloadStage::ChecksumsFetch, error))?
        .error_for_status()
        .redacted()
        .map_err(|error| PiUiError::download(PiDownloadStage::ChecksumsStatus, error))?
        .text()
        .await
        .redacted()
        .map_err(|error| PiUiError::download(PiDownloadStage::ChecksumsRead, error))?;

    let mut expected = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((hash, name)) = line.split_once("  ").or_else(|| line.split_once(' ')) else {
            continue;
        };
        if name.trim() == asset {
            expected = Some(hash.trim().to_ascii_lowercase());
            break;
        }
    }
    let Some(expected) = expected else {
        bail!(Error::Pi(PiFailure::ChecksumMissing {
            file: asset.to_string()
        }));
    };

    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let got = format!("{:x}", hasher.finalize());
    if got != expected {
        bail!(PiUiError::checksum(&expected, &got));
    }
    Ok(())
}

fn extract_tar_gz(bytes: &[u8], dest: &Path) -> Result<()> {
    let gz = GzDecoder::new(bytes);
    let mut archive = Archive::new(gz);
    archive.unpack(dest).context("unpack tar.gz")
}

fn extract_zip(bytes: &[u8], dest: &Path) -> Result<()> {
    let reader = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader).context("open zip")?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).context("zip entry")?;
        let name = file.name().replace('\\', "/");
        if name.contains("..") {
            continue;
        }
        let out = dest.join(&name);
        if file.is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            if let Some(parent) = out.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out_file = File::create(&out).context("create extracted file")?;
            std::io::copy(&mut file, &mut out_file).context("write extracted file")?;
        }
    }
    Ok(())
}

/// 建链接的平台：`bin/pi` 指向包里的程序（解析回包目录，资源仍在旁边）。
/// 不建链接的平台：`managed_executable` 直接指向包里的程序，只清掉旧版本复制到 `bin` 的副本。
fn link_managed_executable(data_dir: &Path) -> Result<()> {
    let bin_dir = managed_bin_dir(data_dir);
    let entry = bin_dir.join(pi_bin_names()[0]);
    if !Os::current().links_managed_binary() {
        if entry.is_file() {
            fs::remove_file(&entry).ok();
        }
        return Ok(());
    }
    fs::create_dir_all(&bin_dir).context("create bin dir")?;
    if entry.exists() || entry.symlink_metadata().is_ok() {
        fs::remove_file(&entry).ok();
    }
    let program = pi_pkg_dir(data_dir).join(pi_bin_names()[0]);
    platform::symlink(&program, &entry).context("symlink managed pi")
}

fn read_pi_version(path: &Path) -> Result<String> {
    let output = platform::hide_console_window(std::process::Command::new(path).arg("--version"))
        .output()
        .with_context(|| format!("run {}", path.display()))?;
    if !output.status.success() {
        bail!("pi --version failed");
    }
    let v = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if v.is_empty() {
        bail!("empty pi version");
    }
    Ok(v)
}

/// 本平台的发布包名与其中程序的文件名（见 `platform::Os::pi_release`）。
fn platform_asset() -> Result<(&'static str, &'static str)> {
    let release = Os::current()
        .pi_release(Arch::current())
        .ok_or(Error::Pi(PiFailure::PlatformUnsupported))?;
    Ok((release.asset, release.program))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rrss_core::network::{ProxyConfig, ProxyRule};
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn install_fixture() -> (tempfile::TempDir, PathBuf, db::PooledConnection) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = db::open_at(&db_path).unwrap();
        pi_store::set_pi_auto_download(&conn, false).unwrap();
        pi_store::set_pi_auto_update(&conn, false).unwrap();
        (dir, db_path, conn)
    }

    fn proxy_rule(uri: &str) -> ProxyRule {
        ProxyRule::custom(uri.replacen("http://", "http://private-user:private-password@", 1))
    }

    fn assert_safe_error(error: &anyhow::Error) {
        let displayed = format!("{error:#} {error:?}");
        assert!(!displayed.contains("private-user"), "{displayed}");
        assert!(!displayed.contains("private-password"), "{displayed}");
        assert!(
            error.chain().all(|source| !source.is::<reqwest::Error>()),
            "raw reqwest source retained: {displayed}"
        );
    }

    #[tokio::test]
    async fn pi_proxy_routes_all_download_requests_and_reloads_next_task() {
        let (dir, db_path, conn) = install_fixture();
        let origin = crate::test_net::proxied_target().await;
        let other_scopes = MockServer::start().await;
        let first_proxy = MockServer::start().await;
        let second_proxy = MockServer::start().await;
        let origin_uri = origin.uri();
        let source = ReleaseSource {
            api: &origin_uri,
            downloads: &origin_uri,
        };
        let mut config = ProxyConfig {
            global: proxy_rule(&other_scopes.uri()),
            content: proxy_rule(&other_scopes.uri()),
            api: proxy_rule(&other_scopes.uri()),
            pi: proxy_rule(&first_proxy.uri()),
            ..ProxyConfig::default()
        };
        let (asset, _) = platform_asset().unwrap();
        let tag = format!("v{}", compat().tested);
        // 只下载本地合成字节：校验通过后在解压时终止，绝不安装或执行真实 Pi。
        let bytes = b"not a real Pi release archive";
        let sums = format!("{:x}  {asset}\n", Sha256::digest(bytes));
        let old = pi_pkg_dir(dir.path()).join("keep-old-package");
        fs::create_dir_all(old.parent().unwrap()).unwrap();
        fs::write(&old, b"unchanged").unwrap();

        for proxy in [&first_proxy, &second_proxy] {
            Mock::given(method("GET"))
                .and(path(format!("/repos/{REPO}/releases")))
                .and(header("user-agent", "rrss-pi-install"))
                .and(header("accept", "application/vnd.github+json"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                    {"tag_name": tag},
                    {"tag_name": "v9.0.0"}
                ])))
                .expect(1)
                .mount(proxy)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/{REPO}/releases/download/{tag}/{asset}")))
                .and(header("user-agent", "rrss-pi-install"))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.to_vec()))
                .expect(1)
                .mount(proxy)
                .await;
            Mock::given(method("GET"))
                .and(path(format!("/{REPO}/releases/download/{tag}/SHA256SUMS")))
                .and(header("user-agent", "rrss-pi-install"))
                .respond_with(ResponseTemplate::new(200).set_body_string(&sums))
                .expect(1)
                .mount(proxy)
                .await;
        }

        for proxy in [&first_proxy, &second_proxy] {
            config.pi = proxy_rule(&proxy.uri());
            network::save(&conn, &config).unwrap();
            let client = install_client(&conn).unwrap();
            assert_eq!(
                fetch_latest_in_line(&client, &source).await.unwrap(),
                compat().tested
            );
            let error = ensure_from(&db_path, dir.path(), true, &source)
                .await
                .unwrap_err();
            assert_safe_error(&error);
            assert!(
                format!("{error:#}").contains(if asset.ends_with(".zip") {
                    "open zip"
                } else {
                    "unpack tar.gz"
                }),
                "{error:#}"
            );
            assert_eq!(fs::read(&old).unwrap(), b"unchanged");
            assert!(!dir.path().join("pi-pkg.staging").exists());
            assert!(!managed_executable(dir.path()).exists());
            assert_eq!(proxy.received_requests().await.unwrap().len(), 3);
        }
        assert_eq!(first_proxy.received_requests().await.unwrap().len(), 3);
        assert!(other_scopes.received_requests().await.unwrap().is_empty());
        assert!(origin.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn skipped_installs_do_not_load_invalid_proxy_configuration() {
        for raw in [
            "not json",
            r#"{"version":999}"#,
            r#"{"pi":{"mode":"custom","url":"ftp://private-user:private-password@localhost"}}"#,
        ] {
            let (dir, db_path, conn) = install_fixture();
            db::set_setting(&conn, network::SETTING_KEY, raw).unwrap();
            assert_eq!(
                ensure_pi_binary(&db_path, dir.path(), false).await.unwrap(),
                EnsureReport::MissingDisabled
            );
            let custom = dir.path().join("custom-pi");
            fs::write(&custom, b"not executed").unwrap();
            db::set_setting(&conn, PI_BINARY_KEY, custom.to_str().unwrap()).unwrap();
            for force in [false, true] {
                assert_eq!(
                    ensure_pi_binary(&db_path, dir.path(), force).await.unwrap(),
                    EnsureReport::SkippedCustomPath
                );
            }
            db::set_setting(&conn, PI_BINARY_KEY, "").unwrap();
            // 需要联网时仍必须报坏配置，不能静默直连。
            let error = ensure_pi_binary(&db_path, dir.path(), true)
                .await
                .unwrap_err();
            assert_safe_error(&error);
            assert!(error.downcast_ref::<rrss_core::Error>().is_some());
            // 已有独立程序但无法读版本、且禁止下载时，同样不能被坏代理阻止。
            let managed = managed_executable(dir.path());
            fs::create_dir_all(managed.parent().unwrap()).unwrap();
            fs::write(&managed, b"\x7fELF").unwrap();
            assert_eq!(
                ensure_pi_binary(&db_path, dir.path(), false).await.unwrap(),
                EnsureReport::UpToDate {
                    version: String::new()
                }
            );
        }
    }

    #[tokio::test]
    async fn pi_proxy_http_and_json_errors_drop_reqwest_sources() {
        let _guard = INSTALL_LOCK.lock().await;
        let (dir, _, conn) = install_fixture();
        let origin = crate::test_net::proxied_target().await;
        let proxy = MockServer::start().await;
        network::save(
            &conn,
            &ProxyConfig {
                pi: proxy_rule(&proxy.uri()),
                ..ProxyConfig::default()
            },
        )
        .unwrap();
        let client = install_client(&conn).unwrap();
        let origin_uri = origin.uri();
        let source = ReleaseSource {
            api: &origin_uri,
            downloads: &origin_uri,
        };
        let (asset, _) = platform_asset().unwrap();
        let tag = format!("v{}", compat().tested);
        for stage in [
            "releases status",
            "releases json",
            "asset status",
            "sums status",
        ] {
            proxy.reset().await;
            Mock::given(method("GET"))
                .respond_with(if stage == "releases json" {
                    ResponseTemplate::new(200).set_body_string("private-password: invalid json")
                } else {
                    ResponseTemplate::new(407).set_body_string("private-user:private-password")
                })
                .expect(1)
                .mount(&proxy)
                .await;
            let error = match stage {
                "releases status" | "releases json" => {
                    fetch_latest_in_line(&client, &source).await.unwrap_err()
                }
                "asset status" => install_release(dir.path(), &client, &tag, &source)
                    .await
                    .unwrap_err(),
                _ => verify_sha256(&client, &tag, asset, b"", &source)
                    .await
                    .unwrap_err(),
            };
            assert_safe_error(&error);
            assert!(format!("{error:#}").contains(match stage {
                "releases status" => "github releases status",
                "releases json" => "parse releases json",
                "asset status" => "pi asset status",
                _ => "SHA256SUMS status",
            }));
            proxy.verify().await;
        }
        assert!(origin.received_requests().await.unwrap().is_empty());
        report_done(dir.path(), PiUiError::Ready.failure("ready"));
    }

    #[tokio::test]
    async fn pi_proxy_connection_errors_drop_reqwest_sources() {
        let _guard = INSTALL_LOCK.lock().await;
        let (dir, _, conn) = install_fixture();
        let origin = crate::test_net::proxied_target().await;
        let origin_uri = origin.uri();
        let source = ReleaseSource {
            api: &origin_uri,
            downloads: &origin_uri,
        };
        // 绑定而不监听的本地端口固定拒绝连接；保持 socket，避免被其他测试抢占。
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        network::save(
            &conn,
            &ProxyConfig {
                pi: proxy_rule(&format!("http://{}", socket.local_addr().unwrap())),
                ..ProxyConfig::default()
            },
        )
        .unwrap();
        let client = install_client(&conn).unwrap();
        let (asset, _) = platform_asset().unwrap();
        let tag = format!("v{}", compat().tested);
        for error in [
            fetch_latest_in_line(&client, &source).await.unwrap_err(),
            install_release(dir.path(), &client, &tag, &source)
                .await
                .unwrap_err(),
            verify_sha256(&client, &tag, asset, b"", &source)
                .await
                .unwrap_err(),
        ] {
            assert_safe_error(&error);
            let ui = PiUiError::project(error);
            assert_eq!(ui.code(), "pi_download_failed");
            assert_eq!(
                serde_json::to_value(ui).unwrap()["params"]["message"],
                "connect"
            );
        }
        report_done(dir.path(), PiUiError::Ready.failure("ready"));
    }

    #[tokio::test]
    async fn pi_proxy_truncated_bodies_drop_reqwest_sources() {
        let _guard = INSTALL_LOCK.lock().await;
        let (dir, _, conn) = install_fixture();
        let origin = crate::test_net::proxied_target().await;
        let origin_uri = origin.uri();
        let source = ReleaseSource {
            api: &origin_uri,
            downloads: &origin_uri,
        };
        let (asset, _) = platform_asset().unwrap();
        let tag = format!("v{}", compat().tested);
        for stage in ["asset", "sums"] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            network::save(
                &conn,
                &ProxyConfig {
                    pi: proxy_rule(&format!("http://{}", listener.local_addr().unwrap())),
                    ..ProxyConfig::default()
                },
            )
            .unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                crate::test_net::read_request_head(&mut stream).await;
                // 故意截断 body，分别覆盖 Response::chunk 与 Response::text。
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nshort",
                    )
                    .await
                    .unwrap();
                stream.shutdown().await.unwrap();
            });
            let client = install_client(&conn).unwrap();
            let error = tokio::time::timeout(Duration::from_secs(5), async {
                if stage == "asset" {
                    install_release(dir.path(), &client, &tag, &source)
                        .await
                        .unwrap_err()
                } else {
                    verify_sha256(&client, &tag, asset, b"", &source)
                        .await
                        .unwrap_err()
                }
            })
            .await
            .unwrap();
            assert_safe_error(&error);
            assert!(
                format!("{error:#}").contains(if stage == "asset" {
                    "read pi asset"
                } else {
                    "read SHA256SUMS"
                }),
                "{error:#}"
            );
            server.await.unwrap();
        }
        assert!(origin.received_requests().await.unwrap().is_empty());
        report_done(dir.path(), PiUiError::Ready.failure("ready"));
    }

    #[test]
    fn both_package_layouts_promote_into_pi_pkg() {
        // Linux tar.gz：外层套 `pi/`；Windows zip：直接平铺。
        for nested in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let staging = dir.path().join("pi-pkg.staging");
            let root = if nested {
                staging.join("pi")
            } else {
                staging.clone()
            };
            fs::create_dir_all(root.join("assets")).unwrap();
            fs::write(root.join("pi.exe"), b"new").unwrap();
            fs::create_dir_all(pi_pkg_dir(dir.path())).unwrap();
            fs::write(pi_pkg_dir(dir.path()).join("pi.exe"), b"old").unwrap();
            assert_eq!(package_root(&staging, "pi.exe"), Some(root.clone()));
            promote(&root, &staging, dir.path()).unwrap();
            assert_eq!(
                fs::read(pi_pkg_dir(dir.path()).join("pi.exe")).unwrap(),
                b"new"
            );
            assert!(pi_pkg_dir(dir.path()).join("assets").is_dir());
            assert!(!staging.exists());
            assert!(!dir.path().join("pi-pkg.old").exists());
        }
    }

    #[test]
    fn a_package_without_the_program_is_rejected_and_cleaned_up() {
        let dir = tempfile::tempdir().unwrap();
        let staging = dir.path().join("pi-pkg.staging");
        fs::create_dir_all(staging.join("docs")).unwrap();
        assert_eq!(package_root(&staging, "pi.exe"), None);
        let err = unpack_and_promote(b"", "pi.tar.gz", "pi.exe", &staging, dir.path()).unwrap_err();
        assert!(!format!("{err:#}").is_empty());
    }

    #[tokio::test]
    async fn manual_install_reports_one_run_at_a_time() {
        let _guard = INSTALL_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        assert!(begin_install(dir.path()));
        assert!(install_running());
        // 已在下载时再点一次只返回当前进度，不起第二个安装。
        assert!(!begin_install(dir.path()));
        let status = install_status(dir.path());
        assert_eq!(status.state, "downloading");
        assert_eq!(status.dir, pi_pkg_dir(dir.path()).display().to_string());
        report_done(
            dir.path(),
            format_report(&EnsureReport::Downloaded {
                version: "0.87.1".into(),
            }),
        );
        assert!(!install_running());
        let wire = serde_json::to_value(install_status(dir.path()).notice.unwrap()).unwrap();
        assert_eq!(wire["code"], "pi_downloaded");
        assert_eq!(wire["params"]["version"], "0.87.1");
        assert!(begin_install(dir.path()));
        report_done(dir.path(), PiUiError::Ready.failure("ready"));
    }

    #[test]
    fn patch_updates_stay_within_the_compat_line() {
        let line = compat().line_label();
        let tags = [
            format!("v{line}.1"),
            format!("v{line}.9"),
            format!("v{line}.10-rc.1"),
            "v9.0.0".to_string(),
            "not-a-tag".to_string(),
        ];
        let latest = latest_in_line(tags.iter().map(String::as_str)).unwrap();
        assert_eq!(latest.to_string(), format!("{line}.9"));
        assert!(latest_in_line(["v9.0.0"].into_iter()).is_none());
    }
}
