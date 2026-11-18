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

test("subscription moves select consumer tests and view changes check generated bindings", () => {
  const filters = (steps) => steps.find((s) => s.cmd?.[0] === "cargo").cmd.slice(7);
  const consumers = [
    "actions::feeds::", "queries::feeds::", "scheduler::tests::",
  ];
  for (const file of ["actions/feeds.rs", "actions/feeds/tests.rs", "queries/feeds.rs", "views/feeds.rs"]) {
    // Moved/deleted files must still select the owning behavior, not disappear.
    const steps = planAffected({ changed: [`crates/rrss-app/src/${file}`], coverage, crates, exists: () => false });
    const selected = filters(steps);
    const source = steps.find((step) => step.cmd?.[0] === "cargo").rustFilters;
    for (const consumer of consumers) {
      assert.ok(selected.includes(consumer), `${file}: ${consumer}`);
      assert.ok(source.required.includes(consumer), `${file}: ${consumer} must match`);
    }
    assert.ok(!selected.includes("views::feeds::"));
    if (file.startsWith("views/")) {
      assert.ok(selected.includes("export_ts::bindings_are_current"));
      assert.ok(source.required.includes("export_ts::bindings_are_current"));
      assert.ok(steps.some((step) => step.name === "前端检查"));
    }
  }
  const root = planAffected({ changed: ["crates/rrss-app/src/views.rs"], coverage, crates });
  assert.ok(filters(root).includes("export_ts::bindings_are_current"));
});

test("thread moves select actions, history queries and send-preparation consumers", () => {
  for (const file of ["actions/threads.rs", "actions/threads/tests.rs", "views/threads.rs", "queries/chat.rs", "chat.rs"]) {
    const steps = planAffected({ changed: [`crates/rrss-app/src/${file}`], coverage, crates, exists: () => false });
    const rust = steps.find((step) => step.cmd?.[0] === "cargo");
    const filters = rust.cmd.slice(7);
    for (const filter of ["actions::threads::", "queries::chat::", "chat::tests::"]) {
      assert.ok(filters.includes(filter), `${file}: ${filter}`);
      assert.ok(rust.rustFilters.required.includes(filter), `${file}: ${filter} must match`);
    }
    assert.ok(!filters.includes("views::threads::"));
    if (file.startsWith("views/")) {
      assert.ok(filters.includes("export_ts::bindings_are_current"));
      assert.ok(rust.rustFilters.required.includes("export_ts::bindings_are_current"));
      assert.ok(steps.some((step) => step.name === "前端检查"));
    }
  }
});

test("reading activity changes require root behavior and binding tests without local tests", () => {
  const behavior = "actions::tests::successful_feedback_events_and_sessions_request_rank_refresh";
  const bindings = "export_ts::bindings_are_current";
  for (const file of ["actions/reading_activity.rs", "views/reading_activity.rs"]) {
    const steps = planAffected({ changed: [`crates/rrss-app/src/${file}`], coverage, crates });
    const rust = steps.find((step) => step.cmd?.[0] === "cargo");
    assert.deepEqual(rust.rustFilters, { required: [behavior, bindings], path: [] });
  }
});

test("rule actions and contracts select write behavior and rule queries", () => {
  for (const [file, required] of [
    ["actions/rules.rs", ["actions::rules::", "queries::rules::"]],
    ["views/rules.rs", ["actions::rules::", "export_ts::bindings_are_current", "queries::rules::"]],
  ]) {
    const steps = planAffected({ changed: [`crates/rrss-app/src/${file}`], coverage, crates });
    const rust = steps.find((step) => step.cmd?.[0] === "cargo");
    assert.deepEqual(rust.rustFilters, { required, path: [] });
  }
});

test("storage actions and contracts select cleanup and backup consumers", () => {
  for (const file of ["actions/storage.rs", "views/storage.rs"]) {
    const steps = planAffected({ changed: [`crates/rrss-app/src/${file}`], coverage, crates });
    const rust = steps.find((step) => step.cmd?.[0] === "cargo");
    const expected = ["actions::storage::", "queries::backup::", "queries::storage::",
      "web::tests::retention_deletes_only_expired_temporary_rows_with_cascades_even_when_disabled",
      "web::tests::citation_and_chat_persistence_collect_before_expiry_cleanup"];
    if (file.startsWith("views/")) expected.push("export_ts::bindings_are_current");
    assert.deepEqual(rust.rustFilters, { required: expected.sort(), path: [] });
  }
});

