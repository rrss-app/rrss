#!/usr/bin/env node
// 从 CHANGELOG.md 取出某个版本的说明，用作 GitHub Release 正文。
// 用法：node scripts/release-notes.mjs <版本号，如 0.3.1 或 v0.3.1> [CHANGELOG 路径]
// 找不到该版本或这一段为空时以非零退出，发版流程据此在推标签前失败。
import { readFileSync } from "node:fs";

export function releaseNotes(changelog, version) {
  const v = version.replace(/^v/, "");
  const lines = changelog.split(/\r?\n/);
  const start = lines.findIndex((line) => line.startsWith(`## [${v}]`));
  if (start < 0) return null;
  let end = lines.findIndex((line, i) => i > start && line.startsWith("## "));
  if (end < 0) end = lines.length;
  const body = lines.slice(start + 1, end).join("\n").trim();
  return body || null;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [version, path = "CHANGELOG.md"] = process.argv.slice(2);
  if (!version) {
    console.error("用法：node scripts/release-notes.mjs <版本号> [CHANGELOG 路径]");
    process.exit(2);
  }
  const notes = releaseNotes(readFileSync(path, "utf8"), version);
  if (!notes) {
    console.error(`${path} 里没有 ${version} 的说明：发版前先把「未发布」一段改成这个版本号与日期。`);
    process.exit(1);
  }
  process.stdout.write(`${notes}\n`);
}
