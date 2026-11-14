import type { Page } from "@playwright/test";
import type { ArticleView, FeedTree, FeedView } from "../src/lib/api";
import { expect, test } from "./fixtures";
import { installMockHost, MOCK_FEEDS, MOCK_FOCUS } from "./mockHost";

// 阶段 23 S1 订阅模式：侧栏订阅树 + 按时间的时间线（推荐系统规格 S1）。
const lastArgs = (page: Page, command: string) => page.evaluate((name) => (window as any).__mockHost.args[name], command);

const baseFeed = MOCK_FEEDS.folders[1].feeds[0];
const feed = (id: number, title: string, folder_id: number, extra: Partial<FeedView> = {}): FeedView =>
  ({ ...baseFeed, id, title, folder_id, url: `https://feeds.example/${id}`, ...extra });

// 技术 ⊃ 前端（子文件夹），新闻；未归类一个源。
const TREE: FeedTree = {
  folders: [
    { id: 10, name: "技术", label: "技术", indent: 0, position: 0, feeds: [feed(11, "Compiler Notes", 10)] },
    { id: 12, name: "前端", label: "技术 / 前端", indent: 1, position: 0, feeds: [feed(13, "CSS Weekly", 12)] },
    { id: 20, name: "新闻", label: "新闻", indent: 0, position: 1, feeds: [feed(21, "晚报", 20, { error: { code: "feed_refresh_http", params: { status: 503 }, detail: "http status 503" } })] },
  ],
  uncategorized: [feed(30, "随笔", 0)],
  folder_combo: ["技术", "技术 / 前端", "新闻"],
};

const baseRow = MOCK_FOCUS.articles.find((row) => row.row_kind === 0)!;
const row = (id: number, title: string, feed_id: number, feed_title: string, published_at: number): ArticleView =>
  ({ ...baseRow, id, title, feed_id, feed_title, published_at, read: false, starred: false, summary: `${title}的摘要`, url: `https://example.test/${id}` });

function timeline() {
  const now = Date.now();
  const today = new Date(); today.setHours(0, 0, 0, 0);
  return [
    row(101, "CSS 嵌套正式可用", 13, "CSS Weekly", now - 30_000),
    row(102, "Rust 1.99 发布", 11, "Compiler Notes", now - 60_000),
    row(103, "城市新闻早报", 21, "晚报", today.getTime() - 3 * 3_600_000),
    row(104, "一篇旧随笔", 30, "随笔", today.getTime() - 50 * 3_600_000),
  ];
}

const sidebarTree = (page: Page) => page.getByRole("tree", { name: "订阅" });
const treeRow = (page: Page, key: string) => page.locator(`.feeds-row[data-key="${key}"]`);
const titles = (page: Page) => page.locator(".feeds-item-title").allTextContents();

test("entering, picking a folder with its subfolder and a single feed, and leaving with the back button and Esc", async ({ page }) => {
  await installMockHost(page, { feeds: TREE, timelineArticles: timeline() });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  await page.locator(".sidebar").getByRole("button", { name: /^订阅/ }).click();

  await expect(sidebarTree(page)).toBeVisible();
  await expect(page.getByRole("heading", { level: 1, name: /全部订阅/ })).toBeVisible();
  await expect.poll(() => titles(page)).toEqual(["CSS 嵌套正式可用", "Rust 1.99 发布", "城市新闻早报", "一篇旧随笔"]);
  await expect(page.locator(".feeds-list .feeds-day").first()).toHaveText("今天");
  await expect(page.locator(".feeds-list .feeds-day").nth(1)).toHaveText("昨天");
  // 文件夹默认收起；抓取失败的源名后有警示点。
  await expect(treeRow(page, "feed:21")).toHaveCount(0);
  await treeRow(page, "folder:20").locator(".feeds-twist").click();
  await expect(treeRow(page, "feed:21").locator(".feeds-row-flag")).toHaveAttribute("title", "源暂时不可用（503）");

  // 文件夹含子文件夹的源；选中写进注册表。
  await treeRow(page, "folder:10").click();
  await expect.poll(() => titles(page)).toEqual(["CSS 嵌套正式可用", "Rust 1.99 发布"]);
  expect((await lastArgs(page, "feeds_page")).scope).toBe("folder:10");
  expect(await lastArgs(page, "set_setting")).toMatchObject({ id: "feeds_scope", value: "folder:10" });
  await expect(treeRow(page, "folder:10")).toHaveAttribute("aria-selected", "true");

  // 单个源：只列它，行里不再重复来源名。
  await treeRow(page, "folder:10").locator(".feeds-twist").click();
  await expect(treeRow(page, "folder:10")).toHaveAttribute("aria-expanded", "true");
  await treeRow(page, "feed:11").click();
  await expect.poll(() => titles(page)).toEqual(["Rust 1.99 发布"]);
  await expect(page.locator(".feeds-item-source")).toHaveCount(0);
  await expect(page.locator(".feeds-path")).toHaveText("订阅 · 技术");

  await page.getByRole("button", { name: "今日焦点" }).click();
  await expect(sidebarTree(page)).toHaveCount(0);
  await expect(page.getByRole("article").first()).toBeVisible();

  // 再进来保留范围；焦点不在输入框时 Esc 回到焦点。
  await page.locator(".sidebar").getByRole("button", { name: /^订阅/ }).click();
  await expect(treeRow(page, "feed:11")).toHaveAttribute("aria-selected", "true");
  await treeRow(page, "feed:11").focus();
  await page.keyboard.press("Escape");
  await expect(sidebarTree(page)).toHaveCount(0);
});

