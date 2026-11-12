import type { Locator, Page } from "@playwright/test";
import { expect, test } from "./fixtures";
import { installMockHost, settingsTab, type MockHostOptions } from "./mockHost";
import { selectionReaderFixture, SELECTION_TRANSLATED_TEXT, TWO_ARTICLE_PAGE } from "../src/test/fixtures/reader";
import { translatePageFixture } from "../src/test/fixtures/settings";
import { shot } from "./shots";

async function openReader(page: Page, options: MockHostOptions = {}) {
  await page.setViewportSize({ width: 1280, height: 800 });
  await installMockHost(page, {
    reader: selectionReaderFixture(), focus: TWO_ARTICLE_PAGE,
    translate: translatePageFixture({ selection_config: { identity: "selection-config-1", service: "DeepL" } }),
    selectionTranslation: SELECTION_TRANSLATED_TEXT, ...options,
  });
  await page.goto("/");
  await page.getByRole("article").first().click();
  const reader = page.locator(".drawer.reader");
  return { reader, body: reader.locator(".drawer-body"), back: reader.getByRole("button", { name: "返回", exact: true }),
    card: page.getByRole("dialog", { name: "划词翻译", exact: true }),
    block: reader.locator('[data-reader-document] > [data-block-id="1"]') };
}

async function select(block: Locator) {
  await block.evaluate(el => {
    const range = document.createRange(); range.selectNodeContents(el);
    const selection = window.getSelection()!; selection.removeAllRanges(); selection.addRange(range);
    el.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, button: 0 }));
  });
}
async function translate(page: Page, block: Locator) {
  await select(block);
  const menu = page.getByRole("menu", { name: "选区", exact: true });
  await expect(menu.getByRole("menuitem")).toHaveText(["复制", "引用", "翻译"]);
  await menu.getByRole("menuitem", { name: "翻译", exact: true }).click();
}
const requests = (page: Page) => page.evaluate(() => (window as any).__mockHost.calls.filter((name: string) => name === "translate_selection").length);
const release = (page: Page) => page.evaluate(() => (window as any).__mockHost.releaseSelectionTranslation());

test("selection translation shows skeleton before the result", async ({ page }) => {
  const { block, card } = await openReader(page, { holdSelectionTranslation: true });
  expect(await requests(page)).toBe(0);
  await translate(page, block);
  await expect(card.getByRole("status", { name: "翻译中" })).toBeVisible();
  await expect(card.locator(".reader-selection-skeleton i")).toHaveCount(2);
  await expect(card.getByRole("button", { name: "复制译文" })).toBeDisabled();
  await expect.poll(() => requests(page)).toBe(1);
  await expect(card.locator(".reader-selection-service")).toHaveText("DeepL");
  await release(page);
  await expect(card.locator(".reader-selection-result")).toHaveText(SELECTION_TRANSLATED_TEXT);
  await expect(card.getByRole("status")).toHaveCount(0);
  await expect(card.getByRole("button", { name: "复制译文" })).toBeEnabled();
});

test("reopening shares both in-flight and completed selection translations", async ({ page }) => {
  const { block, card } = await openReader(page, { holdSelectionTranslation: true });
  await translate(page, block);
  await expect.poll(() => requests(page)).toBe(1);
  await page.keyboard.press("Escape");
  await translate(page, block);
  await expect(card.getByRole("status")).toBeVisible();
  expect(await requests(page)).toBe(1);
  await release(page);
  await expect(card.locator(".reader-selection-result")).toBeVisible();
  await card.getByRole("button", { name: "关闭划词翻译" }).click();
  await translate(page, block);
  await expect(card.locator(".reader-selection-result")).toHaveText(SELECTION_TRANSLATED_TEXT);
  expect(await requests(page)).toBe(1);
});

test("overlong selection stays local and suggests full translation", async ({ page }) => {
  const { reader, card } = await openReader(page, { reader: selectionReaderFixture(true) });
  await translate(page, reader.locator('[data-block-id="0"]'));
  await expect(card.getByRole("alert")).toContainText("超过 2000 字，请改用全文翻译");
  expect(await requests(page)).toBe(0);
});

test("missing service opens the translation settings category", async ({ page }) => {
  const { block, card } = await openReader(page, { translate: translatePageFixture({ selection_config: null }) });
  await translate(page, block);
  await expect(card).toContainText("翻译服务还没配置");
  await card.getByRole("button", { name: "去设置" }).click();
  await expect(card).not.toBeVisible();
  await expect(page.locator(".settings .translate-pane")).toBeVisible();
  await expect(settingsTab(page, "translate")).toHaveAttribute("aria-current", "page");
  expect(await requests(page)).toBe(0);
});

