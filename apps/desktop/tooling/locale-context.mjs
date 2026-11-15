import fs from "node:fs";
import path from "node:path";
import { DESKTOP_ROOT, isDirectRun, toPosix } from "./cli.mjs";
import { flatten, LOCALES_DIR, SOURCE } from "./check-locales.mjs";

// 翻译用的上下文：列出一个命名空间每条源语言文案，以及它在代码里的用处（文件、行与那一行代码），
// 让译者看得出是按钮、标题、提示还是错误。用法：node tooling/locale-context.mjs <命名空间> [键前缀]

const SRC = path.join(DESKTOP_ROOT, "src");
/** 错误码（`errors` 命名空间）按码在运行时取，代码里找不到键；退回到后端声明这个码的那一行。 */
const UI_ERRORS = path.resolve(DESKTOP_ROOT, "../../crates/rrss-app/src/ui_error");
const SKIP = /(^|\/)(locales\/|bindings\.ts$)|\.test\.tsx?$/;

function sources(dir = SRC, out = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const abs = path.join(dir, entry.name);
    if (entry.isDirectory()) sources(abs, out);
    else if (/\.tsx?$/.test(entry.name)) {
      const rel = toPosix(path.relative(SRC, abs));
      if (!SKIP.test(rel)) out.push({ rel, lines: fs.readFileSync(abs, "utf8").split("\n") });
    }
  }
  return out;
}

/** 键（或它的前缀，给 `` t(`a.${x}`) `` 这类动态键）在代码里出现的位置，最多 `limit` 处。 */
export function usages(ns, key, files, limit = 2) {
  const parts = key.replace(/_(zero|one|two|few|many|other)$/, "").split(".");
  for (let n = parts.length; n > 0; n -= 1) {
    const stem = parts.slice(0, n).join(".");
    const exact = n === parts.length;
    const needles = exact
      ? [`"${stem}"`, `"${ns}:${stem}"`, `'${stem}'`, `\`${stem}\``]
      : [`"${stem}.`, `\`${stem}.`, `"${ns}:${stem}.`];
    const found = [];
    for (const { rel, lines } of files) {
      lines.forEach((line, i) => {
        if (found.length < limit && needles.some((needle) => line.includes(needle))) {
          found.push({ at: `${rel}:${i + 1}`, code: line.trim().slice(0, 200), via: exact ? "" : `${stem}.*` });
        }
      });
    }
    if (found.length) return found;
  }
  return [];
}

function rustDeclarations() {
  if (!fs.existsSync(UI_ERRORS)) return [];
  return fs.readdirSync(UI_ERRORS).filter((name) => name.endsWith(".rs") && name !== "tests.rs")
    .map((name) => ({ rel: `rrss-app/src/ui_error/${name}`, lines: fs.readFileSync(path.join(UI_ERRORS, name), "utf8").split("\n") }));
}

/** 后端声明错误码的那一行，连同紧挨着的文档注释。 */
export function declaration(code, rust) {
  for (const { rel, lines } of rust) {
    const i = lines.findIndex((line) => line.includes(`=> "${code}"`));
    if (i < 0) continue;
    let start = i;
    while (start > 0 && lines[start - 1].trim().startsWith("///")) start -= 1;
    return { at: `${rel}:${i + 1}`, code: lines.slice(start, i + 1).map((line) => line.trim()).join(" ").slice(0, 300) };
  }
  return null;
}

/** 用到这个命名空间的文件（`useTranslation("ns")`、`ns: "ns"`、`"ns:key"`），短键只在这里找，免得撞上同名的别处代码。 */
export function namespaceFiles(ns, files) {
  const marks = [`useTranslation("${ns}")`, `ns: "${ns}"`, `"${ns}:`, `\`${ns}:`];
  return files.filter(({ lines }) => lines.some((line) => marks.some((mark) => line.includes(mark))));
}

/**
 * 一块要翻的顶层键：`keys` 指定（逗号分隔），或 `part` 把顶层键按顺序均分（`1/2` 是前一半）。都不给就是整个命名空间。
 */
export function pickGroups(tree, { keys = "", part = "" } = {}) {
  const all = Object.keys(tree);
  if (keys) return new Set(keys.split(",").map((key) => key.trim()).filter(Boolean));
  const match = /^(\d+)\/(\d+)$/.exec(part);
  if (!match) return new Set(all);
  const [index, count] = [Number(match[1]), Number(match[2])];
  const size = Math.ceil(all.length / count);
  return new Set(all.slice((index - 1) * size, index * size));
}

export function context(ns, options = {}) {
  const file = path.join(LOCALES_DIR, SOURCE, `${ns}.json`);
  const tree = JSON.parse(fs.readFileSync(file, "utf8"));
  const groups = pickGroups(tree, options);
  const messages = flatten(ns, tree);
  const files = namespaceFiles(ns, sources());
  const rust = ns === "errors" ? rustDeclarations() : [];
  const out = [`# ${ns}: top-level keys in this part: ${[...groups].join(", ")}`, ""];
  for (const [id, text] of messages) {
    const key = id.slice(ns.length + 1);
    if (!groups.has(key.split(".")[0].replace(/_(zero|one|two|few|many|other)$/, ""))) continue;
    out.push(`## ${key}`, `zh: ${JSON.stringify(text)}`);
    const sites = usages(ns, key, files);
    const declared = !sites.length && rust.length ? declaration(key.split(".")[0], rust) : null;
    if (declared) out.push(`error code declared at ${declared.at}`, `  ${declared.code}`);
    else if (!sites.length) out.push("used: (not found; may be built dynamically)");
    for (const site of sites) out.push(`used: ${site.at}${site.via ? ` (via ${site.via})` : ""}`, `  ${site.code}`);
    out.push("");
  }
  return out.join("\n");
}

if (isDirectRun(import.meta.url)) {
  const [ns, ...rest] = process.argv.slice(2);
  const option = (name) => { const i = rest.indexOf(name); return i >= 0 ? rest[i + 1] ?? "" : ""; };
  if (!ns) {
    process.stderr.write("usage: node tooling/locale-context.mjs <namespace> [--keys a,b,c | --part 1/2]\n");
    process.exit(2);
  }
  process.stdout.write(context(ns, { keys: option("--keys"), part: option("--part") }));
}
