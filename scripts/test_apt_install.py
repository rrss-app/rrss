#!/usr/bin/env python3
"""Exercise CI apt retries with fake privileged commands and a controlled clock."""

from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path

INSTALLER = Path(__file__).resolve().parents[1] / ".github/apt/install.sh"

# No fixture command runs sudo, apt, dpkg, chown, or rm on the host.
FAKE_COMMAND = r'''
import json
import sys
from pathlib import Path

state_path, clock_path = map(Path, sys.argv[1:3])
args = sys.argv[3:]
state = json.loads(state_path.read_text())
noninteractive = args[0] == "DEBIAN_FRONTEND=noninteractive"
if noninteractive:
    args = args[1:]
limit = None
if args[0] == "timeout":
    assert args[1] == "--kill-after=5", args
    limit = int(args[2])
    assert 0 < limit < 900, args
    args = args[3:]

command = args[0]
if command == "apt-get":
    assert "-qq" not in args, args
    if "update" in args:
        phase = "update"
    elif "autoclean" in args:
        phase = "autoclean"
    elif "--download-only" in args:
        assert "--no-download" not in args, args
        phase = "download"
    else:
        assert "--no-download" in args, args
        phase = "install"
elif command == "dpkg":
    assert args[1:] == ["--configure", "-a"], args
    phase = "recover"
elif command == "dpkg-query":
    phase = "query"
elif command == "chown":
    phase = "permissions"
elif command == "rm":
    phase = "prepare"
else:
    raise AssertionError(args)

state["calls"].append({"phase": phase, "args": args, "limit": limit,
                       "noninteractive": noninteractive})
if phase == "query":
    result = state["query"]
elif phase in ("prepare", "autoclean", "permissions") and (
        not state["steps"] or state["steps"][0]["phase"] != phase):
    result = {"status": 0}
else:
    assert state["steps"], (phase, state["calls"])
    result = state["steps"].pop(0)
    assert result["phase"] == phase, (result, phase)

if "clock" in result:
    clock_path.write_text(str(result["clock"]))
if "stdout" in result:
    print(result["stdout"])
if "stderr" in result:
    print(result["stderr"], file=sys.stderr)
state_path.write_text(json.dumps(state))
sys.exit(result.get("status", 0))
'''

LAUNCHER = r'''
apt_fixture_command="$1"
apt_fixture_state="$2"
apt_fixture_clock="$3"
apt_fixture_installer="$4"
apt_fixture_packages="$5"
sudo() {
  local status=0
  python3 "$apt_fixture_command" "$apt_fixture_state" "$apt_fixture_clock" "$@" || status=$?
  SECONDS=$(<"$apt_fixture_clock")
  return "$status"
}
dpkg-query() {
  python3 "$apt_fixture_command" "$apt_fixture_state" "$apt_fixture_clock" dpkg-query "$@"
}
sleep() { :; }
id() { printf '1000\n'; }
source "$apt_fixture_installer" "$apt_fixture_packages"
'''


def step(phase: str, status: int = 0, **fields: object) -> dict:
    return {"phase": phase, "status": status, **fields}


