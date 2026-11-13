import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { DESKTOP_ROOT, isDirectRun } from "./cli.mjs";

// 按改动挑测试：单测交给 `vitest related`（按 import 关系），e2e 按上次运行记下的「用例 → 执行过的 src 文件」挑到单个用例，
// Rust 只跑改到的模块与集成测试文件。拿不准时本地只跑改过的 spec；依赖它的 crate 与全量 e2e 交给 CI（`--full` 仍可本地跑 e2e）。

const REPO = path.resolve(DESKTOP_ROOT, "../..");
const DESKTOP = "apps/desktop/";
const COVERAGE_DIR = path.join(DESKTOP_ROOT, ".cache/e2e-coverage");

/** 无法从执行记录精确选择：本地指定本次路径，跨域回归留给 CI。 */
const E2E_GLOBAL = [
  /^e2e\/(mockHost|fixtures)\.ts$/, /^src\/test\/fixtures\//, /^src\/styles\//, /^src\/main\.tsx$/, /^index\.html$/,
  /^(playwright\.config|vite\.config|dev-server\.config)\.ts$/, /^package\.json$/, /^pnpm-lock\.yaml$/,
  /^e2e\/(?!.*\.spec\.ts$)/,
];
/** 只有类型、不会在浏览器里执行的文件，交给类型检查。 */
const TYPES_ONLY = [/^src\/bindings\.ts$/, /\.d\.ts$/];
const isUnitTest = (file) => /\.test\.tsx?$/.test(file) || file.startsWith("src/test/");

/**
 * @param {object} input
 * @param {string[]} input.changed 相对仓库根的改动文件
 * @param {Array<{ spec: string; line: number; files: string[] }> | null} input.coverage 上次 e2e 的记录；没有时为 null
 * @param {Record<string, { dir: string; deps: string[] }>} input.crates crate 名 → 目录与内部依赖
 * @param {(file: string) => boolean} [input.exists] 删掉的文件不交给测试运行器
 * @param {boolean} [input.full] 需要全量 e2e 时本地也跑；默认留给 CI
 * @param {string[]} [input.e2eSpecs] 显式选择本次路径，覆盖自动选择；仍经过用例数限制
 */
export function planAffected({ changed, coverage, crates, exists = () => true, full = false, e2eSpecs = [] }) {
  const steps = [];
  const desktop = changed.filter((f) => f.startsWith(DESKTOP)).map((f) => f.slice(DESKTOP.length));
  const present = (f) => exists(DESKTOP + f);
  const code = changed.filter((f) => !f.endsWith(".md"));

  if (changed.some((f) => f.endsWith(".md"))) steps.push({ name: "文档", cmd: ["node", "scripts/check-docs.mjs"], cwd: "." });
  if (!code.length && !e2eSpecs.length) return steps;

  // 类型、hooks、依赖与 IPC 契约：前端或桌面命令一改就跑，几秒钟。
  if (desktop.length || changed.some((f) => /^crates\/(rrss-app\/src\/views\.rs|rrss-desktop\/)/.test(f))) {
    steps.push({ name: "前端检查", cmd: ["pnpm", "run", "check:frontend"], cwd: DESKTOP });
  }
  if (desktop.some((f) => f.startsWith("tooling/"))) steps.push({ name: "工具测试", cmd: ["pnpm", "run", "test:tooling"], cwd: DESKTOP });

  const src = desktop.filter((f) => f.startsWith("src/") && /\.tsx?$/.test(f) && present(f));
  if (src.length) steps.push({ name: "单测（相关）", cmd: ["pnpm", "exec", "vitest", "related", "--run", ...src], cwd: DESKTOP });

  const e2e = e2eSpecs.length ? { all: false, specs: [...new Set(e2eSpecs)] } : pickSpecs(desktop, coverage, present);
  const e2eCmd = ["node", "tooling/e2e.mjs"];
  if (e2e.all && full) steps.push({ name: `e2e（全量：${e2e.reason}）`, cmd: [...e2eCmd, "--full"], cwd: DESKTOP });
  else if (e2e.all) {
    steps.push({ name: `e2e 无法自动缩小范围（${e2e.reason}）；用 --e2e e2e/<name>.spec.ts[:line] 指定本次路径，跨域回归留给 CI`, note: true, blocked: !e2e.specs.length });
    if (e2e.specs.length) steps.push({ name: "e2e（改过的 spec）", cmd: [...e2eCmd, ...e2e.specs], cwd: DESKTOP });
  } else if (e2e.specs.length) steps.push({ name: e2eSpecs.length ? "e2e（显式选择）" : "e2e（相关）", cmd: [...e2eCmd, ...e2e.specs], cwd: DESKTOP });

  steps.push(...rustSteps(changed, crates));
  return steps;
}

