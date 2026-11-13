import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { after, test } from "node:test";
import { parseAffectedArgs, planAffected, readCoverage, runSteps } from "./affected.mjs";
import { createFixtureKit } from "./fixture.mjs";

const kit = createFixtureKit();
after(() => kit.cleanup());

const crates = {
  "rrss-render": { dir: "rrss-render", deps: [] },
  "rrss-core": { dir: "rrss-core", deps: ["rrss-render"] },
  "rrss-app": { dir: "rrss-app", deps: ["rrss-core"] },
  "rrss-desktop": { dir: "rrss-desktop", deps: ["rrss-app", "rrss-core"] },
};
const coverage = [
  { spec: "e2e/advanced-settings.spec.ts", line: 13, files: ["src/App.tsx", "src/settings/AdvancedPane.tsx"] },
  { spec: "e2e/advanced-settings.spec.ts", line: 40, files: ["src/App.tsx"] },
  { spec: "e2e/model-settings.spec.ts", line: 20, files: ["src/App.tsx", "src/settings/ModelPane.tsx"] },
];
const plan = (changed, extra = {}) => planAffected({ changed, coverage, crates, ...extra }).map((s) => [s.name, s.cmd.slice(-3).join(" ")]);

test("a settings pane change runs related unit tests and only the e2e cases that executed it", () => {
  assert.deepEqual(plan(["apps/desktop/src/settings/AdvancedPane.tsx"]), [
    ["前端检查", "pnpm run check:frontend"],
    ["单测（相关）", "related --run src/settings/AdvancedPane.tsx"],
    ["e2e（相关）", "node tooling/e2e.mjs e2e/advanced-settings.spec.ts:13"],
  ]);
  // 改过的 spec 整个跑，不再拆行号。
  const e2e = planAffected({ changed: ["apps/desktop/src/App.tsx", "apps/desktop/e2e/advanced-settings.spec.ts"], coverage, crates }).at(-1);
  assert.deepEqual(e2e.cmd.slice(2), ["e2e/advanced-settings.spec.ts", "e2e/model-settings.spec.ts:20"]);
});

test("unknown impact requires an explicit path or changed spec; full regression is opt-in", () => {
  const e2e = (changed, extra) => planAffected({ changed, coverage, crates, ...extra }).filter((s) => s.name.startsWith("e2e"));
  for (const file of ["e2e/mockHost.ts", "e2e/shots.ts", "src/styles/app.css", "src/settings/NewPane.tsx", "dev-server.config.ts"]) {
    const steps = e2e([`apps/desktop/${file}`]);
    assert.equal(steps.length, 1);
    assert.equal(steps[0].blocked, true);
    assert.equal(steps[0].cmd, undefined);
  }
  const changed = ["apps/desktop/src/styles/app.css", "apps/desktop/e2e/advanced-settings.spec.ts"];
  assert.equal(e2e(changed)[0].blocked, false);
  assert.deepEqual(e2e(changed)[1].cmd, ["node", "tooling/e2e.mjs", "e2e/advanced-settings.spec.ts"]);
  assert.deepEqual(e2e(["apps/desktop/src/App.tsx"], { coverage: null, full: true })[0].cmd, ["node", "tooling/e2e.mjs", "--full"]);
  assert.deepEqual(e2e(["apps/desktop/src/App.tsx"], { coverage: null, e2eSpecs: ["e2e/shell.spec.ts:8"] })[0].cmd,
    ["node", "tooling/e2e.mjs", "e2e/shell.spec.ts:8"]);
});

test("explicit e2e selection accepts multiple cases and rejects missing or ambiguous options", () => {
  const args = parseAffectedArgs(["--base", "HEAD", "--dry-run", "--e2e", "e2e/shell.spec.ts:8", "--e2e", "e2e/tray.spec.ts"]);
  assert.deepEqual(args, { base: "HEAD", dry: true, full: false, e2eSpecs: ["e2e/shell.spec.ts:8", "e2e/tray.spec.ts"] });
  for (const argv of [["--base"], ["--e2e"], ["--e2e", ".*"], ["--ful"], ["--full", "--e2e", "e2e/tray.spec.ts"]]) {
    assert.throws(() => parseAffectedArgs(argv));
  }
});

