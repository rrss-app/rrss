#!/usr/bin/env node
// Pi 升级核对（阶段 18「版本升级流程」）：下载独立二进制并校验 SHA256，用固定场景驱动真实 Pi 加本地模拟模型，
// 把 RPC 输出录到 testdata/pi/fixtures/<version>/，对比上一个 tested 版本写出差异报告，并跑 L1 / L3。
// 只生成报告和样本，不修改源码与 pi/compat.json；报告无破坏项、测试全绿后再手动改 compat.json。
//
//   node scripts/pi-compat.mjs <version> [--previous <version>] [--no-test]
//
// 二进制缓存在 .local/pi-compat/<version>/；只支持当前平台的发布包。

import { createHash } from "node:crypto";
import { spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const REPO = "earendil-works/pi";
const CACHE = join(ROOT, ".local/pi-compat");
const SCENARIOS = ["text", "tool_loop", "model_error", "auto_retry", "confirm", "abort"];

function fail(message) {
  console.error(`pi-compat: ${message}`);
  process.exit(1);
}

function parseArgs(argv) {
  const args = { version: null, previous: null, test: true };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a === "--previous") args.previous = argv[++i];
    else if (a === "--no-test") args.test = false;
    else if (!args.version) args.version = a.replace(/^v/, "");
    else fail(`unexpected argument ${a}`);
  }
  if (!args.version || !/^\d+\.\d+\.\d+$/.test(args.version)) {
    fail("usage: node scripts/pi-compat.mjs <x.y.z> [--previous <x.y.z>] [--no-test]");
  }
  if (!args.previous) {
    const tested = JSON.parse(readFileSync(join(ROOT, "pi/compat.json"), "utf8")).tested;
    if (tested !== args.version) args.previous = tested;
  }
  return args;
}

function platformAsset() {
  const arch = process.arch === "arm64" ? "arm64" : "x64";
  if (process.platform === "linux") return [`pi-linux-${arch}.tar.gz`, "pi/pi"];
  if (process.platform === "darwin") return [`pi-darwin-${arch}.tar.gz`, "pi/pi"];
  if (process.platform === "win32") return [`pi-windows-${arch}.zip`, "pi/pi.exe"];
  fail(`unsupported platform ${process.platform}`);
}

async function download(url) {
  const res = await fetch(url);
  if (!res.ok) fail(`GET ${url}: HTTP ${res.status}`);
  return Buffer.from(await res.arrayBuffer());
}

/** 下载、校验 SHA256 并解包；返回可执行文件路径。 */
async function ensureBinary(version) {
  const [asset, inner] = platformAsset();
  const dir = join(CACHE, version);
  const exe = join(dir, inner);
  if (existsSync(exe)) return exe;
  mkdirSync(dir, { recursive: true });
  const base = `https://github.com/${REPO}/releases/download/v${version}`;
  console.log(`downloading ${asset} ${version}`);
  const [bytes, sums] = await Promise.all([download(`${base}/${asset}`), download(`${base}/SHA256SUMS`)]);
  const expected = sums
    .toString()
    .split("\n")
    .map((l) => l.trim().split(/\s+/))
    .find(([, name]) => name === asset)?.[0];
  if (!expected) fail(`SHA256SUMS has no ${asset}`);
  const got = createHash("sha256").update(bytes).digest("hex");
  if (got !== expected) fail(`SHA256 mismatch for ${asset}: expected ${expected}, got ${got}`);
  const archive = join(dir, asset);
  writeFileSync(archive, bytes);
  const unpack = asset.endsWith(".zip")
    ? spawnSync("tar", ["-xf", archive, "-C", dir])
    : spawnSync("tar", ["-xzf", archive, "-C", dir]);
  if (unpack.status !== 0) fail(`unpack ${asset}: ${unpack.stderr}`);
  rmSync(archive);
  return exe;
}

// ---------------------------------------------------------------------------
// 本地模拟模型：按最后一条用户消息里的 [[场景]] 标记回应 OpenAI Chat Completions 流。
// ---------------------------------------------------------------------------

