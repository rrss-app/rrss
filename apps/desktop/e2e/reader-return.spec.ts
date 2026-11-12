import type { Page } from "@playwright/test";
import { expect, test } from "./fixtures";
import { installMockHost, type MockHostOptions } from "./mockHost";
import { readerReturnFixture } from "../src/test/fixtures/reader";
import { shot } from "./shots";

const fixture = readerReturnFixture();
const reader = (page: Page) => page.locator(".drawer.reader");
const body = (page: Page) => reader(page).locator(".drawer-body");
const row = (page: Page, id: number) => page.getByRole("article", { name: fixture.readers[id].title, exact: true });
async function setup(page: Page, options: MockHostOptions = {}) {
  await page.setViewportSize({ width: 1280, height: 800 });
  await installMockHost(page, { ...fixture, ...options });
  await page.goto("/");
  await expect(row(page, 1)).toBeVisible();
  // 指针放在版心中间：停在阅读器左缘 14px 内会在 440ms 后拉出篇目抽屉，Esc 先关抽屉而不返回列表，
  // 断言就取决于阅读器开了多久。
  await page.mouse.move(640, 400);
}
async function opened(page: Page, id: number) {
  await expect(reader(page).locator(".reader-title")).toHaveText(fixture.readers[id].title);
}
async function step(page: Page, key: string, id: number) {
  await page.keyboard.press(key);
  await opened(page, id);
}
async function centered(page: Page, id: number) {
  await expect(row(page, id)).toHaveClass(/focused/);
  await expect.poll(() => row(page, id).evaluate(el => {
    const bounds = el.closest(".scroll")!.getBoundingClientRect(), rect = el.getBoundingClientRect();
    return Math.abs((rect.top + rect.bottom - bounds.top - bounds.bottom) / 2);
  })).toBeLessThan(8);
}
async function scrollToBlock(page: Page, block: number) {
  await body(page).evaluate((el, block) => {
    const node = el.querySelector(`[data-reader-document] > [data-block-id="${block}"]`)!;
    el.scrollTop += node.getBoundingClientRect().top - el.getBoundingClientRect().top;
  }, block);
  await expect.poll(() => blockOffset(page, block)).toBeLessThan(2);
}
const blockOffset = (page: Page, block: number) => body(page).evaluate((el, block) => {
  const node = el.querySelector(`[data-reader-document] > [data-block-id="${block}"]`)!;
  return Math.abs(node.getBoundingClientRect().top - el.getBoundingClientRect().top);
}, block);

test("J/K and next return to the current read row, Enter and o reopen it", async ({ page }) => {
  await setup(page);
  await row(page, 1).click();
  for (const id of [2, 3, 4, 5, 6]) await step(page, "j", id);
  await step(page, "k", 5);
  await reader(page).getByRole("button", { name: "下一篇", exact: true }).click();
  await opened(page, 6);
  await reader(page).getByRole("button", { name: "返回", exact: true }).click();
  await centered(page, 6);
  await expect(row(page, 6)).not.toHaveClass(/unread/);
  await expect(row(page, 5)).not.toHaveClass(/unread/);
  await page.keyboard.press("Enter");
  await opened(page, 6);
  await page.keyboard.press("Backspace");
  await centered(page, 6);
  await page.keyboard.press("o");
  await opened(page, 6);
});

test("the ending next card uses J navigation and shows the final article state", async ({ page }) => {
  await setup(page, { ...readerReturnFixture(3), focusPageSize: 1 });
  await row(page, 1).click();
  await opened(page, 1);
  const next = reader(page).locator(".reader-next");
  await expect(next).toContainText(fixture.readers[2].title);
  await next.click();
  await opened(page, 2);
  await page.keyboard.press("k");
  await opened(page, 1);
  await page.keyboard.press("j");
  await opened(page, 2);
  await next.click();
  await opened(page, 3);
  await expect(next).toContainText("今天的焦点读完了");
  await expect(next).toHaveAttribute("aria-disabled", "true");
  await page.keyboard.press("Escape");
  await expect(row(page, 3)).toHaveClass(/focused/);
});

for (const [name, start, expected, count] of [["next", 3, 4, 6], ["previous", 6, 5, 6], ["empty", 1, 0, 1]] as const) {
  test(`unread return selects ${name} after reloading`, async ({ page }) => {
    const data = readerReturnFixture(count);
    await setup(page, { ...data, focus: { ...data.focus, list_scope: "unread" } });
    await row(page, start).click();
    await opened(page, start);
    await page.keyboard.press("Escape");
    await expect(reader(page)).not.toBeVisible();
    await expect(row(page, start)).toHaveCount(0);
    if (expected) {
      await expect(row(page, expected)).toHaveClass(/focused/);
      await page.keyboard.press("Enter");
      await opened(page, expected);
    } else {
      await expect(page.locator(".article.focused")).toHaveCount(0);
      await page.keyboard.press("Enter");
      await expect(reader(page)).not.toBeVisible();
    }
  });
}

test("blocks survive neighbor navigation and list reopen; a new article starts at the top", async ({ page }) => {
  await setup(page);
  await row(page, 1).click();
  await opened(page, 1);
  await scrollToBlock(page, 8);
  await step(page, "j", 2);
  await expect.poll(() => body(page).evaluate(el => el.scrollTop)).toBe(0);
  await scrollToBlock(page, 5);
  await step(page, "k", 1);
  await expect.poll(() => blockOffset(page, 8)).toBeLessThan(2);
  await step(page, "j", 2);
  await expect.poll(() => blockOffset(page, 5)).toBeLessThan(2);
  await page.keyboard.press("Escape");
  await page.keyboard.press("Enter");
  await opened(page, 2);
  await expect.poll(() => blockOffset(page, 5)).toBeLessThan(2);
});

