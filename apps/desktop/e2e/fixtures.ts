import { createHash } from "node:crypto";
import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import path from "node:path";
import { test as base } from "@playwright/test";

// 用例统一从这里取 `test`：每个用例顺带记下实际执行过的 `src/` 文件，供 `pnpm test:affected` 挑选受影响的 spec。
// 模块加载不算，只有模块里的具名函数真正跑过才算；热更新转换注入的 `$RefreshReg$`、`_c` 之类不计。
export * from "@playwright/test";

const INJECTED = /^(\$|_c\d*$)/;
// Playwright clears test-results before every run; keep the per-worktree index outside it.
export const COVERAGE_DIR = path.resolve(import.meta.dirname, "../.cache/e2e-coverage");

export const test = base.extend<{ recordSources: void }>({
  recordSources: [async ({ page, browserName }, use, info) => {
    if (browserName !== "chromium") return use();
    const specHash = createHash("sha256").update(readFileSync(info.file)).digest("hex");
    await page.coverage.startJSCoverage({ resetOnNavigation: false });
    await use();
    const entries = await page.coverage.stopJSCoverage().catch(() => []);
    const files = new Set<string>();
    for (const entry of entries) {
      const match = new URL(entry.url).pathname.match(/^\/(src\/.+\.tsx?)$/);
      if (match && entry.functions.some((f) => f.functionName && !INJECTED.test(f.functionName) && f.ranges[0]?.count > 0)) {
        files.add(match[1]);
      }
    }
    if (info.status !== "passed" || !files.size) return;
    mkdirSync(COVERAGE_DIR, { recursive: true });
    const spec = path.relative(path.resolve(import.meta.dirname, ".."), info.file).split(path.sep).join("/");
    const target = path.join(COVERAGE_DIR, `${info.testId}.json`);
    const temporary = `${target}.${info.workerIndex}.tmp`;
    writeFileSync(temporary, JSON.stringify({ spec, specHash, line: info.line, title: info.title, files: [...files].sort() }));
    renameSync(temporary, target);
  }, { auto: true }],
});