test("quote original preserves the selected text and its block id", async ({ page }) => {
  const { block, card } = await openReader(page);
  const original = await block.textContent();
  await translate(page, block);
  await expect(card.locator(".reader-selection-result")).toBeVisible();
  await card.getByRole("button", { name: "引用原句" }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.args.cite_selection)).toEqual({ articleId: 1, excerpt: original, blockId: 1 });
  await expect(card).not.toBeVisible();
});

test("card fits the reader, flips above near bottom and restores focus on Escape", async ({ page }) => {
  const { block, body, back, card, reader } = await openReader(page);
  await back.focus();
  await translate(page, block);
  await expect(card.locator(".reader-selection-result")).toBeVisible();
  const bounds = await body.boundingBox(), box = await card.boundingBox(), selected = await block.boundingBox();
  expect(box!.x).toBeGreaterThanOrEqual(bounds!.x);
  expect(box!.x + box!.width).toBeLessThanOrEqual(bounds!.x + bounds!.width);
  expect(box!.y).toBeGreaterThanOrEqual(selected!.y + selected!.height);
  await shot(page, "test-results/selection-translate/light.png");
  await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
  await shot(page, "test-results/selection-translate/dark.png");
  await page.keyboard.press("Escape");
  await expect(back).toBeFocused();
  await expect(reader).toBeVisible();
  await page.setViewportSize({ width: 560, height: 680 });
  const lowBlock = reader.locator('[data-reader-document] > [data-block-id="3"]');
  await lowBlock.evaluate(el => {
    const body = el.closest(".drawer-body")!;
    body.scrollTop += el.getBoundingClientRect().bottom - body.getBoundingClientRect().bottom + 24;
  });
  await back.focus();
  await translate(page, lowBlock);
  await expect(card.locator(".reader-selection-result")).toBeVisible();
  const narrow = await body.boundingBox(), popup = await card.boundingBox(), low = await lowBlock.boundingBox();
  expect(popup!.x).toBeGreaterThanOrEqual(narrow!.x);
  expect(popup!.y).toBeGreaterThanOrEqual(narrow!.y);
  expect(popup!.x + popup!.width).toBeLessThanOrEqual(narrow!.x + narrow!.width);
  expect(popup!.y + popup!.height).toBeLessThanOrEqual(narrow!.y + narrow!.height);
  expect(popup!.y + popup!.height).toBeLessThanOrEqual(low!.y);
  await shot(page, "test-results/selection-translate/narrow.png");
  await page.keyboard.press("Escape");
  await expect(back).toBeFocused();
});

test("outside click and article navigation close the card and ignore late responses", async ({ page }) => {
  const { block, card, reader } = await openReader(page, { holdSelectionTranslation: true });
  await translate(page, block);
  await expect.poll(() => requests(page)).toBe(1);
  await reader.getByRole("heading", { level: 1 }).click();
  await expect(card).not.toBeVisible();
  await translate(page, block);
  await expect(card).toBeVisible();
  await reader.getByRole("button", { name: "下一篇", exact: true }).click();
  await expect(card).not.toBeVisible();
  await release(page);
  await expect(card).not.toBeVisible();
  expect(await requests(page)).toBe(1);
});

test("changing translation configuration invalidates the in-memory result", async ({ page }) => {
  const { block, card, back } = await openReader(page);
  await translate(page, block);
  await expect(card.locator(".reader-selection-result")).toBeVisible();
  await page.keyboard.press("Escape");
  await back.click();
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await settingsTab(page, "translate").click();
  const english = page.locator(".translate-pane").getByRole("button", { name: "英语", exact: true });
  await english.click();
  await expect(english).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.state.translate.selection_config.identity)).not.toBe("selection-config-1");
  await page.locator(".settings").getByRole("button", { name: "返回阅读", exact: true }).click();
  await page.getByRole("article").first().click();
  await translate(page, block);
  await expect(card.locator(".reader-selection-result")).toBeVisible();
  expect(await requests(page)).toBe(2);
});

test("service errors use the existing message without retrying automatically", async ({ page }) => {
  const { block, card } = await openReader(page, { selectionTranslation: { kind: "rate_limited", message: "翻译服务限流，请稍后重试" } });
  await translate(page, block);
  await expect(card.getByRole("alert")).toHaveText("翻译服务限流，请稍后重试");
  expect(await requests(page)).toBe(1);
});

test("digest selection still offers only copy and quote", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "简报", exact: true }).click();
  const block = page.locator('.digest-doc [data-reader-document] [data-block-id="1"]');
  await expect(block).toBeVisible();
  await select(block);
  await expect(page.locator(".sel-menu").getByRole("menuitem")).toHaveText(["复制", "引用"]);
  expect(await requests(page)).toBe(0);
});
