import { expect, test, type Page } from "./fixtures";
import { installMockHost, MOCK_THREADS } from "./mockHost";

// 对话页打磨第一批：打开落底、发送立即显示并到底、失败撤回、贴底跟随与「回到最新」。
const row = (id: number) => ({
  id, role: id % 2 ? "user" : "assistant", text: `第 ${id} 条：${"这一段用来把对话撑长，好看清滚动位置。".repeat(id % 2 ? 2 : 12)}`,
  citations: [], document: null, streaming: false, cite_article_id: 0, cite_title: "",
});
const history = Array.from({ length: 24 }, (_, i) => row(i + 1));

const pane = (page: Page) => page.locator(".chat-pane");
const gap = (page: Page) => pane(page).evaluate((el) => el.scrollHeight - el.clientHeight - el.scrollTop);
const emit = (page: Page, event: string, payload: unknown) =>
  page.evaluate(([name, value]) => (window as any).__mockHost.emit(name, value), [event, payload] as const);

async function openLong(page: Page, width = 1280, options: Parameters<typeof installMockHost>[1] = {}) {
  await page.setViewportSize({ width, height: 800 });
  await installMockHost(page, { messages: { 7: history }, ...options });
  await page.goto("/");
  await page.getByRole("button", { name: MOCK_THREADS[0].title, exact: true }).click();
  await expect(pane(page).locator(".bubble")).toHaveCount(24);
}

test("a long conversation opens at the bottom; a sent message shows at once, glides down and turns solid", async ({ page }) => {
  await openLong(page, 1280, { holdSend: true });
  await expect.poll(() => gap(page)).toBeLessThanOrEqual(1);
  await pane(page).evaluate((el) => { el.scrollTop = 0; });
  const input = page.getByRole("textbox", { name: "对话消息" });
  await input.fill("再总结一下要点");
  await input.press("Enter");
  // 不等宿主：输入框立刻清空，自己的消息以发送中的样子出现在末尾，并滚到底。
  await expect(input).toHaveValue("");
  await expect(input).toBeEnabled();
  const pending = pane(page).locator(".bubble.user[data-pending]");
  await expect(pending).toHaveText("再总结一下要点");
  await expect.poll(() => gap(page)).toBeLessThanOrEqual(1);
  await page.evaluate(() => (window as any).__mockHost.releaseSend());
  await expect(pending).toHaveCount(0);
  await expect(pane(page).locator(".bubble.user").last()).toHaveText("再总结一下要点");
  await expect(pane(page).locator(".bubble.user")).toHaveCount(13);
  await expect(pane(page).locator(".bubble").last()).toContainText("我会结合这些引用继续分析");
});

test("a failed send takes the message back and returns the text to the input", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await installMockHost(page, { holdSend: true, sendFailures: 1 });
  await page.goto("/");
  await page.getByRole("button", { name: "新对话", exact: true }).click();
  const input = page.getByRole("textbox", { name: "对话消息" });
  await input.fill("这条会失败");
  await input.press("Enter");
  await expect(pane(page).locator(".bubble.user[data-pending]")).toHaveText("这条会失败");
  // 发送期间接着打的字排在放回的文字后面。
  await input.fill("补一句");
  await page.evaluate(() => (window as any).__mockHost.releaseSend());
  await expect(page.getByRole("status").filter({ hasText: "模拟发送失败" })).toBeVisible();
  await expect(pane(page).locator(".bubble")).toHaveCount(0);
  await expect(input).toHaveValue("这条会失败\n补一句");
});

