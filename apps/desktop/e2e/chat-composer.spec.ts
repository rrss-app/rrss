import { poolEntryFixture } from "../src/test/fixtures/settings";
import { expect, test, type Page } from "./fixtures";
import { installMockHost, MOCK_POOL } from "./mockHost";
import { shot } from "./shots";

// 对话输入卡一行起、模型行常驻卡下（专题 22 · 输入卡与模型行），菜单里就地改显示名（19-7 对话里）。
const OUT = "../../test-results/chat-composer";
const LONG = "deepseek/deepseek-r1-distill-qwen-32b-instruct-2025-01-20:free";
const POOL = { ...MOCK_POOL, pool: [...MOCK_POOL.pool, poolEntryFixture({ provider_id: 2, model: LONG, chat: true })] };

async function openChat(page: Page) {
  await page.setViewportSize({ width: 1280, height: 760 });
  await installMockHost(page, { pool: POOL });
  await page.goto("/");
  await page.getByRole("button", { name: "新对话", exact: true }).click();
  const composer = page.locator(".composer");
  const model = composer.getByRole("button", { name: "这条对话的模型" });
  await expect(model).toBeEnabled();
  return { composer, card: composer.locator(".input-row"), row: composer.locator(".composer-model-row"), model };
}

const box = async (locator: ReturnType<Page["locator"]>) => (await locator.boundingBox())!;

test("the model row stays under a one-line input card while the card grows and the send key fills in", async ({ page }) => {
  const { composer, card, row, model } = await openChat(page);
  await expect(card.getByRole("button", { name: "这条对话的模型" })).toHaveCount(0);
  await expect(row.locator(".context-meter")).toHaveCount(1);
  const empty = await box(card), under = await box(row);
  expect(under.y).toBeGreaterThanOrEqual(empty.y + empty.height);
  const send = card.getByRole("button", { name: "发送", exact: true });
  await expect(send).toHaveAttribute("data-ghost", "");

  // 长 id 截断在模型行里，不挤输入框。
  await model.click();
  await page.getByRole("option", { name: /deepseek-r1-distill/ }).click();
  await expect(model).toContainText("deepseek-r1");
  const picker = await box(composer.locator(".composer-model-row .model-picker"));
  expect(picker.width).toBeLessThanOrEqual(under.width * 0.7 + 1);
  expect((await box(card.getByRole("textbox"))).width).toBeGreaterThan(empty.width - 80);
  await shots(page, "empty");

  // 多行时只有输入卡长高，模型行不动；有草稿时回车键变实底。
  await card.getByRole("textbox").fill("第一行\n第二行\n第三行\n第四行");
  await expect(send).not.toHaveAttribute("data-ghost", "");
  await expect.poll(async () => (await box(card)).height).toBeGreaterThan(empty.height + 40);
  expect((await box(row)).y).toBeCloseTo(under.y, 0);
  await shots(page, "draft");
});

test("renaming in the conversation menu saves the display name and keeps the menu open", async ({ page }) => {
  const { model } = await openChat(page);
  await model.click();
  const menu = page.getByRole("dialog", { name: "这条对话的模型" });
  const long = menu.getByRole("option", { name: /deepseek-r1-distill/ });
  await expect(long).toContainText("自动");
  await long.hover();
  await shot(page, `${OUT}/menu-1280-light.png`);
  await menu.getByRole("button", { name: /^改名「deepseek-r1/ }).click();
  const input = menu.getByRole("textbox", { name: `「${LONG}」的显示名` });
  await expect(input).toBeFocused();
  await input.fill("  R1 蒸馏  ");
  await shot(page, `${OUT}/rename-1280-light.png`);
  await input.press("Enter");
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.args.update_pool_model?.args))
    .toEqual({ provider_id: 2, model: LONG, display_name: "R1 蒸馏" });
  await expect(input).toHaveCount(0);
  await expect(menu).toBeVisible();
  await expect(menu.getByRole("option", { name: /R1 蒸馏/ })).not.toContainText("自动");
  await expect(menu.getByRole("button", { name: "改名「R1 蒸馏」" })).toBeFocused();

  // Esc 只取消改名，不关菜单；再按一次才关。
  await menu.getByRole("button", { name: "改名「Sonnet」" }).click();
  await menu.getByRole("textbox", { name: "「anthropic/claude-sonnet-4」的显示名" }).fill("别的名字");
  await page.keyboard.press("Escape");
  await expect(menu.locator(".model-picker-rename-edit")).toHaveCount(0);
  await expect(menu).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(menu).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__mockHost.calls.filter((c: string) => c === "update_pool_model").length)).toBe(1);

  await model.click();
  await page.getByRole("option", { name: /R1 蒸馏/ }).click();
  await expect(model).toContainText("R1 蒸馏");
  await expect(model.locator(".model-picker-text")).not.toHaveClass(/mono/);
});

async function shots(page: Page, name: string) {
  for (const width of [1280, 560]) {
    await page.setViewportSize({ width, height: 760 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate((t) => { document.documentElement.dataset.theme = t; }, theme);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true);
      await shot(page, `${OUT}/${name}-${width}-${theme}.png`);
    }
  }
  await page.evaluate(() => { document.documentElement.dataset.theme = "light"; });
  await page.setViewportSize({ width: 1280, height: 760 });
}
