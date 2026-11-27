#!/usr/bin/env python3
"""Offline unit tests for .github/ci/prepare-runner.sh."""

from __future__ import annotations

import os
from pathlib import Path
import stat
import subprocess
import tempfile
import time
import unittest


ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / ".github" / "ci" / "prepare-runner.sh"


def write_exec(path: Path, body: str) -> None:
    path.write_text("#!/usr/bin/env bash\nset -euo pipefail\n" + body, encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


class PrepareRunnerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.cache = self.root / "ci-cache"
        self.runner_cache = self.root / "runner-cache"
        self.bin_dir = self.root / "bin"
        self.marker_dir = self.root / "markers"
        self.bin_dir.mkdir()
        self.marker_dir.mkdir()
        self.gh_env = self.root / "github_env"
        self.gh_path = self.root / "github_path"
        self.gh_env.write_text("", encoding="utf-8")
        self.gh_path.write_text("", encoding="utf-8")
        self.log = self.root / "calls.log"
        self.log.write_text("", encoding="utf-8")

    def tearDown(self) -> None:
        self.tmp.cleanup()

    def base_env(self, self_hosted: bool = True) -> dict[str, str]:
        env = os.environ.copy()
        env["PATH"] = f"{self.bin_dir}:{env.get('PATH', '')}"
        env["RUNNER_ENVIRONMENT"] = "self-hosted" if self_hosted else "github-hosted"
        env["RRSS_CI_CACHE"] = str(self.cache)
        env["RRSS_CI_RUNNER_CACHE"] = str(self.runner_cache)
        env["RRSS_PW_MARKER_DIR"] = str(self.marker_dir)
        env["GITHUB_ENV"] = str(self.gh_env)
        env["GITHUB_PATH"] = str(self.gh_path)
        env["CALL_LOG"] = str(self.log)
        return env

    def run_script(self, *args: str, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["bash", str(SCRIPT), *args],
            env=env or self.base_env(),
            text=True,
            capture_output=True,
            check=False,
        )

    def test_shared_target_scope_and_assets_and_cached_mold(self) -> None:
        (self.cache / "cargo" / "bin").mkdir(parents=True)
        (self.cache / "mold" / "bin").mkdir(parents=True)
        write_exec(self.cache / "cargo" / "bin" / "rustup", 'echo "rustup 1.28.0"\n')
        write_exec(self.cache / "mold" / "bin" / "mold", 'echo "mold 3.0.0"\n')

        env = self.base_env()
        env["RRSS_CI_SHARED_TARGET"] = "1"
        env["RRSS_CI_SHARED_CACHE_ASSETS"] = "1"
        res = self.run_script("--target-scope", "linux", "rust", env=env)
        self.assertEqual(res.returncode, 0, res.stderr)

        exported = self.gh_env.read_text(encoding="utf-8")
        self.assertIn(f"CARGO_TARGET_DIR={self.cache}/target-linux", exported)
        self.assertIn(f"PLAYWRIGHT_BROWSERS_PATH={self.cache}/ms-playwright", exported)
        self.assertIn(f"APT_CACHE_DIR={self.cache}/apt", exported)
        self.assertIn("RRSS_MOLD_READY=1", exported)

        paths = self.gh_path.read_text(encoding="utf-8")
        self.assertIn(f"{self.cache}/mold/bin", paths)

    def test_pnpm_install_prefers_offline_then_falls_back(self) -> None:
        write_exec(
            self.bin_dir / "pnpm",
            'echo "pnpm $*" >> "$CALL_LOG"\n'
            'if [[ "$*" == *"--offline"* ]]; then exit 1; fi\n'
            "exit 0\n",
        )
        res = self.run_script("pnpm-install")
        self.assertEqual(res.returncode, 0, res.stderr)
        calls = self.log.read_text(encoding="utf-8").splitlines()
        self.assertEqual(
            calls,
            [
                "pnpm install --frozen-lockfile --offline",
                "pnpm install --frozen-lockfile --prefer-offline",
            ],
        )

    def test_playwright_skips_network_when_browser_and_deps_ready(self) -> None:
        browsers = self.cache / "ms-playwright"
        (browsers / "chromium_headless_shell-1234").mkdir(parents=True)
        (browsers / ".browser-ready-1.52.0").write_text("", encoding="utf-8")
        write_exec(
            self.bin_dir / "dpkg-query",
            'for _ in 1 2 3 4 5; do echo installed; done\n',
        )
        write_exec(self.bin_dir / "pnpm", 'echo "pnpm $*" >> "$CALL_LOG"\nexit 99\n')

        env = self.base_env()
        env["PLAYWRIGHT_BROWSERS_PATH"] = str(browsers)
        res = self.run_script("ensure-playwright", "1.52.0", env=env)
        self.assertEqual(res.returncode, 0, res.stderr)
        self.assertEqual(self.log.read_text(encoding="utf-8").strip(), "")
        self.assertTrue((self.marker_dir / ".rrss-pw-deps-1.52.0").exists())

    def test_playwright_downloads_only_shell_on_version_bump(self) -> None:
        browsers = self.cache / "ms-playwright"
        (browsers / "chromium_headless_shell-1234").mkdir(parents=True)
        (browsers / ".browser-ready-1.52.0").write_text("", encoding="utf-8")
        write_exec(
            self.bin_dir / "dpkg-query",
            'for _ in 1 2 3 4 5; do echo installed; done\n',
        )
        write_exec(self.bin_dir / "pnpm", 'echo "pnpm $*" >> "$CALL_LOG"\n')

        env = self.base_env()
        env["PLAYWRIGHT_BROWSERS_PATH"] = str(browsers)
        res = self.run_script("ensure-playwright", "1.53.0", env=env)
        self.assertEqual(res.returncode, 0, res.stderr)
        self.assertEqual(
            self.log.read_text(encoding="utf-8").splitlines(),
            ["pnpm exec playwright install --only-shell chromium"],
        )
        self.assertTrue((browsers / ".browser-ready-1.53.0").exists())

    def test_playwright_installs_deps_only_when_browser_cached(self) -> None:
        browsers = self.cache / "ms-playwright"
        (browsers / "chromium_headless_shell-1234").mkdir(parents=True)
        (browsers / ".browser-ready-1.52.0").write_text("", encoding="utf-8")
        write_exec(self.bin_dir / "dpkg-query", "exit 1\n")
        write_exec(self.bin_dir / "pnpm", 'echo "pnpm $*" >> "$CALL_LOG"\n')

        env = self.base_env()
        env["PLAYWRIGHT_BROWSERS_PATH"] = str(browsers)
        res = self.run_script("ensure-playwright", "1.52.0", env=env)
        self.assertEqual(res.returncode, 0, res.stderr)
        self.assertEqual(
            self.log.read_text(encoding="utf-8").splitlines(),
            ["pnpm exec playwright install-deps chromium"],
        )

    def test_rustup_target_skips_when_already_installed(self) -> None:
        write_exec(
            self.bin_dir / "rustup",
            'echo "rustup $*" >> "$CALL_LOG"\n'
            'if [[ "$1" == "target" && "$2" == "list" ]]; then\n'
            '  echo "aarch64-unknown-linux-gnu"\n'
            '  echo "x86_64-pc-windows-gnu"\n'
            "  exit 0\n"
            "fi\n"
            "exit 99\n",
        )
        res = self.run_script("ensure-rust-target", "x86_64-pc-windows-gnu")
        self.assertEqual(res.returncode, 0, res.stderr)
        self.assertEqual(
            self.log.read_text(encoding="utf-8").splitlines(),
            ["rustup target list --installed"],
        )

    def test_fetch_rust_sets_offline_env_without_network(self) -> None:
        write_exec(self.bin_dir / "cargo", 'echo "cargo $*" >> "$CALL_LOG"\nexit 0\n')
        res = self.run_script("fetch-rust")
        self.assertEqual(res.returncode, 0, res.stderr)
        self.assertEqual(
            self.log.read_text(encoding="utf-8").splitlines(),
            ["cargo fetch --locked --offline"],
        )
        self.assertIn("CARGO_NET_OFFLINE=true", self.gh_env.read_text(encoding="utf-8"))

    def test_clean_target_prunes_old_workspace_hashes_and_keeps_active_deps(self) -> None:
        target = self.cache / "target-linux"
        ws_crate = target / "debug" / "build" / "rrss-app"
        dep_crate = target / "debug" / "build" / "tokio"
        ws_crate.mkdir(parents=True)
        dep_crate.mkdir(parents=True)

        now = time.time()
        # 10 workspace hashes -> keep newest 3
        for i in range(10):
            d = ws_crate / f"hash_{i:02d}"
            d.mkdir()
            (d / "dummy").write_text("x", encoding="utf-8")
            ts = now - (10 - i) * 60
            os.utime(d, (ts, ts))

        # 5 dep hashes (3 old > 20d, 2 fresh) with keep-deps=3, max-age=14d -> keep newest 3, drop oldest 2
        for i in range(5):
            d = dep_crate / f"dep_{i:02d}"
            d.mkdir()
            (d / "dummy").write_text("x", encoding="utf-8")
            age_days = 30 - i * 10 if i < 3 else 0
            ts = now - age_days * 86400 - (5 - i) * 60
            os.utime(d, (ts, ts))

        env = self.base_env()
        env["CARGO_TARGET_DIR"] = str(target)
        res = self.run_script(
            "clean-target",
            "--keep-workspace",
            "3",
            "--keep-deps",
            "3",
            "--max-age-days",
            "14",
            env=env,
        )
        self.assertEqual(res.returncode, 0, res.stderr)
        self.assertEqual(
            sorted(p.name for p in ws_crate.iterdir()),
            ["hash_07", "hash_08", "hash_09"],
        )
        self.assertEqual(
            sorted(p.name for p in dep_crate.iterdir()),
            ["dep_02", "dep_03", "dep_04"],
        )


if __name__ == "__main__":
    unittest.main()