test("coverage survives output cleanup and rejects stale line numbers or damaged records", () => {
  const source = 'test("path", () => {});';
  const record = { spec: "e2e/path.spec.ts", line: 1, files: ["src/path.ts"], specHash: createHash("sha256").update(source).digest("hex") };
  const root = kit.makeFixture({
    "e2e/path.spec.ts": source,
    ".cache/e2e-coverage/path.json": JSON.stringify(record),
    ".cache/e2e-coverage/broken.json": "{",
    ".cache/e2e-coverage/deleted.json": JSON.stringify({ ...record, spec: "e2e/deleted.spec.ts" }),
    "test-results/output.txt": "temporary",
  });
  const cache = path.join(root, ".cache/e2e-coverage");
  fs.rmSync(path.join(root, "test-results"), { recursive: true });
  assert.deepEqual(readCoverage(cache, root), [record]);
  fs.writeFileSync(path.join(root, record.spec), `\n${source}`);
  assert.equal(readCoverage(cache, root), null);
});

test("rust changes test only the changed modules and integration files; dependents are left to CI", () => {
  const rust = (changed) => planAffected({ changed, coverage, crates }).filter((s) => s.name.startsWith("Rust")).map((s) => s.cmd?.join(" ") ?? s.name);
  assert.deepEqual(rust(["crates/rrss-core/src/network.rs", "crates/rrss-core/src/network/tests.rs", "crates/rrss-core/src/db/migrations.rs"]),
    ["cargo test --locked -p rrss-core --lib -- db::migrations:: network::"]);
  assert.deepEqual(rust(["crates/rrss-core/src/rank/mod.rs", "crates/rrss-app/tests/pi_runtime.rs"]),
    ["cargo test --locked -p rrss-app --test pi_runtime", "cargo test --locked -p rrss-core --lib -- rank::"]);
  // 入口与非 Rust 文件看不出范围：整个库测试，但仍不带依赖它的 crate。
  assert.deepEqual(rust(["crates/rrss-core/src/schema.sql"]), ["cargo test --locked -p rrss-core --lib"]);
  assert.deepEqual(rust(["crates/rrss-desktop/src/commands/feeds.rs"]), ["Rust rrss-desktop 只有 check，留给 CI"]);
  assert.deepEqual(plan(["crates/rrss-app/src/views.rs"]).map(([name]) => name), ["前端检查", "Rust rrss-app（改动模块）"]);
  assert.deepEqual(plan(["doc/architecture.md"]), [["文档", "node scripts/check-docs.mjs"]]);
  // 删掉的 spec 不交给 Playwright。
  assert.deepEqual(plan(["apps/desktop/e2e/old.spec.ts"], { exists: () => false }), [["前端检查", "pnpm run check:frontend"]]);
});

test("a style change still runs the other checks, then fails on the unverified e2e", () => {
  const steps = planAffected({ changed: ["apps/desktop/src/styles/app.css"], coverage, crates });
  const ran = [];
  const sink = { write: () => {} };
  const spawn = (cmd, args) => { ran.push([cmd, ...args].join(" ")); return { status: 0 }; };
  assert.equal(runSteps(steps, { spawn, stdout: sink, stderr: sink }), 1);
  assert.ok(ran.length > 0, "type and contract checks still run");
  assert.ok(ran.every((cmd) => !cmd.includes("tooling/e2e.mjs")));
  assert.equal(runSteps(steps, { dry: true, spawn, stdout: sink, stderr: sink }), 0);
  const failing = () => ({ status: 3 });
  assert.equal(runSteps(steps, { spawn: failing, stdout: sink, stderr: sink }), 3);
});
