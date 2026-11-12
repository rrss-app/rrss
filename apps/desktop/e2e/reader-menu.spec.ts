import type { Page } from "@playwright/test";
import { expect, test } from "./fixtures";
import { installMockHost, type MockHostOptions } from "./mockHost";
import { readerMenuFixtures, TWO_ARTICLE_PAGE } from "../src/test/fixtures/reader";
import { shot } from "./shots";

const fixture = readerMenuFixtures();
const summaryText = "图书馆开放时间与借阅服务。";

async function openReader(page: Page, options: MockHostOptions) {
  await page.setViewportSize({ width: 1280, height: 800 });
  await installMockHost(page, { reader: fixture.plain, focus: TWO_ARTICLE_PAGE, ...options });
  await page.goto("/");
  await page.getByRole("article").first().click();
  const reader = page.locator(".drawer.reader");
  const body = reader.locator(".drawer-body");
  return {
    reader,
    body,
    menu: page.getByRole("menu", { name: "本篇操作" }),
    paragraph: reader.locator('[data-reader-document] > [data-block-id="2"]'),
    back: reader.getByRole("button", { name: "返回", exact: true }),
    top: reader.getByRole("button", { name: "回到顶部", exact: true }),
  };
}

test("plain reader menu: keyboard, summary and favorite", async ({ page }) => {
  const { reader, body, menu, paragraph, back } = await openReader(page, { holdSummary: true, summary: summaryText });
  const openMenu = async () => {
    await paragraph.click({ button: "right" });
    await expect(menu).toBeVisible();
  };
  await expect(reader.getByRole("button", { name: "回到顶部", exact: true })).not.toBeVisible();
  await back.focus();
  await openMenu();
  await expect(menu.getByRole("menuitem")).toHaveText(["翻译本篇T", "生成本篇摘要", "引用全文到对话", "收藏S", "查看原文", "回到顶部"]);
  await expect(menu.getByRole("menuitem", { name: "翻译本篇" })).toBeFocused();
  await page.keyboard.press("End");
  await expect(menu.getByRole("menuitem", { name: "回到顶部" })).toBeFocused();
  await page.keyboard.press("Home");
  await expect(menu.getByRole("menuitem", { name: "翻译本篇" })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(menu).not.toBeVisible();
  await expect(reader).toBeVisible();
  await expect(back).toBeFocused();

  await openMenu();
  await page.keyboard.press("ArrowDown");
  await expect(menu.getByRole("menuitem", { name: "生成本篇摘要" })).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(menu).not.toBeVisible();
  await openMenu();
  await expect(menu.getByRole("menuitem", { name: "摘要生成中…" })).toBeDisabled();
  await page.keyboard.press("ArrowDown");
  await expect(menu.getByRole("menuitem", { name: "引用全文到对话" })).toBeFocused();
  await page.keyboard.press("Escape");
  await page.evaluate(() => (window as any).__mockHost.releaseSummary());
  await expect(reader.locator(".reader-summary")).toContainText("图书馆开放时间");
  await openMenu();
  await expect(menu.getByRole("menuitem", { name: "重新生成摘要" })).toBeEnabled();
  await page.keyboard.press("Escape");

  await body.evaluate(el => { el.scrollTop = 0; });
  await body.click({ button: "right", position: { x: 720, y: 240 } });
  await expect(menu).toBeVisible();
  await shot(page, "test-results/reader-menu/ctx.png");
  await menu.getByRole("menuitem", { name: "收藏", exact: false }).click();
  await openMenu();
  await expect(menu.getByRole("menuitem", { name: "取消收藏" })).toBeVisible();
  await back.click();
  await expect(reader).not.toBeVisible();
});

test("translated reader menu focuses the current view", async ({ page }) => {
  const { reader, menu, paragraph } = await openReader(page, { readers: { 2: fixture.translated } });
  await reader.getByRole("button", { name: "下一篇", exact: true }).click();
  await expect(reader.locator(".reader-view-rail")).toBeVisible();
  await paragraph.click({ button: "right" });
  await expect(menu).toBeVisible();
  await expect(menu.getByRole("menuitemradio")).toHaveText(["原文", "双语", "译文"]);
  await expect(menu.getByRole("menuitemradio", { name: "原文" })).toBeFocused();
  await expect(menu.getByRole("menuitem", { name: "翻译本篇" })).toHaveCount(0);
  await expect(menu.getByRole("menuitem", { name: "重新翻译全文" })).toBeVisible();
  await expect(menu.getByRole("menuitem", { name: "重新生成摘要" })).toBeVisible();
  await shot(page, "test-results/reader-menu/ctx-translated.png");
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("Enter");
  await expect(menu.getByRole("menuitemradio", { name: "双语" })).toBeChecked();
  await expect(reader.locator(".block-pair").first()).toBeVisible();
  await menu.getByRole("menuitemradio", { name: "原文", exact: true }).click();
  await expect(reader.locator(".block-pair")).toHaveCount(0);
  await page.keyboard.press("Escape");

  await reader.getByRole("radio", { name: "双语" }).click();
  await paragraph.click({ button: "right" });
  await expect(menu.getByRole("menuitemradio", { name: "双语" })).toBeFocused();

  await page.setViewportSize({ width: 560, height: 600 });
  await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
  const body = reader.locator(".drawer-body");
  const bounds = await body.boundingBox();
  await page.mouse.click(bounds!.x + bounds!.width - 20, bounds!.y + bounds!.height - 20, { button: "right" });
  await expect(menu).toBeVisible();
  const box = await menu.boundingBox();
  expect(box!.x).toBeGreaterThanOrEqual(bounds!.x);
  expect(box!.y).toBeGreaterThanOrEqual(bounds!.y);
  expect(box!.x + box!.width).toBeLessThanOrEqual(bounds!.x + bounds!.width);
  expect(box!.y + box!.height).toBeLessThanOrEqual(bounds!.y + bounds!.height);
  await shot(page, "test-results/reader-menu/ctx-narrow-dark.png");
  await page.keyboard.press("Escape");
  await expect(reader).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(reader).not.toBeVisible();
});

test("selection menu and native context targets", async ({ page }) => {
  const { reader, body, menu, paragraph } = await openReader(page, {});
  const link = reader.locator('[data-reader-document] a').first();
  await link.click({ button: "right" });
  const linkMenu = page.getByRole("menu", { name: "链接操作" });
  await expect(linkMenu).toBeVisible();
  await expect(menu).not.toBeVisible();
  await page.keyboard.press("Escape");
  await expect(linkMenu).not.toBeVisible();
  expect(await reader.locator("pre").evaluate(el => el.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true })))).toBe(true);
  await expect(menu).not.toBeVisible();
  const inlineCode = reader.locator("[data-reader-document] code", { hasText: "FTS5" });
  await inlineCode.scrollIntoViewIfNeeded();
  await inlineCode.click({ button: "right" });
  await expect(menu).toBeVisible();
  await page.keyboard.press("Escape");

  await body.evaluate(el => { el.scrollTop = 0; });
  await expect.poll(() => paragraph.evaluate(el => el.getBoundingClientRect().top)).toBeGreaterThan(160);
  await paragraph.evaluate(el => {
    const range = document.createRange(); range.selectNodeContents(el);
    const selection = window.getSelection()!; selection.removeAllRanges(); selection.addRange(range);
    el.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, button: 0 }));
  });
  const selectionMenu = page.getByRole("menu", { name: "选区", exact: true });
  await expect(selectionMenu.getByRole("menuitem")).toHaveText(["复制", "引用", "翻译"]);
  await shot(page, "test-results/reader-menu/select.png");
  const selectedBounds = await paragraph.boundingBox(), selectionBounds = await selectionMenu.boundingBox();
  expect(selectionBounds!.y + selectionBounds!.height).toBeLessThan(selectedBounds!.y);
  await page.keyboard.press("Escape");
  await paragraph.click({ button: "right" });
  await expect(selectionMenu).toBeVisible();
  await expect(menu).not.toBeVisible();
  await selectionMenu.getByRole("menuitem", { name: "引用", exact: true }).click();
  await expect(selectionMenu).not.toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.calls.includes("cite_selection"))).toBe(true);
});

