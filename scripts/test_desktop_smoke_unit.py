#!/usr/bin/env python3
"""desktop-smoke.py 标准库单元测试（不启动 Tauri / WebDriver）。"""

from __future__ import annotations

import json
import sqlite3
import struct
import sys
import tempfile
import types
import unittest
import zlib
from pathlib import Path
from unittest import mock

SCRIPTS = Path(__file__).resolve().parent
_SMOKE_PATH = SCRIPTS / "desktop-smoke.py"
smoke = types.ModuleType("desktop_smoke")
smoke.__file__ = str(_SMOKE_PATH)
sys.modules["desktop_smoke"] = smoke
smoke.__dict__["__name__"] = "desktop_smoke"
exec(compile(_SMOKE_PATH.read_text(encoding="utf-8"), str(_SMOKE_PATH), "exec"), smoke.__dict__)


class PathHelpersTest(unittest.TestCase):
    def test_xdg_rrss_db(self) -> None:
        base = Path("/tmp/xdg")
        self.assertEqual(smoke.xdg_rrss_db(base), Path("/tmp/xdg/rrss/rrss.db"))

    def test_pick_isolated_display_skips_taken(self) -> None:
        def fake_x11_socket(display: str) -> mock.Mock:
            sock = mock.Mock()
            sock.exists.return_value = display == ":99"
            return sock

        with mock.patch.object(smoke, "x11_socket", side_effect=fake_x11_socket):
            self.assertEqual(smoke.pick_isolated_display(":99"), ":100")


class XvfbPolicyTest(unittest.TestCase):
    def test_auto_and_always_manage_xvfb_with_display(self) -> None:
        with mock.patch.dict("os.environ", {"DISPLAY": ":0"}, clear=False):
            self.assertTrue(smoke.should_manage_xvfb("auto"))
            self.assertTrue(smoke.should_manage_xvfb("always"))

    def test_never_skips_xvfb(self) -> None:
        self.assertFalse(smoke.should_manage_xvfb("never"))


