import { mkdirSync } from "node:fs";
import path from "node:path";
import { expect, test, type Page, type Locator } from "./fixtures";
import { installMockHost, MOCK_THREADS, type MockHostOptions } from "./mockHost";
import { readerMenuFixtures, TWO_ARTICLE_PAGE } from "../src/test/fixtures/reader";
import { translatePageFixture } from "../src/test/fixtures/settings";

const fixture = readerMenuFixtures();
const trayFor = (page: Page) => page.getByRole("region", { name: "引用托盘" });
const readerFor = (page: Page) => page.locator(".drawer.reader");
const calls = (page: Page, command: string) => page.evaluate(cmd => (window as any).__mockHost.calls.filter((c: string) => c === cmd).length, command);
async function setup(page: Page, options: MockHostOptions = {}) {
  await page.setViewportSize({ width: 1280, height: 800 });
  await installMockHost(page, { focus: { ...TWO_ARTICLE_PAGE, articles: TWO_ARTICLE_PAGE.articles.map((row, i) => ({ ...row, title: i ? fixture.translated.title : fixture.plain.title })) }, reader: fixture.plain, readers: { 2: fixture.translated }, ...options });
  await page.goto("/");
}
async function open(page: Page) {
  await page.getByRole("article").first().click();
  await expect(readerFor(page)).toBeVisible();
}
async function citeFull(page: Page) {
  await readerFor(page).locator("summary[aria-label=更多]").click();
  await readerFor(page).getByRole("button", { name: "引用全文到对话", exact: true }).click();
  await expect(trayFor(page)).toBeVisible();
}
async function select(page: Page, block: Locator) {
  await block.scrollIntoViewIfNeeded();
  await block.evaluate(el => {
    const range = document.createRange(); range.selectNodeContents(el);
    const selection = window.getSelection()!; selection.removeAllRanges(); selection.addRange(range);
    el.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, button: 0 }));
  });
  await expect(page.getByRole("menu", { name: "选区", exact: true })).toBeVisible();
}
async function citeSpan(page: Page) {
  await select(page, readerFor(page).locator('[data-reader-document] > [data-block-id="2"]'));
  await page.getByRole("menuitem", { name: "引用", exact: true }).click();
}
async function targetMenu(page: Page) {
  await trayFor(page).getByRole("button", { name: /^发到 / }).click();
  return page.getByRole("menu", { name: "发到", exact: true });
}

