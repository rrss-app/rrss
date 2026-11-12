#!/usr/bin/env python3
"""Verify the real Tauri/Vite/MCP development entry under an isolated Xvfb display."""
from __future__ import annotations
import hashlib
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sqlite3
import struct
import subprocess
import sys
import tempfile
import time
import zlib

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("rrss_desktop_smoke", ROOT / "scripts/desktop-smoke.py")
smoke = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = smoke
spec.loader.exec_module(smoke)


def seed_digest_fixture(db: Path, work: Path) -> None:
    """The caller has just created this temporary DB; no user library is accepted."""
    if not db.is_relative_to(work):
        raise smoke.SmokeError("Digest fixture requires the temporary smoke database")
    fake = work / "fake-pi"
    shutil.copyfile(ROOT / "testdata/pi/digest_rpc.py", fake)
    fake.chmod(0o700)
    now = int(time.time() * 1000)
    with sqlite3.connect(db) as conn:
        conn.execute("INSERT INTO settings (key, value) VALUES ('pi_binary', ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", (str(fake),))
        conn.execute("INSERT OR IGNORE INTO ai_providers (id, name, base_url, api_key, model, kind, protocol, created_at) VALUES (61, '冒烟对话', 'http://127.0.0.1:1/v1', 'sk-smoke', 'smoke-model', 'openai_compat', 'chat', ?)", (now,))
        conn.execute("UPDATE task_bindings SET provider_id = 61 WHERE task = 'chat'")
        conn.execute("INSERT INTO feeds (id, url, title, last_fetched_at, interval_secs, created_at) VALUES (71, 'https://example.invalid/feed', '测试源', ?, 86400, ?)", (now, now))
        for article_id, title in ((71, "第一条测试文章"), (72, "第二条测试文章")):
            document = json.dumps({"v": 1, "blocks": [{"t": "p", "inlines": [{"t": "t", "text": title + "的本地正文"}]}]}, ensure_ascii=False)
            conn.execute("INSERT INTO articles (id, feed_id, guid, title, canonical_url, blocks_json, read, created_at) VALUES (?, 71, ?, ?, ?, ?, 1, ?)",
                         (article_id, str(article_id), title, f"https://example.invalid/{article_id}", document, now))
        items = json.dumps([{"sources": [{"article_id": aid, "title": title, "feed_title": "测试源"}]} for aid, title in ((71, "第一条测试文章"), (72, "第二条测试文章"))], ensure_ascii=False)
        conn.execute("INSERT INTO digests (id, period, period_start, body, items_json, created_at) VALUES (71, 'daily', 1, ?, ?, ?)", ("- 第一条测试文章\n- 第二条测试文章", items, now))
        conn.execute("INSERT INTO digests (id, period, period_start, body, items_json, created_at) VALUES (72, 'daily', 2, '- 较新一期，不能误用', '[]', ?)", (now,))


def seed_reader_fixture(db: Path, work: Path) -> None:
    """Seed only the disposable database, including an intentional write failure."""
    if not db.is_relative_to(work):
        raise smoke.SmokeError("Reader fixture requires the temporary smoke database")
    now = int(time.time() * 1000)
    document = json.dumps({"v": 1, "blocks": [{"t": "p", "inlines": [{"t": "t", "text": "Reader IPC fixture"}]}]})
    with sqlite3.connect(db) as connection:
        connection.execute("INSERT INTO feeds (id, url, title, last_fetched_at, interval_secs, created_at) VALUES (81, 'https://example.invalid/reader', 'Reader fixture', ?, 86400, ?)", (now, now))
        for article_id in (81, 82):
            connection.execute("INSERT INTO articles (id, feed_id, guid, title, blocks_json, read, published_at, created_at) VALUES (?, 81, ?, 'Reader fixture', ?, 0, ?, ?)", (article_id, str(article_id), document, now, now))
        connection.execute("CREATE TRIGGER reject_reader_mark BEFORE UPDATE OF read ON articles WHEN OLD.id = 82 BEGIN SELECT RAISE(ABORT, 'reader-write-rejected'); END")