test("long measured list centers an offscreen current article on return", async ({ page }) => {
  await setup(page);
  await row(page, 1).click();
  for (let id = 2; id <= 20; id++) await step(page, "j", id);
  await page.keyboard.press("Escape");
  await centered(page, 20);
  await expect(row(page, 20)).toBeInViewport();
  await shot(page, "test-results/reader-return/centered-light.png");
  await page.keyboard.press("j");
  await page.keyboard.press("Enter");
  await opened(page, 21);
  await step(page, "k", 20);
  for (const [width, colorScheme] of [[1280, "dark"], [880, "dark"], [880, "light"]] as const) {
    await page.setViewportSize({ width, height: 800 });
    await page.emulateMedia({ colorScheme });
    await page.keyboard.press("Escape");
    await centered(page, 20);
    await shot(page, `test-results/reader-return/centered-${width}-${colorScheme}.png`);
    await page.keyboard.press("Enter");
    await opened(page, 20);
  }
});

test("keyboard list to Enter, j, Escape returns to the new row", async ({ page }) => {
  await setup(page);
  await page.keyboard.press("j");
  await page.keyboard.press("Enter");
  await opened(page, 1);
  await step(page, "j", 2);
  await page.keyboard.press("Escape");
  await expect(row(page, 2)).toHaveClass(/focused/);
  await page.keyboard.press("Enter");
  await opened(page, 2);
});

test("a changed source_hash resets a previously read article to the top", async ({ page }) => {
  const first = fixture.readers[1];
  await setup(page, { readerSequences: { 1: [first, { ...first, source_hash: "replaced-body" }] } });
  await row(page, 1).click();
  await opened(page, 1);
  await scrollToBlock(page, 8);
  await step(page, "j", 2);
  await step(page, "k", 1);
  await expect.poll(() => body(page).evaluate(el => el.scrollTop)).toBe(0);
});

test("unread return finds the next row beyond the reloaded first page", async ({ page }) => {
  await setup(page, { focus: { ...fixture.focus, list_scope: "unread" }, focusPageSize: 8 });
  await row(page, 5).click();
  for (let id = 6; id <= 18; id++) await step(page, "j", id);
  await page.keyboard.press("Escape");
  await expect(row(page, 18)).toHaveCount(0);
  await expect(row(page, 19)).toHaveClass(/focused/);
  await expect(row(page, 19)).toBeInViewport();
  await page.keyboard.press("Enter");
  await opened(page, 19);
});

test("Esc closes tools even after focus has left them, then returns", async ({ page }) => {
  await setup(page);
  await row(page, 1).click();
  await opened(page, 1);
  await reader(page).locator("summary").click();
  await reader(page).getByRole("button", { name: "返回", exact: true }).focus();
  await page.keyboard.press("Escape");
  await expect(reader(page)).toBeVisible();
  await expect(reader(page).locator(".reader-tools")).not.toHaveAttribute("open", "");
  await page.keyboard.press("Escape");
  await expect(row(page, 1)).toHaveClass(/focused/);
});

test("search returns to the search overlay, then to the underlying view", async ({ page }) => {
  await setup(page);
  await page.keyboard.press("Control+k");
  const search = page.getByRole("dialog", { name: "搜索文章" });
  await search.getByRole("searchbox").fill("阅读位置 6");
  await search.getByRole("option").first().click();
  await opened(page, 6);
  await expect(reader(page).locator(".reader-position")).toHaveCount(0);
  await expect(reader(page).locator(".reader-next")).toBeVisible();
  await expect(reader(page).locator(".reader-next")).toContainText("搜索结果读完了");
  // N4：从搜索打开的文章返回到搜索浮层，查询仍在；再 Esc 才回到底下的视图，且不定位列表行。
  await page.keyboard.press("Escape");
  await expect(reader(page)).not.toBeVisible();
  await expect(search).toBeVisible();
  await expect(search.getByRole("searchbox")).toHaveValue("阅读位置 6");
  await page.keyboard.press("Escape");
  await expect(search).not.toBeVisible();
  await expect(page.locator(".focus")).toBeVisible();
  await expect(page.locator(".article.focused")).toHaveCount(0);
});

test("positions remain available after more than eight different articles", async ({ page }) => {
  await setup(page);
  await row(page, 1).click();
  await opened(page, 1);
  await scrollToBlock(page, 8);
  for (let id = 2; id <= 11; id++) await step(page, "j", id);
  for (let id = 10; id >= 1; id--) await step(page, "k", id);
  await expect.poll(() => blockOffset(page, 8)).toBeLessThan(2);
});

test("scrolling and loading more after return does not recenter the old row", async ({ page }) => {
  const data = readerReturnFixture(80);
  await setup(page, { ...data, focusPageSize: 20 });
  await row(page, 1).click();
  await expect(reader(page).locator(".reader-position")).toHaveText("1 / 20+");
  for (let id = 2; id <= 6; id++) await step(page, "j", id);
  await page.keyboard.press("Escape");
  await centered(page, 6);
  const scroll = page.locator(".focus .scroll");
  const top = await scroll.evaluate(el => {
    el.scrollTop = el.scrollHeight;
    return el.scrollTop;
  });
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.calls.filter((c: string) => c === "focus_stream").length)).toBeGreaterThan(1);
  await expect.poll(() => scroll.evaluate(el => el.scrollTop)).toBeGreaterThanOrEqual(top - 2);
  await expect(row(page, 6)).not.toBeInViewport();
});
