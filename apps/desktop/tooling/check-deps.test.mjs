import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { after, test } from "node:test";
import { collectModuleRefs, runCheckDeps } from "./check-deps.mjs";
import { captureIo, createFixtureKit } from "./fixture.mjs";

const API = `export const api = {};\n`;
const INDEX = `export function open() { return 1; }\n`;
const HOST = `export function host() { return 2; }\n`;
const { makeFixture, cleanup } = createFixtureKit();
after(cleanup);

function run(argv) {
  const io = captureIo();
  const code = runCheckDeps(argv, io);
  return { code, io };
}

function emptyBaseline(dir) {
  const file = path.join(dir, "deps.json");
  fs.writeFileSync(file, `${JSON.stringify({ version: 1, items: [] }, null, 2)}\n`);
  return file;
}

test("unknown flag and missing value fail", () => {
  const unknown = run(["--nope"]);
  assert.equal(unknown.code, 1);
  assert.match(unknown.io.stderrText, /unknown argument/);
  const missing = run(["--root"]);
  assert.equal(missing.code, 1);
  assert.match(missing.io.stderrText, /missing value/);
});

test("parses import, re-export, dynamic import, require, and ImportTypeNode", () => {
  const dir = makeFixture({
    "src/lib/sample.ts": `
      import { a } from "./a";
      import type { B } from "./b";
      export { c } from "./c";
      export * from "./d";
      const dyn = () => import("./e");
      const req = require("./f");
      type T = import("./b").B;
      void a; void dyn; void req;
    `,
    "src/lib/a.ts": "export const a = 1;\n",
    "src/lib/b.ts": "export type B = number;\n",
    "src/lib/c.ts": "export const c = 1;\n",
    "src/lib/d.ts": "export const d = 1;\n",
    "src/lib/e.ts": "export const e = 1;\n",
    "src/lib/f.ts": "export const f = 1;\n",
  });
  const refs = collectModuleRefs(path.join(dir, "src/lib/sample.ts"), fs.readFileSync(path.join(dir, "src/lib/sample.ts"), "utf8"));
  const specs = refs.map((ref) => `${ref.kind}:${ref.spec}`).sort();
  assert.deepEqual(specs, [
    "dynamic-import:./e",
    "import-type-node:./b",
    "import-type:./b",
    "import:./a",
    "re-export:./c",
    "re-export:./d",
    "require:./f",
  ]);
});

test("production cannot import test, Tauri outside api.ts, or unclassified dirs", () => {
  const dir = makeFixture({
    "src/lib/api.ts": `import { invoke } from "@tauri-apps/api/core";\nexport const api = invoke;\n`,
    "src/lib/leak.ts": `import { clock } from "../test/clock";\nexport const x = clock;\n`,
    "src/test/clock.ts": `export const clock = 1;\n`,
    "src/components/Bypass.tsx": `import { invoke } from "@tauri-apps/api/core";\nexport const n = invoke;\n`,
    "src/widgets/sneaky.ts": `export const sneak = 1;\n`,
  });
  const baseline = emptyBaseline(dir);
  const result = run(["--root", dir, "--baseline", baseline]);
  assert.equal(result.code, 1);
  assert.match(result.io.stdoutText, /deps\/test-import/);
  assert.match(result.io.stdoutText, /deps\/tauri-bypass/);
  assert.match(result.io.stdoutText, /deps\/unclassified/);
  assert.doesNotMatch(result.io.stdoutText, /src\/lib\/api\.ts.*deps\/tauri-bypass/);
});

test("production cannot import tooling, config, unknown, or type-only test paths", () => {
  const dir = makeFixture({
    "src/lib/typed.ts": `export type T = import("../test/clock").Clock;\n`,
    "src/lib/tool.ts": `import { helper } from "../../tooling/helper";\nexport const h = helper;\n`,
    "src/lib/cfg.ts": `import { flag } from "../../vite.config";\nexport const f = flag;\n`,
    "src/lib/unknown.ts": `import { sneak } from "../widgets/sneaky";\nexport const s = sneak;\n`,
    "src/test/clock.ts": `export type Clock = number;\n`,
    "src/widgets/sneaky.ts": `export const sneak = 1;\n`,
    "tooling/helper.ts": `export const helper = 1;\n`,
    "vite.config.ts": `export const flag = 1;\n`,
  });
  const result = run(["--root", dir, "--baseline", emptyBaseline(dir)]);
  assert.equal(result.code, 1);
  assert.match(result.io.stdoutText, /deps\/test-import/);
  assert.match(result.io.stdoutText, /src\/lib\/typed\.ts/);
  assert.match(result.io.stdoutText, /deps\/non-production/);
  assert.match(result.io.stdoutText, /src\/lib\/tool\.ts/);
  assert.match(result.io.stdoutText, /src\/lib\/cfg\.ts/);
  assert.match(result.io.stdoutText, /src\/lib\/unknown\.ts/);
});