test("subscription consumer filters still match after scheduler tests are renamed", () => {
  const steps = planAffected({ changed: ["crates/rrss-app/src/views/feeds.rs"], coverage, crates });
  const rust = steps.filter((step) => step.cmd?.[0] === "cargo");
  const listing = [
    "actions::feeds::tests::folder_moves_can_be_undone: test",
    "queries::feeds::tests::feed_tree: test",
    "scheduler::tests::renamed_add_regression: test",
    "scheduler::tests::renamed_import_regression: test",
    "export_ts::bindings_are_current: test",
    "5 tests, 0 benchmarks",
  ].join("\r\n");
  const ran = [];
  let output = "";
  const sink = { write: (text) => { output += text; } };
  const spawn = (cmd, args) => { ran.push([cmd, ...args]); return { status: 0, stdout: listing }; };
  assert.equal(runSteps(rust, { spawn, stdout: sink, stderr: sink }), 0);
  assert.equal(ran.length, 2);
  assert.deepEqual(ran[0], [...rust[0].cmd.slice(0, rust[0].cmd.indexOf("--")), "--", "--list"]);
  assert.deepEqual(ran[1], rust[0].cmd);
  assert.ok(output.includes("✓"));
  ran.length = 0;
  assert.equal(runSteps(rust, { dry: true, spawn, stdout: sink, stderr: sink }), 0);
  assert.equal(ran.length, 0);
});

test("zero matching required Rust filters fail even when another filter matches", () => {
  const step = { name: "Rust rrss-app", cmd: ["cargo", "test", "-p", "rrss-app", "--lib", "--", "actions::feeds::", "scheduler::tests::"], cwd: ".",
    rustFilters: { required: ["actions::feeds::", "scheduler::tests::"], path: [] } };
  for (const listing of ["", "actions::feeds::tests::folder_move: test\n1 test, 0 benchmarks\n"]) {
    const ran = [];
    let output = "";
    const sink = { write: (text) => { output += text; } };
    const spawn = (cmd, args) => { ran.push([cmd, ...args]); return { status: 0, stdout: listing }; };
    assert.equal(runSteps([step], { spawn, stdout: sink, stderr: sink }), 1);
    assert.equal(ran.length, 1, "tests must not run with an unmatched filter");
    assert.ok(output.includes("Rust rrss-app"));
    assert.ok(output.includes("scheduler::tests::"));
    assert.ok(!output.includes("✓"));
  }
});

test("unmatched path filters fall back to the entire library with one discovery call", () => {
  for (const [crate, source, filter] of [
    ["rrss-render", "sanitize.rs", "sanitize::"],
    ["rrss-core", "interests/prepare.rs", "interests::prepare::"],
  ]) {
    const steps = planAffected({ changed: [`crates/${crate}/src/${source}`], coverage, crates });
    const rust = steps.find((step) => step.cmd?.[0] === "cargo");
    assert.deepEqual(rust.rustFilters, { required: [], path: [filter] });
    const target = ["cargo", "test", "--locked", "-p", crate, "--lib"];
    const ran = [];
    let output = "";
    const sink = { write: (text) => { output += text; } };
    const spawn = (cmd, args) => { ran.push([cmd, ...args]); return { status: 0, stdout: "tests::public_behavior: test\n1 test, 0 benchmarks\n" }; };
    assert.equal(runSteps([rust], { spawn, stdout: sink, stderr: sink }), 0);
    assert.deepEqual(ran, [[...target, "--", "--list"], target]);
    assert.ok(output.includes(filter));
    assert.ok(output.includes(`running entire library tests: ${target.join(" ")}`));
    ran.length = 0;
    assert.equal(runSteps([rust], { dry: true, spawn, stdout: sink, stderr: sink }), 0);
    assert.equal(ran.length, 0);
  }
});

test("a missing required binding check fails despite a matching path filter", () => {
  const steps = planAffected({ changed: ["crates/rrss-app/src/views.rs"], coverage, crates });
  const rust = steps.find((step) => step.cmd?.[0] === "cargo");
  assert.deepEqual(rust.rustFilters, { required: ["export_ts::bindings_are_current"], path: ["views::"] });
  const ran = [];
  let output = "";
  const sink = { write: (text) => { output += text; } };
  const spawn = (cmd, args) => { ran.push([cmd, ...args]); return { status: 0, stdout: "views::tests::excerpt: test\n1 test, 0 benchmarks\n" }; };
  assert.equal(runSteps([rust], { spawn, stdout: sink, stderr: sink }), 1);
  assert.equal(ran.length, 1, "a missing required filter must prevent test execution");
  assert.ok(output.includes(rust.name));
  assert.ok(output.includes("export_ts::bindings_are_current"));
  assert.ok(!output.includes("✓"));
  assert.ok(!output.includes("running entire library tests"));
});

test("empty Rust targets and failed test discovery cannot report success", () => {
  const step = { name: "Rust rrss-app", cmd: ["cargo", "test", "-p", "rrss-app", "--test", "consumer"], cwd: "." };
  for (const result of [{ status: 0, stdout: "0 tests, 0 benchmarks\n" }, { status: 3 }, { status: null }]) {
    const ran = [];
    let output = "";
    const sink = { write: (text) => { output += text; } };
    const spawn = (cmd, args) => { ran.push([cmd, ...args]); return result; };
    assert.equal(runSteps([step], { spawn, stdout: sink, stderr: sink }), result.status || 1);
    assert.deepEqual(ran, [[...step.cmd, "--", "--list"]]);
    assert.ok(!output.includes("✓"));
  }
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
