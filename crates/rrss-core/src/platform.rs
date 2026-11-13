//! 平台差异的唯一出口。业务代码不写 `cfg(windows)` / `cfg(unix)`，只问这里。
//!
//! - 「这个平台该怎么做」写成 [`Os`] / [`Arch`] 的纯方法：编译期由 [`Os::current`] 选定，
//!   测试里可以直接构造任意平台，在 Linux 上也能把 Windows 分支测一遍。
//! - 只有真正调用系统接口的少数函数（磁盘剩余空间、进程是否存活、建链接、字体检测）
//!   在本文件里按平台实现；不支持的平台返回 `None`，由调用方降级。
//!
//! 加一个新平台：在 [`Os`] / [`Arch`] 加分支，补齐下面各方法与系统接口函数，再补测试。

use std::ffi::OsStr;
use std::path::Path;
use std::sync::OnceLock;

/// 启动时的沙箱请求状态；不代表已验证 WebKit 的编译选项或实际进程隔离。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebkitSandboxStatus {
    pub requested: bool,
    pub reason: &'static str,
}

static WEBKIT_SANDBOX: OnceLock<Option<WebkitSandboxStatus>> = OnceLock::new();

fn webkit_sandbox_from_environment(
    disable: Option<&OsStr>,
    force: Option<&OsStr>,
) -> Option<WebkitSandboxStatus> {
    (disable.is_some() || force.is_some()).then_some(WebkitSandboxStatus {
        requested: !disable.is_some_and(|value| value != OsStr::new("0"))
            && force == Some(OsStr::new("1")),
        reason: "set by environment",
    })
}

fn webkit_sandbox_policy(
    os: Os,
    bwrap_exists: bool,
    bwrap_usable: bool,
) -> Option<WebkitSandboxStatus> {
    (os == Os::Linux).then_some(WebkitSandboxStatus {
        requested: bwrap_exists && bwrap_usable,
        reason: if !bwrap_exists {
            "bubblewrap missing"
        } else if !bwrap_usable {
            "bubblewrap namespace probe failed"
        } else {
            "bubblewrap probe passed; WebKit sandbox requested, not verified"
        },
    })
}