test("dotted module names such as ./dev-server.config resolve to .ts files", () => {
  const dir = makeFixture({
    "dev-server.config.ts": `export const port = 1;\n`,
    "vite.config.ts": `import { port } from "./dev-server.config";\nexport const p = port;\n`,
  });
  const result = run(["--root", dir, "--baseline", emptyBaseline(dir)]);
  assert.doesNotMatch(result.io.stdoutText, /deps\/unresolved/);
});

test("shared and lib cannot import app or features; app public feature index is allowed", () => {
  const bad = makeFixture({
    "src/shared/util.ts": `import { App } from "../app/shell";\nimport { open } from "../features/reader";\nexport const x = App;\nexport const y = open;\n`,
    "src/lib/back.ts": `import { App } from "../app/shell";\nexport const z = App;\n`,
    "src/app/shell.ts": `export const App = 1;\n`,
    "src/features/reader/index.ts": INDEX,
  });
  const badResult = run(["--root", bad, "--baseline", emptyBaseline(bad)]);
  assert.equal(badResult.code, 1);
  assert.match(badResult.io.stdoutText, /deps\/layer/);
  assert.match(badResult.io.stdoutText, /shared may not import app/);
  assert.match(badResult.io.stdoutText, /shared may not import feature:reader/);
  assert.match(badResult.io.stdoutText, /lib may not import app/);

  const allowed = makeFixture({
    "src/App.tsx": `import { open } from "./features/reader";\nexport const App = open;\n`,
    "src/app/shell.ts": `import { open } from "../features/reader";\nexport const App = open;\n`,
    "src/lib/api.ts": API,
    "src/features/reader/index.ts": INDEX,
    "src/features/reader/Host.ts": HOST,
    "src/features/chat/Pane.ts": `import { open } from "../reader";\nexport const pane = open;\n`,
  });
  const ok = run(["--root", allowed, "--baseline", emptyBaseline(allowed)]);
  assert.equal(ok.code, 0, ok.io.stderrText + ok.io.stdoutText);
});

test("feature public index is allowed; deep import fails; new edge on baselined file still fails", () => {
  const allowed = makeFixture({
    "src/App.tsx": `import { open } from "./features/reader";\nexport const App = open;\n`,
    "src/lib/api.ts": API,
    "src/features/reader/index.ts": INDEX,
    "src/features/reader/Host.ts": HOST,
    "src/features/chat/Pane.ts": `import { open } from "../reader";\nexport const pane = open;\n`,
  });
  const okBaseline = emptyBaseline(allowed);
  const ok = run(["--root", allowed, "--baseline", okBaseline]);
  assert.equal(ok.code, 0, ok.io.stderrText + ok.io.stdoutText);

  const deep = makeFixture({
    "src/lib/api.ts": API,
    "src/features/reader/index.ts": INDEX,
    "src/features/reader/Host.ts": HOST,
    "src/features/chat/Pane.ts": `import { host } from "../reader/Host";\nexport const pane = host;\n`,
  });
  const deepBaseline = emptyBaseline(deep);
  const deepResult = run(["--root", deep, "--baseline", deepBaseline]);
  assert.equal(deepResult.code, 1);
  assert.match(deepResult.io.stdoutText, /deps\/feature-deep/);

  const stock = makeFixture({
    "src/lib/api.ts": API,
    "src/lib/bad.ts": `import { Card } from "../components/Card";\nexport const x = Card;\n`,
    "src/components/Card.tsx": `export const Card = 1;\n`,
  });
  const baseline = path.join(stock, "deps.json");
  assert.equal(run(["--root", stock, "--write-baseline", "--baseline", baseline]).code, 0);
  assert.equal(run(["--root", stock, "--baseline", baseline]).code, 0);
  fs.writeFileSync(
    path.join(stock, "src/lib/bad.ts"),
    `import { Card } from "../components/Card";\nimport { Other } from "../components/Other";\nexport const x = Card;\nexport const y = Other;\n`,
  );
  fs.writeFileSync(path.join(stock, "src/components/Other.tsx"), `export const Other = 2;\n`);
  const extra = run(["--root", stock, "--baseline", baseline]);
  assert.equal(extra.code, 1);
  assert.match(extra.io.stderrText, /new/);
});

test("does not skip unknown top-level source directories", () => {
  const dir = makeFixture({
    "src/lib/ok.ts": `export const ok = 1;\n`,
    "orphan/code.ts": `export const orphan = 1;\n`,
  });
  const result = run(["--root", dir, "--baseline", emptyBaseline(dir)]);
  assert.equal(result.code, 1);
  assert.match(result.io.stdoutText, /unclassified directory orphan/);
});
