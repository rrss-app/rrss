import { expect, test, type Page } from "./fixtures";
import { installMockHost, settingsTab } from "./mockHost";
import { shot } from "./shots";

// 对话只列「用于对话」的模型（阶段 19-7）：对话助手页选默认，对话里临时换。
const OUT = "../../test-results/model-pool";

async function shots(page: Page, name: string, widths = [1280, 560]) {
  for (const width of widths) {
    await page.setViewportSize({ width, height: 760 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate((t) => { document.documentElement.dataset.theme = t; }, theme);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true);
      await shot(page, `${OUT}/${name}-${width}-${theme}.png`);
    }
  }
  await page.setViewportSize({ width: 1280, height: 760 });
}

test("the assistant picks the chat default from chat models only", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  await settingsTab(settings, "assistant").click();
  await expect(settings.getByText("Sonnet · low")).toBeVisible();
  await settings.getByRole("button", { name: "对话模型", exact: true }).click();
  const menu = page.getByRole("dialog", { name: "对话模型" });
  await expect(menu.getByRole("option")).toHaveCount(2);
  await menu.getByRole("combobox", { name: "搜索模型" }).fill("qwen");
  await expect(menu.getByRole("option")).toHaveCount(1);
  await shots(page, "assistant-picker");
  await page.keyboard.press("Enter");
  await expect(settings.getByRole("button", { name: "对话模型", exact: true })).toHaveText("qwen2.5:14b 本机 Ollama");
  const args = await page.evaluate(() => (window as any).__mockHost.args.save_model_bindings.args);
  expect(args).toEqual({ bindings: [{ task: "chat", provider_id: 1, model: "qwen2.5:14b" }] });
  await expect(settings.getByText("qwen2.5:14b · off")).toBeVisible();
});

test("switching the model inside a conversation only changes that conversation", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "深度对话", exact: true }).click();
  const chip = page.getByRole("button", { name: "这条对话的模型" });
  await expect(chip).toHaveText(/^Sonnet\s*low$/);
  await chip.click();
  const menu = page.getByRole("dialog", { name: "这条对话的模型" });
  await expect(menu).toContainText("只影响这条对话之后的回复");
  await expect(menu.getByRole("option")).toHaveCount(2);
  await shots(page, "chat-picker");
  await menu.getByRole("option", { name: /qwen2\.5:14b/ }).click();
  await expect(chip).toContainText("qwen2.5:14b");
  await shots(page, "chat-chip", [1280]);
  await page.getByRole("textbox", { name: "对话消息" }).fill("这周有什么值得读？");
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.args.send_message?.args.model))
    .toEqual({ provider_id: 1, model: "qwen2.5:14b" });
});
