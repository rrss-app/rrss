#!/usr/bin/env node
// Offline documentation checks. No app build, browser, network, or dependencies.
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, extname, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
// 根目录文档；公开导出版没有 AGENTS.md，缺的文件跳过。
const files = ["README.md", "README.en.md", "CHANGELOG.md", "AGENTS.md", "CONTRIBUTING.md", "SECURITY.md"].map((name) => resolve(root, name)).filter((path) => existsSync(path));
function collect(directory) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) collect(path);
    else if (entry.isFile() && entry.name.endsWith(".md")) files.push(path);
  }
}
for (const directory of ["doc", "testdata", "pi"]) if (existsSync(resolve(root, directory))) collect(resolve(root, directory));

function withoutFences(text) {
  let fence = null;
  return text.split("\n").map((line) => {
    const marker = line.match(/^ {0,3}(`{3,}|~{3,})/);
    if (marker) {
      if (!fence) fence = marker[1];
      else if (marker[1][0] === fence[0] && marker[1].length >= fence.length) fence = null;
      return "";
    }
    return fence ? "" : line;
  }).join("\n");
}

const documents = new Map(files.map((file) => [file, withoutFences(readFileSync(file, "utf8"))]));
const anchorCache = new Map();
function anchors(file) {
  if (anchorCache.has(file)) return anchorCache.get(file);
  const body = documents.get(file) ?? withoutFences(readFileSync(file, "utf8"));
  const result = new Set();
  const occurrences = new Map();
  for (const match of body.matchAll(/^ {0,3}#{1,6}\s+(.+?)\s*#*\s*$/gm)) {
    const slug = match[1].replace(/<[^>]*>/g, "").replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
      .toLowerCase().replace(/[^\p{L}\p{N}\p{M}_\-\s]/gu, "").trim().replace(/\s/g, "-");
    const n = occurrences.get(slug) ?? 0;
    occurrences.set(slug, n + 1);
    result.add(n ? `${slug}-${n}` : slug);
  }
  for (const match of body.matchAll(/<a\s+[^>]*(?:id|name)=["']([^"']+)["'][^>]*>/g)) result.add(match[1]);
  anchorCache.set(file, result);
  return result;
}

const errors = [];
let localLinks = 0;
for (const [file, body] of documents) {
  const name = relative(root, file).split(sep).join("/");
  if (name.startsWith("doc/") && !["doc/91-next.md", "doc/history.md"].includes(name)
      && /^\*\*状态[：:]/m.test(body)) {
    errors.push(`${name}: 实施状态应只维护在 doc/91-next.md`);
  }
  const targets = [
    ...body.matchAll(/!?\[[^\]\n]*\]\(\s*(?:<([^>]+)>|([^\s)]+))(?:\s+["'][^\n]*?["'])?\s*\)/g),
    ...body.matchAll(/^ {0,3}\[[^\]\n]+\]:\s*(?:<([^>]+)>|([^\s]+))/gm),
  ];
  for (const match of targets) {
    const href = match[1] ?? match[2];
    if (/^(?:[a-z][\w+.-]*:|\/\/)/i.test(href)) continue;
    const line = body.slice(0, match.index).split("\n").length;
    const fail = (reason) => errors.push(`${name}:${line}: ${href} — ${reason}`);
    let path, fragment;
    try {
      const hash = href.indexOf("#");
      path = decodeURIComponent((hash < 0 ? href : href.slice(0, hash)).split("?")[0]);
      fragment = hash < 0 ? "" : decodeURIComponent(href.slice(hash + 1));
    } catch { fail("无效 URL 编码"); continue; }
    const target = path ? resolve(dirname(file), path) : file;
    if (!target.startsWith(root + sep)) { fail("目标在项目之外"); continue; }
    localLinks++;
    if (!existsSync(target)) { fail("文件不存在"); continue; }
    if (fragment && extname(target) === ".md" && statSync(target).isFile() && !anchors(target).has(fragment)) {
      fail("锚点不存在");
    }
  }
}
if (errors.length) {
  console.error(errors.join("\n"));
  process.exitCode = 1;
} else {
  console.log(`文档检查通过：${files.length} 份 Markdown，${localLinks} 处本地链接；外部 URL 不联网检查。`);
}
