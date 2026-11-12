import { expect, test } from "./fixtures";
import { installMockHost } from "./mockHost";
import { readerReturnFixture } from "../src/test/fixtures/reader";
import { shot } from "./shots";

/** 审阅截图的其余宽度 / 配色；不截图时不改视口，后续断言都在默认 1280 亮色下跑。 */
const SHOT_MATRIX = process.env.RRSS_SHOTS ? [[1280, "dark"], [560, "light"], [560, "dark"]] as const : [];

test("keyboard ending flow focuses before opening the next focus article", async ({ page }) => {
  const fixture = readerReturnFixture(2);
  await installMockHost(page, fixture);
  await page.goto("/");
  await page.getByRole("article", { name: fixture.readers[1].title, exact: true }).click();
  const reader = page.locator(".drawer.reader");
  const body = reader.locator(".drawer-body");
  await body.evaluate((el) => { el.scrollTop = el.scrollHeight; });
  await page.evaluate(() => document.body.focus());
  await page.keyboard.press("Space");
  await expect.poll(() => page.evaluate(() => document.activeElement?.className)).toContain("reader-next");
  await expect(reader.locator(".reader-ending-hint")).toHaveText("读到这里了。按 Space 聚焦下一篇，或按 J 继续。");
  await shot(page, "test-results/worker-reader-ending/focus-space-light-1280.png");
  for (const [width, colorScheme] of SHOT_MATRIX) {
    await page.setViewportSize({ width, height: 800 });
    await page.emulateMedia({ colorScheme });
    await shot(page, `test-results/worker-reader-ending/focus-space-${width}-${colorScheme}.png`);
  }
  await page.keyboard.press("Enter");
  await expect(reader.locator(".reader-title")).toHaveText(fixture.readers[2].title);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("article", { name: fixture.readers[2].title, exact: true })).toHaveClass(/focused/);
});

test("search ending shows next result metadata and returns to the search session", async ({ page }) => {
  const fixture = readerReturnFixture(3);
  await installMockHost(page, fixture);
  await page.goto("/");
  // 列表出来说明根壳已挂好快捷键，再按 Ctrl+K；否则满载时偶发按早了。
  await expect(page.locator(".article").first()).toBeVisible();
  await page.keyboard.press("Control+k");
  const search = page.getByRole("dialog", { name: "搜索文章" });
  await search.getByRole("searchbox").fill("阅读位置");
  await search.getByRole("option").first().click();
  const reader = page.locator(".drawer.reader");
  await expect(reader.locator(".reader-next")).toContainText("下一条结果");
  await expect(reader.locator(".reader-next")).toContainText(/第 1 \/ 共 \d+ 条/);
  await shot(page, "test-results/worker-reader-ending/search-next-light-1280.png");
  await page.keyboard.press("j");
  await expect(reader.locator(".reader-next")).toContainText(/第 2 \/ 共 \d+ 条/);
  await page.keyboard.press("j");
  await expect(reader.locator(".reader-next")).toContainText("搜索结果读完了");
  await shot(page, "test-results/worker-reader-ending/search-end-light-1280.png");
  for (const [width, colorScheme] of SHOT_MATRIX) {
    await page.setViewportSize({ width, height: 800 });
    await page.emulateMedia({ colorScheme });
    await shot(page, `test-results/worker-reader-ending/search-end-${width}-${colorScheme}.png`);
  }
  await reader.locator(".return-search").click();
  await expect(search).toBeVisible();
  await expect(search.getByRole("option").nth(2)).toHaveAttribute("aria-selected", "true");
});

test("uncached open shows the delayed reader skeleton and cached open skips it", async ({ page }) => {
  const fixture = readerReturnFixture(2);
  await installMockHost(page, { ...fixture, holdOpen: true });
  await page.goto("/");
  await page.getByRole("article", { name: fixture.readers[1].title, exact: true }).click();
  await expect(page.locator("[data-reader-opening-skeleton]")).toBeVisible();
  await shot(page, "test-results/worker-reader-ending/opening-skeleton-light-1280.png");
  for (const [width, colorScheme] of SHOT_MATRIX) {
    await page.setViewportSize({ width, height: 800 });
    await page.emulateMedia({ colorScheme });
    await shot(page, `test-results/worker-reader-ending/opening-skeleton-${width}-${colorScheme}.png`);
  }
  await page.evaluate(() => (window as any).__mockHost.releaseOpen());
  await expect(page.locator(".reader-title")).toHaveText(fixture.readers[1].title);
  await page.keyboard.press("Escape");
  await page.getByRole("article", { name: fixture.readers[1].title, exact: true }).click();
  await expect(page.locator("[data-reader-opening-skeleton]")).toHaveCount(0);
});
