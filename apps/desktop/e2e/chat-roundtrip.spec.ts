import { mkdirSync } from "node:fs";
import path from "node:path";
import { expect, test, type Page } from "./fixtures";
import { installMockHost, MOCK_MESSAGES, MOCK_THREADS, type MockHostOptions } from "./mockHost";
import { readerReturnFixture } from "../src/test/fixtures/reader";

const fixture = readerReturnFixture(3);
const articleTitle = fixture.readers[1].title;
const question = "本地优先阅读如何保留上下文？";
const reader = (page: Page) => page.locator(".drawer.reader");
const body = (page: Page) => reader(page).locator(".drawer-body");
const tray = (page: Page) => page.getByRole("region", { name: "引用托盘" });
const backchip = (page: Page) => page.locator(".chat-backchip");
const block = (page: Page, id = 8) => body(page).locator(`[data-reader-document] > [data-block-id="${id}"]`);
async function setup(page: Page, options: MockHostOptions = {}) {
  await page.setViewportSize({ width: 1280, height: 800 });
  await installMockHost(page, { ...fixture, ...options });
  await page.goto("/");
}
async function scrollToBlock(page: Page, id = 8) {
  await body(page).evaluate((el, id) => {
    const target = el.querySelector(`[data-reader-document] > [data-block-id="${id}"]`)!;
    el.scrollTop += target.getBoundingClientRect().top - el.getBoundingClientRect().top - 120;
  }, id);
  await expect.poll(() => offset(page, id)).toBeCloseTo(120, 0);
}
const offset = (page: Page, id = 8) => block(page, id).evaluate(el => el.getBoundingClientRect().top - el.closest(".drawer-body")!.getBoundingClientRect().top);
async function citeFull(page: Page) {
  await reader(page).locator('summary[aria-label="更多"]').click();
  await reader(page).getByRole("button", { name: "引用全文到对话", exact: true }).click();
}
async function citeSpan(page: Page) {
  await block(page).evaluate(el => {
    const range = document.createRange(); range.selectNodeContents(el);
    const selection = window.getSelection()!; selection.removeAllRanges(); selection.addRange(range);
    el.dispatchEvent(new MouseEvent("mouseup", { bubbles: true, button: 0 }));
  });
  await page.getByRole("menuitem", { name: "引用", exact: true }).click();
}
async function send(page: Page, text = question) {
  await tray(page).getByRole("textbox").fill(text);
  await tray(page).getByRole("button", { name: "发送", exact: true }).click();
  await expect(reader(page)).not.toBeVisible();
  await expect(page.locator(".chat-pane .bubble.user").last()).toContainText(text);
  await expect(page.locator(".chat-pane .bubble.assistant").last()).toContainText("我会结合这些引用继续分析");
}
async function startRoundtrip(page: Page) {
  await page.getByRole("article", { name: articleTitle, exact: true }).click();
  await scrollToBlock(page);
  await citeSpan(page);
  await send(page);
  await expect(backchip(page)).toContainText(`回到原文 ·${articleTitle}`);
}
async function returnToArticle(page: Page) {
  await backchip(page).click();
  await expect(reader(page).locator(".reader-title")).toHaveText(articleTitle);
  await expect(reader(page).getByRole("button", { name: "对话", exact: true })).toBeVisible();
}