test("reader citation entrances share a persistent tray, deduplicate full articles and obey Escape", async ({ page }) => {
  await setup(page, { translate: translatePageFixture({ selection_config: { identity: "citation-selection", service: "DeepL" } }), selectionTranslation: "这是一段翻译。" });
  await open(page);
  const reader = readerFor(page), tray = trayFor(page);
  const body = reader.locator(".drawer-body");
  const paragraph = reader.locator('[data-reader-document] > [data-block-id="2"]');
  await citeSpan(page);
  await expect(tray.locator(".reader-citation-chip")).toHaveCount(1);
  await paragraph.click({ button: "right" });
  await page.getByRole("menuitem", { name: "引用全文到对话" }).click();
  await expect(tray.locator(".reader-citation-chip")).toHaveCount(2);
  await citeFull(page); // 更多菜单的全文引用不重复。
  await expect(tray.locator(".reader-citation-chip")).toHaveCount(2);
  await body.evaluate(el => { el.scrollTop = el.scrollHeight; });
  await reader.getByRole("button", { name: "引用到对话", exact: true }).click();
  await expect(tray.locator(".reader-citation-chip")).toHaveCount(2);
  await body.evaluate(el => { el.scrollTop = 0; });
  await select(page, paragraph);
  await page.getByRole("menuitem", { name: "翻译", exact: true }).click();
  await expect(page.locator(".reader-selection-result")).toBeVisible();
  await page.getByRole("button", { name: "引用原句" }).click();
  await expect(tray.locator(".reader-citation-chip")).toHaveCount(3);
  await reader.getByRole("button", { name: "下一篇", exact: true }).click();
  await expect(tray.locator(".reader-citation-chip")).toHaveCount(3);
  await citeFull(page);
  await expect(tray.locator(".reader-citation-chip")).toHaveCount(4);
  const input = tray.getByRole("textbox");
  const before = await calls(page, "open_article");
  await input.fill("j k x s m t u l");
  await input.press("j"); await input.press("k"); await input.press("x"); await input.press("s");
  expect(await calls(page, "open_article")).toBe(before);
  expect(await calls(page, "rate_article")).toBe(0);
  expect(await calls(page, "star_article")).toBe(0);
  await reader.getByRole("button", { name: "篇目", exact: true }).click();
  await expect(page.locator(".reader-article-drawer")).toBeVisible();
  // 钉住时托盘随正文区居中；回顶按钮在托盘之上。
  await page.getByRole("button", { name: "钉住", exact: true }).click();
  const region = await reader.boundingBox(), dock = await tray.boundingBox();
  expect(Math.abs((region!.x + region!.width / 2) - (dock!.x + dock!.width / 2))).toBeLessThan(1);
  expect(dock!.width).toBeLessThanOrEqual(Math.min(680, region!.width - 120));
  await body.evaluate(el => { el.scrollTop = el.clientHeight + 200; });
  const topButton = reader.getByRole("button", { name: "回到顶部", exact: true });
  await expect(topButton).toBeVisible();
  const backTop = await topButton.boundingBox();
  expect(backTop!.y + backTop!.height).toBeLessThan(dock!.y);
  await page.getByRole("button", { name: "取消钉住", exact: true }).click();
  // 发到菜单优先于未钉住抽屉（用键盘触发同时存在的状态）。
  await tray.getByRole("button", { name: /^发到 / }).dispatchEvent("click");
  await expect(page.getByRole("menu", { name: "发到", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu", { name: "发到", exact: true })).not.toBeVisible();
  await expect(page.locator(".reader-article-drawer")).toBeVisible();
  // 未钉住的抽屉先于托盘关闭；即使输入框聚焦，Esc 仍交给叠层。
  await page.keyboard.press("Escape");
  await expect(page.locator(".reader-article-drawer")).not.toBeVisible();
  await input.focus();
  await targetMenu(page);
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu", { name: "发到", exact: true })).not.toBeVisible();
  await expect(tray).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(tray).toBeVisible(); await expect(reader).toBeVisible();
  await tray.getByRole("button", { name: "移除引用 1", exact: true }).click();
  await expect(tray.locator(".reader-citation-chip")).toHaveCount(3);
  await tray.getByRole("button", { name: "清空" }).click();
  await expect(tray.locator(".reader-citation-chip")).toHaveCount(0);
  await expect(input).toHaveValue("j k x s m t u ljkxs");
  await input.focus();
  await page.keyboard.press("Escape");
  await expect(tray).not.toBeVisible(); await expect(reader).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(reader).not.toBeVisible();
});

test("tray sends to new or recent thread, keeps the target draft and preserves reader position", async ({ page }) => {
  await setup(page);
  await page.getByRole("button", { name: MOCK_THREADS[0].title, exact: true }).click();
  await page.getByRole("textbox", { name: "对话消息" }).fill("目标原有草稿");
  await page.getByRole("button", { name: "今日焦点", exact: true }).click();
  await open(page);
  const tray = trayFor(page), reader = readerFor(page);
  await citeFull(page);
  await tray.getByRole("textbox").fill("本次收集的问题");
  const menu = await targetMenu(page);
  await expect(menu.getByRole("menuitemradio", { name: "新对话", exact: true })).toBeChecked();
  await menu.getByRole("menuitemradio", { name: MOCK_THREADS[0].title, exact: true }).click();
  await expect(tray.getByRole("textbox")).toHaveValue("目标原有草稿\n本次收集的问题");
  // 阅读中经搜索换篇：托盘与改选的目标都保留。
  await page.keyboard.press("Control+k");
  const search = page.getByRole("dialog", { name: "搜索文章" });
  await search.getByRole("searchbox").fill(fixture.translated.title);
  await search.getByRole("option").first().click();
  await expect(reader.getByRole("heading", { name: fixture.translated.title, level: 1 })).toBeVisible();
  await expect(tray.getByRole("button", { name: `发到 ${MOCK_THREADS[0].title}` })).toBeVisible();
  await expect(tray.locator(".reader-citation-chip")).toHaveCount(1);
  await tray.getByRole("textbox").fill("在已有会话中继续分析");
  await reader.locator(".drawer-body").evaluate(el => { el.scrollTop = 650; });
  const top = await reader.locator(".drawer-body").evaluate(el => el.scrollTop);
  await tray.getByRole("button", { name: "发送", exact: true }).click();
  await expect(reader).not.toBeVisible();
  await expect(page.locator('.thread-item[data-on="1"]')).toHaveText(MOCK_THREADS[0].title);
  await expect(page.locator(".chat-pane .bubble.user").last()).toContainText("在已有会话中继续分析");
  await expect(page.locator(".chat-pane .bubble.assistant").last()).toContainText("我会结合这些引用继续分析");
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.args.send_message.args.threadId)).toBe(MOCK_THREADS[0].id);
  await expect(page.getByRole("textbox", { name: "对话消息" })).toHaveValue("");
  await page.getByRole("button", { name: "今日焦点", exact: true }).click();
  await open(page);
  await expect.poll(() => reader.locator(".drawer-body").evaluate(el => el.scrollTop)).toBeGreaterThan(top - 100);
  await citeFull(page);
  await expect(tray.getByRole("button", { name: "发到 新对话" })).toBeVisible();
  await tray.getByRole("textbox").fill("新的引用问题");
  await tray.getByRole("button", { name: "发送", exact: true }).click();
  await expect(reader).not.toBeVisible();
  await expect(page.locator(".thread-list .thread-item:not(.new-thread)").first()).toHaveText("新的引用问题");
  await expect(page.locator(".thread-list .thread-item:not(.new-thread)").first()).toHaveAttribute("data-on", "1");
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.args.send_message.args.threadId)).toBe(0);
});