class DepsReportTest(unittest.TestCase):
    def test_all_present(self) -> None:
        with mock.patch.object(smoke, "which", side_effect=lambda n: Path(f"/usr/bin/{n}")):
            rep = smoke.check_dependencies(need_xvfb=True)
        self.assertTrue(rep.ok)
        self.assertEqual(rep.missing, [])

    def test_missing_driver(self) -> None:
        def fake(which: str):
            if which == "tauri-driver":
                return None
            return Path(f"/usr/bin/{which}")

        with mock.patch.object(smoke, "which", side_effect=fake):
            rep = smoke.check_dependencies(need_xvfb=False)
        self.assertFalse(rep.ok)
        self.assertIn("tauri-driver", rep.missing)

    def test_native_driver_path_required(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            missing = Path(tmp) / "no-webkit-here"
            with mock.patch.object(smoke, "which", side_effect=lambda n: Path(f"/usr/bin/{n}")):
                rep = smoke.check_dependencies(need_xvfb=False, native_driver=str(missing))
            self.assertFalse(rep.ok)
            self.assertTrue(any("WebKitWebDriver" in m for m in rep.missing))

    def test_native_driver_path_ok(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            wk = Path(tmp) / "WebKitWebDriver"
            wk.write_text("", encoding="utf-8")

            def fake_which(name: str):
                if name == "tauri-driver":
                    return Path("/usr/bin/tauri-driver")
                return None

            with mock.patch.object(smoke, "which", side_effect=fake_which):
                rep = smoke.check_dependencies(need_xvfb=False, native_driver=str(wk))
            self.assertTrue(rep.ok, rep.missing)


class WebDriverClientTest(unittest.TestCase):
    def test_url_join(self) -> None:
        client = smoke.WebDriverClient("http://127.0.0.1:4444/")
        self.assertEqual(client._url("/session"), "http://127.0.0.1:4444/session")

    def test_find_error_uses_selector_value(self) -> None:
        client = smoke.WebDriverClient("http://127.0.0.1:4444/")
        client.session_id = "abc"
        with mock.patch.object(
            client,
            "_request",
            return_value={"value": {}},
        ):
            with self.assertRaises(smoke.SmokeError) as ctx:
                client.find("css selector", ".missing")
        self.assertIn("css selector", str(ctx.exception))
        self.assertIn(".missing", str(ctx.exception))

    def test_url_error_becomes_smoke_error(self) -> None:
        import urllib.error

        client = smoke.WebDriverClient("http://127.0.0.1:4444/")
        with mock.patch(
            "urllib.request.urlopen",
            side_effect=urllib.error.URLError("refused"),
        ):
            with self.assertRaises(smoke.SmokeError) as ctx:
                client._request("GET", "/status")
        self.assertIn("连接失败", str(ctx.exception))


class PortWaitTest(unittest.TestCase):
    def test_busy_port_rejected_before_driver(self) -> None:
        with mock.patch.object(smoke, "port_is_open", return_value=True):
            with self.assertRaises(smoke.SmokeError) as ctx:
                smoke.ensure_driver_port_free("127.0.0.1", 4444)
        self.assertIn("已被占用", str(ctx.exception))

    def test_driver_exit_before_port_ready(self) -> None:
        proc = mock.Mock()
        proc.name = "tauri-driver"
        proc.popen.poll.return_value = 1
        proc.popen.returncode = 1
        with mock.patch.object(smoke, "port_is_open", return_value=False):
            with self.assertRaises(smoke.SmokeError) as ctx:
                smoke.wait_for_port("127.0.0.1", 4444, 1.0, proc=proc)
        self.assertIn("tauri-driver", str(ctx.exception))


class ParserTest(unittest.TestCase):
    def test_help_does_not_run_smoke(self) -> None:
        with self.assertRaises(SystemExit) as ctx:
            smoke.main(["--help"])
        self.assertEqual(ctx.exception.code, 0)

    def test_requires_output_dir(self) -> None:
        with self.assertRaises(SystemExit):
            smoke.main([])


class PatchDbTest(unittest.TestCase):
    def test_patch_pi_and_exhausts_ai_budget(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            db = Path(tmp) / "rrss.db"
            conn = sqlite3.connect(db)
            conn.execute(
                "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)"
            )
            conn.execute(
                "CREATE TABLE ai_usage ("
                "day TEXT PRIMARY KEY, tokens INTEGER NOT NULL DEFAULT 0, "
                "prompt_tokens INTEGER NOT NULL DEFAULT 0, "
                "completion_tokens INTEGER NOT NULL DEFAULT 0)"
            )
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('pi_auto_download', '1')"
            )
            conn.commit()
            conn.close()
            smoke.patch_test_db(db)
            conn = sqlite3.connect(db)
            pi = conn.execute(
                "SELECT value FROM settings WHERE key='pi_auto_download'"
            ).fetchone()[0]
            budget = int(
                conn.execute(
                    "SELECT value FROM settings WHERE key='ai_daily_budget_tokens'"
                ).fetchone()[0]
            )
            used = conn.execute(
                "SELECT tokens FROM ai_usage WHERE day=?",
                (smoke.local_day(),),
            ).fetchone()[0]
            conn.close()
            self.assertEqual(pi, "0")
            self.assertGreater(budget, 0)
            self.assertGreaterEqual(used, budget)


class SmokeLoggerTest(unittest.TestCase):
    def test_truncates_each_run(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "smoke.log"
            path.write_text("old\n", encoding="utf-8")
            logger = smoke.SmokeLogger(path)
            logger("one")
            logger("two")
            text = path.read_text(encoding="utf-8")
            self.assertNotIn("old", text)
            self.assertIn("one", text)
            self.assertIn("two", text)


class XvfbArgsTest(unittest.TestCase):
    def test_xvfb_includes_nolisten_tcp(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp) / "xvfb.log"
            with mock.patch.object(smoke.subprocess, "Popen") as pop:
                pop.return_value = mock.Mock(pid=123)
                smoke.start_xvfb(":100", log)
            cmd = pop.call_args[0][0]
            self.assertEqual(cmd[0], "Xvfb")
            self.assertIn("-nolisten", cmd)
            self.assertIn("tcp", cmd)


class ManagedProcessTest(unittest.TestCase):
    def test_terminate_signals_process_group(self) -> None:
        popen = mock.Mock()
        popen.pid = 4242
        popen.poll.return_value = None
        popen.wait.return_value = 0
        proc = smoke.ManagedProcess("test", popen, log_handle=mock.Mock())
        with mock.patch.object(smoke.os, "killpg") as killpg:
            proc.terminate()
        self.assertEqual(killpg.call_args_list, [
            mock.call(4242, smoke.signal.SIGTERM),
            mock.call(4242, smoke.signal.SIGKILL),
        ])

    def test_cleans_group_after_parent_already_exited(self) -> None:
        popen = mock.Mock(pid=4242)
        popen.poll.return_value = 0
        proc = smoke.ManagedProcess("test", popen)
        with mock.patch.object(smoke.os, "killpg") as killpg:
            proc.terminate()
        killpg.assert_any_call(4242, smoke.signal.SIGTERM)


class ExistingDatabaseTest(unittest.TestCase):
    def test_refuses_to_overwrite_database(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            xdg = Path(tmp)
            db = smoke.xdg_rrss_db(xdg)
            db.parent.mkdir()
            db.write_bytes(b"keep existing data")
            with self.assertRaises(smoke.SmokeError):
                smoke.init_isolated_db(Path("/unused/rrss"), xdg, lambda _: None)
            self.assertEqual(db.read_bytes(), b"keep existing data")


class PerfHostReportTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        import importlib.util

        path = SCRIPTS / "dev-smoke.py"
        spec = importlib.util.spec_from_file_location("rrss_dev_smoke_unit", path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        cls.dev = module

    def test_default_reports_do_not_collide(self) -> None:
        base = Path("/tmp/rrss-perf-out")
        paths = [self.dev.resolve_perf_report(base, scene, None) for scene in ("minimal", "daily", "stress")]
        self.assertEqual(len(set(paths)), 3)
        self.assertEqual(paths[0].name, "perf-host-minimal.json")

    def test_explicit_directory_is_per_scene(self) -> None:
        path = self.dev.resolve_perf_report(Path("/unused"), "stress", "/tmp/keep/")
        self.assertEqual(path, Path("/tmp/keep/perf-host-stress.json"))
        exact = self.dev.resolve_perf_report(Path("/unused"), "minimal", "/tmp/keep/custom.json")
        self.assertEqual(exact, Path("/tmp/keep/custom.json"))

    def test_real_page_size_is_not_the_mock_page_size(self) -> None:
        scale = {"articles": 80, "messages": 20, "blocks": 40, "images": 4, "mockPageSize": 20}
        expect = self.dev.build_perf_expect("minimal", scale, 3)
        self.assertEqual(expect["pageSize"], 40)
        self.assertEqual(expect["messagePage"], 40)
        self.assertNotEqual(expect["pageSize"], scale["mockPageSize"])
        self.assertNotIn("mockPageSize", expect)

    def test_source_has_no_hardcoded_scene_table(self) -> None:
        text = (SCRIPTS / "dev-smoke.py").read_text(encoding="utf-8")
        self.assertNotIn("PERF_SCENES", text)

    def test_loader_matches_live_scenes_and_rejects_drift(self) -> None:
        live = self.dev.load_perf_scenes()
        self.assertEqual(live["minimal"], {
            "articles": 80, "messages": 20, "blocks": 40, "images": 4, "mockPageSize": 20,
            "streamSnapshots": 60, "streamIntervalMs": 30,
        })
        self.assertEqual(live["daily"]["articles"], 500)
        self.assertEqual(live["stress"]["messages"], 800)
        self.assertEqual(live["stress"]["images"], 64)
        drifted = json.loads(json.dumps(live))
        drifted["daily"]["messages"] = 199
        with self.assertRaises(self.dev.smoke.SmokeError):
            self.dev.assert_same_scale(live, drifted)
        broken = json.loads(json.dumps(live))
        del broken["minimal"]["messages"]
        with self.assertRaises(self.dev.smoke.SmokeError):
            self.dev.assert_scene_scale(broken)

    def test_loader_uses_node_scene_script(self) -> None:
        payload = {
            "minimal": {"articles": 80, "messages": 20, "blocks": 40, "images": 4, "mockPageSize": 20, "streamSnapshots": 60, "streamIntervalMs": 30},
            "daily": {"articles": 500, "messages": 200, "blocks": 400, "images": 24, "mockPageSize": 40, "streamSnapshots": 60, "streamIntervalMs": 30},
            "stress": {"articles": 3000, "messages": 800, "blocks": 1200, "images": 64, "mockPageSize": 40, "streamSnapshots": 60, "streamIntervalMs": 30},
        }
        proc = mock.Mock(returncode=0, stdout=json.dumps(payload), stderr="")
        with mock.patch.object(self.dev.subprocess, "run", return_value=proc) as run:
            loaded = self.dev.load_perf_scenes()
        self.assertEqual(loaded, payload)
        command = run.call_args.args[0]
        self.assertEqual(command[0], "node")
        self.assertIn("--experimental-strip-types", command)
        self.assertTrue(any(part.endswith("perf-scene-config.mjs") for part in command))
        self.assertEqual(run.call_args.kwargs["cwd"], self.dev.ROOT)

    def test_seed_perf_fixture_feed_stays_fresh_for_longest_window(self) -> None:
        """due_feeds is last_fetched_at + interval_secs*1000 <= now. Stress measures for 720s."""
        fixed_now_s = 1_800_000_000
        fixed_now_ms = fixed_now_s * 1000
        window_ms = 720 * 1000
        origin = self.dev.PERF_TIME_ORIGIN_MS
        scenes = {
            "minimal": {"articles": 3, "messages": 2, "blocks": 1, "images": 0, "mockPageSize": 20},
        }
        schema = """
            CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE feeds (
                id INTEGER PRIMARY KEY,
                url TEXT NOT NULL UNIQUE,
                title TEXT,
                last_fetched_at INTEGER,
                interval_secs INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE articles (
                id INTEGER PRIMARY KEY,
                feed_id INTEGER NOT NULL,
                guid TEXT,
                title TEXT NOT NULL,
                blocks_json TEXT,
                read INTEGER NOT NULL,
                score INTEGER NOT NULL,
                published_at INTEGER,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE threads (
                id INTEGER PRIMARY KEY,
                title TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE messages (
                id INTEGER PRIMARY KEY,
                thread_id INTEGER NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
        """

        def feed_due(last_fetched_at: int | None, interval_secs: int, now_ms: int) -> bool:
            if last_fetched_at is None:
                return True
            return last_fetched_at + interval_secs * 1000 <= now_ms

        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            db = work / "rrss.db"
            with sqlite3.connect(db) as conn:
                conn.executescript(schema)
            with mock.patch.object(self.dev.time, "time", return_value=fixed_now_s):
                self.dev.seed_perf_fixture(db, work, "minimal", scenes)
            with sqlite3.connect(db) as conn:
                url, last_fetched_at, interval_secs, created_at = conn.execute(
                    "SELECT url, last_fetched_at, interval_secs, created_at FROM feeds"
                ).fetchone()
                articles = conn.execute(
                    "SELECT id, published_at, created_at FROM articles ORDER BY id"
                ).fetchall()
                thread = conn.execute(
                    "SELECT created_at, updated_at FROM threads"
                ).fetchone()
                messages = conn.execute(
                    "SELECT id, created_at FROM messages ORDER BY id"
                ).fetchall()
        self.assertEqual(url, "https://example.invalid/perf")
        self.assertEqual(created_at, origin)
        self.assertEqual(thread, (origin, origin))
        self.assertEqual(messages, [(1, origin + 1), (2, origin + 2)])
        self.assertEqual(
            articles,
            [(article_id, origin + (article_id - 1) * 3_600_000, origin) for article_id in (1, 2, 3)],
        )
        for offset in range(0, window_ms + 1, 1000):
            self.assertFalse(
                feed_due(last_fetched_at, interval_secs, fixed_now_ms + offset),
                f"feed is due {offset}ms into the 720s window",
            )
        self.assertEqual(last_fetched_at, fixed_now_ms)


class MediaFixtureTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        import importlib.util

        path = SCRIPTS / "dev-smoke.py"
        spec = importlib.util.spec_from_file_location("rrss_dev_smoke_media", path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        cls.dev = module

    def test_seed_is_isolated_real_png_and_not_due(self) -> None:
        fixed_now_s = 1_800_000_000
        fixed_now_ms = fixed_now_s * 1000
        window_ms = 720 * 1000
        schema = """
            CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE feeds (
                id INTEGER PRIMARY KEY,
                url TEXT NOT NULL UNIQUE,
                title TEXT,
                last_fetched_at INTEGER,
                interval_secs INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE articles (
                id INTEGER PRIMARY KEY,
                feed_id INTEGER NOT NULL,
                guid TEXT,
                title TEXT NOT NULL,
                blocks_json TEXT,
                read INTEGER NOT NULL,
                score INTEGER NOT NULL,
                published_at INTEGER,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE media (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                article_id INTEGER,
                url TEXT NOT NULL,
                local_path TEXT,
                mime TEXT,
                kind TEXT NOT NULL DEFAULT 'image'
                    CHECK (kind IN ('image', 'video', 'iframe', 'other')),
                bytes INTEGER,
                created_at INTEGER NOT NULL,
                accessed_at INTEGER NOT NULL DEFAULT 0
            );
        """
        with tempfile.TemporaryDirectory() as outside, tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            with self.assertRaises(self.dev.smoke.SmokeError):
                self.dev.seed_media_fixture(Path(outside) / "rrss.db", work)
            db = work / "data" / "rrss" / "rrss.db"
            db.parent.mkdir(parents=True)
            with sqlite3.connect(db) as conn:
                conn.executescript(schema)
                conn.execute("INSERT INTO settings (key, value) VALUES ('pi_auto_download', '0')")
                conn.execute("INSERT INTO settings (key, value) VALUES ('pi_auto_update', '0')")
                conn.execute("INSERT INTO settings (key, value) VALUES ('ai_daily_budget_tokens', '1')")
            with mock.patch.object(self.dev.time, "time", return_value=fixed_now_s):
                expect = self.dev.seed_media_fixture(db, work)
            self.assertTrue(db.is_relative_to(work))
            image = Path(expect["localPath"])
            self.assertTrue(image.is_relative_to(work))
            self.assertEqual(image.parent, db.parent / "cache" / "media")
            self.assertTrue(image.is_file())
            with sqlite3.connect(db) as conn:
                settings = dict(conn.execute("SELECT key, value FROM settings").fetchall())
                feed = conn.execute(
                    "SELECT url, last_fetched_at, interval_secs FROM feeds"
                ).fetchone()
                document = conn.execute("SELECT blocks_json FROM articles WHERE id = ?", (expect["articleId"],)).fetchone()[0]
                media = conn.execute(
                    "SELECT id, article_id, url, local_path, mime, kind, bytes FROM media"
                ).fetchall()
            self.assertEqual(settings["pi_auto_download"], "0")
            self.assertEqual(settings["pi_auto_update"], "0")
            self.assertEqual(settings["ai_daily_budget_tokens"], "1")
            self.assertEqual(len(media), 1)
            media_id, article_id, url, local_path, mime, kind, size = media[0]
            src = json.loads(document)["blocks"][0]["src"]
            self.assertEqual(src, url)
            self.assertEqual(src, expect["src"])
            self.assertFalse(src.startswith(("http://", "https://")))
            self.assertEqual(article_id, expect["articleId"])
            self.assertEqual(media_id, expect["mediaId"])
            self.assertEqual(local_path, str(image))
            self.assertEqual(mime, "image/png")
            self.assertEqual(kind, "image")
            self.assertEqual(size, image.stat().st_size)
            self.assertEqual(expect["bytes"], size)
            width, height = self.parse_png(image.read_bytes())
            self.assertEqual((width, height), (expect["width"], expect["height"]))
            self.assertGreater(width, 0)
            self.assertGreater(height, 0)
            last_fetched_at, interval_secs = feed[1], feed[2]
            self.assertEqual(last_fetched_at, fixed_now_ms)

            def feed_due(now_ms: int) -> bool:
                if last_fetched_at is None:
                    return True
                return last_fetched_at + interval_secs * 1000 <= now_ms

            self.assertFalse(feed_due(fixed_now_ms))
            self.assertFalse(feed_due(fixed_now_ms + window_ms))
            self.assertTrue(feed_due(fixed_now_ms + interval_secs * 1000))

    def test_media_mode_is_exclusive_and_publishes_expect(self) -> None:
        parser = self.dev.argument_parser()
        self.assertIsNone(self.dev.fixture_mode_error(parser.parse_args(["--media"])))
        for extra in (["--digest"], ["--reader"], ["--settings"], ["--perf", "minimal"], ["--stream"]):
            error = self.dev.fixture_mode_error(parser.parse_args(["--media", *extra]))
            self.assertIsNotNone(error)
        expect = {"src": "rrss-media:fixture", "articleId": 101, "width": 2, "height": 2}
        enabled = self.dev.media_probe_env(expect)
        self.assertEqual(enabled["RRSS_TEST_MEDIA"], "1")
        self.assertEqual(json.loads(enabled["RRSS_MEDIA_EXPECT"]), expect)
        self.assertEqual(self.dev.media_probe_env(None)["RRSS_TEST_MEDIA"], "0")

    def parse_png(self, data: bytes) -> tuple[int, int]:
        signature = b"\x89PNG\r\n\x1a\n"
        self.assertTrue(data.startswith(signature))
        offset = len(signature)
        width = height = None
        saw_iend = False
        while offset + 12 <= len(data):
            length = struct.unpack(">I", data[offset:offset + 4])[0]
            tag = data[offset + 4:offset + 8]
            chunk = data[offset + 8:offset + 8 + length]
            crc_offset = offset + 8 + length
            self.assertGreaterEqual(len(data), crc_offset + 4)
            crc = struct.unpack(">I", data[crc_offset:crc_offset + 4])[0]
            self.assertEqual(crc, zlib.crc32(tag + chunk) & 0xFFFFFFFF, tag)
            if tag == b"IHDR":
                width, height, depth, color, compression, fltr, interlace = struct.unpack(">IIBBBBB", chunk)
                self.assertEqual((depth, color, compression, fltr, interlace), (8, 6, 0, 0, 0))
            if tag == b"IEND":
                saw_iend = True
                self.assertEqual(length, 0)
                break
            offset = crc_offset + 4
        self.assertTrue(saw_iend)
        self.assertIsNotNone(width)
        return width, height


if __name__ == "__main__":
    unittest.main()
