#!/usr/bin/env python3
"""
RRSS 桌面无头 WebDriver 冒烟（Linux/WSL：Xvfb + WebKitWebDriver + tauri-driver）。

官方链路见 https://v2.tauri.app/develop/tests/webdriver/
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable, Mapping, Sequence, TextIO

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_BINARY = REPO_ROOT / "target" / "debug" / "rrss"

# 输出目录固定 artifact（每次运行覆盖，避免无限增长）
ARTIFACT_NAMES = (
    "smoke.log",
    "tauri-driver.log",
    "xvfb.log",
    "home-focus.png",
    "settings-rules.png",
    "failure.png",
)

# 测试库：正日预算 + 当天用量已满 → ensure_auto_job_budget 跳过自动作业（0 表示不限额）
TEST_AI_DAILY_BUDGET = 1000

TAURI_INVOKE_ASYNC = """
const cmd = arguments[0];
const args = arguments[1];
const done = arguments[arguments.length - 1];
(async () => {
  try {
    const ti = window.__TAURI_INTERNALS__;
    if (!ti || typeof ti.invoke !== "function") {
      done(JSON.stringify({ ok: false, error: "missing __TAURI_INTERNALS__.invoke" }));
      return;
    }
    const data = await ti.invoke(cmd, args);
    done(JSON.stringify({ ok: true, data }));
  } catch (err) {
    done(JSON.stringify({ ok: false, error: String(err) }));
  }
})();
"""

TAURI_HAS_INTERNALS = """
return !!(window.__TAURI_INTERNALS__ && typeof window.__TAURI_INTERNALS__.invoke === "function");
"""


class SmokeError(Exception):
    """冒烟失败（含诊断上下文）。"""


@dataclass
class DepsReport:
    ok: bool
    missing: list[str] = field(default_factory=list)
    hints: list[str] = field(default_factory=list)


def repo_relative(path: Path) -> str:
    try:
        return str(path.relative_to(REPO_ROOT))
    except ValueError:
        return str(path)


def which(name: str) -> Path | None:
    found = shutil.which(name)
    return Path(found) if found else None


def resolve_webkit_driver(explicit: str | None) -> Path | None:
    if explicit:
        path = Path(explicit).expanduser().resolve()
        return path if path.is_file() else None
    return which("WebKitWebDriver")


def check_dependencies(*, need_xvfb: bool, native_driver: str | None = None) -> DepsReport:
    missing: list[str] = []
    hints: list[str] = []

    if not which("tauri-driver"):
        missing.append("tauri-driver")
        hints.append("cargo install tauri-driver --locked")

    wk = resolve_webkit_driver(native_driver or None)
    if not wk:
        if native_driver:
            missing.append(f"WebKitWebDriver ({native_driver})")
        else:
            missing.append("WebKitWebDriver")
        hints.append("Debian/Ubuntu: sudo apt install webkit2gtk-driver")

    if need_xvfb and not which("Xvfb"):
        missing.append("Xvfb")
        hints.append("Debian/Ubuntu: sudo apt install xvfb")

    return DepsReport(ok=not missing, missing=missing, hints=hints)


def should_manage_xvfb(mode: str) -> bool:
    """除 never 外一律由脚本自启隔离虚拟显示（不受外层 DISPLAY 影响）。"""
    if mode == "never":
        return False
    if mode in ("auto", "always"):
        return True
    raise SmokeError(f"未知 xvfb 模式: {mode!r}")


def display_number(display: str) -> int:
    head = display.lstrip(":").split(".", 1)[0]
    return int(head)


def x11_socket(display: str) -> Path:
    return Path(f"/tmp/.X11-unix/X{display_number(display)}")


def x11_accepting(display: str) -> bool:
    # WSLg can expose a read-only /tmp/.X11-unix. Xvfb still serves Linux's
    # abstract Unix socket, which X11 clients can use without a filesystem entry.
    path = str(x11_socket(display))
    for address in ("\0" + path, path):
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
                client.settimeout(0.2)
                client.connect(address)
                return True
        except OSError:
            continue
    return False


def pick_isolated_display(preferred: str) -> str:
    """Avoid both filesystem and abstract Unix sockets used by an existing display."""
    base = display_number(preferred)
    for offset in range(32):
        num = base + offset
        disp = f":{num}"
        if not x11_socket(disp).exists() and not x11_accepting(disp):
            return disp
    raise SmokeError(f"无法在 {preferred} 起找 32 个连续空闲 DISPLAY")


def xdg_rrss_db(xdg_data_home: Path) -> Path:
    return xdg_data_home / "rrss" / "rrss.db"


def local_day() -> str:
    return time.strftime("%Y-%m-%d")


def patch_test_db(db_path: Path) -> None:
    """空库补丁：禁 Pi 自动拉取/更新；用正预算 + 当天用量耗尽自动 AI 作业。"""
    conn = sqlite3.connect(db_path)
    try:
        updates = {
            "pi_auto_download": "0",
            "pi_auto_update": "0",
            "ai_daily_budget_tokens": str(TEST_AI_DAILY_BUDGET),
        }
        for key, value in updates.items():
            conn.execute(
                "INSERT INTO settings (key, value) VALUES (?, ?) "
                "ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (key, value),
            )
        day = local_day()
        conn.execute(
            "INSERT INTO ai_usage (day, tokens, prompt_tokens, completion_tokens) "
            "VALUES (?, ?, 0, 0) "
            "ON CONFLICT(day) DO UPDATE SET tokens = excluded.tokens",
            (day, TEST_AI_DAILY_BUDGET),
        )
        conn.commit()
    finally:
        conn.close()


def init_isolated_db(binary: Path, xdg_data_home: Path, log: Callable[[str], None]) -> Path:
    xdg_data_home.mkdir(parents=True, exist_ok=True)
    db_path = xdg_rrss_db(xdg_data_home)
    if db_path.exists():
        raise SmokeError(f"拒绝覆盖已有数据库: {db_path}")

    env = os.environ.copy()
    env["RRSS_DB"] = str(db_path)
    log(f"初始化全新测试库：RRSS_DB={db_path}")
    proc = subprocess.run(
        [str(binary), "tool", "list_feeds"],
        env=env,
        capture_output=True,
        text=True,
        timeout=120,
    )
    if proc.returncode != 0:
        raise SmokeError(
            "rrss tool list_feeds 失败（无法创建 schema）:\n"
            + (proc.stderr or proc.stdout or "(无输出)")
        )
    if not db_path.is_file():
        raise SmokeError(f"预期数据库不存在: {db_path}")
    patch_test_db(db_path)
    return db_path


def port_is_open(host: str, port: int, timeout: float = 0.35) -> bool:
    try:
        with socket.create_connection((host, port), timeout=timeout):
            return True
    except OSError:
        return False


def ensure_driver_port_free(host: str, port: int) -> None:
    if port_is_open(host, port):
        raise SmokeError(
            f"WebDriver 端口 {host}:{port} 已被占用；请停止已有 driver 或换 --driver-url"
        )


def wait_for_port(
    host: str,
    port: int,
    timeout: float,
    *,
    proc: ManagedProcess | None = None,
) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if proc is not None and proc.popen.poll() is not None:
            raise SmokeError(
                f"{proc.name} 在端口就绪前已退出 (code={proc.popen.returncode})"
            )
        if port_is_open(host, port):
            return
        time.sleep(0.15)
    raise SmokeError(f"等待 {host}:{port} 超时 ({timeout}s)")


def wait_for_xvfb(display: str, proc: ManagedProcess, timeout: float) -> None:
    sock = x11_socket(display)
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if proc.popen.poll() is not None:
            raise SmokeError(
                f"Xvfb 提前退出 (code={proc.popen.returncode})，见 {proc.log_path}"
            )
        if x11_accepting(display):
            return
        time.sleep(0.1)
    raise SmokeError(f"Xvfb DISPLAY={display} 在 {timeout}s 内未就绪")


class SmokeLogger:
    def __init__(self, path: Path) -> None:
        self.path = path
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text("", encoding="utf-8")

    def __call__(self, msg: str) -> None:
        line = f"[{time.strftime('%H:%M:%S')}] {msg}"
        print(line, flush=True)
        with self.path.open("a", encoding="utf-8") as fh:
            fh.write(line.rstrip() + "\n")


class WebDriverClient:
    """最小 W3C WebDriver JSON 客户端（标准库）。"""

    def __init__(self, base_url: str, timeout: float = 30.0) -> None:
        self.base = base_url.rstrip("/")
        self.timeout = timeout
        self.session_id: str | None = None

    def _url(self, path: str) -> str:
        if not path.startswith("/"):
            path = "/" + path
        return f"{self.base}{path}"

    def _request(
        self,
        method: str,
        path: str,
        body: Mapping[str, Any] | None = None,
    ) -> dict[str, Any]:
        data = None
        headers = {"Content-Type": "application/json; charset=utf-8"}
        if body is not None:
            data = json.dumps(body).encode("utf-8")
        req = urllib.request.Request(self._url(path), data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                raw = resp.read().decode("utf-8") or "{}"
                return json.loads(raw)
        except urllib.error.HTTPError as err:
            detail = err.read().decode("utf-8", errors="replace")
            raise SmokeError(f"WebDriver {method} {path} → HTTP {err.code}: {detail}") from None
        except urllib.error.URLError as err:
            raise SmokeError(f"WebDriver {method} {path} 连接失败: {err.reason}") from None
        except (TimeoutError, socket.timeout) as err:
            raise SmokeError(f"WebDriver {method} {path} 超时: {err}") from None
        except json.JSONDecodeError as err:
            raise SmokeError(f"WebDriver {method} {path} 响应非 JSON: {err}") from None

    def new_session(self, capabilities: dict[str, Any]) -> None:
        payload = {"capabilities": {"alwaysMatch": capabilities}}
        res = self._request("POST", "/session", payload)
        value = res.get("value") or res
        sid = value.get("sessionId") or value.get("session_id")
        if not sid:
            raise SmokeError(f"无法创建 WebDriver 会话: {json.dumps(res)[:500]}")
        self.session_id = sid
        self._request(
            "POST",
            f"/session/{self.session_id}/timeouts",
            {"implicit": 0, "script": 90000},
        )

    def delete_session(self) -> None:
        if not self.session_id:
            return
        sid = self.session_id
        self.session_id = None
        try:
            self._request("DELETE", f"/session/{sid}")
        except SmokeError:
            pass

    def _session_path(self, suffix: str) -> str:
        if not self.session_id:
            raise SmokeError("WebDriver 会话未建立")
        return f"/session/{self.session_id}{suffix}"

    def find(self, using: str, value: str) -> str:
        res = self._request(
            "POST",
            self._session_path("/element"),
            {"using": using, "value": value},
        )
        elem = res.get("value") or res
        elem_id = elem.get("element-6066-11e4-a52e-4f735466cecf") or elem.get("ELEMENT")
        if not elem_id:
            raise SmokeError(
                f"找不到元素 {using}={value!r}: {json.dumps(res)[:300]}"
            )
        return elem_id

    def find_css(self, selector: str) -> str:
        return self.find("css selector", selector)

    def find_xpath(self, xpath: str) -> str:
        return self.find("xpath", xpath)

    def find_css_optional(self, selector: str) -> str | None:
        try:
            return self.find_css(selector)
        except SmokeError:
            return None

    def click(self, element_id: str) -> None:
        self._request("POST", self._session_path(f"/element/{element_id}/click"), {})

    def get_text(self, element_id: str) -> str:
        res = self._request("GET", self._session_path(f"/element/{element_id}/text"))
        value = res.get("value")
        return value if isinstance(value, str) else str(value)

    def execute_sync(self, script: str, *args: Any) -> Any:
        res = self._request(
            "POST",
            self._session_path("/execute/sync"),
            {"script": script, "args": list(args)},
        )
        return res.get("value")

    def execute_async(self, script: str, *args: Any, async_timeout: float = 60.0) -> Any:
        old = self.timeout
        self.timeout = max(old, async_timeout)
        try:
            res = self._request(
                "POST",
                self._session_path("/execute/async"),
                {"script": script, "args": list(args)},
            )
        finally:
            self.timeout = old
        return res.get("value")

    def screenshot_png(self) -> bytes:
        res = self._request("GET", self._session_path("/screenshot"))
        value = res.get("value")
        if not isinstance(value, str):
            raise SmokeError("截图响应无效")
        return base64.b64decode(value)

    def wait_css(self, selector: str, timeout: float, poll: float = 0.25) -> str:
        deadline = time.monotonic() + timeout
        last: SmokeError | None = None
        while time.monotonic() < deadline:
            try:
                return self.find_css(selector)
            except SmokeError as err:
                last = err
                time.sleep(poll)
        raise SmokeError(f"等待元素 {selector!r} 超时: {last}")

    def wait_css_gone(self, selector: str, timeout: float, poll: float = 0.25) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.execute_sync("return document.querySelector(arguments[0]) === null;", selector):
                return
            time.sleep(poll)
        raise SmokeError(f"等待元素消失 {selector!r} 超时")

    def wait_xpath(self, xpath: str, timeout: float, poll: float = 0.25) -> str:
        deadline = time.monotonic() + timeout
        last: SmokeError | None = None
        while time.monotonic() < deadline:
            try:
                return self.find_xpath(xpath)
            except SmokeError as err:
                last = err
                time.sleep(poll)
        raise SmokeError(f"等待 XPath {xpath!r} 超时: {last}")


@dataclass
class ManagedProcess:
    name: str
    popen: subprocess.Popen[Any]
    log_path: Path | None = None
    log_handle: TextIO | subprocess.DEVNULL.__class__ | None = None

    def terminate(self, grace: float = 3.0) -> None:
        # start_new_session=True makes this PID the process-group owner. Clean its
        # children even when the driver itself exited before us.
        pid = self.popen.pid
        try:
            os.killpg(pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            self.popen.wait(timeout=grace)
        except subprocess.TimeoutExpired:
            pass
        try:
            os.killpg(pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        if self.popen.poll() is None:
            self.popen.wait(timeout=5)
        self._close_log()

    def _close_log(self) -> None:
        if self.log_handle is not None and self.log_handle not in (subprocess.DEVNULL,):
            try:
                self.log_handle.close()
            except OSError:
                pass
            self.log_handle = None


class ProcessStack:
    """LIFO 清理子进程（Xvfb、tauri-driver）。"""

    def __init__(self) -> None:
        self._items: list[ManagedProcess] = []

    def add(self, proc: ManagedProcess) -> None:
        self._items.append(proc)

    def cleanup(self) -> None:
        while self._items:
            proc = self._items.pop()
            try:
                proc.terminate()
            except OSError:
                pass


def start_xvfb(display: str, log_path: Path | None) -> ManagedProcess:
    log_fh: TextIO | Any = subprocess.DEVNULL
    if log_path:
        log_path.parent.mkdir(parents=True, exist_ok=True)
        log_path.write_bytes(b"")
        log_fh = open(log_path, "wb")
    try:
        pop = subprocess.Popen(
            [
                "Xvfb",
                display,
                "-screen",
                "0",
                "1280x720x24",
                "-ac",
                "-nolisten",
                "tcp",
                "+extension",
                "GLX",
                "+render",
                "-noreset",
            ],
            stdout=log_fh,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
    except FileNotFoundError as err:
        raise SmokeError("未找到 Xvfb，请安装 xvfb 或使用 --xvfb never + 外层 xvfb-run") from err
    return ManagedProcess("Xvfb", pop, log_path, log_fh)


def start_tauri_driver(
    *,
    port: int,
    env: Mapping[str, str],
    native_driver: Path | None,
    log_path: Path | None,
) -> ManagedProcess:
    driver = which("tauri-driver")
    if not driver:
        raise SmokeError("tauri-driver 不在 PATH")
    cmd = [str(driver), "--port", str(port)]
    if native_driver:
        cmd.extend(["--native-driver", str(native_driver)])
    log_fh: TextIO | Any = subprocess.DEVNULL
    if log_path:
        log_path.parent.mkdir(parents=True, exist_ok=True)
        log_path.write_bytes(b"")
        log_fh = open(log_path, "wb")
    pop = subprocess.Popen(
        cmd,
        env=dict(env),
        stdout=log_fh,
        stderr=subprocess.STDOUT,
        start_new_session=True,
    )
    return ManagedProcess("tauri-driver", pop, log_path, log_fh)


def copy_if_exists(src: Path, dest: Path) -> None:
    if src.is_file():
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dest)


def tauri_invoke(client: WebDriverClient, cmd: str, args: dict[str, Any]) -> Any:
    raw = client.execute_async(TAURI_INVOKE_ASYNC, cmd, args, async_timeout=90.0)
    if not isinstance(raw, str):
        raise SmokeError(f"invoke {cmd} 返回非字符串: {raw!r}")
    parsed = json.loads(raw)
    if not parsed.get("ok"):
        raise SmokeError(f"invoke {cmd} 失败: {parsed.get('error')}")
    return parsed.get("data")


def save_png(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)


def run_smoke(config: argparse.Namespace) -> None:
    binary = Path(config.binary).resolve()
    if not binary.is_file():
        raise SmokeError(f"应用二进制不存在: {binary}（请先 cargo build -p rrss-desktop）")

    out_dir = Path(config.output_dir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    log = SmokeLogger(out_dir / "smoke.log")

    use_xvfb = should_manage_xvfb(config.xvfb)
    native_path = Path(config.native_driver).expanduser().resolve() if config.native_driver else None

    deps = check_dependencies(need_xvfb=use_xvfb, native_driver=config.native_driver or None)
    if not deps.ok:
        lines = ["依赖缺失，无法运行桌面 WebDriver 冒烟：", *[f"  - {m}" for m in deps.missing]]
        lines.extend(["安装提示：", *[f"  · {h}" for h in deps.hints]])
        raise SmokeError("\n".join(lines))

    work_parent = Path(config.work_dir).resolve() if config.work_dir else out_dir
    work_parent.mkdir(parents=True, exist_ok=True)
    # Never reuse or delete a database already present in a caller-supplied path.
    work_dir = Path(tempfile.mkdtemp(prefix="rrss-smoke-", dir=str(work_parent)))

    xdg_data_home = work_dir / "xdg-data"
    log(f"隔离数据目录 XDG_DATA_HOME={xdg_data_home}")

    procs = ProcessStack()
    driver = WebDriverClient(config.driver_url, timeout=config.webdriver_timeout)
    db_path: Path | None = None

    base_env = os.environ.copy()
    base_env["GDK_BACKEND"] = "x11"
    base_env["XDG_DATA_HOME"] = str(xdg_data_home)
    base_env.pop("RRSS_DB", None)
    if config.rrss_log:
        base_env["RRSS_LOG"] = config.rrss_log

    try:
        db_path = init_isolated_db(binary, xdg_data_home, log)
        log(
            f"测试库已写入 Pi 禁自动拉取/更新，AI 日预算={TEST_AI_DAILY_BUDGET} 且当天用量已满"
        )

        display = pick_isolated_display(config.display)
        if use_xvfb:
            xvfb_log = work_dir / "xvfb.log"
            log(f"启动 Xvfb DISPLAY={display}（-nolisten tcp）")
            xvfb_proc = start_xvfb(display, xvfb_log)
            procs.add(xvfb_proc)
            base_env["DISPLAY"] = display
            wait_for_xvfb(display, xvfb_proc, config.xvfb_ready_timeout)
        elif config.xvfb == "never" and not base_env.get("DISPLAY"):
            raise SmokeError("--xvfb never 需要外层已设置 DISPLAY（例如 xvfb-run）")

        driver_log = work_dir / "tauri-driver.log"
        parsed = urllib.parse.urlparse(config.driver_url)
        host = parsed.hostname or "127.0.0.1"
        port = parsed.port or 4444
        ensure_driver_port_free(host, port)

        log(f"启动 tauri-driver → {config.driver_url}")
        wk = resolve_webkit_driver(config.native_driver or None)
        driver_proc = start_tauri_driver(
            port=port,
            env=base_env,
            native_driver=wk,
            log_path=driver_log,
        )
        procs.add(driver_proc)
        wait_for_port(host, port, config.driver_boot_timeout, proc=driver_proc)

        caps: dict[str, Any] = {
            "browserName": "wry",
            "tauri:options": {"application": str(binary)},
        }
        log("创建 WebDriver 会话并拉起 RRSS…")
        driver.new_session(caps)
        driver.wait_css(".shell", config.app_ready_timeout)
        driver.wait_css(".brand", config.app_ready_timeout)

        has_tauri = driver.execute_sync(TAURI_HAS_INTERNALS)
        if not has_tauri:
            save_png(out_dir / "failure.png", driver.screenshot_png())
            raise SmokeError(
                "窗口内不存在 __TAURI_INTERNALS__.invoke — 可能是 Vite 预览而非 Tauri 二进制"
            )
        log("已确认 __TAURI_INTERNALS__")

        focus = tauri_invoke(
            driver,
            "focus_stream",
            {
                "expandedCluster": None,
                "feedId": None,
                "more": False,
                "excludeIds": None,
                "excludeClusterIds": None,
            },
        )
        if not isinstance(focus, dict) or "articles" not in focus:
            raise SmokeError(f"focus_stream 响应异常: {focus!r}")
        preview_titles = {
            "我把 MacBook 刘海变成了 AI agent 的工头",
            "Rust 1.97.1 点发行说明",
        }
        titles = {a.get("title") for a in focus.get("articles") or [] if isinstance(a, dict)}
        if titles & preview_titles:
            raise SmokeError(
                "focus_stream 含 preview.ts 样例标题，疑似未走真实后端"
            )
        log(f"focus_stream OK（{len(focus.get('articles') or [])} 条可见文章）")

        diag = tauri_invoke(driver, "diagnostics", {})
        db_line = diag.get("db") if isinstance(diag, dict) else None
        if not isinstance(db_line, str) or str(db_path) not in db_line:
            raise SmokeError(
                f"diagnostics 未指向隔离库。\n  期望包含: {db_path}\n  实际 db 字段: {db_line!r}"
            )
        log("diagnostics 已确认隔离 rrss.db")

        focus_nav = driver.wait_css('nav.nav button.nav-item[data-on="1"]', config.app_ready_timeout)
        nav_text = driver.get_text(focus_nav)
        if "焦点" not in nav_text:
            raise SmokeError(f"默认主导航不是今日焦点: {nav_text!r}")

        settings_btn = driver.find_xpath(
            "//div[contains(@class,'sidebar-foot')]//button[normalize-space()='设置']"
        )
        driver.click(settings_btn)
        driver.wait_css('div.settings-drawer[role="dialog"]', config.app_ready_timeout)
        log("已进入设置")

        rules_tab = driver.find_xpath(
            "//nav[@aria-label='设置分类']/button[@data-settings-tab='rules']"
        )
        driver.click(rules_tab)
        driver.wait_xpath(
            "//div[contains(@class,'drawer-body')]//button[contains(normalize-space(),'+ 新建规则')]",
            config.app_ready_timeout,
        )
        time.sleep(0.3)
        save_png(out_dir / "settings-rules.png", driver.screenshot_png())
        log(f"截图 → {out_dir / 'settings-rules.png'}")

        close_btn = driver.find_xpath(
            "//div[contains(@class,'drawer-head')]//button[normalize-space()='返回']"
        )
        driver.click(close_btn)
        driver.wait_css_gone('div.settings-drawer[role="dialog"]', config.app_ready_timeout)
        driver.wait_css_gone("div.overlay div.drawer-head", config.app_ready_timeout)
        driver.wait_css('nav.nav button.nav-item[data-on="1"]', config.app_ready_timeout)
        save_png(out_dir / "home-focus.png", driver.screenshot_png())
        log(f"截图 → {out_dir / 'home-focus.png'}")
        log("冒烟通过")

    except Exception:
        try:
            if driver.session_id:
                save_png(out_dir / "failure.png", driver.screenshot_png())
        except SmokeError:
            pass
        raise
    finally:
        driver.delete_session()
        procs.cleanup()
        copy_if_exists(work_dir / "tauri-driver.log", out_dir / "tauri-driver.log")
        copy_if_exists(work_dir / "xvfb.log", out_dir / "xvfb.log")
        if config.clean_work_dir:
            shutil.rmtree(work_dir, ignore_errors=True)


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        description="RRSS 桌面 Tauri WebDriver 无头冒烟（Linux/WSL + Xvfb）",
    )
    p.add_argument(
        "--binary",
        default=str(DEFAULT_BINARY),
        help=f"Tauri 二进制路径（默认 {repo_relative(DEFAULT_BINARY)}）",
    )
    p.add_argument(
        "--output-dir",
        required=True,
        help="日志与 PNG 输出目录（固定文件名，每次运行覆盖）",
    )
    p.add_argument(
        "--work-dir",
        default="",
        help="隔离 XDG/中间文件目录；默认在 output-dir 下创建临时目录",
    )
    p.add_argument(
        "--driver-url",
        default="http://127.0.0.1:4444",
        help="tauri-driver WebDriver 基址",
    )
    p.add_argument(
        "--native-driver",
        default="",
        help="WebKitWebDriver 可执行文件路径（优先于 PATH）",
    )
    p.add_argument(
        "--display",
        default=":99",
        help="首选 Xvfb DISPLAY；若 socket 已占用则递增编号",
    )
    p.add_argument(
        "--xvfb",
        choices=("auto", "always", "never"),
        default="auto",
        help="auto/always=脚本自启隔离 Xvfb；never=用外层 xvfb-run 且需已有 DISPLAY",
    )
    p.add_argument("--webdriver-timeout", type=float, default=30.0)
    p.add_argument("--driver-boot-timeout", type=float, default=45.0)
    p.add_argument("--xvfb-ready-timeout", type=float, default=15.0)
    p.add_argument("--app-ready-timeout", type=float, default=90.0)
    p.add_argument(
        "--rrss-log",
        default="",
        help="可选 RRSS_LOG 过滤器",
    )
    p.add_argument(
        "--keep-work-dir",
        action="store_true",
        help="保留此次新建的临时 work-dir（调试用；默认清理）",
    )
    return p


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    args.clean_work_dir = not args.keep_work_dir

    try:
        run_smoke(args)
        return 0
    except SmokeError as err:
        print(f"SMOKE FAILED: {err}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("中断", file=sys.stderr)
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
