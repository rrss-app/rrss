import { expect, test, type Page } from "./fixtures";
import { installMockHost, MOCK_MESSAGES, MOCK_THREADS } from "./mockHost";

test("chat input keeps model selection, explains Enter, replaces send with stop and refreshes the title", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "新对话", exact: true }).click();
  await expect(page.locator(".chat-header h1")).toHaveText("新对话");
  const line = page.locator(".composer .input-row");
  const model = page.locator(".composer").getByRole("button", { name: "这条对话的模型" });
  await model.click();
  await page.getByRole("option", { name: /qwen2\.5:14b/ }).click();
  await expect(model).toContainText("qwen2.5:14b");
  await line.getByRole("textbox", { name: "对话消息" }).fill("这周有什么值得读？");
  const send = line.getByRole("button", { name: "发送", exact: true });
  await send.hover();
  await expect(page.getByRole("tooltip")).toHaveText("Enter 发送 · Shift+Enter 换行");
  await send.click();
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.args.send_message?.args.model))
    .toEqual({ provider_id: 1, model: "qwen2.5:14b" });
  await expect(page.locator(".chat-header h1")).toHaveText("这周有什么值得读？");
  await expect(page.locator('.thread-item[data-on="1"]')).toHaveText("这周有什么值得读？");
  // 流式快照不改标题；完成后的原有会话列表刷新带回新标题。
  await page.evaluate(() => {
    const host = (window as any).__mockHost;
    host.emit("chat:start", 401);
    host.emit("chat:delta", { thread_id: 401, text: "正在整理值得读的内容。", document: null });
  });
  await expect(line.getByRole("button", { name: "停止生成", exact: true })).toBeVisible();
  await expect(send).toHaveCount(0);
  await line.getByRole("button", { name: "停止生成", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.calls.includes("abort_generation"))).toBe(true);
  await page.evaluate(() => {
    const host = (window as any).__mockHost;
    host.state.threads[0].title = "本周阅读推荐";
    host.emit("chat:done", 401);
  });
  await expect(page.locator(".chat-header h1")).toHaveText("本周阅读推荐");
});

const OUT = "../../test-results/chat-look/current";
async function screenshot(page: Page, name: string, theme: string) {
  await page.mouse.move(1279, 799);
  await page.evaluate(() => document.fonts.ready);
  await page.screenshot({ path: `${OUT}/${name}-1280-${theme}.png`, animations: "disabled" });
}

for (const theme of ["light", "dark"]) test(`chat appearance review ${theme}`, async ({ page }) => {
  test.skip(process.env.RRSS_SHOTS !== "1", "仅在外观审阅时生成截图");
  await page.setViewportSize({ width: 1280, height: 800 });
  const question = MOCK_MESSAGES[7][0];
  const full = question.citations[0];
  await installMockHost(page, { messages: { ...MOCK_MESSAGES, 7: [
    { ...question, citations: [{ ...full, kind: "span", excerpt: "主要修了三类问题，建议在 CI 通过后升级。", block_id: 0 }, full] },
    MOCK_MESSAGES[7][1],
  ] } });
  await page.goto("/");
  await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme);
  await page.getByRole("button", { name: MOCK_THREADS[0].title, exact: true }).click();
  await expect(page.locator(".chat-pane .citation-chip")).toHaveCount(2);
  await expect(page.locator(".chat-pane .bubble.assistant")).toBeVisible();
  await screenshot(page, "chat-after-send", theme);
  await page.evaluate(() => {
    const host = (window as any).__mockHost;
    host.emit("chat:start", 7);
    host.emit("chat:delta", { thread_id: 7, text: "我正在核对回归修复涉及的版本与来源。", document: null });
  });
  await expect(page.getByRole("button", { name: "停止生成", exact: true })).toBeVisible();
  await screenshot(page, "chat-generating", theme);
  await page.evaluate(() => (window as any).__mockHost.emit("chat:done", 7));
  await page.getByRole("button", { name: "简报", exact: true }).click();
  await page.getByRole("region", { name: "当期简报追问" }).scrollIntoViewIfNeeded();
  await page.getByRole("textbox", { name: "追问当前简报" }).fill("展开第二条，解释按需加载的取舍。");
  await screenshot(page, "digest-followup", theme);
  await page.getByRole("button", { name: "今日焦点", exact: true }).click();
  const article = page.getByRole("article").first();
  await article.hover();
  await article.getByRole("button", { name: "引用", exact: true }).click();
  await page.getByRole("button", { name: "关闭通知", exact: true }).click();
  await page.getByRole("textbox", { name: "对话消息" }).fill("结合这些引用解释一下");
  await screenshot(page, "composer-with-citations", theme);
});
