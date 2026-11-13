import assert from "node:assert/strict";
import { test } from "node:test";
import { LOCAL_TEST_LIMIT, parseE2eArgs, runE2e } from "./e2e.mjs";
import { captureIo } from "./fixture.mjs";

function runner(count, listing = {}) {
  const io = captureIo();
  const calls = [];
  const options = { ...io, cli: "playwright-cli", spawn(_bin, args) {
    calls.push(args);
    if (!args.includes("--list")) return { status: 0 };
    return { status: 0, stdout: JSON.stringify({ errors: [], suites: [{ suites: [{ specs: [{
      file: "path.spec.ts", line: 7, title: "path", tests: Array.from({ length: count }, () => ({ projectName: "chromium" })),
    }] }] }] }), ...listing };
  } };
  return { calls, run: (args) => runE2e(args, options) };
}

test("bare invocation refuses a full run without even collecting tests", () => {
  const r = runner(100);
  assert.equal(r.run([]), 1);
  assert.equal(r.calls.length, 0);
});

test("collection budget applies to matched executions even within one spec or a broad grep", () => {
  for (const args of [["e2e/path.spec.ts"], ["--grep", ".*"], ["e2e/path.spec.ts:7", "--repeat-each=20"]]) {
    const r = runner(LOCAL_TEST_LIMIT + 1);
    assert.equal(r.run(args), 1);
    assert.equal(r.calls.length, 1);
    assert.ok(r.calls[0].includes("--list"));
  }
});

test("a bounded selection runs exactly the filters that were collected", () => {
  const r = runner(LOCAL_TEST_LIMIT);
  const filters = ["e2e/path.spec.ts:7", "--grep", "path", "--workers=1"];
  assert.equal(r.run(filters), 0);
  assert.deepEqual(r.calls, [
    ["playwright-cli", "test", "--workers=2", ...filters, "--list", "--reporter=json"],
    ["playwright-cli", "test", "--workers=2", "--reporter=line", ...filters],
  ]);
});

test("dry runs and listing never execute tests; explicit full runs preserve CI shard flags", () => {
  const r = runner(2);
  assert.equal(r.run(["e2e/path.spec.ts", "--dry-run"]), 0);
  assert.equal(r.calls.length, 1);
  const list = runner(100);
  assert.equal(list.run(["--list"]), 0);
  assert.ok(list.calls[0].includes("--list"));
  const full = runner(100);
  assert.equal(full.run(["--full", "--shard=2/3", "--workers=4"]), 0);
  assert.deepEqual(full.calls, [["playwright-cli", "test", "--reporter=line", "--shard=2/3", "--workers=4"]]);
});

test("empty matches, collection failures and invalid reports never start a browser", () => {
  for (const [count, result] of [[0, {}], [2, { status: 2 }], [2, { stdout: "{" }], [2, { stdout: '{"errors":[{}],"suites":[]}' }]]) {
    const r = runner(count, result);
    assert.notEqual(r.run(["e2e/path.spec.ts"]), 0);
    assert.equal(r.calls.length, 1);
  }
});

test("intentional larger runs require a valid explicit budget", () => {
  for (const value of ["0", "-1", "1.2", "wat", "Infinity"]) assert.throws(() => parseE2eArgs(["--max-tests", value]));
  assert.throws(() => parseE2eArgs(["--max-tests"]));
  const r = runner(15);
  assert.equal(r.run(["e2e/path.spec.ts", "--max-tests=15"]), 0);
  assert.equal(r.calls.length, 2);
});

test("argument separators and interactive modes cannot bypass collection", () => {
  const r = runner(20);
  assert.equal(r.run(["--", "e2e/path.spec.ts"]), 1);
  assert.equal(r.calls.length, 1);
  assert.ok(!r.calls[0].includes("--"));
  for (const args of [["--ui"], ["--ui-port=9000"], ["--debug"], ["--grep", "--full"]]) {
    assert.throws(() => parseE2eArgs(args));
  }
});

test("possible retries count against the budget before any test executes", () => {
  const report = { config: { projects: [{ name: "chromium", retries: 10 }] }, suites: [{ specs: [{
    file: "path.spec.ts", line: 7, title: "path", tests: [{ projectName: "chromium" }],
  }] }] };
  const r = runner(1, { stdout: JSON.stringify(report) });
  assert.equal(r.run(["e2e/path.spec.ts", "--retries=10"]), 1);
  assert.equal(r.calls.length, 1);
});