test("return to top and Home deferral", async ({ page }) => {
  const { reader, body, menu, paragraph, back, top } = await openReader(page, {});
  const openMenu = async () => {
    await paragraph.click({ button: "right" });
    await expect(menu).toBeVisible();
  };
  const scrollFar = async () => {
    await body.evaluate(el => { el.scrollTop = el.clientHeight + 200; });
    await expect(top).toBeVisible();
  };
  await expect(top).not.toBeVisible();
  await body.evaluate(el => { el.scrollTop = el.clientHeight - 1; });
  await expect(top).not.toBeVisible();
  await body.evaluate(el => { el.scrollTop = 0; });
  await openMenu();
  await scrollFar();
  await expect(menu).not.toBeVisible();
  await top.click();
  await expect.poll(() => body.evaluate(el => el.scrollTop)).toBe(0);
  await expect(top).not.toBeVisible();
  await scrollFar();
  await back.focus();
  await page.keyboard.press("Home");
  await expect.poll(() => body.evaluate(el => el.scrollTop)).toBe(0);

  await scrollFar();
  const far = await body.evaluate(el => el.scrollTop);
  await body.evaluate(el => { const input = document.createElement("input"); input.setAttribute("aria-label", "测试输入"); el.prepend(input); input.focus({ preventScroll: true }); });
  await page.keyboard.press("Home");
  expect(await body.evaluate(el => el.scrollTop)).toBe(far);
  await body.getByRole("textbox", { name: "测试输入" }).evaluate(el => el.remove());
  await back.focus();
  await back.dispatchEvent("keydown", { key: "Home", isComposing: true });
  await back.dispatchEvent("keydown", { key: "Home", ctrlKey: true });
  expect(await body.evaluate(el => el.scrollTop)).toBe(far);
  await body.evaluate(el => { el.scrollTop = el.scrollHeight; });
  await shot(page, "test-results/reader-menu/end.png");
  await page.keyboard.press("Home");
  await expect.poll(() => body.evaluate(el => el.scrollTop)).toBe(0);
  await expect(reader).toBeVisible();
});