function sse(res, chunks, gapMs = 0) {
  res.writeHead(200, { "content-type": "text/event-stream" });
  let i = 0;
  const next = () => {
    if (res.destroyed) return;
    if (i >= chunks.length) {
      res.end("data: [DONE]\n\n");
      return;
    }
    const delta = chunks[i];
    i += 1;
    res.write(`data: ${JSON.stringify({ id: "c", object: "chat.completion.chunk", created: 0, model: "test-model", choices: [{ index: 0, ...delta }] })}\n\n`);
    setTimeout(next, gapMs);
  };
  next();
}

const text = (content, finish = null) => ({ delta: { content }, finish_reason: finish });
const toolCall = (name, args) => [
  { delta: { role: "assistant", tool_calls: [{ index: 0, id: "call_1", type: "function", function: { name, arguments: "" } }] }, finish_reason: null },
  { delta: { tool_calls: [{ index: 0, function: { arguments: JSON.stringify(args) } }] }, finish_reason: null },
  { delta: {}, finish_reason: "tool_calls" },
];

function startMock() {
  const server = createServer((req, res) => {
    let raw = "";
    req.on("data", (c) => (raw += c));
    req.on("end", () => {
      const body = JSON.parse(raw || "{}");
      const messages = body.messages || [];
      const users = messages.filter((m) => m.role === "user");
      const last = JSON.stringify(users[users.length - 1]?.content ?? "");
      const hasTool = messages.some((m) => m.role === "tool");
      const scenario = (last.match(/\[\[(\w+)\]\]/) || [])[1] || "text";
      server.requests.push({ scenario, body });
      if (scenario === "model_error") {
        res.writeHead(401, { "content-type": "application/json" });
        res.end(JSON.stringify({ error: { message: "Incorrect API key provided: sk-***", type: "invalid_request_error" } }));
      } else if (scenario === "auto_retry") {
        res.writeHead(529, { "content-type": "application/json" });
        res.end(JSON.stringify({ error: { message: "Overloaded", type: "overloaded_error" } }));
      } else if (scenario === "tool_loop" && !hasTool) {
        sse(res, toolCall("search_articles", { query: "rust" }));
      } else if (scenario === "tool_loop") {
        sse(res, [text("Rust 1.90 发布了。"), { delta: {}, finish_reason: "stop" }]);
      } else if (scenario === "confirm" && !hasTool) {
        sse(res, toolCall("fetch_url", { url: "https://example.org" }));
      } else if (scenario === "confirm") {
        sse(res, [text("已读取网页。"), { delta: {}, finish_reason: "stop" }]);
      } else if (scenario === "abort") {
        sse(res, Array.from({ length: 200 }, () => text("字")), 50);
      } else {
        sse(res, [text("你好，"), text("有什么可以帮你？"), { delta: {}, finish_reason: "stop" }]);
      }
    });
  });
  server.requests = [];
  return new Promise((ok) => server.listen(0, "127.0.0.1", () => ok(server)));
}

// ---------------------------------------------------------------------------
// 驱动真实 Pi：托管模式同款的隔离参数，RRSS 扩展配一个桩工具。
// ---------------------------------------------------------------------------

function workspace(port) {
  const work = mkdtempSync(join(tmpdir(), "pi-compat-"));
  const agent = join(work, "agent");
  const project = join(work, "project");
  mkdirSync(agent, { recursive: true });
  mkdirSync(join(project, ".pi"), { recursive: true });
  writeFileSync(
    join(agent, "models.json"),
    JSON.stringify({ providers: { rrss: { baseUrl: `http://127.0.0.1:${port}/v1`, api: "openai-completions", apiKey: "$RRSS_PI_KEY", models: [{ id: "test-model" }] } } }),
  );
  // 自动重试最多 3 次、间隔很短，录制不必等秒级退避。
  writeFileSync(join(agent, "settings.json"), JSON.stringify({ retry: { baseDelayMs: 5 }, cacheWarming: "off", enableInstallTelemetry: false }));
  writeFileSync(join(project, ".pi/settings.json"), JSON.stringify({ defaultTools: [] }));
  const manifest = join(work, "tool-manifest.json");
  writeFileSync(
    manifest,
    JSON.stringify({
      tools: [
        { name: "search_articles", label: "搜索本地文章", description: "Search local articles", parameters: [{ name: "query", kind: "string", description: "query" }] },
        { name: "fetch_url", label: "读取网页", description: "Fetch a web page", confirm: true, parameters: [{ name: "url", kind: "string", description: "url" }] },
      ],
    }),
  );
  const stub = join(work, "rrss-stub");
  writeFileSync(stub, `#!/bin/sh\necho '{"ok":true}'\n`, { mode: 0o755 });
  return { work, agent, project, manifest, stub };
}

