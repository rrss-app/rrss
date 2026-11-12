import type { Page } from "@playwright/test";
import type { FeedTree, FeedView } from "../src/lib/api";
import { expect, test } from "./fixtures";
import { installMockHost, MOCK_FEEDS, MOCK_FOCUS } from "./mockHost";

// 阶段 23 S2 订阅栏：混排、拖动、行菜单与撤销（推荐系统规格 S2）。
const lastArgs = (page: Page, command: string) => page.evaluate((name) => (window as any).__mockHost.args[name], command);
const baseFeed = MOCK_FEEDS.folders[1].feeds[0];
const feed = (id: number, title: string, folder_id: number, position: number): FeedView =>
  ({ ...baseFeed, id, title, folder_id, position, url: `https://feeds.example/${id}`, error: "" });

// 根层：随笔 0、技术 1、新闻 2。技术里有两个源。
const TREE: FeedTree = {
  folders: [
    { id: 10, name: "技术", label: "技术", indent: 0, position: 1, feeds: [feed(11, "Compiler Notes", 10, 0), feed(12, "CSS Weekly", 10, 1)] },
    { id: 20, name: "新闻", label: "新闻", indent: 0, position: 2, feeds: [feed(21, "晚报", 20, 0)] },
  ],
  uncategorized: [feed(30, "随笔", 0, 0)],
  folder_combo: ["技术", "新闻"],
};

const baseRow = MOCK_FOCUS.articles.find((row) => row.row_kind === 0)!;
const row = (id: number, title: string, feed_id: number, feed_title: string) =>
  ({ ...baseRow, id, title, feed_id, feed_title, published_at: Date.now(), read: false, starred: false, summary: "", url: `https://example.test/${id}` });

const keys = (page: Page) => page.locator(".feeds-row").evaluateAll((rows) => rows.map((row) => (row as HTMLElement).dataset.key));
const treeRow = (page: Page, key: string) => page.locator(`.feeds-row[data-key="${key}"]`);

async function openFeeds(page: Page) {
  await installMockHost(page, {
    feeds: TREE,
    timelineArticles: [row(1, "一篇", 11, "Compiler Notes"), row(2, "二篇", 30, "随笔"), row(3, "三篇", 21, "晚报")],
  });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  await page.locator(".sidebar").getByRole("button", { name: /^订阅/ }).click();
  await expect.poll(() => keys(page)).toEqual(["all", "feed:30", "folder:10", "folder:20"]);
}

async function drag(page: Page, from: string, to: string, zone: "before" | "into") {
  const source = await treeRow(page, from).boundingBox();
  const target = await treeRow(page, to).boundingBox();
  if (!source || !target) throw new Error("missing row box");
  const startX = source.x + 48;
  const startY = source.y + source.height / 2;
  await page.mouse.move(startX, startY);
  await page.mouse.down();
  await page.mouse.move(startX, startY + 8);
  const y = zone === "before" ? target.y + 2 : target.y + target.height / 2;
  await page.mouse.move(target.x + 48, y, { steps: 8 });
  await page.mouse.up();
}

test("dragging a feed to the front of the root does not select it", async ({ page }) => {
  await openFeeds(page);
  await treeRow(page, "folder:10").locator(".feeds-twist").click();
  await drag(page, "feed:11", "feed:30", "before");
  await expect(page.locator(".toast")).toContainText("已把「Compiler Notes」移到根层");
  await expect.poll(() => keys(page)).toEqual(["all", "feed:11", "feed:30", "folder:10", "feed:12", "folder:20"]);
  await expect(treeRow(page, "all")).toHaveAttribute("aria-selected", "true");
  expect(await lastArgs(page, "move_tree_items")).toMatchObject({ items: ["feed:11"], parentFolderId: null, before: "feed:30" });
});

test("dragging a feed onto a folder moves it inside and expands that folder", async ({ page }) => {
  await openFeeds(page);
  await drag(page, "feed:30", "folder:20", "into");
  await expect(page.locator(".toast")).toContainText("已把「随笔」移到「新闻」");
  await expect(treeRow(page, "folder:20")).toHaveAttribute("aria-expanded", "true");
  await expect.poll(() => keys(page)).toEqual(["all", "folder:10", "folder:20", "feed:21", "feed:30"]);
  expect(await lastArgs(page, "move_tree_items")).toMatchObject({ items: ["feed:30"], parentFolderId: 20, before: null });
});

test("Alt+ArrowUp moves a row before its previous sibling", async ({ page }) => {
  await openFeeds(page);
  await treeRow(page, "folder:10").focus();
  await page.keyboard.press("Alt+ArrowUp");
  await expect(page.locator(".toast")).toContainText("已把「技术」移到根层");
  await expect.poll(() => keys(page)).toEqual(["all", "folder:10", "feed:30", "folder:20"]);
  expect(await lastArgs(page, "move_tree_items")).toMatchObject({ items: ["folder:10"], parentFolderId: null, before: "feed:30" });
});

test("the row menu moves a feed into a folder", async ({ page }) => {
  await openFeeds(page);
  await treeRow(page, "feed:30").hover();
  await treeRow(page, "feed:30").getByRole("button", { name: "随笔 的操作" }).click();
  await page.getByRole("menuitem", { name: "移到文件夹…" }).click();
  await page.getByRole("menuitem", { name: "技术" }).click();
  await expect(page.locator(".toast")).toContainText("已把「随笔」移到「技术」");
  await expect.poll(() => keys(page)).toEqual(["all", "folder:10", "feed:11", "feed:12", "feed:30", "folder:20"]);
  expect(await lastArgs(page, "move_tree_items")).toMatchObject({ items: ["feed:30"], parentFolderId: 10, before: null });
});