test("citation → tray → chat → original position, click/Escape roundtrip and draft preservation", async ({ page }) => {
  await setup(page); await startRoundtrip(page);
  const thread = await page.locator('.thread-item[data-on="1"]').textContent();
  await returnToArticle(page);
  // 允许一个块内的恢复误差；输入行 / 托盘高度变化不会把阅读位置带回顶部。
  expect(Math.abs(await offset(page) - 120)).toBeLessThan(180);
  const back = reader(page).getByRole("button", { name: "对话", exact: true });
  await back.hover();
  await expect(page.getByRole("tooltip")).toHaveText(`回到对话「${question}」 · Esc`);
  await page.keyboard.press("Escape");
  await expect(reader(page)).not.toBeVisible();
  await expect(page.locator('.thread-item[data-on="1"]')).toHaveText(thread!);
  const input = page.getByRole("textbox", { name: "对话消息" });
  await input.fill("未发出的草稿");
  await input.dispatchEvent("keydown", { key: "Escape", isComposing: true });
  await input.dispatchEvent("keydown", { key: "Escape", keyCode: 229 });
  await expect(reader(page)).not.toBeVisible();
  await input.press("Escape");
  await expect(reader(page)).toBeVisible();
  expect(Math.abs(await offset(page) - 120)).toBeLessThan(180);
  await page.keyboard.press("Backspace");
  await expect(reader(page)).not.toBeVisible();
  await expect(input).toHaveValue("未发出的草稿");
});

test("Q and first context action focus the thread tray, append answers and respect Escape priority", async ({ page }) => {
  await setup(page); await startRoundtrip(page); await returnToArticle(page);
  await page.keyboard.press("q");
  const input = tray(page).getByRole("textbox");
  await expect(input).toBeFocused();
  await expect(input).toHaveAttribute("placeholder", `继续问「${question}」…`);
  await expect(tray(page).locator(".reader-citation-row")).toHaveCount(0);
  await expect(tray(page).getByRole("button", { name: `发到 ${question}` })).toBeVisible();
  await input.press("q"); await expect(input).toHaveValue("q"); await input.fill("");
  await reader(page).getByRole("button", { name: "篇目", exact: true }).click();
  await tray(page).getByRole("button", { name: /^发到 / }).dispatchEvent("click");
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu", { name: "发到", exact: true })).not.toBeVisible();
  await expect(page.locator(".reader-article-drawer")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator(".reader-article-drawer")).not.toBeVisible();
  await expect(tray(page)).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(tray(page)).not.toBeVisible(); await expect(reader(page)).toBeVisible();
  await block(page).click({ button: "right" });
  const menu = page.getByRole("menu", { name: "本篇操作" });
  await expect(menu.getByRole("menuitem").first()).toContainText("在对话中继续提问");
  await page.keyboard.press("q"); await expect(tray(page)).not.toBeVisible();
  await menu.getByRole("menuitem", { name: /在对话中继续提问/ }).click();
  await expect(input).toBeFocused();
  await send(page, "补充一个问题");
  await expect(page.locator(".chat-pane .bubble.user")).toHaveCount(2);
  await expect(page.locator(".chat-pane .bubble.assistant")).toHaveCount(2);
  await expect(page.locator('.thread-item[data-on="1"]')).toHaveText(question);
  await returnToArticle(page); await citeFull(page);
  await reader(page).locator(".drawer-body").focus();
  await page.keyboard.press("q");
  await expect(input).toBeFocused();
  await expect(tray(page).locator(".citation-chip")).toHaveCount(1);
});