test("digest source defaults to the same period draft and opens its conversation after sending", async ({ page }) => {
  await setup(page);
  await page.getByRole("button", { name: "简报", exact: true }).click();
  await page.getByRole("textbox", { name: "追问当前简报" }).fill("本期已有草稿");
  await page.locator(".digest-ref").first().click();
  await citeFull(page);
  const tray = trayFor(page);
  await expect(tray.getByRole("textbox")).toHaveValue("本期已有草稿");
  await expect(tray.getByRole("button", { name: "发到 本期追问" })).toBeVisible();
  const menu = await targetMenu(page);
  await expect(menu.getByRole("menuitemradio", { name: "本期追问", exact: true })).toBeChecked();
  await page.keyboard.press("Escape");
  await tray.getByRole("button", { name: "发送", exact: true }).click();
  await expect(readerFor(page)).not.toBeVisible();
  await expect(page.locator(".chat-pane .bubble.user").last()).toContainText("本期已有草稿");
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.args.send_message.args)).toMatchObject({ threadId: 301, digestId: 101, text: "本期已有草稿" });
  await page.getByRole("button", { name: "简报", exact: true }).click();
  await expect(page.getByRole("textbox", { name: "追问当前简报" })).toHaveValue("");
  await expect(page.locator(".digest-discussion .bubble.user").last()).toContainText("本期已有草稿");
});

test("failed sends retain the draft, composition never sends and in-flight sends are exclusive", async ({ page }) => {
  await setup(page, { holdSend: true, sendFailures: 1 }); await open(page); await citeFull(page);
  const tray = trayFor(page), input = tray.getByRole("textbox");
  await input.fill("保留问题与引用");
  await input.dispatchEvent("keydown", { key: "Enter", isComposing: true });
  await input.dispatchEvent("keydown", { key: "Enter", keyCode: 229 });
  expect(await calls(page, "send_message")).toBe(0);
  await input.press("Enter");
  await expect.poll(() => calls(page, "send_message")).toBe(1);
  await input.dispatchEvent("keydown", { key: "Enter" });
  await input.dispatchEvent("keydown", { key: "Enter" });
  expect(await calls(page, "send_message")).toBe(1);
  await page.evaluate(() => (window as any).__mockHost.releaseSend());
  await expect(page.getByRole("status").filter({ hasText: "模拟发送失败" })).toBeVisible();
  await expect(input).toHaveValue("保留问题与引用");
  await expect(tray.locator(".reader-citation-chip")).toHaveCount(1);
  await input.press("Enter");
  await expect.poll(() => calls(page, "send_message")).toBe(2);
  await page.evaluate(() => (window as any).__mockHost.releaseSend());
  await expect(readerFor(page)).not.toBeVisible();
  await expect(page.locator(".chat-pane .bubble.user")).toHaveCount(1);
});

test("an accepted send after leaving the reader does not dismiss another overlay", async ({ page }) => {
  await setup(page, { holdSend: true }); await open(page); await citeFull(page);
  await trayFor(page).getByRole("textbox").fill("等待接受的提问");
  await trayFor(page).getByRole("textbox").press("Enter");
  await expect.poll(() => calls(page, "send_message")).toBe(1);
  await readerFor(page).getByRole("button", { name: "返回", exact: true }).click();
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await page.evaluate(() => (window as any).__mockHost.releaseSend());
  await expect(page.locator(".thread-list .thread-title").first()).toHaveText("等待接受的提问");
  await expect(page.locator(".settings-drawer")).toBeVisible();
  await expect(readerFor(page)).not.toBeVisible();
});

for (const theme of ["light", "dark"]) test(`citation tray review screenshots ${theme}`, async ({ page }) => {
  test.skip(process.env.RRSS_SHOTS !== "1", "仅在外观审阅时生成截图");
  await setup(page); await open(page);
  await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme);
  await citeSpan(page); await citeFull(page);
  const tray = trayFor(page), reader = readerFor(page);
  await tray.getByRole("textbox").fill("结合这两处引用解释一下");
  await reader.locator(".drawer-body").evaluate(el => { el.scrollTop = 0; });
  const dir = path.resolve("../../test-results/citation-tray/current");
  mkdirSync(dir, { recursive: true });
  const screenshot = async (name: string) => {
    await page.mouse.move(1279, 799);
    await page.screenshot({ path: path.join(dir, `${name}-1280-${theme}.png`), animations: "disabled" });
  };
  await screenshot("citation-tray");
  await targetMenu(page); await screenshot("send-to"); await page.keyboard.press("Escape");
  await tray.getByRole("button", { name: "清空" }).click();
  await screenshot("input-only");
  await citeFull(page); await citeSpan(page);
  await tray.getByRole("button", { name: "发送", exact: true }).click();
  await expect(reader).not.toBeVisible();
  await expect(page.locator(".chat-pane .bubble.assistant").last()).toBeVisible();
  await screenshot("chat-after-send");
});
