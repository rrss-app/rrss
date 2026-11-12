import { expect, test, type Page } from "./fixtures";
import { installMockHost, MOCK_FOCUS, MOCK_MESSAGES, MOCK_READER, type MockHostOptions } from "./mockHost";
import { shot } from "./shots";

const OUT = "../../test-results/worker-error-recovery";
const reader = (page: Page) => page.locator(".drawer.reader");
const repair = (page: Page) => page.evaluate(() => (window as any).__mockHost.repairBlocks());

async function setup(page: Page, width: number, options: MockHostOptions) {
  await page.setViewportSize({ width, height: 800 });
  await page.emulateMedia({ reducedMotion: "reduce" });
  // 入口校验会清洗未知节点。只在 e2e 浏览器里替换渲染函数，模拟坏块引起的组件异常；
  // 宿主载荷、IPC、ErrorBoundary 与恢复动作仍使用应用实现。
  await page.route("**/src/components/BlockView.tsx*", async route => {
    const response = await route.fetch();
    const source = await response.text();
    const entry = "const digestUnsafe =";
    expect(source).toContain(entry);
    await route.fulfill({ response, body: source.replace(entry,
      'if (document?.__rrssMockBroken) throw new Error("mock block renderer failed");\n  ' + entry) });
  });
  await installMockHost(page, options);
  await page.addInitScript(() => {
    const host = (window as any).__TAURI_INTERNALS__;
    const invoke = host.invoke;
    host.invoke = (command: string, args: unknown) => {
      if (command === "plugin:opener|open_url") {
        (window as any).__mockHost.calls.push(command);
        (window as any).__mockHost.args[command] = args;
        return Promise.resolve();
      }
      return invoke(command, args);
    };
  });
  await page.goto("/");
  await expect(page.locator(".article").first()).toBeVisible();
}

async function shots(page: Page, area: string, state: "normal" | "error", width: number) {
  if (process.env.RRSS_SHOTS !== "1") return;
  await page.evaluate(() => document.fonts.ready);
  for (const theme of ["light", "dark"]) {
    await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme);
    await shot(page, `${OUT}/${area}-${state}-${width}-${theme}.png`);
  }
}

async function escapeSearch(page: Page) {
  const opener = await page.evaluateHandle(() => document.activeElement);
  await page.keyboard.press("Control+k");
  const search = page.getByRole("dialog", { name: "搜索文章" });
  await expect(search).toBeVisible();
  await expect(search.getByRole("searchbox")).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(search).toHaveCount(0);
  await expect.poll(() => opener.evaluate(element => element === document.activeElement && element?.isConnected)).toBe(true);
  await opener.dispose();
}