function startPi(exe, ws, extraArgs = []) {
  const args = [
    "--mode", "rpc", "--approve", "--session-dir", join(ws.work, "sessions"),
    "--no-context-files", "--no-prompt-templates", "--no-skills",
    "-e", join(ROOT, "pi/extensions/rrss-tools.ts"),
    "--no-builtin-tools", "--tools", "search_articles,fetch_url",
    "--model", "rrss/test-model", "--thinking", "off", ...extraArgs,
  ];
  const env = {
    ...process.env,
    HOME: ws.work,
    PI_CODING_AGENT_DIR: ws.agent,
    PI_OFFLINE: "1",
    PI_SKIP_VERSION_CHECK: "1",
    RRSS_PI_KEY: "sk-test",
    RRSS_TOOL: ws.stub,
    RRSS_TOOL_MANIFEST: ws.manifest,
  };
  const child = spawn(exe, args, { cwd: ws.project, env, stdio: ["pipe", "pipe", "pipe"] });
  const pi = { child, lines: [], waiters: [], stderr: "" };
  let buf = "";
  child.stdout.on("data", (chunk) => {
    buf += chunk;
    let i;
    while ((i = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, i);
      buf = buf.slice(i + 1);
      if (!line.trim()) continue;
      pi.lines.push(line);
      for (const w of [...pi.waiters]) w();
    }
  });
  child.stderr.on("data", (c) => (pi.stderr += c));
  child.on("exit", () => pi.waiters.forEach((w) => w()));
  return pi;
}

function send(pi, command) {
  pi.child.stdin.write(`${JSON.stringify(command)}\n`);
}

/** 等到某一行满足条件；返回它的下标。 */
function waitFor(pi, from, pred, what, timeoutMs = 30_000) {
  return new Promise((ok, err) => {
    const check = () => {
      for (let i = from; i < pi.lines.length; i += 1) {
        const ev = JSON.parse(pi.lines[i]);
        if (pred(ev)) {
          pi.waiters = pi.waiters.filter((w) => w !== check);
          clearTimeout(timer);
          ok(i);
          return true;
        }
      }
      if (pi.child.exitCode !== null) {
        err(new Error(`pi exited while waiting for ${what}: ${pi.stderr.slice(-600)}`));
        return true;
      }
      return false;
    };
    const timer = setTimeout(() => err(new Error(`timed out waiting for ${what}`)), timeoutMs);
    if (!check()) pi.waiters.push(check);
  });
}

async function stop(pi) {
  pi.child.stdin.end();
  await new Promise((ok) => {
    const t = setTimeout(() => {
      pi.child.kill();
      ok();
    }, 5000);
    pi.child.on("exit", () => {
      clearTimeout(t);
      ok();
    });
  });
}