def seed_settings_fixture(db: Path, work: Path) -> None:
    """Seed an endpoint, two feeds and a folder, plus write failures keyed on marker values."""
    if not db.is_relative_to(work):
        raise smoke.SmokeError("Settings fixture requires the temporary smoke database")
    now = int(time.time() * 1000)
    with sqlite3.connect(db) as connection:
        connection.execute("INSERT INTO ai_providers (id, name, base_url, api_key, model, kind, protocol, created_at) VALUES (91, '本机测试', 'http://127.0.0.1:1/v1', 'sk-seeded', 'seed-model', 'openai_compat', 'chat', ?)", (now,))
        connection.execute("CREATE TRIGGER reject_ai_smoke BEFORE UPDATE OF compat_json ON ai_providers WHEN NEW.name = '不该落库' BEGIN SELECT RAISE(ABORT, 'ai-write-rejected'); END")
        connection.execute("INSERT INTO folders (id, name, created_at) VALUES (93, '冒烟文件夹', ?)", (now,))
        connection.execute("INSERT INTO feeds (id, url, title, last_fetched_at, interval_secs, created_at) VALUES (92, 'https://example.invalid/settings-a', '源 A', ?, 86400, ?)", (now, now))
        connection.execute("INSERT INTO feeds (id, url, title, last_fetched_at, interval_secs, created_at) VALUES (94, 'https://example.invalid/settings-taken', '源 B', ?, 86400, ?)", (now, now))
        connection.execute("INSERT INTO threads (id, title, created_at, updated_at) VALUES (95, NULL, ?, ?)", (now, now))
        connection.executemany(
            "INSERT INTO messages (id, thread_id, role, content, created_at) VALUES (?, 95, 'user', ?, ?)",
            [(i, f"m{i:02d}", now + i) for i in range(1, 46)],
        )
        connection.execute("INSERT INTO digests (id, period, period_start, body, items_json, created_at) VALUES (96, 'daily', 1, '冒烟简报正文', '[]', ?)", (now,))


# Scale is read from apps/desktop/perf/scenes.ts. Real IPC page size stays the
# production focus/message window (40), not the mock host pageSize (minimal is 20).
REAL_FOCUS_PAGE_SIZE = 40
REAL_MESSAGE_PAGE = 40
PERF_THREAD_ID = 1001
PERF_ARTICLE_ID = 1
PERF_TIME_ORIGIN_MS = 1704096000000
PERF_SCENE_NAMES = ("minimal", "daily", "stress")
MEDIA_FEED_ID = 101
MEDIA_ARTICLE_ID = 101
MEDIA_SRC = "rrss-media:fixture"
MEDIA_PNG_WIDTH = 2
MEDIA_PNG_HEIGHT = 2


