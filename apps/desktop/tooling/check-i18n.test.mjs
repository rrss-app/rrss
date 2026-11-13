import assert from "node:assert/strict";
import { test } from "node:test";
import { countLiterals, diffCounts } from "./check-i18n.mjs";

test("counts Han literals in strings, templates and JSX text, not comments", () => {
  const source = [
    "// 注释不算",
    "/** 文档注释也不算 */",
    "const a = \"标题\";",
    "const b = `共 ${n} 篇`;",
    "const c = <p title=\"提示\">正文 {n} <b>English</b></p>;",
    "const d = t(\"search.title\");",
  ].join("\n");
  assert.equal(countLiterals(source), 5);
});

test("a file may not exceed its baseline; drops are reported as stale", () => {
  const baseline = { "a.tsx": 3, "b.tsx": 2 };
  assert.deepEqual(diffCounts({ "a.tsx": 3, "b.tsx": 2 }, baseline), { added: [], stale: [] });
  assert.deepEqual(diffCounts({ "a.tsx": 4, "b.tsx": 2, "c.tsx": 1 }, baseline).added.map((x) => x.file), ["a.tsx", "c.tsx"]);
  assert.deepEqual(diffCounts({ "a.tsx": 3 }, baseline).stale.map((x) => x.file), ["b.tsx"]);
});
