import { expect, test } from "./fixtures";
import { installMockHost } from "./mockHost";
import { readerReturnFixture, readerView } from "../src/test/fixtures/reader";
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

test("related reading opens as a detour and returns to the article it came from", async ({ page }) => {
  const fixture = readerReturnFixture(3);
  const document = fixture.readers[1].document;
  const row = (id: number, read = false) => ({ article_id: id, feed_id: 1, feed_title: "示例信源", icon_href: "", title: `Related ${id}`, published_at: null, one_liner: "", read, similarity: 0.9 });
  await installMockHost(page, {
    ...fixture,
    readers: { ...fixture.readers, 10: readerView(10, { title: "Related 10", document }), 11: readerView(11, { title: "Related 11", document }) },
    related: { 1: [row(10), row(11, true)] },
  });
  await page.goto("/");
  await page.getByRole("article", { name: fixture.readers[1].title, exact: true }).click();
  const reader = page.locator(".drawer.reader");
  const title = reader.locator(".reader-title");
  const rows = reader.locator(".reader-related-row");
  await expect(rows).toHaveCount(2);
  // 放在下一篇卡之后，不抢下一篇的位置。
  expect(await reader.locator(".reader-ending").evaluate(el => {
    const next = el.querySelector(".reader-next"), related = el.querySelector(".reader-related");
    return !!next && !!related && !!(next.compareDocumentPosition(related) & Node.DOCUMENT_POSITION_FOLLOWING);
  })).toBe(true);
  await expect(rows.nth(1)).toHaveAttribute("data-read", "true");

  await rows.first().click();
  await expect(title).toHaveText("Related 10");
  await expect(reader.getByRole("button", { name: "回到上一篇" })).toBeVisible();
  await expect(reader.getByRole("group", { name: "上下篇" })).toHaveCount(0);
  await expect(rows).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(title).toHaveText(fixture.readers[1].title);
  await expect(rows.first()).toHaveAttribute("data-read", "true");

  // 岔路里按 J：回到原来那篇再按原列表走到下一篇。
  await rows.first().click();
  await expect(title).toHaveText("Related 10");
  await page.evaluate(() => (window.document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press("j");
  await expect(title).toHaveText(fixture.readers[2].title);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("article", { name: fixture.readers[2].title, exact: true })).toHaveClass(/focused/);
  await expect.poll(() => page.evaluate(() => ((window as any).__mockHost.signals.sessions as Array<{ via: string }>).map(s => s.via))).toContain("related");
});