def load_perf_scenes() -> dict:
    """Load the only scene scale. No second copy of the counts lives in this file."""
    proc = subprocess.run(
        ["node", "--experimental-strip-types", str(ROOT / "scripts/perf-scene-config.mjs")],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        detail = (proc.stderr or proc.stdout or "perf scene config failed").strip()
        raise smoke.SmokeError(detail)
    try:
        scenes = json.loads(proc.stdout)
    except json.JSONDecodeError as err:
        raise smoke.SmokeError(f"perf scene config was not JSON: {err}") from err
    return assert_scene_scale(scenes)


def assert_scene_scale(scenes: dict) -> dict:
    if tuple(scenes) != PERF_SCENE_NAMES:
        raise smoke.SmokeError(f"perf scene names drifted: {list(scenes)}")
    required = ("articles", "messages", "blocks", "images", "mockPageSize", "streamSnapshots", "streamIntervalMs")
    for name in PERF_SCENE_NAMES:
        row = scenes[name]
        if not isinstance(row, dict):
            raise smoke.SmokeError(f"{name} scale is not an object")
        for key in required:
            value = row.get(key)
            if isinstance(value, bool) or not isinstance(value, int) or value < 0:
                raise smoke.SmokeError(f"{name}.{key} is not a non-negative integer")
        if row["images"] > row["blocks"]:
            raise smoke.SmokeError(f"{name} has more images than blocks")
    return scenes


def assert_same_scale(left: dict, right: dict) -> None:
    if left != right:
        raise smoke.SmokeError("perf scene scale drifted from apps/desktop/perf/scenes.ts")


def build_perf_expect(scene: str, scale: dict, db_bytes: int) -> dict:
    return {
        "scene": scene,
        "articles": scale["articles"],
        "messages": scale["messages"],
        "blocks": scale["blocks"],
        "images": scale["images"],
        "pageSize": REAL_FOCUS_PAGE_SIZE,
        "messagePage": REAL_MESSAGE_PAGE,
        "threadId": PERF_THREAD_ID,
        "articleId": PERF_ARTICLE_ID,
        "dbBytes": db_bytes,
    }


def resolve_perf_report(default_dir: Path, scene: str, explicit: str | None) -> Path:
    """Per-scene file unless the caller points at one json path. Directories do not collide."""
    filename = f"perf-host-{scene}.json"
    if not explicit:
        return default_dir / filename
    raw = Path(explicit)
    if explicit.endswith(("/", "\\")) or raw.suffix.lower() != ".json":
        return raw / filename
    return raw


def seed_perf_fixture(db: Path, work: Path, scene: str, scenes: dict | None = None) -> dict:
    """Fill only this disposable database with one perf scene. No network and no media files."""
    if not db.is_relative_to(work):
        raise smoke.SmokeError("Perf fixture requires the temporary smoke database")
    spec = (scenes or load_perf_scenes())[scene]
    if spec["images"] > spec["blocks"]:
        raise smoke.SmokeError("Perf scene has more images than blocks")
    slots = {min(spec["blocks"] - 1, (i * spec["blocks"]) // spec["images"]) for i in range(spec["images"])}
    if len(slots) != spec["images"]:
        raise smoke.SmokeError("Perf image slots collided")
    blocks = []
    for index in range(spec["blocks"]):
        if index in slots:
            image_id = len([slot for slot in slots if slot <= index])
            blocks.append({"t": "img", "src": f"rrss-perf:{image_id}", "alt": f"合成配图 {image_id}"})
        else:
            blocks.append({"t": "p", "inlines": [{"t": "t", "text": f"合成段落{index + 1:04d} 固定种子内容，不含密钥。"}]})
    long_document = json.dumps({"v": 1, "blocks": blocks}, ensure_ascii=False)
    short_document = json.dumps({"v": 1, "blocks": [{"t": "p", "inlines": [{"t": "t", "text": "短文"}]}]}, ensure_ascii=False)
    with sqlite3.connect(db) as connection:
        connection.execute("INSERT INTO settings (key, value) VALUES ('cluster_group_enabled', '0') ON CONFLICT(key) DO UPDATE SET value = excluded.value")
        fetched_at = int(time.time() * 1000)
        connection.execute(
            "INSERT INTO feeds (id, url, title, last_fetched_at, interval_secs, created_at) VALUES (1, 'https://example.invalid/perf', '合成源', ?, 86400, ?)",
            (fetched_at, PERF_TIME_ORIGIN_MS),
        )
        connection.executemany(
            "INSERT INTO articles (id, feed_id, guid, title, blocks_json, read, score, published_at, created_at) VALUES (?, 1, ?, ?, ?, 0, 0, ?, ?)",
            [
                (
                    article_id,
                    str(article_id),
                    "合成文章 0001 · 长文多图" if article_id == PERF_ARTICLE_ID else f"合成文章 {article_id:04d}",
                    long_document if article_id == PERF_ARTICLE_ID else short_document,
                    PERF_TIME_ORIGIN_MS + (article_id - 1) * 3_600_000,
                    PERF_TIME_ORIGIN_MS,
                )
                for article_id in range(1, spec["articles"] + 1)
            ],
        )
        connection.execute(
            "INSERT INTO threads (id, title, created_at, updated_at) VALUES (?, NULL, ?, ?)",
            (PERF_THREAD_ID, PERF_TIME_ORIGIN_MS, PERF_TIME_ORIGIN_MS),
        )
        connection.executemany(
            "INSERT INTO messages (id, thread_id, role, content, created_at) VALUES (?, ?, 'user', ?, ?)",
            [(i, PERF_THREAD_ID, f"m{i:04d}", PERF_TIME_ORIGIN_MS + i) for i in range(1, spec["messages"] + 1)],
        )
    return build_perf_expect(scene, spec, db.stat().st_size)


def encode_fixture_png(width: int = MEDIA_PNG_WIDTH, height: int = MEDIA_PNG_HEIGHT) -> bytes:
    """Smallest RGBA PNG the standard library can prove: signature, chunk CRC, IHDR size."""
    if width < 1 or height < 1:
        raise smoke.SmokeError("fixture png size must be positive")

    def chunk(tag: bytes, data: bytes) -> bytes:
        crc = zlib.crc32(tag + data) & 0xFFFFFFFF
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", crc)

    pixel = bytes((0x33, 0x66, 0x99, 0xFF))
    raw = b"".join(b"\x00" + pixel * width for _ in range(height))
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


def seed_media_fixture(db: Path, work: Path) -> dict:
    """One feed, one article, one local PNG. The image token is not HTTP, so startup refresh will not fetch it."""
    if not db.is_relative_to(work):
        raise smoke.SmokeError("Media fixture requires the temporary smoke database")
    now = int(time.time() * 1000)
    png = encode_fixture_png()
    directory = db.parent / "cache" / "media"
    if not directory.is_relative_to(work):
        raise smoke.SmokeError("Media cache escaped the temporary smoke directory")
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / (hashlib.sha256(MEDIA_SRC.encode("utf-8")).hexdigest() + ".png")
    path.write_bytes(png)
    document = json.dumps(
        {"v": 1, "blocks": [{"t": "img", "src": MEDIA_SRC, "alt": "夹具图"}]},
        ensure_ascii=False,
    )
    with sqlite3.connect(db) as connection:
        connection.execute(
            "INSERT INTO feeds (id, url, title, last_fetched_at, interval_secs, created_at) VALUES (?, 'https://example.invalid/media', '媒体夹具', ?, 86400, ?)",
            (MEDIA_FEED_ID, now, now),
        )
        connection.execute(
            "INSERT INTO articles (id, feed_id, guid, title, blocks_json, read, score, published_at, created_at) VALUES (?, ?, 'media-1', '媒体夹具', ?, 0, 0, ?, ?)",
            (MEDIA_ARTICLE_ID, MEDIA_FEED_ID, document, now, now),
        )
        cursor = connection.execute(
            "INSERT INTO media (article_id, url, local_path, mime, kind, bytes, created_at, accessed_at) VALUES (?, ?, ?, 'image/png', 'image', ?, ?, ?)",
            (MEDIA_ARTICLE_ID, MEDIA_SRC, str(path), len(png), now, now),
        )
        media_id = cursor.lastrowid
    return {
        "articleId": MEDIA_ARTICLE_ID,
        "feedId": MEDIA_FEED_ID,
        "src": MEDIA_SRC,
        "mediaId": media_id,
        "localPath": str(path),
        "mime": "image/png",
        "kind": "image",
        "width": MEDIA_PNG_WIDTH,
        "height": MEDIA_PNG_HEIGHT,
        "bytes": len(png),
    }


def seed_stream_fixture(db: Path, work: Path, scenes: dict) -> dict:
    """Point Pi at a local script that emits the minimal scene's snapshot count. No model."""
    if not db.is_relative_to(work):
        raise smoke.SmokeError("Stream fixture requires the temporary smoke database")
    spec = scenes["minimal"]
    snapshots = spec["streamSnapshots"]
    interval_ms = spec["streamIntervalMs"]
    if snapshots < 1 or interval_ms < 1:
        raise smoke.SmokeError("minimal stream scale is not positive")
    fake = work / "fake-pi"
    shutil.copyfile(ROOT / "testdata/pi/stream_rpc.py", fake)
    fake.chmod(0o700)
    with sqlite3.connect(db) as connection:
        connection.execute(
            "INSERT INTO settings (key, value) VALUES ('pi_binary', ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            (str(fake),),
        )
        connection.execute(
            "INSERT OR IGNORE INTO ai_providers (id, name, base_url, api_key, model, kind, protocol, created_at) VALUES (61, '冒烟对话', 'http://127.0.0.1:1/v1', 'sk-smoke', 'smoke-model', 'openai_compat', 'chat', ?)",
            (int(time.time() * 1000),),
        )
        connection.execute("UPDATE task_bindings SET provider_id = 61 WHERE task = 'chat'")
    return {"snapshots": snapshots, "intervalMs": interval_ms}


def argument_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--digest", action="store_true", help="Also test digest IPC with an isolated fake Pi RPC process")
    parser.add_argument("--reader", action="store_true", help="Test pure reader queries and explicit open actions through real IPC")
    parser.add_argument("--settings", action="store_true", help="Test the transactional AI / feed / Pi settings saves through real IPC")
    parser.add_argument("--perf", choices=PERF_SCENE_NAMES, help="Seed one perf scene and record real IPC size and time")
    parser.add_argument("--perf-output", help="Directory or .json path for the perf report. Default: test-results/development/perf-host-<scene>.json")
    parser.add_argument("--media", action="store_true", help="Verify a cached PNG through the real media protocol and reader")
    parser.add_argument("--stream", action="store_true", help="Count one minimal-scene stream through the real host")
    parser.add_argument("--switch", action="store_true", help="Time minimal hot view switches on the real host")
    parser.add_argument("--walk", action="store_true", help="Re-check open, search, Escape, and focus during stream")
    parser.add_argument("--output-dir", type=Path, default=ROOT / "test-results/development", help="Logs and reports for this run")
    return parser


def fixture_mode_error(args: argparse.Namespace) -> str | None:
    selected = [
        name
        for name, enabled in (
            ("--digest", args.digest),
            ("--reader", args.reader),
            ("--settings", args.settings),
            ("--perf", args.perf),
            ("--media", args.media),
            ("--stream", args.stream),
            ("--switch", args.switch),
            ("--walk", args.walk),
        )
        if enabled
    ]
    if len(selected) > 1:
        return "这些夹具不能同时使用: " + " ".join(selected)
    return None


def load_switch_budget() -> dict:
    """Warmup and sample count live in scenes.ts. This does not copy them."""
    proc = subprocess.run(
        [
            "node", "--experimental-strip-types", "--input-type=module", "-e",
            "import { SCENES } from './apps/desktop/perf/scenes.ts';"
            "const s = SCENES.minimal;"
            "process.stdout.write(JSON.stringify({warmup:s.warmup,samples:s.viewSwitchRepeats,interactionP95Ms:s.targets.interactionP95Ms}));",
        ],
        cwd=ROOT, capture_output=True, text=True, check=False,
    )
    if proc.returncode != 0:
        raise smoke.SmokeError((proc.stderr or proc.stdout or "switch budget failed").strip())
    try:
        budget = json.loads(proc.stdout)
    except json.JSONDecodeError as err:
        raise smoke.SmokeError(f"switch budget was not JSON: {err}") from err
    for key in ("warmup", "samples", "interactionP95Ms"):
        value = budget.get(key)
        if isinstance(value, bool) or not isinstance(value, int) or value < 1:
            raise smoke.SmokeError(f"switch budget {key} is not a positive integer")
    return budget


def seed_walk_fixture(db: Path, work: Path) -> None:
    """One visible article plus a short fake stream. The 8 deltas only check focus isolation."""
    if not db.is_relative_to(work):
        raise smoke.SmokeError("Walk fixture requires the temporary smoke database")
    fake = work / "fake-pi"
    shutil.copyfile(ROOT / "testdata/pi/stream_rpc.py", fake)
    fake.chmod(0o700)
    now = int(time.time() * 1000)
    title = "步行检索文章"
    document = json.dumps({"v": 1, "blocks": [{"t": "p", "inlines": [{"t": "t", "text": title}]}]}, ensure_ascii=False)
    with sqlite3.connect(db) as connection:
        connection.execute(
            "INSERT INTO settings (key, value) VALUES ('pi_binary', ?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            (str(fake),),
        )
        connection.execute(
            "INSERT INTO settings (key, value) VALUES ('focus_filter', 'all') ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        )
        connection.execute(
            "INSERT OR IGNORE INTO ai_providers (id, name, base_url, api_key, model, kind, protocol, created_at) VALUES (61, '冒烟对话', 'http://127.0.0.1:1/v1', 'sk-smoke', 'smoke-model', 'openai_compat', 'chat', ?)",
            (int(time.time() * 1000),),
        )
        connection.execute("UPDATE task_bindings SET provider_id = 61 WHERE task = 'chat'")
        connection.execute(
            "INSERT INTO feeds (id, url, title, last_fetched_at, interval_secs, created_at) VALUES (111, 'https://example.invalid/walk', '步行源', ?, 86400, ?)",
            (now, now),
        )
        connection.execute(
            "INSERT INTO articles (id, feed_id, guid, title, content_md, blocks_json, read, score, published_at, created_at) VALUES (111, 111, 'walk-1', ?, ?, ?, 0, 100, ?, ?)",
            (title, title, document, now, now),
        )


def walk_probe_env(enabled: bool) -> dict:
    if not enabled:
        return {"RRSS_TEST_WALK": "0"}
    return {
        "RRSS_TEST_WALK": "1",
        "RRSS_STREAM_SNAPSHOTS": "8",
        "RRSS_STREAM_INTERVAL_MS": "30",
    }


def switch_probe_env(expect: dict | None) -> dict:
    if not expect:
        return {"RRSS_TEST_SWITCH": "0", "RRSS_SWITCH_EXPECT": "{}"}
    return {
        "RRSS_TEST_SWITCH": "1",
        "RRSS_SWITCH_EXPECT": json.dumps(expect, ensure_ascii=False),
    }


def stream_probe_env(expect: dict | None) -> dict:
    """Publish the minimal stream count. The fake Pi reads the same numbers from the process environment."""
    if not expect:
        return {
            "RRSS_TEST_STREAM": "0",
            "RRSS_STREAM_EXPECT": "{}",
            "RRSS_STREAM_SNAPSHOTS": "",
            "RRSS_STREAM_INTERVAL_MS": "",
        }
    return {
        "RRSS_TEST_STREAM": "1",
        "RRSS_STREAM_EXPECT": json.dumps(expect, ensure_ascii=False),
        "RRSS_STREAM_SNAPSHOTS": str(expect["snapshots"]),
        "RRSS_STREAM_INTERVAL_MS": str(expect["intervalMs"]),
    }


def media_probe_env(expect: dict | None) -> dict:
    """Publish the cached-PNG fixture for scripts/mcp-probe.mjs."""
    return {
        "RRSS_TEST_MEDIA": "1" if expect is not None else "0",
        "RRSS_MEDIA_EXPECT": json.dumps(expect or {}, ensure_ascii=False),
    }


def main() -> int:
    parser = argument_parser()
    args = parser.parse_args()
    if args.perf_output and not args.perf:
        print("DEVELOPMENT SMOKE FAILED: --perf-output 需要同时指定 --perf", file=sys.stderr)
        return 1
    mix = fixture_mode_error(args)
    if mix:
        print(f"DEVELOPMENT SMOKE FAILED: {mix}", file=sys.stderr)
        return 1
    try:
        scenes = load_perf_scenes() if args.perf or args.stream or args.switch else None
    except smoke.SmokeError as err:
        print(f"DEVELOPMENT SMOKE FAILED: {err}", file=sys.stderr)
        return 1
    output = ROOT / "test-results/development"
    output.mkdir(parents=True, exist_ok=True)
    procs = smoke.ProcessStack()
    try:
        if not smoke.which("Xvfb"):
            raise smoke.SmokeError("Install xvfb before testing the development window")
        for port in (1420, 9223):
            smoke.ensure_driver_port_free("127.0.0.1", port)
        with tempfile.TemporaryDirectory(prefix="rrss-dev-", dir=output) as tmp:
            work = Path(tmp)
            try:
                db = smoke.init_isolated_db(ROOT / "target/debug/rrss", work / "data", print)
                if args.digest:
                    seed_digest_fixture(db, work)
                if args.reader:
                    seed_reader_fixture(db, work)
                if args.settings:
                    seed_settings_fixture(db, work)
                perf_expect = seed_perf_fixture(db, work, args.perf, scenes) if args.perf else None
                media_expect = seed_media_fixture(db, work) if args.media else None
                stream_expect = seed_stream_fixture(db, work, scenes) if args.stream else None
                switch_expect = None
                if args.switch:
                    seed_perf_fixture(db, work, "minimal", scenes)
                    switch_expect = load_switch_budget()
                if args.walk:
                    seed_walk_fixture(db, work)
                perf_report = resolve_perf_report(output, args.perf, args.perf_output) if args.perf else None
                if perf_report:
                    perf_report.parent.mkdir(parents=True, exist_ok=True)
                display = smoke.pick_isolated_display(":99")
                xvfb = smoke.start_xvfb(display, work / "xvfb.log")
                procs.add(xvfb)
                smoke.wait_for_xvfb(display, xvfb, 15)
                env = os.environ.copy()
                env.update({
                    "DISPLAY": display, "GDK_BACKEND": "x11",
                    "RRSS_DEV_ISOLATE": "1", "RRSS_DEV_XDG_DATA_HOME": str(work / "data"),
                    "RRSS_EXPECTED_DB": str(db), "RRSS_EXPECT_DEV": "1",
                    "RRSS_TEST_DIGEST": "1" if args.digest else "0",
                    "RRSS_TEST_READER": "1" if args.reader else "0",
                    "RRSS_TEST_SETTINGS": "1" if args.settings else "0",
                    "RRSS_TEST_PERF": "1" if perf_expect else "0",
                    "RRSS_PERF_EXPECT": json.dumps(perf_expect or {}, ensure_ascii=False),
                    "RRSS_PERF_REPORT": str(perf_report or ""),
                    **media_probe_env(media_expect),
                    **stream_probe_env(stream_expect),
                    **switch_probe_env(switch_expect),
                    **walk_probe_env(args.walk),
                })
                env.pop("WAYLAND_DISPLAY", None)
                log = (work / "development.log").open("wb")
                proc = subprocess.Popen(
                    ["bash", str(ROOT / "scripts/dev-desktop.sh"), "--mcp"],
                    cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT,
                    start_new_session=True,
                )
                dev = smoke.ManagedProcess("tauri-dev", proc, work / "development.log", log)
                procs.add(dev)
                smoke.wait_for_port("127.0.0.1", 1420, 30, proc=dev)
                smoke.wait_for_port("127.0.0.1", 9223, 420, proc=dev)
                probe = subprocess.run(
                    ["node", str(ROOT / "scripts/mcp-probe.mjs")],
                    cwd=ROOT, env=env, capture_output=True, text=True,
                    timeout={"minimal": 240, "daily": 360, "stress": 720}.get(args.perf or "", 180 if args.stream or args.switch or args.walk else 120),
                )
                (output / "mcp.log").write_text(probe.stdout + probe.stderr, encoding="utf-8")
                print(probe.stdout, end="")
                if probe.returncode:
                    raise smoke.SmokeError(probe.stderr)
                print("Tauri + Vite + MCP development smoke passed")
            finally:
                procs.cleanup()
                for name in ("development.log", "xvfb.log"):
                    smoke.copy_if_exists(work / name, output / name)
        return 0
    except (smoke.SmokeError, subprocess.TimeoutExpired) as err:
        print(f"DEVELOPMENT SMOKE FAILED: {err}", file=sys.stderr)
        return 1
    finally:
        procs.cleanup()


if __name__ == "__main__":
    raise SystemExit(main())