/** 一个场景：新进程、新会话，发 `rrss-1` 提示，录到 agent_settled。 */
async function record(exe, port, scenario) {
  const ws = workspace(port);
  const pi = startPi(exe, ws);
  send(pi, { id: "setup-1", type: "new_session" });
  await waitFor(pi, 0, (e) => e.type === "response" && e.id === "setup-1", "new_session");
  const from = pi.lines.length;
  send(pi, { id: "rrss-1", type: "prompt", message: `[[${scenario}]] 场景 ${scenario}` });
  if (scenario === "confirm") {
    const at = await waitFor(pi, from, (e) => e.type === "extension_ui_request" && e.method === "confirm", "confirm request");
    send(pi, { type: "extension_ui_response", id: JSON.parse(pi.lines[at]).id, confirmed: true });
  }
  if (scenario === "abort") {
    await waitFor(pi, from, (e) => e.type === "message_update" && e.assistantMessageEvent?.type === "text_delta", "first delta");
    send(pi, { id: "rrss-2", type: "abort" });
  }
  const end = await waitFor(pi, from, (e) => e.type === "agent_settled", "agent_settled", 60_000);
  let lines = pi.lines.slice(from, end + 1);
  if (scenario === "abort") {
    // abort 的响应可能晚于 agent_settled，一并收进来。
    const at = await waitFor(pi, from, (e) => e.type === "response" && e.id === "rrss-2", "abort response");
    if (at > end) lines = lines.concat(pi.lines.slice(end + 1, at + 1));
  }
  await stop(pi);
  rmSync(ws.work, { recursive: true, force: true });
  // 去掉本机路径：临时工作目录和仓库目录（Pi 的默认提示里带着发布包的文档路径）。
  return lines.map((l) => l.split(ws.work).join("<work>").split(ROOT).join("<repo>")).join("\n") + "\n";
}

/** 上一版本建的会话能否被新版本恢复，且带着旧上下文。 */
async function resumeCheck(prevExe, exe, port, mock) {
  const ws = workspace(port);
  const old = startPi(prevExe, ws);
  send(old, { id: "a", type: "new_session" });
  await waitFor(old, 0, (e) => e.id === "a", "new_session");
  send(old, { id: "b", type: "prompt", message: "旧会话里的暗号是 K-OLD" });
  await waitFor(old, 0, (e) => e.type === "agent_settled", "old turn");
  send(old, { id: "c", type: "get_state" });
  const at = await waitFor(old, 0, (e) => e.id === "c", "get_state");
  const session = JSON.parse(old.lines[at]).data.sessionFile;
  await stop(old);
  const cur = startPi(exe, ws);
  send(cur, { id: "d", type: "switch_session", sessionPath: session });
  const sw = JSON.parse(cur.lines[await waitFor(cur, 0, (e) => e.id === "d", "switch_session")]);
  let carried = false;
  if (sw.success && !sw.data?.cancelled) {
    const before = mock.requests.length;
    send(cur, { id: "e", type: "prompt", message: "暗号是什么" });
    await waitFor(cur, 0, (e) => e.type === "agent_settled", "resumed turn");
    carried = mock.requests.slice(before).some((r) => JSON.stringify(r.body).includes("K-OLD"));
  }
  await stop(cur);
  rmSync(ws.work, { recursive: true, force: true });
  return { switched: Boolean(sw.success && !sw.data?.cancelled), error: sw.error || null, carried };
}

// ---------------------------------------------------------------------------
// 差异报告：CLI 参数、RPC 命令与事件、设置键、环境变量。
// ---------------------------------------------------------------------------

