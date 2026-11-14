import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { countRustLiterals, runCheck, scan } from "./check-rust-i18n.mjs";

test("Rust lexer distinguishes strings, escaped Han, raw strings and nested comments", () => {
  const source = [
    '// "注释"', '/* outer /* "嵌套" */ "注释" */',
    'let a = "文字\\\"仍在字串";', 'let b = r###"中文 /* // " } #[test]"###;',
    'let c = "\\u{4e2d}";', 'let c = \'中\';', "let lifetime: &'static str = \"English\";",
    String.raw`let escaped = "\\u{4e2d}";`,
    '#[error("错误：{0}")] struct Failure(String);', '#[doc = "注释"] struct Documented;',
  ].join("\n");
  assert.equal(countRustLiterals(source), 4);
});

test("test modules and standalone test items are skipped; production cfg is counted", () => {
  const source = [
    '#[cfg(test)] mod tests { #[test] fn a() { let _ = "测试"; } }',
    '#[tokio::test(flavor = "current_thread")] async fn b() { let _ = "测试"; }',
    '#[cfg(all(unix, test))] fn c() { let _ = "测试"; }',
    '#[cfg(not(test))] fn d() { let _ = "生产"; }',
    '#[cfg(any(test, feature = "enabled"))] fn e() { let _ = "生产"; }',
    'fn f() { let _ = "生产"; }',
  ].join("\n");
  assert.equal(countRustLiterals(source), 3);
});

test("ratchet ignores test files and documented exemptions, never writes while checking", t => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "rrss-rust-i18n-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const file = path.join(root, "ui.rs"), baseline = path.join(root, "baseline.json");
  fs.writeFileSync(file, 'fn run() { let _ = "已有"; }');
  fs.writeFileSync(path.join(root, "prompt.rs"), 'const PROMPT: &str = "模型";');
  fs.writeFileSync(path.join(root, "test_fixture.rs"), 'const FIXTURE: &str = "测试";');
  const options = { root, baseline, exemptions: { "prompt.rs": "Model instructions, not UI text." } };
  const writes = [];
  const io = { stdout: { write: value => writes.push(value) }, stderr: { write: value => writes.push(value) } };
  assert.deepEqual(scan(root, options.exemptions), { "ui.rs": 1 });
  assert.equal(runCheck(["--update"], io, options), 0);
  const original = fs.readFileSync(baseline, "utf8");
  assert.equal(runCheck([], io, options), 0);
  fs.writeFileSync(file, 'fn run() { let _ = "新增"; let _ = "新增"; }');
  assert.equal(runCheck([], io, options), 1);
  fs.writeFileSync(file, "fn run() {}");
  assert.equal(runCheck([], io, options), 0);
  assert.equal(fs.readFileSync(baseline, "utf8"), original);
  assert.throws(() => runCheck([], io, { ...options, exemptions: { "prompt.rs": "" } }), /reason/);
});