for (const method of ["j", "drawer", "next", "ending", "dislike", "search"] as const) test(`from-chat state expires on ${method} navigation without changing tray target`, async ({ page }) => {
  await setup(page); await startRoundtrip(page); await returnToArticle(page);
  await page.keyboard.press("q"); await tray(page).getByRole("textbox").fill("保留的输入");
  await page.keyboard.press("Escape"); // 仅收起输入行，草稿保留。
  if (method === "drawer") {
    await reader(page).getByRole("button", { name: "篇目", exact: true }).click();
    await page.locator('[data-reader-drawer-row="2"] .reader-article-drawer-main').click();
  } else if (method === "next") await reader(page).getByRole("button", { name: "下一篇", exact: true }).click();
  else if (method === "ending") await reader(page).locator(".reader-next").click();
  else if (method === "dislike") await reader(page).getByRole("button", { name: "踩并看下一篇" }).click();
  else if (method === "search") {
    await page.keyboard.press("Control+k");
    await page.getByRole("searchbox").fill(fixture.readers[2].title);
    await page.getByRole("option").first().click();
  } else await page.keyboard.press("j");
  await expect(reader(page).locator(".reader-title")).toHaveText(fixture.readers[2].title);
  // 返回目标不再是对话；从搜索换篇时返回回到搜索（N4）。
  await expect(reader(page).getByRole("button", { name: method === "search" ? "回到搜索" : "返回", exact: true })).toBeVisible();
  await page.keyboard.press("q"); await expect(tray(page)).not.toBeVisible();
  if (method === "j") {
    await page.keyboard.press("k");
    await expect(reader(page).locator(".reader-title")).toHaveText(articleTitle);
    await expect(reader(page).getByRole("button", { name: "返回", exact: true })).toBeVisible();
    await page.keyboard.press("q"); await expect(tray(page)).not.toBeVisible();
    await page.keyboard.press("j");
    await expect(reader(page).locator(".reader-title")).toHaveText(fixture.readers[2].title);
  }
  await block(page, 2).click({ button: "right" });
  await expect(page.getByRole("menuitem", { name: /在对话中继续提问/ })).toHaveCount(0);
  await page.keyboard.press("Escape");
  await citeFull(page);
  await expect(tray(page).getByRole("textbox")).toHaveValue("保留的输入");
  await expect(tray(page).getByRole("button", { name: `发到 ${question}` })).toBeVisible();
  await send(page, "换篇后的追加问题");
  await expect(backchip(page)).toContainText(fixture.readers[2].title);
  await expect(page.locator('.thread-item[data-on="1"]')).toHaveText(question);
  // 新一轮从对话回来后再换篇，切回也不会复活这轮的返回目标。
  await backchip(page).click();
  await expect(reader(page).locator(".reader-title")).toHaveText(fixture.readers[2].title);
  await page.keyboard.press("j");
  await expect(reader(page).locator(".reader-title")).toHaveText(fixture.readers[3].title);
  await page.keyboard.press("k");
  await expect(reader(page).locator(".reader-title")).toHaveText(fixture.readers[2].title);
  await expect(reader(page).getByRole("button", { name: "返回", exact: true })).toBeVisible();
  await page.keyboard.press("q"); await expect(tray(page)).not.toBeVisible();
});

function citedMessages() {
  const citation = { ...MOCK_MESSAGES[7][0].citations[0], article_id: 1, title: articleTitle, kind: "span", excerpt: "第九块摘录", block_id: 8 };
  return [{ ...MOCK_MESSAGES[7][0], citations: [citation, { ...citation, kind: "article", block_id: 8 }, { ...citation, block_id: 999 }, { ...citation, block_id: 0 }] }];
}
test("message chips locate and highlight spans; full, missing and invalid blocks restore the saved position", async ({ page }) => {
  const errors: string[] = []; page.on("pageerror", error => errors.push(error.message));
  await setup(page, { messages: { 7: citedMessages() } });
  await page.getByRole("button", { name: MOCK_THREADS[0].title, exact: true }).click();
  const chips = page.locator(".chat-pane .citation-open");
  await chips.first().click();
  await expect(block(page)).toHaveClass(/reader-block-highlight/);
  expect(await offset(page)).toBeCloseTo(120, 0);
  await expect(reader(page).getByRole("button", { name: "篇目", exact: true })).toHaveCount(0);
  await expect(block(page)).not.toHaveClass(/reader-block-highlight/, { timeout: 2500 });
  await scrollToBlock(page, 5); await page.keyboard.press("Escape");
  for (const index of [1, 2, 3]) {
    await chips.nth(index).click();
    await expect(reader(page)).toBeVisible();
    expect(Math.abs(await offset(page, 5) - 120)).toBeLessThan(180);
    await expect(reader(page).locator(".reader-block-highlight")).toHaveCount(0);
    await page.keyboard.press("Escape");
  }
  await page.emulateMedia({ reducedMotion: "reduce" });
  await chips.first().click();
  await expect(block(page)).toHaveClass(/reader-block-highlight/);
  expect(await block(page).evaluate(el => getComputedStyle(el).transitionDuration)).toBe("0s");
  expect(errors).toEqual([]);
});