function surface(exe) {
  const docs = join(dirname(exe), "docs");
  const files = existsSync(docs) ? readdirSync(docs) : [];
  const read = (name) => (existsSync(join(docs, name)) ? readFileSync(join(docs, name), "utf8") : "");
  // 0.87 起 RPC 文档拆成 rpc.md / rpc-commands.md / rpc-extension-ui.md，按前缀全部读入。
  const rpcDocs = files.filter((f) => /^rpc.*\.md$/.test(f)).map(read).join("\n");
  const help = spawnSync(exe, ["--help"], { encoding: "utf8", env: { ...process.env, PI_OFFLINE: "1" } }).stdout || "";
  const uniq = (xs) => [...new Set(xs)].sort();
  return {
    "CLI 参数": uniq([...help.matchAll(/(?:^|\s)(--[a-z][a-z0-9-]*)/g)].map((m) => m[1])),
    "RPC 命令": uniq([...rpcDocs.matchAll(/^#{3,4} ([a-z_]+)$/gm)].map((m) => m[1])),
    "事件类型": uniq([...(read("json.md") + rpcDocs).matchAll(/"type":\s*"([a-z_]+)"/g)].map((m) => m[1])),
    "设置键": uniq([...read("settings.md").matchAll(/^\| `([A-Za-z][\w.]*)` \|/gm)].map((m) => m[1])),
    "环境变量": uniq([...(read("environment-variables.md") + help).matchAll(/\b(PI_[A-Z_]+)\b/g)].map((m) => m[1])),
  };
}

function diffReport(version, previous, cur, prev) {
  const out = [
    `# Pi ${version} 兼容核对`,
    "",
    previous ? `对比上一 tested 版本 ${previous}。` : "没有上一版本可对比。",
    "",
    "CLI 参数取自 `pi --help`；RPC 命令、事件类型、设置键和环境变量取自发布包里的文档，文档改版会造成误报。判断事件是否还在，以下面「录制中出现的事件」和 L1 / L3 测试为准。",
    "",
  ];
  for (const [section, items] of Object.entries(cur)) {
    const before = prev?.[section] ?? [];
    const added = items.filter((x) => !before.includes(x));
    const removed = before.filter((x) => !items.includes(x));
    out.push(`## ${section}`, "");
    if (!prev) out.push(`${items.length} 项。`, "");
    else if (!added.length && !removed.length) out.push("无变化。", "");
    else {
      if (removed.length) out.push(`- **移除（可能破坏）**：${removed.map((x) => `\`${x}\``).join("、")}`);
      if (added.length) out.push(`- 新增：${added.map((x) => `\`${x}\``).join("、")}`);
      out.push("");
    }
  }
  return out;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const exe = await ensureBinary(args.version);
  const prevExe = args.previous ? await ensureBinary(args.previous) : null;
  const mock = await startMock();
  const port = mock.address().port;

  const dir = join(ROOT, "testdata/pi/fixtures", args.version);
  mkdirSync(dir, { recursive: true });
  for (const scenario of SCENARIOS) {
    console.log(`recording ${scenario}`);
    writeFileSync(join(dir, `${scenario}.jsonl`), await record(exe, port, scenario));
  }

  const report = diffReport(args.version, args.previous, surface(exe), prevExe ? surface(prevExe) : null);
  report.push("## 录制", "", `场景：${SCENARIOS.join("、")}。本地模拟模型，见 \`scripts/pi-compat.mjs\`。压缩不在录制范围内，仍由 \`docs-0.87/\` 的手写样本覆盖。`, "");
  const seen = new Set();
  for (const scenario of SCENARIOS) {
    for (const line of readFileSync(join(dir, `${scenario}.jsonl`), "utf8").split("\n").filter(Boolean)) {
      const ev = JSON.parse(line);
      seen.add(ev.type === "message_update" ? `message_update/${ev.assistantMessageEvent?.type}` : ev.type);
    }
  }
  report.push("## 录制中出现的事件", "", [...seen].sort().map((t) => `\`${t}\``).join("、"), "");
  if (prevExe) {
    const r = await resumeCheck(prevExe, exe, port, mock);
    report.push("## 会话恢复", "", `${args.previous} 创建的会话：切换${r.switched ? "成功" : `失败（${r.error}）`}；${r.carried ? "新一轮请求带着旧上下文" : "新一轮没有带上旧上下文"}。`, "");
  }
  mock.close();
  writeFileSync(join(dir, "REPORT.md"), report.join("\n"));
  console.log(`wrote ${dir}/REPORT.md`);

  if (args.test) {
    const run = (cmd, argv, env = {}) => {
      console.log(`$ ${cmd} ${argv.join(" ")}`);
      const r = spawnSync(cmd, argv, { cwd: ROOT, stdio: "inherit", env: { ...process.env, ...env } });
      if (r.status !== 0) fail(`${cmd} ${argv.join(" ")} failed`);
    };
    run("cargo", ["test", "-p", "rrss-ai", "--test", "pi_protocol"]);
    run("cargo", ["build", "-p", "rrss-desktop"]);
    run("cargo", ["test", "-p", "rrss-app", "--test", "pi_real"], { RRSS_PI_BIN: exe });
  }
  const cached = readdirSync(CACHE).join(", ");
  console.log(`done. cached binaries: ${cached}`);
}

main().catch((err) => fail(err.stack || String(err)));
