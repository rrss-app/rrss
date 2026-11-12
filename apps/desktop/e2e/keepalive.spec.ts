import { expect, test, type Page } from "./fixtures";
import { installMockHost, MOCK_FOCUS } from "./mockHost";
import { shot } from "./shots";

// 阶段 17：三个主栏目到过才挂、切走只隐藏；真实布局下核对唯一可见栏、scrollTop 与切换前后的请求数。
const template = MOCK_FOCUS.articles.find((row) => row.row_kind === 0)!;
const longFocus = {
  ...MOCK_FOCUS,
  articles: Array.from({ length: 60 }, (_, i) => ({ ...template, id: 1000 + i, cluster_id: 0, title: `保活测试条目 ${i + 1}` })),
};

type Pane = { mounted: boolean; shown: boolean; top: number; room: number };

async function panes(page: Page) {
  return page.evaluate(() => {
    const read = (selector: string) => {
      const pane = document.querySelector<HTMLElement>(selector);
      const scroll = pane?.matches(".scroll") ? pane : pane?.querySelector<HTMLElement>(".scroll");
      return {
        mounted: !!pane,
        shown: !!pane && !pane.closest("[hidden]") && pane.getClientRects().length > 0,
        top: scroll?.scrollTop ?? 0,
        room: scroll ? scroll.scrollHeight - scroll.clientHeight : 0,
      };
    };
    return { focus: read(".focus"), digest: read(".digest"), chat: read(".chat-pane"), list: document.querySelector<HTMLElement>(".article-list")?.style.height ?? "" };
  }) as Promise<{ focus: Pane; digest: Pane; chat: Pane; list: string }>;
}

async function calls(page: Page) {
  const list = await page.evaluate(() => (window as unknown as { __mockHost: { calls: string[] } }).__mockHost.calls.slice());
  return (command: string) => list.filter((item) => item === command).length;
}