/// 在任何 WebContext、异步运行时或工作线程创建前配置 WebKitGTK 沙箱。
/// 用户可设 `WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1` 关闭沙箱。
///
/// # Safety
/// 调用方必须处于进程启动的单线程阶段，不能有其他线程读取环境。
pub unsafe fn init_webkit_sandbox() -> Option<WebkitSandboxStatus> {
    *WEBKIT_SANDBOX.get_or_init(|| {
        let os = Os::current();
        if os != Os::Linux {
            return None;
        }
        let disable = std::env::var_os("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS");
        let force = std::env::var_os("WEBKIT_FORCE_SANDBOX");
        if let Some(status) = webkit_sandbox_from_environment(disable.as_deref(), force.as_deref())
        {
            return Some(status);
        }
        // 参照 WebKit 2.52 WTF/glib/Sandbox.cpp 的容器探测；不输出环境或错误详情。
        let probe = std::process::Command::new("bwrap")
            .args([
                "--ro-bind",
                "/",
                "/",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--unshare-all",
                "/usr/bin/true",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let exists = !matches!(&probe, Err(err) if err.kind() == std::io::ErrorKind::NotFound);
        let status = webkit_sandbox_policy(os, exists, probe.is_ok_and(|s| s.success()))?;
        // SAFETY: 调用方保证尚未创建其他线程；WebKit 还没有读取这些变量。
        unsafe {
            if status.requested {
                std::env::set_var("WEBKIT_FORCE_SANDBOX", "1");
                std::env::set_var("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS", "0");
            } else {
                // 旧版默认关闭且允许 FORCE=1 覆盖；新版使用独立的禁用变量。
                std::env::remove_var("WEBKIT_FORCE_SANDBOX");
                std::env::set_var("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS", "1");
            }
        }
        Some(status)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Windows,
    MacOs,
    Android,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemProxySnapshot {
    pub http: Option<String>,
    pub https: Option<String>,
    pub all: Option<String>,
    pub no_proxy: Option<String>,
    pub source: &'static str,
    pub pac: Option<String>,
    /// Keep unsupported WinINet policy visible to stricter consumers (webviews).
    pub bypass_configured: bool,
    pub auto_detect: bool,
    pub unavailable: bool,
}

impl Default for SystemProxySnapshot {
    fn default() -> Self {
        Self {
            http: None,
            https: None,
            all: None,
            no_proxy: None,
            source: "环境变量",
            pac: None,
            bypass_configured: false,
            auto_detect: false,
            unavailable: false,
        }
    }
}

/// 返回当前平台的原生系统代理快照。环境变量由 `rrss-core::network` 合并，
/// Windows 额外读取当前用户的 Internet Settings；其他平台暂以标准环境变量为系统代理入口。
pub fn system_proxy() -> SystemProxySnapshot {
    native_system_proxy()
}

#[cfg(not(windows))]
fn native_system_proxy() -> SystemProxySnapshot {
    SystemProxySnapshot::default()
}

#[cfg(windows)]
fn native_system_proxy() -> SystemProxySnapshot {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;

    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings");
    let Ok(key) = key else {
        return SystemProxySnapshot {
            unavailable: true,
            ..Default::default()
        };
    };
    let mut unavailable = false;
    macro_rules! read {
        ($name:literal, $ty:ty) => {
            match key.get_value::<$ty, _>($name) {
                Ok(value) => Some(value),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(_) => {
                    unavailable = true;
                    None
                }
            }
        };
    }
    let enabled = read!("ProxyEnable", u32).unwrap_or(0) != 0;
    let proxy = read!("ProxyServer", String);
    let override_list = read!("ProxyOverride", String);
    let pac = read!("AutoConfigURL", String).filter(|v| !v.trim().is_empty());
    let mut auto_detect = read!("AutoDetect", u32).unwrap_or(0) != 0;
    // WinINet stores the GUI's automatic settings in connection flags too;
    // AutoDetect alone is not an authoritative snapshot. Never read/log the
    // blob outside this platform boundary or expose its embedded addresses.
    match key
        .open_subkey("Connections")
        .and_then(|key| key.get_raw_value("DefaultConnectionSettings"))
    {
        Ok(value) => match windows_auto_proxy(&value.bytes) {
            Some(enabled) => auto_detect |= enabled,
            None => unavailable = true,
        },
        Err(_) => unavailable = true,
    }
    let bypass_configured = override_list
        .as_deref()
        .is_some_and(|v| !v.trim().is_empty());
    if !enabled || proxy.as_deref().is_none_or(|v| v.trim().is_empty()) {
        return SystemProxySnapshot {
            source: "Windows Internet 设置",
            pac,
            auto_detect,
            bypass_configured,
            unavailable,
            ..SystemProxySnapshot::default()
        };
    }
    let mut snapshot = parse_windows_proxy_server(proxy.as_deref().unwrap());
    snapshot.no_proxy = override_list.and_then(|v| normalize_windows_override(&v));
    snapshot.source = "Windows Internet 设置";
    snapshot.pac = pac;
    snapshot.unavailable |= unavailable;
    snapshot.auto_detect = auto_detect;
    snapshot.bypass_configured = bypass_configured;
    snapshot
}

/// 解析 Windows `ProxyServer`：既支持单个 `host:port`，也支持
/// `http=...;https=...;socks=...` 的按协议格式。该函数保持跨平台可测试。
pub fn parse_windows_proxy_server(raw: &str) -> SystemProxySnapshot {
    let mut snapshot = SystemProxySnapshot {
        source: "Windows Internet 设置",
        ..SystemProxySnapshot::default()
    };
    let entries: Vec<(&str, &str)> = raw
        .split(';')
        .filter_map(|item| item.split_once('='))
        .collect();
    if entries.is_empty() {
        let value = windows_proxy_url(raw, "http");
        snapshot.http = value.clone();
        snapshot.https = value;
        return snapshot;
    }
    for (kind, value) in entries {
        let value = windows_proxy_url(value, kind);
        match kind.to_ascii_lowercase().as_str() {
            "http" => snapshot.http = value,
            "https" => snapshot.https = value,
            "socks" => snapshot.all = value,
            _ => {}
        }
    }
    snapshot.unavailable =
        snapshot.http.is_none() && snapshot.https.is_none() && snapshot.all.is_none();
    snapshot
}

fn windows_proxy_url(raw: &str, kind: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.contains("://") {
        return Some(raw.to_owned());
    }
    let scheme = if kind.eq_ignore_ascii_case("socks") {
        "socks5"
    } else {
        "http"
    };
    Some(format!("{scheme}://{raw}"))
}

/// Windows `ProxyOverride` → RRSS 绕过列表。`<local>`（不含点的主机名）没有等价写法，
/// 局部通配（`*internal*`）等 RRSS 不支持的项直接略过：整份列表因一项无效而让系统代理
/// 不可用，比少绕过一项更糟。
pub fn normalize_windows_override(raw: &str) -> Option<String> {
    let mut values = Vec::new();
    for item in raw.split(';').map(str::trim).filter(|v| !v.is_empty()) {
        if item.eq_ignore_ascii_case("<local>") {
            continue;
        }
        let item = crate::network::normalize_system_no_proxy_entry(item);
        let normalized = windows_override_entry(item);
        if normalized.is_empty() || crate::network::normalize_no_proxy(&normalized).is_err() {
            continue;
        }
        if !values
            .iter()
            .any(|old: &String| old.eq_ignore_ascii_case(&normalized))
        {
            values.push(normalized);
        }
    }
    (!values.is_empty()).then(|| values.join(","))
}

/// Windows Internet 设置允许 `127.*`、`192.168.*` 这类 IPv4 通配，
/// RRSS 的代理客户端使用 CIDR，因此在平台边界完成等价转换。
fn windows_override_entry(raw: &str) -> String {
    if raw == "*" {
        return "*".into();
    }
    let parts: Vec<&str> = raw.split('.').collect();
    let wildcard_at = parts.iter().position(|part| *part == "*");
    if let Some(wildcard_at) = wildcard_at
        && parts.len() <= 4
        && wildcard_at > 0
        && parts[wildcard_at..].iter().all(|part| *part == "*")
        && parts[..wildcard_at]
            .iter()
            .all(|part| part.parse::<u8>().is_ok())
    {
        let mut octets = parts[..wildcard_at]
            .iter()
            .map(|part| (*part).to_owned())
            .collect::<Vec<_>>();
        octets.resize(4, "0".into());
        return format!("{}/{}", octets.join("."), wildcard_at * 8);
    }
    raw.to_ascii_lowercase()
}

/// 自绘标题栏上窗口三键（最小化 / 最大化 / 关闭）的外观。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowControls {
    /// Windows：46px 宽的方键，关闭键悬停红底。
    Windows,
    /// Linux 桌面：GNOME 式圆键。
    Gnome,
}

/// 托管 Pi 的发布包：文件名，以及包里程序的文件名。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PiRelease {
    pub asset: &'static str,
    pub program: &'static str,
}

impl Os {
    /// 当前编译目标。
    pub const fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "android") {
            Self::Android
        } else {
            Self::Other
        }
    }

    pub const fn is_windows(self) -> bool {
        matches!(self, Self::Windows)
    }

    /// 自定义协议在 WebView 里的来源。Tauri 2：Linux / macOS 为 `<scheme>://localhost`，
    /// Windows / Android 的 WebView 不认自定义协议，改走 `http://<scheme>.localhost`。
    pub fn protocol_origin(self, scheme: &str) -> String {
        match self {
            Self::Windows | Self::Android => format!("http://{scheme}.localhost"),
            _ => format!("{scheme}://localhost"),
        }
    }

    /// Pi 可执行文件可能的名字，按优先级。Windows 也接受无后缀的 `pi`（手动放置的包装脚本）。
    pub fn pi_program_names(self) -> &'static [&'static str] {
        match self {
            Self::Windows => &["pi.exe", "pi"],
            _ => &["pi"],
        }
    }

    /// GitHub Releases 上本平台的 Pi 独立包；没有发布包的平台为 `None`。
    pub fn pi_release(self, arch: Arch) -> Option<PiRelease> {
        let (asset, program) = match (self, arch) {
            (Self::Linux, Arch::X86_64) => ("pi-linux-x64.tar.gz", "pi"),
            (Self::Linux, Arch::Aarch64) => ("pi-linux-arm64.tar.gz", "pi"),
            (Self::Windows, Arch::X86_64) => ("pi-windows-x64.zip", "pi.exe"),
            (Self::Windows, Arch::Aarch64) => ("pi-windows-arm64.zip", "pi.exe"),
            (Self::MacOs, Arch::X86_64) => ("pi-darwin-x64.tar.gz", "pi"),
            (Self::MacOs, Arch::Aarch64) => ("pi-darwin-arm64.tar.gz", "pi"),
            _ => return None,
        };
        Some(PiRelease { asset, program })
    }

    /// 托管 Pi 是否在 `bin/` 下建符号链接作入口。Windows 普通用户建不了符号链接，
    /// 复制出去又会和包里的 assets / native 等资源分家，所以直接用包目录里的程序。
    pub const fn links_managed_binary(self) -> bool {
        !self.is_windows()
    }

    /// 用系统文件管理器 / 默认程序打开路径的命令。
    pub const fn file_opener(self) -> &'static str {
        match self {
            Self::Windows => "explorer",
            Self::MacOs => "open",
            _ => "xdg-open",
        }
    }

    /// 打开文件时不许改名 / 删除（Windows）：替换正在使用的数据库、程序前必须等旧进程退出。
    pub const fn locks_open_files(self) -> bool {
        self.is_windows()
    }

    /// 主窗口不用系统标题栏，窗口三键由前端按这里给的样子画。
    pub const fn window_controls(self) -> WindowControls {
        match self {
            Self::Windows => WindowControls::Windows,
            _ => WindowControls::Gnome,
        }
    }
}