test("undo restores the tree order from before the move", async ({ page }) => {
  await openFeeds(page);
  await treeRow(page, "folder:20").hover();
  await treeRow(page, "folder:20").getByRole("button", { name: "新闻 的操作" }).click();
  await page.getByRole("menuitem", { name: "移到最上面" }).click();
  await expect.poll(() => keys(page)).toEqual(["all", "folder:20", "feed:30", "folder:10"]);
  await page.locator(".toast-action", { hasText: "撤销" }).click();
  await expect.poll(() => keys(page)).toEqual(["all", "feed:30", "folder:10", "folder:20"]);
  const layout = await lastArgs(page, "restore_tree_layout");
  expect(layout.layout).toEqual(expect.arrayContaining([{ item: "folder:20", parent_folder_id: null, position: 2 }]));
});

test("creating a folder, putting two feeds in it, and undoing", async ({ page }) => {
  await openFeeds(page);
  await page.getByRole("button", { name: "新建", exact: true }).click();
  await page.getByRole("menuitem", { name: "新建文件夹" }).click();
  const name = page.getByRole("textbox", { name: "新文件夹名称" });
  await name.fill("新夹");
  await name.press("Enter");
  const picker = page.getByRole("dialog", { name: /把订阅放进新夹/ });
  await expect(picker).toBeVisible();
  await picker.getByRole("checkbox", { name: "Compiler Notes" }).click();
  await picker.getByRole("checkbox", { name: "CSS Weekly" }).click();
  await picker.getByRole("button", { name: "放进 2 个" }).click();
  await expect(page.locator(".toast")).toContainText("已把 2 项移到「新夹」");
  await expect(treeRow(page, "feed:11")).toBeVisible();
  await page.locator(".toast-action", { hasText: "撤销" }).click();
  await expect(treeRow(page, "feed:11")).toHaveCount(0);
  await treeRow(page, "folder:10").locator(".feeds-twist").click();
  await expect(treeRow(page, "feed:11")).toBeVisible();
});

test("moving three selected feeds from the multi-select bar", async ({ page }) => {
  await openFeeds(page);
  await treeRow(page, "folder:10").locator(".feeds-twist").click();
  await page.getByRole("button", { name: "整理" }).click();
  await treeRow(page, "feed:30").click();
  await treeRow(page, "feed:11").click();
  await treeRow(page, "feed:12").click();
  await page.getByRole("button", { name: "移到…" }).click();
  await page.getByRole("menuitem", { name: "新闻" }).click();
  await expect.poll(() => lastArgs(page, "move_tree_items")).toMatchObject({ items: ["feed:30", "feed:11", "feed:12"], parentFolderId: 20, before: null });
  await expect(page.getByRole("button", { name: "整理" })).toBeVisible();
});

test("shift-click checks the visible rows in between", async ({ page }) => {
  await openFeeds(page);
  await page.getByRole("button", { name: "整理" }).click();
  await treeRow(page, "feed:30").click();
  await treeRow(page, "folder:20").click({ modifiers: ["Shift"] });
  await expect(treeRow(page, "feed:30")).toHaveAttribute("data-picked", "1");
  await expect(treeRow(page, "folder:10")).toHaveAttribute("data-picked", "1");
  await expect(treeRow(page, "folder:20")).toHaveAttribute("data-picked", "1");
  await expect(treeRow(page, "all")).not.toHaveAttribute("data-picked", "1");
});

test("pinning a folder and reordering it inside the pin group", async ({ page }) => {
  await openFeeds(page);
  await treeRow(page, "folder:20").hover();
  await treeRow(page, "folder:20").getByRole("button", { name: "新闻 的操作" }).click();
  await page.getByRole("menuitem", { name: "置顶", exact: true }).click();
  await treeRow(page, "feed:30").hover();
  await treeRow(page, "feed:30").getByRole("button", { name: "随笔 的操作" }).click();
  await page.getByRole("menuitem", { name: "置顶", exact: true }).click();
  await expect(page.locator(".feeds-sec").first()).toHaveText("置顶");
  const source = await page.locator(".feeds-row[data-key='pin:folder:20']").boundingBox();
  const target = await page.locator(".feeds-row[data-key='pin:feed:30']").boundingBox();
  if (!source || !target) throw new Error("missing pin row");
  await page.mouse.move(source.x + 40, source.y + source.height / 2);
  await page.mouse.down();
  await page.mouse.move(source.x + 40, source.y + source.height / 2 + 8);
  await page.mouse.move(target.x + 40, target.y + target.height - 2, { steps: 8 });
  await page.mouse.up();
  await expect.poll(() => lastArgs(page, "set_setting")).toMatchObject({ id: "feeds_pinned", value: "feed:30,folder:20" });
});

test("marking a folder as a focus column writes the setting", async ({ page }) => {
  await openFeeds(page);
  await treeRow(page, "folder:10").hover();
  await treeRow(page, "folder:10").getByRole("button", { name: "技术 的操作" }).click();
  await page.getByRole("menuitem", { name: "设为焦点栏目" }).click();
  await expect.poll(() => lastArgs(page, "set_setting")).toMatchObject({ id: "focus_columns", value: "folder:10" });
});

test("the place-feeds panel fills the window at 560px", async ({ page }) => {
  await openFeeds(page);
  await page.setViewportSize({ width: 560, height: 800 });
  await treeRow(page, "folder:10").hover();
  await treeRow(page, "folder:10").getByRole("button", { name: "技术 的操作" }).click();
  await page.getByRole("menuitem", { name: "添加源…" }).click();
  const picker = page.locator(".feed-picker");
  await expect(picker).toBeVisible();
  const box = await picker.boundingBox();
  expect(box && box.x).toBeLessThan(16);
  expect(box && box.x + box.width).toBeGreaterThan(540);
});