async function scrollTo(page: Page, selector: string, top: number) {
  await page.evaluate(([sel, value]) => {
    const el = document.querySelector<HTMLElement>(sel as string)!;
    el.scrollTop = value as number;
  }, [selector, top] as const);
  await expect.poll(async () => page.evaluate((sel) => document.querySelector<HTMLElement>(sel)!.scrollTop, selector)).toBe(top);
  // 让滚动事件与虚拟列表测量落定。
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

async function go(page: Page, name: string) {
  await page.getByRole("button", { name, exact: true }).click();
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

test("main views stay mounted once opened, keep scroll and do not refetch on switch", async ({ page }) => {
  await page.setViewportSize({ width: 1200, height: 520 });
  await installMockHost(page, { focus: longFocus });
  await page.goto("/");
  await expect(page.getByText("保活测试条目 1", { exact: true })).toBeVisible();
  // 设置预载（约 800 毫秒）之后，仍没有挂上或请求简报 / 对话。
  await page.waitForTimeout(1000);
  let count = await calls(page);
  for (const command of ["digest_list", "digest", "digest_thread", "messages"]) expect(count(command), command).toBe(0);
  let state = await panes(page);
  expect(state.digest.mounted).toBe(false);
  expect(state.chat.mounted).toBe(false);
  await expect(page.locator(".composer")).toHaveCount(0);

  await scrollTo(page, ".focus .scroll", 1200);
  const focusReads = count("focus_stream");
  const listHeight = (await panes(page)).list;

  await go(page, "简报");
  await expect(page.locator(".digest-doc h3").first()).toBeVisible();
  state = await panes(page);
  expect([state.focus.shown, state.digest.shown, state.chat.mounted]).toEqual([false, true, false]);
  // 隐藏的焦点列表不因 0 高度改写行尺寸，也不续页。
  expect(state.list).toBe(listHeight);
  count = await calls(page);
  expect(count("focus_stream")).toBe(focusReads);
  const digestReads = { list: count("digest_list"), body: count("digest"), thread: count("digest_thread") };
  // 开发模式 StrictMode 会让首个期列表请求重复一次；正文与追问各一次。
  expect(digestReads.list).toBeGreaterThan(0);
  expect([digestReads.body, digestReads.thread]).toEqual([1, 1]);
  expect(state.digest.room).toBeGreaterThan(40);
  await scrollTo(page, ".digest .scroll", 40);

  await go(page, "今日焦点");
  await expect(page.getByRole("button", { name: "全部已读" })).toBeVisible();
  state = await panes(page);
  expect([state.focus.shown, state.digest.shown]).toEqual([true, false]);
  expect(state.focus.top).toBe(1200);
  expect(state.list).toBe(listHeight);
  // 虚拟列表按恢复后的位置渲染，视口里有卡片而不是空白。
  expect(await page.evaluate(() => {
    const scroll = document.querySelector<HTMLElement>(".focus .scroll")!.getBoundingClientRect();
    return [...document.querySelectorAll(".focus .article-list > div")].some((row) => {
      const rect = row.getBoundingClientRect();
      return rect.bottom > scroll.top + 8 && rect.top < scroll.bottom - 8;
    });
  })).toBe(true);
  await shot(page, "test-results/keepalive-focus-return.png");
  count = await calls(page);
  expect(count("focus_stream")).toBe(focusReads);

  await go(page, "简报");
  state = await panes(page);
  expect([state.focus.shown, state.digest.shown]).toEqual([false, true]);
  expect(state.digest.top).toBe(40);
  count = await calls(page);
  // 回到已打开的简报只允许静默重读期列表。
  expect(count("digest")).toBe(digestReads.body);
  expect(count("digest_thread")).toBe(digestReads.thread);
  expect(count("digest_list")).toBe(digestReads.list + 1);
  await shot(page, "test-results/keepalive-digest-return.png");

  // 模拟会话较短，压低视口让消息区可滚动。
  await page.setViewportSize({ width: 1200, height: 320 });
  await go(page, "深度对话");
  await page.getByRole("button", { name: "查证 Rust 1.97 的回归修复" }).click();
  await expect(page.getByText("借用检查回归")).toBeVisible();
  count = await calls(page);
  const messageReads = count("messages");
  expect(messageReads).toBe(1);
  state = await panes(page);
  expect([state.focus.shown, state.digest.shown, state.chat.shown]).toEqual([false, false, true]);
  const chatTop = Math.min(30, state.chat.room);
  expect(chatTop).toBeGreaterThan(0);
  await scrollTo(page, ".chat-pane", chatTop);

  await go(page, "简报");
  state = await panes(page);
  expect([state.digest.shown, state.chat.shown, state.chat.mounted]).toEqual([true, false, true]);
  expect(state.digest.top).toBe(40);
  await go(page, "深度对话");
  await expect(page.getByText("借用检查回归")).toBeVisible();
  state = await panes(page);
  expect([state.focus.shown, state.digest.shown, state.chat.shown]).toEqual([false, false, true]);
  expect(state.chat.top).toBe(chatTop);
  count = await calls(page);
  expect(count("messages")).toBe(messageReads);
  expect(count("focus_stream")).toBe(focusReads);
});

test("background updates keep the focus scroll position and offer new articles at the top", async ({ page }) => {
  await page.setViewportSize({ width: 1200, height: 520 });
  await installMockHost(page, { focus: longFocus });
  await page.goto("/");
  await expect(page.getByText("保活测试条目 1", { exact: true })).toBeVisible();
  await scrollTo(page, ".focus .scroll", 1200);
  // 阶段 17 · 后台更新不动视口：抓到一篇新文章，排在第一，但不插进正在看的列表。
  await page.evaluate((row) => {
    const host = (window as any).__mockHost;
    host.state.focus.articles.unshift(row);
    host.emit("feed:updated", { run_id: "bg", feed_id: row.feed_id, generation: 1, status: "ok", inserted: 1, error: null, derived_degraded: false });
  }, { ...template, id: 999, cluster_id: 0, title: "后台抓到的新文章" });
  const pill = page.getByRole("button", { name: "有 1 篇新内容", exact: true });
  await expect(pill).toBeVisible();
  expect((await panes(page)).focus.top).toBe(1200);
  await pill.click();
  await expect(page.getByText("后台抓到的新文章", { exact: true })).toBeVisible();
  expect((await panes(page)).focus.top).toBe(0);
  await expect(pill).toHaveCount(0);
});
