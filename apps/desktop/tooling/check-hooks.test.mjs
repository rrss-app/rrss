import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { after, test } from "node:test";
import { runCheckHooks } from "./check-hooks.mjs";
import { captureIo, createFixtureKit } from "./fixture.mjs";

const { makeFixture, cleanup } = createFixtureKit();
after(cleanup);

const CONDITIONAL = `import { useState } from "react";
export function Bad({ flag }: { flag: boolean }) {
  if (flag) {
    useState(0);
  }
  return null;
}
`;

const MISSING_DEPS = `import { useEffect } from "react";
export function Missing({ value }: { value: number }) {
  useEffect(() => {
    console.log(value);
  }, []);
  return null;
}
`;

const TWO_MISSING = `import { useEffect } from "react";
export function Two({ a, b }: { a: number; b: number }) {
  useEffect(() => {
    console.log(a);
  }, []);
  useEffect(() => {
    console.log(b);
  }, []);
  return null;
}
`;

const CLEAN = `import { useEffect, useState } from "react";
export function Clean() {
  const [n, setN] = useState(0);
  useEffect(() => {
    setN(1);
  }, []);
  return n;
}
`;

const SYNTAX_ERROR = `export function Broken({
`;

const DISABLED = `import { useEffect } from "react";
export function Escape({ value }: { value: number }) {
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => {
    console.log(value);
  }, []);
  return null;
}
`;

async function run(argv) {
  const io = captureIo();
  const code = await runCheckHooks(argv, io);
  return { code, io };
}

test("unknown flag and missing baseline fail", async () => {
  const unknown = await run(["--wat"]);
  assert.equal(unknown.code, 1);
  assert.match(unknown.io.stderrText, /unknown argument/);

  const missing = await run(["--root", makeFixture({ "src/App.tsx": CLEAN }), "--baseline", "/tmp/rrss-missing-hooks.json"]);
  assert.equal(missing.code, 1);
  assert.match(missing.io.stderrText, /missing baseline/);
});

test("detects conditional hook and missing deps", async () => {
  const dir = makeFixture({
    "src/conditional.tsx": CONDITIONAL,
    "src/missing.tsx": MISSING_DEPS,
  });
  const write = await run(["--root", dir, "--write-baseline", "--baseline", path.join(dir, "empty.json")]);
  assert.equal(write.code, 0);
  const report = await run(["--root", dir, "--report", "--baseline", path.join(dir, "empty.json")]);
  assert.equal(report.code, 0);
  assert.match(report.io.stdoutText, /react-hooks\/rules-of-hooks/);
  assert.match(report.io.stdoutText, /react-hooks\/exhaustive-deps/);
});

test("stock baseline passes; new issue on same file fails; stale exemption fails", async () => {
  const dir = makeFixture({ "src/Widget.tsx": MISSING_DEPS });
  const baseline = path.join(dir, "hooks.json");
  assert.equal((await run(["--root", dir, "--write-baseline", "--baseline", baseline])).code, 0);
  assert.equal((await run(["--root", dir, "--baseline", baseline])).code, 0);

  fs.writeFileSync(path.join(dir, "src/Widget.tsx"), TWO_MISSING);
  const added = await run(["--root", dir, "--baseline", baseline]);
  assert.equal(added.code, 1);
  assert.match(added.io.stderrText, /new/);

  fs.writeFileSync(path.join(dir, "src/Widget.tsx"), CLEAN);
  const stale = await run(["--root", dir, "--baseline", baseline]);
  assert.equal(stale.code, 1);
  assert.match(stale.io.stderrText, /stale/);
});

test("clean fixture with empty baseline passes", async () => {
  const dir = makeFixture({ "src/Clean.tsx": CLEAN });
  const baseline = path.join(dir, "hooks.json");
  fs.writeFileSync(baseline, `${JSON.stringify({ version: 1, items: [] }, null, 2)}\n`);
  const result = await run(["--root", dir, "--baseline", baseline]);
  assert.equal(result.code, 0);
  assert.match(result.io.stdoutText, /0 new, 0 stale/);
});

test("parser errors fail and cannot be written into the baseline", async () => {
  const dir = makeFixture({ "src/Broken.tsx": SYNTAX_ERROR });
  const baseline = path.join(dir, "hooks.json");
  const write = await run(["--root", dir, "--write-baseline", "--baseline", baseline]);
  assert.equal(write.code, 1);
  assert.match(write.io.stderrText, /fatal|Parsing error|parser/i);
  assert.equal(fs.existsSync(baseline), false);

  const check = await run(["--root", dir, "--baseline", "/tmp/rrss-missing-hooks.json"]);
  assert.equal(check.code, 1);
  assert.match(check.io.stderrText, /cannot be baselined/);
});

test("inline eslint-disable does not hide exhaustive-deps", async () => {
  const dir = makeFixture({ "src/Escape.tsx": DISABLED });
  const baseline = path.join(dir, "hooks.json");
  fs.writeFileSync(baseline, `${JSON.stringify({ version: 1, items: [] }, null, 2)}\n`);
  const result = await run(["--root", dir, "--baseline", baseline]);
  assert.equal(result.code, 1);
  assert.match(result.io.stdoutText, /react-hooks\/exhaustive-deps/);
});