for (const width of [1280, 560]) {
  test(`reader error, retry, return and another article at ${width}`, async ({ page }) => {
    const second = { ...MOCK_READER, id: 2, source_hash: "recovery-reader-2", title: MOCK_FOCUS.articles[1].title, url: "https://example.test/2" };
    await setup(page, width, { brokenBlocks: { reader: [1] }, readers: { 2: second } });
    await page.locator(".article").first().click();
    const alert = reader(page).locator(".reader-body-error[role=alert]");
    await expect(alert).toBeVisible();
    await expect(reader(page).locator(".reader-title")).toBeVisible();
    await expect(reader(page).locator(".reader-summary")).toBeVisible();
    await expect(reader(page).locator(".reader-ending")).toBeVisible();
    await expect(reader(page).locator("[data-reader-document]")).toHaveCount(0);
    await shots(page, "reader", "error", width);

    await alert.getByRole("button", { name: "查看原文", exact: true }).click();
    await expect.poll(() => page.evaluate(() => (window as any).__mockHost.args["plugin:opener|open_url"]?.url)).toBe(MOCK_READER.url);
    await repair(page);
    await alert.getByRole("button", { name: "重试", exact: true }).click();
    await expect(alert).toHaveCount(0);
    await expect(reader(page).locator("[data-reader-document] > [data-block-id]")).toHaveCount(8);
    await reader(page).getByRole("button", { name: "返回", exact: true }).focus();
    await escapeSearch(page);
    await expect(reader(page)).toBeVisible();
    await reader(page).getByRole("button", { name: "返回", exact: true }).click();
    await expect(reader(page)).toHaveCount(0);
    const card = page.getByRole("article", { name: second.title, exact: true });
    await card.click();
    await expect(reader(page).locator(".reader-title")).toHaveText(second.title);
    await expect(reader(page).locator(".reader-body-error")).toHaveCount(0);
    await shots(page, "reader", "normal", width);
    await reader(page).getByRole("button", { name: "返回", exact: true }).focus();
    await expect(reader(page).getByRole("button", { name: "返回", exact: true })).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(reader(page)).toHaveCount(0);
    await page.getByRole("button", { name: "简报", exact: true }).click();
    await expect(page.locator(".digest")).toBeVisible();
    await page.getByRole("button", { name: "今日焦点", exact: true }).click();
    await expect(page.locator(".focus")).toBeVisible();
  });

  test(`digest error, sources, retry and navigation at ${width}`, async ({ page }) => {
    await setup(page, width, { brokenBlocks: { digest: [101] } });
    await page.getByRole("button", { name: "简报", exact: true }).click();
    const digest = page.locator(".digest");
    const alert = digest.locator(".digest-doc-error[role=alert]");
    await expect(alert).toBeVisible();
    await expect(digest.locator(".digest-doc-head")).toBeVisible();
    await expect(digest.locator(".digest-discussion .bubble.assistant")).toBeVisible();
    await digest.locator(".digest-source-index summary").first().click();
    const source = digest.getByRole("button", { name: /来源 1：/ }).first();
    await expect(source).toBeVisible();
    await shots(page, "digest", "error", width);
    await source.click();
    await expect(reader(page)).toBeVisible();
    await reader(page).getByRole("button", { name: "返回", exact: true }).focus();
    await escapeSearch(page);
    await expect(reader(page)).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(reader(page)).toHaveCount(0);
    await expect(alert).toBeVisible();
    await repair(page);
    await alert.getByRole("button", { name: "重试", exact: true }).click();
    await expect(alert).toHaveCount(0);
    await expect(digest.locator("[data-digest-prose]")).toBeVisible();
    await shots(page, "digest", "normal", width);
    await page.getByRole("button", { name: "今日焦点", exact: true }).click();
    await expect(page.locator(".focus")).toBeVisible();
    await page.getByRole("button", { name: "简报", exact: true }).click();
    await expect(digest).toBeVisible();
    await page.keyboard.press("/");
    await expect(page.getByRole("textbox", { name: "追问当前简报" })).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(digest).toBeVisible();
  });

  test(`one chat message fails, another renders and sending continues at ${width}`, async ({ page }) => {
    const normal = { ...MOCK_MESSAGES[7][1], id: 20 };
    await setup(page, width, { brokenBlocks: { messages: [2] }, messages: { ...MOCK_MESSAGES, 7: [...MOCK_MESSAGES[7], normal] } });
    await page.getByRole("button", { name: "查证 Rust 1.97 的回归修复", exact: true }).click();
    const chat = page.locator(".chat-pane");
    const alert = chat.locator(".message-error[role=alert]");
    await expect(alert).toHaveCount(1);
    await expect(alert).toBeVisible();
    await expect(chat.locator(".bubble.user .message-body")).toBeVisible();
    await expect(chat.locator(".bubble.assistant [data-reader-document]")).toHaveCount(1);
    await shots(page, "chat", "error", width);
    const input = page.getByRole("textbox", { name: "对话消息" });
    await input.fill("继续检查恢复路径");
    await expect(input).toBeFocused();
    await input.press("Enter");
    await expect(input).toBeEnabled();
    await expect(input).toHaveValue("");
    await expect(chat.locator(".bubble.assistant")).toHaveCount(3);
    await expect(alert).toHaveCount(1);
    await input.focus();
    await escapeSearch(page);
    await expect(chat).toBeVisible();
    await page.getByRole("button", { name: "简报", exact: true }).click();
    await expect(page.locator(".digest")).toBeVisible();
    await page.getByRole("button", { name: "深度对话", exact: true }).click();
    await expect(chat).toBeVisible();
    await input.fill("下一轮草稿");
    await expect(input).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(chat).toBeVisible();
    await expect(input).toBeFocused();
    await expect(input).toHaveValue("下一轮草稿");
    await repair(page);
    await page.getByRole("button", { name: "Tauri IPC 线程模型", exact: true }).click();
    await page.getByRole("button", { name: "查证 Rust 1.97 的回归修复", exact: true }).click();
    await expect(alert).toHaveCount(0);
    await expect(chat.locator(".bubble.assistant [data-reader-document]")).toHaveCount(2);
    await shots(page, "chat", "normal", width);
  });
}