class AptInstallTest(unittest.TestCase):
    def run_installer(self, steps: list[dict], *, cached: bool = False,
                      query: dict | None = None) -> tuple[subprocess.CompletedProcess, list[dict]]:
        with tempfile.TemporaryDirectory(prefix="rrss-apt-test-") as directory:
            root = Path(directory)
            packages = root / "packages.txt"
            packages.write_text("# Fictional fixture packages\nfixture-a\n\nfixture-b\n")
            cache = root / "cache"
            if cached:
                (cache / "lists").mkdir(parents=True)
                (cache / "lists/fixture_Packages.lz4").touch()
            fake = root / "fake.py"
            fake.write_text(FAKE_COMMAND)
            state = root / "state.json"
            state.write_text(json.dumps({"steps": steps, "calls": [], "query": query}))
            clock = root / "clock"
            clock.write_text("0")
            result = subprocess.run(
                ["bash", "-c", LAUNCHER, "apt-fixture", str(fake), str(state),
                 str(clock), str(INSTALLER), str(packages)],
                # A minimal, explicit environment avoids inheriting host proxy or auth settings.
                env={"PATH": "/usr/bin:/bin", "LC_ALL": "C", "APT_CACHE_DIR": str(cache),
                     "RUNNER_ENVIRONMENT": "self-hosted" if query else "github-hosted"},
                capture_output=True, text=True, timeout=15,
            )
            fixture = json.loads(state.read_text())
            self.assertNotIn("Traceback", result.stderr, result.stderr)
            self.assertEqual(fixture["steps"], [], result.stdout + result.stderr)
            return result, fixture["calls"]

    def phases(self, calls: list[dict]) -> list[str]:
        return [call["phase"] for call in calls
                if call["phase"] not in ("prepare", "autoclean", "permissions")]

    def test_cached_packages_never_contact_the_mirror(self) -> None:
        result, calls = self.run_installer([step("install")], cached=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.phases(calls), ["install"])
        self.assertEqual(calls[-1]["phase"], "permissions")

    def test_cold_cache_downloads_before_offline_install(self) -> None:
        result, calls = self.run_installer([step("update"), step("download"), step("install")])
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.phases(calls), ["update", "download", "install"])
        self.assertTrue(all(call["noninteractive"] for call in calls))

    def test_stale_cache_recovers_before_network_fallback(self) -> None:
        result, calls = self.run_installer(
            [step("install", 100), step("recover"), step("update"),
             step("download"), step("install")], cached=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.phases(calls), ["install", "recover", "update", "download", "install"])

    def test_download_timeout_retries_without_configuring_packages(self) -> None:
        result, calls = self.run_installer(
            [step("update"), step("download", 124, stderr="fixture download stalled"),
             step("update"), step("download"), step("install")],
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.phases(calls), ["update", "download", "update", "download", "install"])
        self.assertIn("stage=download_1 outcome=timeout exit_code=124", result.stdout)
        self.assertIn("fixture download stalled", result.stderr)

    def test_interrupted_install_recovers_before_retry(self) -> None:
        result, calls = self.run_installer(
            [step("update"), step("download"), step("install", 137), step("recover"),
             step("update"), step("download"), step("install")],
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.phases(calls),
                         ["update", "download", "install", "recover", "update", "download", "install"])
        self.assertIn("stage=install_1 outcome=killed exit_code=137", result.stdout)

    def test_failed_recovery_stops_further_installation(self) -> None:
        for code in (100, 124):
            with self.subTest(code=code):
                result, calls = self.run_installer([step("install", 100), step("recover", code)], cached=True)
                self.assertEqual(result.returncode, code, result.stdout + result.stderr)
                self.assertEqual(self.phases(calls), ["install", "recover"])
                self.assertTrue(calls[-1]["noninteractive"])
                self.assertLessEqual(calls[-1]["limit"], 60)

    def test_index_failure_preserves_exit_code_without_installing(self) -> None:
        result, calls = self.run_installer([step("update", 100)] * 3)
        self.assertEqual(result.returncode, 100, result.stdout + result.stderr)
        self.assertEqual(self.phases(calls), ["update"] * 3)
        self.assertIn("stage=update_3 outcome=failed exit_code=100", result.stdout)

    def test_later_attempt_only_receives_the_remaining_budget(self) -> None:
        result, calls = self.run_installer([
            step("update"), step("download", 124, clock=700),
            step("update", 124, clock=775),
        ])
        self.assertEqual(result.returncode, 124, result.stdout + result.stderr)
        self.assertEqual(self.phases(calls), ["update", "download", "update"])
        self.assertEqual(calls[-1]["limit"], 75)

    def test_exhausted_budget_prevents_starting_the_next_stage(self) -> None:
        result, calls = self.run_installer([step("update", clock=775)])
        self.assertEqual(result.returncode, 124, result.stdout + result.stderr)
        self.assertEqual(self.phases(calls), ["update"])
        self.assertIn("stage=download_1 outcome=budget_exhausted", result.stdout)

    def test_recovery_also_uses_the_shared_budget(self) -> None:
        result, calls = self.run_installer([
            step("update"), step("download"), step("install", 100, clock=730),
            step("recover", 124, clock=775),
        ])
        self.assertEqual(result.returncode, 124, result.stdout + result.stderr)
        self.assertEqual(calls[-1]["phase"], "recover")
        self.assertEqual(calls[-1]["limit"], 45)

    def test_self_hosted_installed_packages_skip_apt(self) -> None:
        result, calls = self.run_installer([], query={"stdout": "installed\ninstalled"})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.phases(calls), ["query"])
        self.assertEqual(len(calls), 1)

    def test_cleanup_timeout_still_returns_cache_ownership(self) -> None:
        result, calls = self.run_installer(
            [step("install"), step("autoclean", 124), step("permissions")], cached=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual([call["phase"] for call in calls[-2:]], ["autoclean", "permissions"])
        self.assertLessEqual(calls[-2]["limit"], 30)
        self.assertLessEqual(calls[-1]["limit"], 15)

    def test_failed_cache_ownership_prevents_reporting_success(self) -> None:
        result, calls = self.run_installer([step("install"), step("permissions", 100)], cached=True)
        self.assertEqual(result.returncode, 100, result.stdout + result.stderr)
        self.assertEqual(calls[-1]["phase"], "permissions")

    def test_self_hosted_missing_packages_use_the_normal_flow(self) -> None:
        result, calls = self.run_installer(
            [step("update"), step("download"), step("install")],
            query={"stdout": "installed\nnot-installed"},
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(self.phases(calls), ["query", "update", "download", "install"])


if __name__ == "__main__":
    unittest.main()
