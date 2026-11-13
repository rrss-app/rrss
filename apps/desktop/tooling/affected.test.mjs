import assert from "node:assert/strict";
import { test } from "node:test";
import { planAffected } from "./affected.mjs";

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
    ["e2e（相关）", "test --reporter=line e2e/advanced-settings.spec.ts:13"],
  ]);
  // 改过的 spec 整个跑，不再拆行号。
  const e2e = planAffected({ changed: ["apps/desktop/src/App.tsx", "apps/desktop/e2e/advanced-settings.spec.ts"], coverage, crates }).at(-1);
  assert.deepEqual(e2e.cmd.slice(5), ["e2e/advanced-settings.spec.ts", "e2e/model-settings.spec.ts:20"]);
});

test("shared host fixtures, styles, unknown files or a missing record leave the full e2e run to CI unless --full", () => {
  const e2e = (changed, extra) => planAffected({ changed, coverage, crates, ...extra }).filter((s) => s.name.startsWith("e2e")).map((s) => s.name);
  assert.deepEqual(e2e(["apps/desktop/e2e/mockHost.ts"]), ["e2e 全量留给 CI（e2e/mockHost.ts）；本地要跑加 --full"]);
  assert.deepEqual(e2e(["apps/desktop/src/styles/app.css", "apps/desktop/e2e/advanced-settings.spec.ts"]),
    ["e2e 全量留给 CI（src/styles/app.css）；本地要跑加 --full", "e2e（改过的 spec）"]);
  assert.deepEqual(e2e(["apps/desktop/src/settings/NewPane.tsx"], { full: true }), ["e2e（全量：src/settings/NewPane.tsx 不在覆盖记录里）"]);
  assert.deepEqual(e2e(["apps/desktop/src/App.tsx"], { coverage: null, full: true }), ["e2e（全量：还没有 e2e 覆盖记录）"]);
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