test("leaving subscriptions returns to the view that opened them", async ({ page }) => {
  await installMockHost(page, { feeds: TREE, timelineArticles: timeline() });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  // 从简报进入：返回按钮用简报的名称，点它回到简报。
  await page.getByRole("button", { name: "简报", exact: true }).click();
  await expect(page.getByRole("heading", { level: 3, name: "今日三条主线" })).toBeVisible();
  await page.locator(".sidebar").getByRole("button", { name: /^订阅/ }).click();
  const back = page.locator(".feeds-back");
  await expect(back).toHaveText("简报");
  await expect(back).toHaveAttribute("title", "回到简报（Esc）");
  await back.click();
  await expect(sidebarTree(page)).toHaveCount(0);
  await expect(page.getByRole("heading", { level: 3, name: "今日三条主线" })).toBeVisible();

  // 再进订阅，焦点不在输入框时 Esc 也回到简报。
  await page.locator(".sidebar").getByRole("button", { name: /^订阅/ }).click();
  await expect(back).toHaveText("简报");
  await treeRow(page, "all").focus();
  await page.keyboard.press("Escape");
  await expect(sidebarTree(page)).toHaveCount(0);
  await expect(page.getByRole("heading", { level: 3, name: "今日三条主线" })).toBeVisible();
});

test("tree keyboard follows the tree view pattern, and the name filter keeps matching subtrees", async ({ page }) => {
  const many = Array.from({ length: 22 }, (_, i) => feed(200 + i, `杂项源 ${i + 1}`, 0));
  await installMockHost(page, { feeds: { ...TREE, uncategorized: [...TREE.uncategorized, ...many] }, timelineArticles: timeline() });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  await page.locator(".sidebar").getByRole("button", { name: /^订阅/ }).click();
  const focused = () => page.evaluate(() => (document.activeElement as HTMLElement | null)?.dataset.key ?? document.activeElement?.tagName);

  await treeRow(page, "all").focus();
  await page.keyboard.press("ArrowDown");
  expect(await focused()).toBe("folder:10");
  await page.keyboard.press("ArrowRight");
  await expect(treeRow(page, "folder:10")).toHaveAttribute("aria-expanded", "true");
  await page.keyboard.press("ArrowRight");
  expect(await focused()).toBe("folder:12");
  await page.keyboard.press("ArrowLeft");
  expect(await focused()).toBe("folder:10");
  await page.keyboard.press("ArrowLeft");
  await expect(treeRow(page, "folder:10")).toHaveAttribute("aria-expanded", "false");
  await page.keyboard.press("End");
  // 根层按 position 混排：这些源都是 0，新闻文件夹是 1，所以它排在最后。
  expect(await focused()).toBe("folder:20");
  await page.keyboard.press("Home");
  expect(await focused()).toBe("all");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(treeRow(page, "folder:10")).toHaveAttribute("aria-selected", "true");

  // 筛选框常驻；命中源时保留它所在的文件夹链并展开。
  const filter = page.getByRole("textbox", { name: "按名称筛选订阅源" });
  await filter.fill("CSS");
  await expect(page.locator(".feeds-row")).toHaveCount(3);
  await expect(treeRow(page, "all")).toHaveCount(0);
  await expect(treeRow(page, "feed:13").locator("mark")).toHaveText("CSS");
  await page.keyboard.press("ArrowDown");
  expect(await focused()).toBe("folder:10");
  // 命中文件夹名时保留整个子树。
  await filter.fill("技术");
  await expect(treeRow(page, "feed:11")).toBeVisible();
  await expect(treeRow(page, "feed:13")).toBeVisible();
  // 筛选框里 Esc 先清空，不离开订阅模式。
  await filter.press("Escape");
  await expect(filter).toHaveValue("");
  await expect(sidebarTree(page)).toBeVisible();
});

