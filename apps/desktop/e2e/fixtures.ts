import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { test as base } from "@playwright/test";

// 用例统一从这里取 `test`：每个用例顺带记下实际执行过的 `src/` 文件，供 `pnpm test:affected` 挑选受影响的 spec。
// 模块加载不算，只有模块里的具名函数真正跑过才算；热更新转换注入的 `$RefreshReg$`、`_c` 之类不计。
export * from "@playwright/test";

const INJECTED = /^(\$|_c\d*$)/;
export const COVERAGE_DIR = path.resolve(import.meta.dirname, "../test-results/e2e-coverage");

export const test = base.extend<{ recordSources: void }>({
  recordSources: [async ({ page, browserName }, use, info) => {
    if (browserName !== "chromium") return use();
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
    mkdirSync(COVERAGE_DIR, { recursive: true });
    const spec = path.relative(path.resolve(import.meta.dirname, ".."), info.file).split(path.sep).join("/");
    writeFileSync(path.join(COVERAGE_DIR, `${info.testId}.json`), JSON.stringify({ spec, line: info.line, title: info.title, files: [...files].sort() }));
  }, { auto: true }],
});