test("scrolling up during a reply stops following; the jump key announces new replies and returns to the bottom", async ({ page }) => {
  await openLong(page);
  const jump = page.locator(".jump-latest");
  await emit(page, "chat:start", 7);
  await emit(page, "chat:delta", { thread_id: 7, text: "先看第一条。", document: null });
  await expect.poll(() => gap(page)).toBeLessThanOrEqual(1);
  await expect(jump).not.toHaveAttribute("data-show");
  // 贴底时回复变长仍在底部。
  await emit(page, "chat:delta", { thread_id: 7, text: `先看第一条。\n${"补充说明。".repeat(80)}`, document: null });
  await expect.poll(() => gap(page)).toBeLessThanOrEqual(1);
  const box = (await pane(page).boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.wheel(0, -900);
  await expect(jump).toHaveAttribute("data-show", "");
  await expect(jump).toHaveAccessibleName("回到最新");
  const before = await gap(page);
  await emit(page, "chat:delta", { thread_id: 7, text: `先看第一条。\n${"补充说明。".repeat(160)}`, document: null });
  // 不被拉回底部；按键展开成「有新回复」。
  await expect(jump).toHaveAttribute("data-new", "");
  await expect(jump).toHaveAccessibleName("有新回复");
  expect(await gap(page)).toBeGreaterThan(before);
  // 挂在输入区上方，输入长高时一起上移。
  const input = page.getByRole("textbox", { name: "对话消息" });
  const oneLine = (await jump.boundingBox())!.y;
  await input.fill("第一行\n第二行\n第三行");
  await expect.poll(async () => (await jump.boundingBox())!.y).toBeLessThan(oneLine - 40);
  await jump.click();
  await expect.poll(() => gap(page)).toBeLessThanOrEqual(1);
  await expect(jump).not.toHaveAttribute("data-show");
  // 回到底部后恢复跟随。
  await emit(page, "chat:delta", { thread_id: 7, text: `先看第一条。\n${"补充说明。".repeat(240)}`, document: null });
  await expect.poll(() => gap(page)).toBeLessThanOrEqual(1);
});

test("the jump key sits outside the column on wide panes and inside it on narrow ones", async ({ page }) => {
  for (const [width, outside] of [[1440, true], [900, false]] as const) {
    await openLong(page, width);
    await pane(page).evaluate((el) => { el.scrollTop = 0; });
    const jump = page.locator(".jump-latest[data-show]");
    await expect(jump).toBeVisible();
    const key = (await jump.boundingBox())!;
    const card = (await page.locator(".composer .input-row").boundingBox())!;
    if (outside) expect(key.x).toBeGreaterThanOrEqual(card.x + card.width + 12);
    else expect(Math.abs(key.x + key.width - (card.x + card.width))).toBeLessThanOrEqual(1);
    expect(key.y + key.height).toBeLessThanOrEqual(card.y);
  }
});

test("the jump key keeps a solid face when hovered and pressed, so text underneath never shows through", async ({ page }) => {
  await openLong(page);
  await pane(page).evaluate((el) => { el.scrollTop = 0; });
  const jump = page.locator(".jump-latest[data-show]");
  await expect(jump).toBeVisible();
  // 通用按钮的悬停 / 按下底色是半透明薄雾；这里按画到画布上的不透明度核对，不比具体色值。
  const alpha = () => jump.evaluate((el) => {
    const ctx = document.createElement("canvas").getContext("2d")!;
    ctx.fillStyle = getComputedStyle(el).backgroundColor;
    ctx.fillRect(0, 0, 1, 1);
    return ctx.getImageData(0, 0, 1, 1).data[3];
  });
  await jump.hover();
  await expect.poll(alpha).toBe(255);
  await page.mouse.down();
  await expect.poll(alpha).toBe(255);
  await page.mouse.up();
});

test("a long conversation title ends with an ellipsis instead of scrolling the conversation list sideways", async ({ page }) => {
  const long = "这是不是就是 APT 攻击？我怎么感觉这件事情不太对劲，而且越想越不对";
  await page.setViewportSize({ width: 1280, height: 800 });
  await installMockHost(page, { threads: [{ ...MOCK_THREADS[0], title: long }, MOCK_THREADS[1]] });
  await page.goto("/");
  const item = page.getByRole("button", { name: long, exact: true });
  await item.click();
  const list = page.locator(".thread-list");
  expect(await list.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(0);
  const title = item.locator(".thread-title");
  expect(await title.evaluate((el) => el.scrollWidth > el.clientWidth)).toBe(true);
  expect((await item.boundingBox())!.width).toBeLessThanOrEqual((await list.boundingBox())!.width);
});