test("reader J/K follow the timeline, and the trailing entry opens the hidden list for this scope", async ({ page }) => {
  const now = Date.now();
  await installMockHost(page, {
    feeds: TREE, timelineArticles: timeline(), feedsHiddenCount: 1,
    hidden: { articles: [{ id: 901, title: "被规则藏起来的一篇", feed_id: 11, feed_title: "Compiler Notes", published_at: now - 3_600_000, score: -20, spot_liked: false, reasons: [] }],
      rule_counts: [], spot_liked_count: 0 },
  });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  await page.locator(".sidebar").getByRole("button", { name: /^订阅/ }).click();
  await page.locator(".feeds-item", { hasText: "CSS 嵌套正式可用" }).click();
  await expect.poll(async () => (await lastArgs(page, "open_article"))?.id).toBe(101);
  await expect(page.locator(".feeds-item", { hasText: "CSS 嵌套正式可用" })).toHaveAttribute("data-read", "1");
  await page.keyboard.press("j");
  await expect.poll(async () => (await lastArgs(page, "open_article"))?.id).toBe(102);
  await page.keyboard.press("j");
  await expect.poll(async () => (await lastArgs(page, "open_article"))?.id).toBe(103);
  await page.keyboard.press("k");
  await expect.poll(async () => (await lastArgs(page, "open_article"))?.id).toBe(102);
  await page.keyboard.press("Escape");

  await page.getByRole("button", { name: "另有 1 篇被规则隐藏" }).click();
  const dialog = page.getByRole("dialog", { name: "被隐藏的文章" });
  await expect(dialog.getByText("被规则藏起来的一篇")).toBeVisible();
  expect(await lastArgs(page, "hidden_page")).toMatchObject({ scope: "all", listScope: "all" });
});

test("the saved scope and list survive a restart, and the tree opens down to the selection", async ({ page }) => {
  await installMockHost(page, { feeds: TREE, timelineArticles: timeline(), settingValues: { feeds_scope: "folder:12", feeds_list_scope: "unread" } });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  await page.locator(".sidebar").getByRole("button", { name: /^订阅/ }).click();
  // 进入时展开到选中项。
  await expect(treeRow(page, "folder:12")).toHaveAttribute("aria-selected", "true");
  await expect(treeRow(page, "folder:10")).toHaveAttribute("aria-expanded", "true");
  await expect(page.locator(".feeds-view .toolbar").getByRole("button", { name: "未读", exact: true })).toHaveAttribute("data-on", "1");
  await expect.poll(() => titles(page)).toEqual(["CSS 嵌套正式可用"]);
  await expect(page.locator(".feeds-path")).toHaveText("订阅 · 技术");
});

test("narrow windows keep the tree and the timeline usable", async ({ page }) => {
  await installMockHost(page, { feeds: TREE, timelineArticles: timeline() });
  await page.setViewportSize({ width: 560, height: 800 });
  await page.goto("/");
  await page.locator(".sidebar").getByRole("button", { name: /^订阅/ }).click();
  await treeRow(page, "folder:20").click();
  await expect.poll(() => titles(page)).toEqual(["城市新闻早报"]);
  const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
  expect(overflow).toBeLessThanOrEqual(0);
  await expect(page.locator(".feeds-item").first()).toBeInViewport();
});

test("a focus card's source name opens subscriptions on that feed, with its folder expanded", async ({ page }) => {
  await installMockHost(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  const card = page.getByRole("article").filter({ has: page.locator("button.article-source", { hasText: "Compiler Notes" }) }).first();
  await card.locator("button.article-source").click();
  await expect(sidebarTree(page)).toBeVisible();
  // 只进订阅模式，不打开阅读器。
  await expect(page.locator(".drawer.reader")).toHaveCount(0);
  await expect(treeRow(page, "feed:2")).toHaveAttribute("aria-selected", "true");
  await expect(treeRow(page, "folder:2")).toHaveAttribute("aria-expanded", "true");
  await expect(page.getByRole("heading", { level: 1, name: /Compiler Notes/ })).toBeVisible();
  expect((await lastArgs(page, "feeds_page")).scope).toBe("feed:2");
  await expect(page.locator(".feeds-item").first()).toBeVisible();
});