function pickSpecs(desktop, coverage, present) {
  const specs = new Set(desktop.filter((f) => /^e2e\/.+\.spec\.ts$/.test(f) && present(f)));
  const global = desktop.find((f) => E2E_GLOBAL.some((re) => re.test(f)));
  if (global) return { all: true, reason: global, specs: [...specs].sort() };
  const runtime = desktop.filter((f) => f.startsWith("src/") && /\.tsx?$/.test(f) && !isUnitTest(f) && !TYPES_ONLY.some((re) => re.test(f)));
  if (runtime.length && !coverage) return { all: true, reason: "还没有 e2e 覆盖记录", specs: [...specs].sort() };
  for (const file of runtime) {
    const hits = coverage.filter((c) => c.files.includes(file));
    // 没有任何用例执行过：新文件或记录过旧，保守起见全量。
    if (!hits.length) return { all: true, reason: `${file} 不在覆盖记录里`, specs: [...specs].sort() };
    // 改过的 spec 整个文件跑；其余只跑执行过这个文件的用例（`文件:行号`）。
    for (const hit of hits) if (!specs.has(hit.spec)) specs.add(`${hit.spec}:${hit.line}`);
  }
  return { all: false, specs: [...specs].filter((s) => !s.includes(":") || !specs.has(s.split(":")[0])).sort() };
}

/** 没有单测、check 又依赖 WebKitGTK 的 crate：本地不跑，交给 CI。 */
const RUST_CHECK_ONLY = new Set(["rrss-desktop"]);

/**
 * 只测改到的模块：`src/a/b.rs`、`src/a/b/mod.rs`、`src/a/b/tests.rs` → 库测试过滤 `a::b::`；`tests/x.rs` → `--test x`。
 * 入口、`Cargo.toml` 与非 `.rs` 文件（SQL、数据）看不出范围，跑整个库测试。依赖它的 crate 和其余集成测试留给 CI。
 */
function rustSteps(changed, crates) {
  const steps = [];
  const env = { RRSS_SKIP_FRONTEND: "1" };
  for (const [name, { dir }] of Object.entries(crates).sort(([a], [b]) => a.localeCompare(b))) {
    const prefix = `crates/${dir}/`;
    const files = changed.filter((f) => f.startsWith(prefix)).map((f) => f.slice(prefix.length));
    if (!files.length) continue;
    if (RUST_CHECK_ONLY.has(name)) { steps.push({ name: `Rust ${name} 只有 check，留给 CI`, note: true }); continue; }
    const filters = new Set();
    const tests = new Set();
    let whole = false;
    for (const file of files) {
      const integration = /^tests\/([^/]+)\.rs$/.exec(file);
      if (integration) { tests.add(integration[1]); continue; }
      const parts = (/^src\/(.+)\.rs$/.exec(file)?.[1].split("/") ?? []).filter((part) => part !== "mod");
      if (parts.at(-1) === "tests") parts.pop();
      if (!parts.length || parts[0] === "bin" || (parts.length === 1 && ["lib", "main"].includes(parts[0]))) whole = true;
      else filters.add(`${parts.join("::")}::`);
    }
    if (whole || filters.size) {
      const filter = whole ? [] : ["--", ...[...filters].sort()];
      steps.push({ name: `Rust ${name}（${whole ? "整个库测试" : "改动模块"}）`, cmd: ["cargo", "test", "--locked", "-p", name, "--lib", ...filter], cwd: ".", env });
    }
    for (const test of [...tests].sort()) {
      steps.push({ name: `Rust ${name}（tests/${test}.rs）`, cmd: ["cargo", "test", "--locked", "-p", name, "--test", test], cwd: ".", env });
    }
  }
  return steps;
}

function git(...args) {
  return execFileSync("git", args, { cwd: REPO, encoding: "utf8" }).split("\n").filter(Boolean);
}

/** 相对 `base` 的全部改动：已提交、暂存、未暂存与未跟踪的文件。 */
function changedFiles(base) {
  const since = git("merge-base", base, "HEAD")[0];
  return [...new Set([
    ...git("diff", "--name-only", since),
    ...git("ls-files", "--others", "--exclude-standard"),
  ])].sort();
}