impl Arch {
    pub const fn current() -> Self {
        if cfg!(target_arch = "x86_64") {
            Self::X86_64
        } else if cfg!(target_arch = "aarch64") {
            Self::Aarch64
        } else {
            Self::Other
        }
    }
}

// ---------------------------------------------------------------------------
// 系统接口：按平台实现，不支持时返回 None。
// ---------------------------------------------------------------------------

/// `path` 所在文件系统的剩余可用字节；取不到时为 `None`（目前 Windows 未实现）。
#[cfg(unix)]
pub fn available_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` 是以 NUL 结尾的路径，`st` 是可写的 statvfs。
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    // 各平台字段宽度不同（有的是 u32），统一转成 u64。
    #[allow(clippy::unnecessary_cast)]
    Some(st.f_bavail as u64 * st.f_frsize as u64)
}

#[cfg(not(unix))]
pub fn available_bytes(_path: &Path) -> Option<u64> {
    None
}

/// 系统托盘能否显示。Linux 的托盘靠 AppIndicator 动态库，运行时才加载，缺库时建托盘会失败甚至中止，
/// 所以先试着加载一次；Windows 与 macOS 总有托盘。有库但桌面不显示托盘图标（如未装扩展的 GNOME）时查不出来。
#[cfg(target_os = "linux")]
pub fn tray_available() -> bool {
    const LIBRARIES: [&str; 4] = [
        "libayatana-appindicator3.so.1",
        "libayatana-appindicator3.so",
        "libappindicator3.so.1",
        "libappindicator3.so",
    ];
    LIBRARIES.iter().any(|name| {
        let Ok(name) = std::ffi::CString::new(*name) else {
            return false;
        };
        // SAFETY: `name` 以 NUL 结尾；只检查能否加载，加载成功就立即释放句柄。
        let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_LAZY | libc::RTLD_LOCAL) };
        if handle.is_null() {
            return false;
        }
        unsafe { libc::dlclose(handle) };
        true
    })
}

#[cfg(not(target_os = "linux"))]
pub fn tray_available() -> bool {
    true
}

/// 进程是否还在运行；本平台查不到时为 `None`（只有 Linux 读 `/proc`）。
/// 僵尸进程的文件已经全部关闭，只是还没被回收，算已退出。
#[cfg(target_os = "linux")]
pub fn process_alive(pid: u32) -> Option<bool> {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return Some(false);
    };
    // 格式为 `pid (comm) state ...`，comm 里可能有括号，取最后一个右括号之后的字段。
    Some(
        stat.rsplit_once(')')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .is_some_and(|state| !matches!(state, "Z" | "X" | "x")),
    )
}

#[cfg(not(target_os = "linux"))]
pub fn process_alive(_pid: u32) -> Option<bool> {
    None
}

/// Windows 上 `CreateProcess` 的 `CREATE_NO_WINDOW`：控制台子进程不再自己开一个控制台窗口。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 后台启动的子进程不弹控制台窗口。Windows 上图形界面进程启动 `pi.exe` 这类控制台程序时，
/// 系统会为它新开一个窗口（标题就是程序名）；其他平台没有这个问题，什么也不做。
/// 子进程再启动的进程（Pi 调 `rrss tool`、npm）沿用它不可见的控制台，同样不弹窗。
pub fn hide_console_window(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// 同 [`hide_console_window`]，给 tokio 的异步子进程用。
pub fn hide_console_window_async(
    cmd: &mut tokio::process::Command,
) -> &mut tokio::process::Command {
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// 从终端运行 `rrss tool …` 时把输出接回那个终端。Windows 发布版是图形界面程序
/// （`windows_subsystem = "windows"`，启动时不带控制台窗口），没有自己的标准输出；
/// 只在标准输出与错误都没被重定向时附着启动它的控制台。Pi 用管道调用工具时句柄已经有效，什么也不做；
/// 没有父控制台（从资源管理器启动）时附着失败，输出照旧丢弃。其他平台什么也不做。
pub fn attach_parent_console() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::Console::{
            ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
        };
        // SAFETY: 两个函数只读写本进程的标准句柄表，参数都是常量。
        unsafe {
            let unset = |id| {
                let handle = GetStdHandle(id);
                handle.is_null() || handle == INVALID_HANDLE_VALUE
            };
            if unset(STD_OUTPUT_HANDLE) && unset(STD_ERROR_HANDLE) {
                AttachConsole(ATTACH_PARENT_PROCESS);
            }
        }
    }
}

/// Stable log description, including Windows exception codes and Unix signals.
pub fn exit_description(status: std::process::ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal {signal}");
        }
    }
    describe_exit_code(Os::current(), status.code())
}

fn describe_exit_code(os: Os, code: Option<i32>) -> String {
    match code {
        Some(code) if os == Os::Windows => format!("code {code} (0x{:08X})", code as u32),
        Some(code) => format!("code {code}"),
        None => "unknown exit status".into(),
    }
}

/// 建符号链接 `link → target`。没有符号链接的平台返回错误，调用方应先看 [`Os::links_managed_binary`]。
pub fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link)
    }
    #[cfg(not(unix))]
    {
        let _ = (target, link);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "symlinks are not used on this platform",
        ))
    }
}

/// 已安装的中文字体族（诊断用）；本平台查不到时为 `None`（只有 Linux 走 fontconfig）。
pub fn installed_cjk_fonts() -> Option<Vec<&'static str>> {
    if Os::current() != Os::Linux {
        return None;
    }
    let out = std::process::Command::new("fc-list")
        .args([":lang=zh", "family"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    Some(
        ["Noto Sans CJK", "Noto Serif CJK", "Source Han", "WenQuanYi"]
            .into_iter()
            .filter(|needle| text.contains(needle))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn exit_descriptions_preserve_windows_exception_codes_and_unix_signals() {
        use super::*;
        assert_eq!(describe_exit_code(Os::Linux, Some(3)), "code 3");
        assert_eq!(
            describe_exit_code(Os::Windows, Some(0)),
            "code 0 (0x00000000)"
        );
        assert_eq!(
            describe_exit_code(Os::Windows, Some(0xC0000005u32 as i32)),
            "code -1073741819 (0xC0000005)"
        );
        assert_eq!(describe_exit_code(Os::Other, None), "unknown exit status");
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(
                exit_description(std::process::ExitStatus::from_raw(9)),
                "signal 9"
            );
            assert_eq!(
                exit_description(std::process::ExitStatus::from_raw(3 << 8)),
                "code 3"
            );
        }
    }
    use super::*;

    const ALL: [Os; 5] = [Os::Linux, Os::Windows, Os::MacOs, Os::Android, Os::Other];

    #[test]
    fn webkit_sandbox_respects_explicit_environment() {
        for (disable, force, requested) in [
            (None, None, None),
            (Some("1"), None, Some(false)),
            (Some("0"), None, Some(false)),
            (None, Some("1"), Some(true)),
            (None, Some("0"), Some(false)),
            (Some("1"), Some("1"), Some(false)),
            (Some("0"), Some("1"), Some(true)),
            (Some("0"), Some("0"), Some(false)),
            (Some(""), Some("1"), Some(false)),
            (None, Some(""), Some(false)),
        ] {
            let status =
                webkit_sandbox_from_environment(disable.map(OsStr::new), force.map(OsStr::new));
            assert_eq!(status.map(|s| s.requested), requested);
            if let Some(status) = status {
                assert_eq!(status.reason, "set by environment");
            }
        }
    }

    #[test]
    fn webkit_sandbox_requires_working_bubblewrap_only_on_linux() {
        for os in ALL {
            for (exists, usable) in [(false, false), (true, false), (true, true)] {
                let status = webkit_sandbox_policy(os, exists, usable);
                if os == Os::Linux {
                    let status = status.unwrap();
                    assert_eq!(status.requested, exists && usable);
                } else {
                    assert_eq!(status, None);
                }
            }
        }
        assert_ne!(
            webkit_sandbox_policy(Os::Linux, false, false)
                .unwrap()
                .reason,
            webkit_sandbox_policy(Os::Linux, true, false)
                .unwrap()
                .reason,
        );
    }

    #[test]
    fn converts_windows_wildcard_addresses_to_cidr() {
        assert_eq!(windows_override_entry("127.*"), "127.0.0.0/8");
        assert_eq!(windows_override_entry("192.168.*"), "192.168.0.0/16");
        assert_eq!(windows_override_entry("172.16.*"), "172.16.0.0/16");
        assert_eq!(windows_override_entry("10.*"), "10.0.0.0/8");
        assert_eq!(windows_override_entry("localhost"), "localhost");
    }
    #[test]
    fn windows_override_keeps_supported_entries_and_skips_the_rest() {
        assert_eq!(
            normalize_windows_override(
                "<local>;127.*;*.corp.example;intranet:8080;*internal*;::1;10.*.*.*;Localhost;localhost;:8080"
            )
            .as_deref(),
            Some("127.0.0.0/8,corp.example,intranet,::1,10.0.0.0/8,localhost")
        );
        assert_eq!(normalize_windows_override("<local>;*foo*"), None);
    }
    #[test]
    fn parses_windows_proxy_server_shapes() {
        let one = parse_windows_proxy_server("proxy.example:8080");
        assert_eq!(one.http.as_deref(), Some("http://proxy.example:8080"));
        assert_eq!(one.https.as_deref(), Some("http://proxy.example:8080"));
        let split = parse_windows_proxy_server(
            "http=proxy.example:8080;https=https.example:8443;socks=socks.example:1080",
        );
        assert_eq!(split.http.as_deref(), Some("http://proxy.example:8080"));
        assert_eq!(split.https.as_deref(), Some("http://https.example:8443"));
        assert_eq!(split.all.as_deref(), Some("socks5://socks.example:1080"));
    }

    #[test]
    fn media_origin_follows_tauri_per_platform() {
        assert_eq!(Os::Linux.protocol_origin("rrss"), "rrss://localhost");
        assert_eq!(Os::MacOs.protocol_origin("rrss"), "rrss://localhost");
        assert_eq!(Os::Windows.protocol_origin("rrss"), "http://rrss.localhost");
        assert_eq!(Os::Android.protocol_origin("rrss"), "http://rrss.localhost");
    }

    #[test]
    fn every_platform_with_a_release_names_its_program_first() {
        for os in ALL {
            for arch in [Arch::X86_64, Arch::Aarch64, Arch::Other] {
                if let Some(release) = os.pi_release(arch) {
                    assert_eq!(release.program, os.pi_program_names()[0], "{os:?} {arch:?}");
                    assert!(release.asset.ends_with(".zip") == os.is_windows(), "{os:?}");
                }
            }
        }
        assert!(Os::Linux.pi_release(Arch::Other).is_none());
        assert!(Os::Android.pi_release(Arch::Aarch64).is_none());
    }

    #[test]
    fn windows_uses_the_packaged_program_and_waits_for_locked_files() {
        assert!(!Os::Windows.links_managed_binary());
        assert!(Os::Windows.locks_open_files());
        assert!(Os::Linux.links_managed_binary());
        assert!(!Os::Linux.locks_open_files());
        assert_eq!(Os::Windows.file_opener(), "explorer");
        assert_eq!(Os::Linux.file_opener(), "xdg-open");
    }

    #[test]
    fn window_controls_follow_the_desktop() {
        assert_eq!(Os::Windows.window_controls(), WindowControls::Windows);
        assert_eq!(Os::Linux.window_controls(), WindowControls::Gnome);
    }

    #[test]
    fn system_queries_answer_or_decline_on_this_platform() {
        let dir = tempfile::tempdir().unwrap();
        assert!(available_bytes(dir.path()).is_none_or(|n| n > 0));
        if Os::current() == Os::Linux {
            assert_eq!(process_alive(std::process::id()), Some(true));
        } else {
            assert_eq!(process_alive(std::process::id()), None);
        }
    }
}

/// A closed WebView2 browser process can still hold files. Do not retry/block;
/// a fresh session is used immediately and startup retries abandoned directories.
pub fn remove_web_session(path: &std::path::Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error)
            if Os::current().is_windows()
                && matches!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::WouldBlock
                ) =>
        {
            tracing::warn!(error_kind = ?error.kind(), "original-page directory still in use; cleanup deferred to next startup");
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Wry has no explicit direct proxy mode. Windows accepts a stable browser
/// argument; Linux is configured through rrss-desktop::webview_native before navigation.
pub fn webview_direct_args(os: Os) -> crate::Result<&'static str> {
    if os.is_windows() {
        Ok("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --no-proxy-server")
    } else if os == Os::Linux {
        Ok("")
    } else {
        Err(crate::Error::InvalidNetworkProxy(
            "当前平台尚不能保证原网页直连，请改用阅读视图或系统浏览器。".into(),
        ))
    }
}

/// WinINet default connection flags (DWORD at offset 8). AUTO_PROXY_URL = 4,
/// AUTO_DETECT = 8. A truncated snapshot cannot prove the absence of auto proxy.
pub fn windows_auto_proxy(settings: &[u8]) -> Option<bool> {
    let flags = u32::from_le_bytes(settings.get(8..12)?.try_into().ok()?);
    Some(flags & (4 | 8) != 0)
}

/// Pi 的 stdio transport 展开独立的 ~ 和 ~/ 前缀；检测采用同样行为。
pub fn expand_mcp_home(value: &str) -> std::ffi::OsString {
    if value == "~" || value.starts_with("~/") || value.starts_with("~\\") {
        if let Some(home) = directories::BaseDirs::new() {
            return if value == "~" {
                home.home_dir().as_os_str().to_owned()
            } else {
                home.home_dir().join(&value[2..]).into_os_string()
            };
        }
    }
    value.into()
}

/// MCP 的命令查找：Linux/macOS 保持 PATH 语义；Windows 同时查 npm/uv 的脚本入口。
pub fn mcp_command(program: &str, cwd: &Path, path: Option<&OsStr>) -> tokio::process::Command {
    let resolved = resolve_mcp_command(Os::current(), program, cwd, path);
    let mut command = tokio::process::Command::new(resolved);
    command.current_dir(cwd).kill_on_drop(true);
    hide_console_window_async(&mut command);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    command
}

fn resolve_mcp_command(
    os: Os,
    program: &str,
    cwd: &Path,
    path: Option<&OsStr>,
) -> std::path::PathBuf {
    if os != Os::Windows {
        return program.into();
    }
    let program_path = Path::new(program);
    let dirs: Vec<_> = if program_path.components().count() > 1 || program_path.is_absolute() {
        vec![cwd.to_path_buf()]
    } else {
        path.map(std::env::split_paths)
            .into_iter()
            .flatten()
            .collect()
    };
    // 没写扩展名时只认可执行的扩展名：npm 在同一目录放了无扩展名的 sh 脚本（`npx`）和 `npx.cmd`，
    // CreateProcess 不能直接执行前者。
    let exts: &[&str] = if program_path.extension().is_some() {
        &[""]
    } else {
        &[".exe", ".cmd", ".bat", ".com"]
    };
    for dir in dirs {
        let base = dir.join(program);
        for ext in exts {
            let mut candidate = base.as_os_str().to_os_string();
            candidate.push(ext);
            let candidate = std::path::PathBuf::from(candidate);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    program.into()
}

/// 只持有本次启动的子进程组；超时、取消、提前返回时连同后代一起结束。
pub struct ChildTree {
    #[cfg(unix)]
    pid: u32,
    #[cfg(windows)]
    job: isize,
}
impl ChildTree {
    pub fn attach(child: &mut tokio::process::Child) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                pid: child
                    .id()
                    .ok_or_else(|| std::io::Error::other("子进程已经退出"))?,
            })
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    let _ = child.start_kill();
                    return Err(std::io::Error::last_os_error());
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let configured = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as _,
                    std::mem::size_of_val(&info) as u32,
                ) != 0;
                let assigned = child
                    .raw_handle()
                    .is_some_and(|handle| AssignProcessToJobObject(job, handle as _) != 0);
                if !configured || !assigned {
                    let error = std::io::Error::last_os_error();
                    CloseHandle(job);
                    let _ = child.start_kill();
                    return Err(error);
                }
                Ok(Self { job: job as isize })
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = child.start_kill();
            Err(std::io::Error::other("此平台不支持安全结束 MCP 子进程"))
        }
    }
}
impl Drop for ChildTree {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.pid as i32), libc::SIGKILL);
        }
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job as _);
        }
    }
}

#[cfg(test)]
mod mcp_platform_tests {
    use super::*;
    #[test]
    fn windows_command_lookup_finds_npm_shims_and_preserves_explicit_paths() {
        let dir = tempfile::tempdir().unwrap();
        let cmd = dir.path().join("npx.cmd");
        std::fs::write(&cmd, "fixture").unwrap();
        // npm 同时放的无扩展名 sh 脚本不能被选中。
        std::fs::write(dir.path().join("npx"), "#!/bin/sh").unwrap();
        assert_eq!(
            resolve_mcp_command(Os::Windows, "npx", dir.path(), Some(dir.path().as_os_str())),
            cmd
        );
        assert_eq!(
            resolve_mcp_command(Os::Windows, cmd.to_str().unwrap(), dir.path(), None),
            cmd
        );
        assert_eq!(
            resolve_mcp_command(Os::Linux, "npx", dir.path(), Some(dir.path().as_os_str())),
            std::path::PathBuf::from("npx")
        );
        assert_eq!(
            resolve_mcp_command(
                Os::Windows,
                "missing",
                dir.path(),
                Some(dir.path().as_os_str())
            ),
            std::path::PathBuf::from("missing")
        );
    }
}
