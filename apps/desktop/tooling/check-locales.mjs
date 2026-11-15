import fs from "node:fs";
import path from "node:path";
import { DESKTOP_ROOT, isDirectRun } from "./cli.mjs";

// 语言包一致性：每个非源语言与源语言（zh-CN）的键一一对应（复数按各语言的复数类别比较），
// 每条的 {{变量}} 与 <标签> 和源语言一致，译文里没有中文，以冒号结尾的前缀键译文也以冒号结尾。
// 有了这道检查，发出去的语言包不会靠回退显示中文。

export const LOCALES_DIR = path.join(DESKTOP_ROOT, "src", "locales");
export const SOURCE = "zh-CN";
const HAN = /\p{Script=Han}/u;
const PLURAL = /_(zero|one|two|few|many|other)$/;

/**
 * 只有源语言用到的键：中文日期按模板与中文数字拼，其他语言交给 `Intl`（见 `lib/time.ts`）。
 * 其他语言可以不写这些键。
 */
export const SOURCE_ONLY = new Set([
  "focus:masthead.digits", "focus:masthead.ten", "focus:masthead.weekdays", "focus:masthead.date",
  "shell:time.monthDay", "shell:time.date", "shell:time.fullDateTime",
]);

/** 语言选项用各自的语言写（「简体中文」「English」），不随界面语言翻译，译文里允许出现中文。 */
export const AUTONYMS = new Set(["settingsSystem:registry.ui_language.options.zh-CN"]);

/** 命名空间文件压平成 `ns:a.b.c` → 文字。 */
export function flatten(ns, tree, prefix = "", out = new Map()) {
  for (const [key, value] of Object.entries(tree)) {
    const id = prefix ? `${prefix}.${key}` : key;
    if (typeof value === "string") out.set(`${ns}:${id}`, value);
    else if (value && typeof value === "object" && !Array.isArray(value)) flatten(ns, value, id, out);
    else out.set(`${ns}:${id}`, String(value));
  }
  return out;
}

export function readLocale(dir) {
  const out = new Map();
  for (const file of fs.readdirSync(dir).filter((name) => name.endsWith(".json")).sort()) {
    flatten(file.slice(0, -5), JSON.parse(fs.readFileSync(path.join(dir, file), "utf8")), "", out);
  }
  return out;
}

const vars = (text) => new Set([...text.matchAll(/\{\{\s*([\w.]+)[^}]*\}\}/g)].map((m) => m[1]));
const tags = (text) => [...text.matchAll(/<\/?([\w-]+)\s*\/?>/g)].map((m) => m[0].replace(/\s+/g, "")).sort();

/** 去掉复数后缀，同一句的各个复数形式归到一个基键下。 */
function group(messages) {
  const out = new Map();
  for (const [key, text] of messages) {
    const match = PLURAL.exec(key);
    const base = match ? key.slice(0, -match[0].length) : key;
    const entry = out.get(base) ?? { plural: false, forms: new Map() };
    if (match) entry.plural = true;
    entry.forms.set(match ? match[1] : "", text);
    out.set(base, entry);
  }
  return out;
}

/** 一个译文语言对照源语言的全部问题。 */
export function compareLocale(locale, source, target) {
  const problems = [];
  const src = group(source);
  const dst = group(target);
  const categories = new Intl.PluralRules(locale).resolvedOptions().pluralCategories;
  for (const [base, entry] of src) {
    const own = dst.get(base);
    if (!own) {
      if (!SOURCE_ONLY.has(base)) problems.push(`${base}: missing`);
      continue;
    }
    if (entry.plural !== own.plural) {
      problems.push(`${base}: ${entry.plural ? "needs plural forms" : "must not use plural forms"}`);
      continue;
    }
    if (entry.plural) {
      for (const category of categories) {
        if (!own.forms.has(category)) problems.push(`${base}_${category}: missing plural form`);
      }
      for (const category of own.forms.keys()) {
        if (!categories.includes(category)) problems.push(`${base}_${category}: ${locale} has no "${category}" plural form`);
      }
    }
    const sourceText = [...entry.forms.values()][0];
    const sourceVars = vars(sourceText);
    const sourceTags = JSON.stringify(tags(sourceText));
    for (const [form, text] of own.forms) {
      const key = form ? `${base}_${form}` : base;
      const found = vars(text);
      // 「1 篇」这类单数形式可以不写 {{count}}。
      for (const name of sourceVars) if (!found.has(name) && !(name === "count" && form === "one")) problems.push(`${key}: missing {{${name}}}`);
      for (const name of found) if (!sourceVars.has(name)) problems.push(`${key}: unknown {{${name}}}`);
      if (JSON.stringify(tags(text)) !== sourceTags) problems.push(`${key}: tags differ from source`);
      if (HAN.test(text) && !AUTONYMS.has(key)) problems.push(`${key}: contains Chinese`);
      if (/[：:]$/.test(sourceText) && !/[：:]$/.test(text)) problems.push(`${key}: prefix must end with a colon`);
    }
  }
  for (const base of dst.keys()) if (!src.has(base)) problems.push(`${base}: not in source`);
  return problems;
}

export function runCheckLocales(dir = LOCALES_DIR, io = { stdout: process.stdout, stderr: process.stderr }) {
  const source = readLocale(path.join(dir, SOURCE));
  const others = fs.readdirSync(dir, { withFileTypes: true }).filter((entry) => entry.isDirectory() && entry.name !== SOURCE).map((entry) => entry.name).sort();
  let failed = false;
  for (const locale of others) {
    const problems = compareLocale(locale, source, readLocale(path.join(dir, locale)));
    if (problems.length) {
      failed = true;
      io.stderr.write(`locales check failed for ${locale} (${problems.length}):\n`);
      for (const problem of problems.slice(0, 50)) io.stderr.write(`  ${problem}\n`);
      if (problems.length > 50) io.stderr.write(`  … ${problems.length - 50} more\n`);
    }
  }
  if (!failed) io.stdout.write(`locales check ok: ${source.size} source messages, ${others.length ? others.join(", ") : "no other locales yet"}\n`);
  return failed ? 1 : 0;
}

if (isDirectRun(import.meta.url)) {
  process.exit(runCheckLocales());
}
