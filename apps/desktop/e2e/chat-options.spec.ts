import { expect, test, type Page } from "./fixtures";
import { installMockHost } from "./mockHost";
import { shot } from "./shots";

const OUT = "../../test-results/chat-options";
const lastSend = (page: Page) => page.evaluate(() => (window as any).__mockHost.args.send_message.args);

async function shots(page: Page, name: string) {
  for (const width of [1280, 560]) {
    await page.setViewportSize({ width, height: 760 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate((t) => { document.documentElement.dataset.theme = t; }, theme);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true);
      await shot(page, `${OUT}/${name}-${width}-${theme}.png`);
    }
  }
  await page.setViewportSize({ width: 1280, height: 760 });
}

test("the model chip carries this conversation's thinking level and the ring opens context usage", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "深度对话", exact: true }).click();
  const chip = page.getByRole("button", { name: "这条对话的模型" });
  await expect(chip).toHaveText(/^Sonnet\s*low$/);

  // 还没发过：圆环为空，详情说明上限来自引擎默认值。
  await page.getByRole("button", { name: /^上下文用量：还没有/ }).click();
  const tip = page.getByRole("dialog", { name: "上下文用量" });
  await expect(tip).toContainText("还没有用量记录");
  await expect(tip).toContainText("上限 200K 是对话引擎给 Sonnet 的默认值");
  await page.keyboard.press("Escape");
  await expect(tip).toBeHidden();

  await chip.click();
  const thinking = page.getByRole("group", { name: "思考强度" });
  await expect(thinking.getByRole("button")).toHaveText(["off", "low", "med", "high"]);
  await expect(thinking.getByRole("button", { name: "low", exact: true })).toHaveAttribute("aria-pressed", "true");
  await thinking.getByRole("button", { name: "high", exact: true }).click();
  await expect(chip).toHaveText(/^Sonnet\s*high$/);
  // 按钮在输入卡右侧：列表右缘贴着它向上展开，不跑到输入卡左端。
  const box = async (selector: string) => (await page.locator(selector).first().boundingBox())!;
  const [menu, trigger, card] = [await box(".model-picker-menu"), await chip.boundingBox(), await box(".composer")];
  expect(menu.x + menu.width).toBeGreaterThanOrEqual(trigger!.x + trigger!.width);
  expect(menu.x + menu.width).toBeLessThanOrEqual(card.x + card.width);
  expect(menu.y + menu.height).toBeLessThanOrEqual(trigger!.y);
  await shots(page, "menu");
  await page.keyboard.press("Escape");

  await page.getByRole("textbox", { name: "对话消息" }).fill("这周有什么值得读？");
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect.poll(() => lastSend(page)).toMatchObject({ thinking: "high" });
  // 回复结算后按事件换成这条对话的用量。
  const ring = page.getByRole("button", { name: /^上下文用量 \d+%/ });
  await ring.click();
  await expect(tip).toContainText(/上下文 \d+ \/ 200K · \d+%/);
  await expect(tip).toContainText("上一轮结束时这条对话占用的上下文");
  await page.keyboard.press("Escape");
  // 压缩或下一轮结算的快照整份替换；80% 起转警示色并提示会压缩。
  await page.evaluate(() => {
    const host = (window as any).__mockHost;
    const id = host.state.threads[0].id;
    host.emit("chat:context", { thread_id: id, context: { used: 170_000, window: 200_000, at: Date.now() } });
  });
  await expect(page.getByRole("button", { name: /^上下文用量 85%/ })).toHaveAttribute("data-tone", "warn");
  await page.getByRole("button", { name: /^上下文用量 85%/ }).click();
  await expect(tip).toContainText("上下文 170K / 200K · 85%");
  await expect(tip).toContainText("快到上限了");
  await shots(page, "usage");
  await page.keyboard.press("Escape");

  // 选回模型默认档：跟随模型，发送时清除线程上的选择。
  await chip.click();
  await page.getByRole("group", { name: "思考强度" }).getByRole("button", { name: "low", exact: true }).click();
  await expect(page.getByRole("status").filter({ hasText: "思考强度跟随模型" })).toBeVisible();
  await page.keyboard.press("Escape");
  await page.getByRole("textbox", { name: "对话消息" }).fill("继续");
  await page.getByRole("button", { name: "发送", exact: true }).click();
  await expect.poll(() => lastSend(page)).toMatchObject({ text: "继续", thinking: "" });
});
