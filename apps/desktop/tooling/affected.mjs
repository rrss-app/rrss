import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { DESKTOP_ROOT, isDirectRun } from "./cli.mjs";

// 按改动挑测试：单测交给 `vitest related`（按 import 关系），e2e 按上次运行记下的「用例 → 执行过的 src 文件」挑到单个用例，
// Rust 按改动的 crate 及依赖它的 crate。拿不准时本地只跑改过的 spec，全量 e2e 交给 CI（`--full` 仍可本地跑）。

const REPO = path.resolve(DESKTOP_ROOT, "../..");
const DESKTOP = "apps/desktop/";
const COVERAGE_DIR = path.join(DESKTOP_ROOT, "test-results/e2e-coverage");

/** 改了就全量跑 e2e：宿主模拟、样式、入口与构建配置影响所有页面，覆盖记录也看不出来。 */
const E2E_GLOBAL = [
  /^e2e\/(mockHost|fixtures)\.ts$/, /^src\/test\/fixtures\//, /^src\/styles\//, /^src\/main\.tsx$/, /^index\.html$/,
  /^(playwright\.config|vite\.config)\.ts$/, /^package\.json$/, /^pnpm-lock\.yaml$/,
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
 */
export function planAffected({ changed, coverage, crates, exists = () => true, full = false }) {
  const steps = [];
  const desktop = changed.filter((f) => f.startsWith(DESKTOP)).map((f) => f.slice(DESKTOP.length));
  const present = (f) => exists(DESKTOP + f);
  const code = changed.filter((f) => !f.endsWith(".md"));

  if (changed.some((f) => f.endsWith(".md"))) steps.push({ name: "文档", cmd: ["node", "scripts/check-docs.mjs"], cwd: "." });
  if (!code.length) return steps;

  // 类型、hooks、依赖与 IPC 契约：前端或桌面命令一改就跑，几秒钟。
  if (desktop.length || changed.some((f) => /^crates\/(rrss-app\/src\/views\.rs|rrss-desktop\/)/.test(f))) {
    steps.push({ name: "前端检查", cmd: ["pnpm", "run", "check:frontend"], cwd: DESKTOP });
  }
  if (desktop.some((f) => f.startsWith("tooling/"))) steps.push({ name: "工具测试", cmd: ["pnpm", "run", "test:tooling"], cwd: DESKTOP });

  const src = desktop.filter((f) => f.startsWith("src/") && /\.tsx?$/.test(f) && present(f));
  if (src.length) steps.push({ name: "单测（相关）", cmd: ["pnpm", "exec", "vitest", "related", "--run", ...src], cwd: DESKTOP });

  const e2e = pickSpecs(desktop, coverage, present);
  if (e2e.all && full) steps.push({ name: `e2e（全量：${e2e.reason}）`, cmd: ["pnpm", "exec", "playwright", "test", "--reporter=line"], cwd: DESKTOP });
  else if (e2e.all) {
    steps.push({ name: `e2e 全量留给 CI（${e2e.reason}）；本地要跑加 --full`, note: true });
    if (e2e.specs.length) steps.push({ name: "e2e（改过的 spec）", cmd: ["pnpm", "exec", "playwright", "test", "--reporter=line", ...e2e.specs], cwd: DESKTOP });
  } else if (e2e.specs.length) steps.push({ name: "e2e（相关）", cmd: ["pnpm", "exec", "playwright", "test", "--reporter=line", ...e2e.specs], cwd: DESKTOP });

  const touched = Object.entries(crates).filter(([, c]) => changed.some((f) => f.startsWith(`crates/${c.dir}/`))).map(([name]) => name);
  const rust = dependents(touched, crates);
  // 桌面 crate 的测试不需要打包界面；前端已由上面的步骤覆盖。
  if (rust.length) steps.push({ name: "Rust", cmd: ["cargo", "test", "--locked", ...rust.flatMap((name) => ["-p", name])], cwd: ".", env: { RRSS_SKIP_FRONTEND: "1" } });
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

function dependents(touched, crates) {
  const out = new Set(touched);
  let grew = true;
  while (grew) {
    grew = false;
    for (const [name, c] of Object.entries(crates)) {
      if (!out.has(name) && c.deps.some((d) => out.has(d))) { out.add(name); grew = true; }
    }
  }
  return [...out].sort();
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

function readCoverage() {
  if (!fs.existsSync(COVERAGE_DIR)) return null;
  const records = fs.readdirSync(COVERAGE_DIR).filter((f) => f.endsWith(".json"))
    .map((f) => JSON.parse(fs.readFileSync(path.join(COVERAGE_DIR, f), "utf8")))
    // 已删除的 spec 留下的记录不再有效。
    .filter((r) => fs.existsSync(path.join(DESKTOP_ROOT, r.spec)));
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

export function runAffected(argv = process.argv.slice(2)) {
  const dry = argv.includes("--dry-run");
  const full = argv.includes("--full");
  const baseAt = argv.indexOf("--base");
  const base = baseAt >= 0 ? argv[baseAt + 1] : "origin/main";
  const changed = changedFiles(base);
  const steps = planAffected({ changed, coverage: readCoverage(), crates: readCrates(), exists: (f) => fs.existsSync(path.join(REPO, f)), full });
  process.stdout.write(`相对 ${base} 改动 ${changed.length} 个文件。\n`);
  if (!steps.length) { process.stdout.write("没有需要跑的检查。\n"); return 0; }
  for (const step of steps) {
    if (step.note) { process.stdout.write(`\n· ${step.name}\n`); continue; }
    process.stdout.write(`\n▶ ${step.name}：${step.cmd.join(" ")}\n`);
    if (dry) continue;
    const result = spawnSync(step.cmd[0], step.cmd.slice(1), { cwd: path.join(REPO, step.cwd), stdio: "inherit", env: { ...process.env, ...step.env } });
    if (result.status !== 0) { process.stderr.write(`\n✗ ${step.name} 失败\n`); return result.status ?? 1; }
  }
  if (!dry) process.stdout.write("\n✓ 受影响的检查全部通过\n");
  return 0;
}

if (isDirectRun(import.meta.url)) {
  process.exit(runAffected());
}
