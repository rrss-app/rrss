import fs from "node:fs";
import path from "node:path";
import ts from "typescript";
import { DESKTOP_ROOT, isDirectRun, parseArgs, readJson, toPosix, writeJson } from "./cli.mjs";

// 界面多语言的棘轮：统计源码里还没抽进语言包的中文字面量（字符串、模板串、JSX 文本，不算注释），
// 每个文件的个数不许超过基线。减少了只提示：多个改动并行迁移时不用都改基线文件，一批合并完再用 `--update` 收紧。

export const DEFAULT_I18N_BASELINE = path.join(DESKTOP_ROOT, "tooling", "i18n-baseline.json");
const SRC = path.join(DESKTOP_ROOT, "src");
const HAN = /\p{Script=Han}/u;
/** 不算界面文案的地方：测试与替身、生成的绑定、语言包本身。 */
const SKIP = /(^|\/)(test\/|locales\/|bindings\.ts$)|\.test\.tsx?$/;

/** 一个文件里含中文的字面量个数。 */
export function countLiterals(source, fileName = "file.tsx") {
  const file = ts.createSourceFile(fileName, source, ts.ScriptTarget.Latest, false, fileName.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  let count = 0;
  const visit = (node) => {
    const literal = ts.isStringLiteralLike(node) || ts.isJsxText(node) || ts.isTemplateHead(node) || ts.isTemplateMiddle(node) || ts.isTemplateTail(node);
    if (literal && HAN.test(node.text)) count += 1;
    ts.forEachChild(node, visit);
  };
  visit(file);
  return count;
}

function walk(dir, out = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const abs = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(abs, out);
    else if (/\.tsx?$/.test(entry.name)) out.push(abs);
  }
  return out;
}

export function scan(root = SRC) {
  const counts = {};
  for (const abs of walk(root).sort()) {
    const rel = toPosix(path.relative(root, abs));
    if (SKIP.test(rel)) continue;
    const count = countLiterals(fs.readFileSync(abs, "utf8"), rel);
    if (count > 0) counts[rel] = count;
  }
  return counts;
}

/** `added`：比基线多了，检查失败；`stale`：比基线少了，只提示收紧基线。 */
export function diffCounts(actual, baseline) {
  const added = [];
  const stale = [];
  for (const file of new Set([...Object.keys(actual), ...Object.keys(baseline)])) {
    const now = actual[file] ?? 0;
    const allowed = baseline[file] ?? 0;
    if (now > allowed) added.push({ file, now, allowed });
    else if (now < allowed) stale.push({ file, now, allowed });
  }
  return { added, stale };
}

const total = (counts) => Object.values(counts).reduce((sum, n) => sum + n, 0);

export function runCheckI18n(argv = [], io = { stdout: process.stdout, stderr: process.stderr }) {
  const args = parseArgs(argv, { flags: ["update"] });
  const actual = scan();
  if (args.flags.update) {
    writeJson(DEFAULT_I18N_BASELINE, { files: actual });
    io.stdout.write(`i18n baseline updated: ${total(actual)} literals in ${Object.keys(actual).length} files\n`);
    return 0;
  }
  const { added, stale } = diffCounts(actual, readJson(DEFAULT_I18N_BASELINE).files);
  if (added.length) {
    io.stderr.write("i18n check failed: 新增了没进语言包的中文文案，请写进 src/locales 并用 t() 取：\n");
    for (const item of added) io.stderr.write(`  ${item.file}  ${item.allowed} → ${item.now}\n`);
    return 1;
  }
  if (stale.length) {
    io.stdout.write(`i18n: ${stale.length} 个文件的中文文案比基线少，可运行 \`node tooling/check-i18n.mjs --update\` 收紧基线\n`);
  }
  io.stdout.write(`i18n check ok: ${total(actual)} literals left in ${Object.keys(actual).length} files\n`);
  return 0;
}

if (isDirectRun(import.meta.url)) {
  process.exit(runCheckI18n(process.argv.slice(2)));
}