export function readCoverage(directory = COVERAGE_DIR, desktopRoot = DESKTOP_ROOT) {
  if (!fs.existsSync(directory)) return null;
  const records = [];
  const hashes = new Map();
  for (const file of fs.readdirSync(directory).filter((f) => f.endsWith(".json"))) {
    try {
      const record = JSON.parse(fs.readFileSync(path.join(directory, file), "utf8"));
      if (!/^e2e\/.+\.spec\.ts$/.test(record.spec) || record.spec.includes("..") ||
        !Number.isInteger(record.line) || record.line < 1 || !Array.isArray(record.files) ||
        !record.files.every((f) => typeof f === "string")) continue;
      if (!hashes.has(record.spec)) hashes.set(record.spec, createHash("sha256").update(fs.readFileSync(path.join(desktopRoot, record.spec))).digest("hex"));
      // Spec 改过后旧行号不可信；删除、过旧或未写完整的记录都不用于选择。
      if (record.specHash === hashes.get(record.spec)) records.push(record);
    } catch { /* 缓存只是选择提示，不阻止类型 / 单元检查，也不能触发全量。 */ }
  }
  return records.length ? records : null;
}

function readCrates() {
  const meta = JSON.parse(execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1", "--offline"], { cwd: REPO, encoding: "utf8" }));
  const names = new Set(meta.packages.map((p) => p.name));
  return Object.fromEntries(meta.packages.map((p) => [p.name, {
    dir: path.basename(path.dirname(p.manifest_path)),
    deps: p.dependencies.map((d) => d.name).filter((d) => names.has(d)),
  }]));
}

export function parseAffectedArgs(argv) {
  const options = { dry: false, full: false, base: "origin/main", e2eSpecs: [] };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--dry-run") options.dry = true;
    else if (arg === "--full") options.full = true;
    else if (arg === "--base" || arg === "--e2e") {
      const value = argv[++i];
      if (!value || value.startsWith("-")) throw new Error(`missing value for ${arg}`);
      if (arg === "--base") options.base = value;
      else if (/^e2e\/.+\.spec\.ts(?::\d+)?$/.test(value)) options.e2eSpecs.push(value);
      else throw new Error("--e2e requires e2e/<name>.spec.ts[:line]");
    } else throw new Error(`unknown argument: ${arg}`);
  }
  if (options.full && options.e2eSpecs.length) throw new Error("choose either --full or --e2e");
  return options;
}

/** 依次执行计划；e2e 选不出范围时其余检查照跑，最后报未验证的 e2e 并以失败结束，不能报成全部通过。 */
export function runSteps(steps, { dry = false, spawn = spawnSync, stdout = process.stdout, stderr = process.stderr } = {}) {
  if (!steps.length) { stdout.write("没有需要跑的检查。\n"); return 0; }
  for (const step of steps) {
    if (step.note) { stdout.write(`\n· ${step.name}\n`); continue; }
    stdout.write(`\n▶ ${step.name}：${step.cmd.join(" ")}\n`);
    if (dry) continue;
    const result = spawn(step.cmd[0], step.cmd.slice(1), { cwd: path.join(REPO, step.cwd), stdio: "inherit", env: { ...process.env, ...step.env } });
    if (result.status !== 0) { stderr.write(`\n✗ ${step.name} 失败\n`); return result.status ?? 1; }
  }
  const blocked = steps.filter((step) => step.blocked);
  if (blocked.length) {
    if (!dry) stderr.write(`\n✗ 其余检查已通过，e2e 尚未验证：\n${blocked.map((step) => `  ${step.name}`).join("\n")}\n`);
    return dry ? 0 : 1;
  }
  if (!dry) stdout.write("\n✓ 受影响的检查全部通过\n");
  return 0;
}

export function runAffected(argv = process.argv.slice(2)) {
  const { dry, full, base, e2eSpecs } = parseAffectedArgs(argv);
  const changed = changedFiles(base);
  const crates = changed.some((f) => f.startsWith("crates/") || /^Cargo\.(toml|lock)$/.test(f)) ? readCrates() : {};
  const steps = planAffected({ changed, coverage: readCoverage(), crates, exists: (f) => fs.existsSync(path.join(REPO, f)), full, e2eSpecs });
  process.stdout.write(`相对 ${base} 改动 ${changed.length} 个文件。\n`);
  return runSteps(steps, { dry });
}

if (isDirectRun(import.meta.url)) {
  try { process.exit(runAffected()); }
  catch (error) { process.stderr.write(`${error.message}\n`); process.exit(1); }
}