test("digest followup chips use block anchors while retaining the digest tray source", async ({ page }) => {
  await setup(page, { messages: { 301: citedMessages() } });
  await page.getByRole("button", { name: "简报", exact: true }).click();
  await expect(page.locator(".digest-pane .chat-backchip")).toHaveCount(0);
  await page.locator(".digest-discussion .citation-open").first().click();
  await expect(block(page)).toHaveClass(/reader-block-highlight/);
  await expect(reader(page).getByRole("button", { name: "返回", exact: true })).toBeVisible();
  await page.keyboard.press("q"); await expect(tray(page)).not.toBeVisible();
  await citeFull(page);
  await expect(tray(page).getByRole("button", { name: "发到 本期追问" })).toBeVisible();
});

test("chat Escape without an origin does nothing; model menu wins and origins reset on reload", async ({ page }) => {
  await setup(page);
  await page.getByRole("button", { name: MOCK_THREADS[0].title, exact: true }).click();
  await page.getByRole("textbox", { name: "对话消息" }).press("Escape");
  await expect(reader(page)).not.toBeVisible(); await expect(backchip(page)).toHaveCount(0);
  await page.getByRole("button", { name: "今日焦点", exact: true }).click();
  await page.getByRole("article", { name: articleTitle, exact: true }).click();
  await citeFull(page);
  await tray(page).getByRole("button", { name: /^发到 / }).click();
  await page.getByRole("menuitemradio", { name: MOCK_THREADS[0].title, exact: true }).click();
  await send(page);
  await expect(backchip(page)).toBeVisible();
  await page.getByRole("button", { name: "这条对话的模型", exact: true }).click();
  const menu = page.getByRole("dialog", { name: "这条对话的模型", exact: true });
  await expect(menu).toBeVisible();
  await menu.getByRole("combobox").dispatchEvent("keydown", { key: "Escape", isComposing: true });
  await expect(menu).toBeVisible();
  await page.getByRole("textbox", { name: "对话消息" }).focus();
  await page.keyboard.press("Escape");
  await expect(menu).not.toBeVisible(); await expect(reader(page)).not.toBeVisible();
  await page.keyboard.press("Escape"); await expect(reader(page)).toBeVisible();
  await page.reload();
  await page.getByRole("button", { name: MOCK_THREADS[0].title, exact: true }).click();
  await expect(backchip(page)).toHaveCount(0);
});

// 与 visual spec 一样，这四条用例直接输出本功能的审阅图；可用 -g 单独重拍。
for (const theme of ["light", "dark"]) test(`roundtrip review screenshots ${theme}`, async ({ page }) => {
  test.skip(process.env.RRSS_SHOTS !== "1", "仅在外观审阅时生成截图");
  await setup(page);
  await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme);
  await page.evaluate(() => document.fonts.ready);
  await startRoundtrip(page);
  const dir = path.resolve("../../test-results/chat-roundtrip/current"); mkdirSync(dir, { recursive: true });
  const shot = (name: string) => page.screenshot({ path: path.join(dir, `${name}-1280-${theme}.png`), animations: "disabled" });
  await page.mouse.move(1279, 799); await shot("chat-backchip");
  await returnToArticle(page);
  await reader(page).getByRole("button", { name: "对话", exact: true }).hover();
  await expect(page.getByRole("tooltip")).toBeVisible(); await shot("reader-from-chat");
  await page.mouse.move(1279, 799); await page.keyboard.press("q");
  await expect(tray(page).getByRole("textbox")).toBeFocused(); await shot("reader-ask-tray");
  await page.keyboard.press("Escape"); await block(page).click({ button: "right" });
  await expect(page.getByRole("menuitem", { name: /在对话中继续提问/ })).toBeVisible(); await shot("reader-context-ask");
  await page.keyboard.press("Escape"); await page.keyboard.press("Escape");
  await page.locator(".chat-pane .citation-open").first().click();
  await expect(block(page)).toHaveClass(/reader-block-highlight/); await shot("block-highlight");
});
