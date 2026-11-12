import { expect, test, type Page } from "./fixtures";
import { installMockHost, MOCK_FOCUS, type MockHostOptions } from "./mockHost";

const article = MOCK_FOCUS.articles.find((row) => row.row_kind === 0)!;

function longFocus() {
  const copy = (id: number, feed: number, title: string) => ({ ...article, id, feed_id: feed, cluster_id: 0, title, read: false });
  return {
    ...MOCK_FOCUS,
    articles: [
      ...Array.from({ length: 30 }, (_, i) => copy(2000 + i, 2, `栏目长文 ${i + 1}`)),
      ...Array.from({ length: 30 }, (_, i) => copy(3000 + i, 1, `主焦点长文 ${i + 1}`)),
    ],
  };
}

async function boot(page: Page, options: MockHostOptions = {}) {
  await installMockHost(page, options);
  await page.goto("/");
  await expect(page.getByRole("button", { name: "添加栏目" })).toBeVisible();
}

async function addColumn(page: Page, name: string) {
  await page.getByRole("button", { name: "添加栏目" }).click();
  await page.getByRole("menuitem", { name: name, exact: true }).click();
  await expect(page.locator(".nav-column-name", { hasText: name })).toBeVisible();
}

async function openColumn(page: Page, name: string) {
  await page.locator(".nav-column", { hasText: name }).getByRole("button", { name: new RegExp(name) }).first().click();
  await expect(page.locator(".focus:not([hidden]) .focus-kicker")).toHaveText("今日焦点 · 栏目");
}

async function columnSetting(page: Page) {
  return page.evaluate(() => {
    const values = (window as unknown as { __mockHost: { state: { settings: { values: { id: string; value: string }[] } } } }).__mockHost.state.settings.values;
    return values.find((item) => item.id === "focus_columns")?.value ?? "";
  });
}

async function visibleScroll(page: Page) {
  return page.evaluate(() => {
    const pane = [...document.querySelectorAll<HTMLElement>(".focus")].find((el) => !el.hidden);
    return pane?.querySelector<HTMLElement>(".scroll")?.scrollTop ?? -1;
  });
}

async function scrollVisible(page: Page, fraction: number) {
  await page.evaluate((ratio) => {
    const pane = [...document.querySelectorAll<HTMLElement>(".focus")].find((el) => !el.hidden);
    const scroll = pane?.querySelector<HTMLElement>(".scroll");
    if (!scroll) throw new Error("no visible focus scroll");
    const room = scroll.scrollHeight - scroll.clientHeight;
    scroll.scrollTop = Math.max(120, Math.round(room * ratio));
    scroll.dispatchEvent(new Event("scroll"));
  }, fraction);
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  const top = await visibleScroll(page);
  expect(top).toBeGreaterThan(80);
  await expect.poll(() => visibleScroll(page)).toBe(top);
  return top;
}

test("adding a column opens only that folder", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await boot(page);
  await addColumn(page, "English");
  await expect.poll(() => columnSetting(page)).toBe("folder:2");
  await openColumn(page, "English");
  const visible = page.locator(".focus:not([hidden])");
  await expect(visible.locator(".focus-masthead h1")).toContainText("English");
  // 刊头的篇数与侧栏栏目行、订阅树是同一个可见未读数。
  const unread = await page.locator(".nav-column", { hasText: "English" }).locator(".nav-count").textContent();
  await expect(visible.locator(".focus-masthead h1 small")).toHaveText(`${unread} 篇未读 · 只看「English」文件夹`);
  await expect(visible.getByRole("article", { name: "sqlite-vec 在桌面端单库里的取舍：WAL、FTS5 与向量检索如何共存" })).toBeVisible();
  await expect(visible.getByRole("article", { name: "我用旧平板做了一块桌面信息屏" })).toHaveCount(0);
  await expect(visible.getByRole("article", { name: "React 19 编译器在中大型应用里的实际收益" })).toHaveCount(0);
  await expect(visible.getByRole("button", { name: /今日简报/ })).toHaveCount(0);
  expect(await page.evaluate(() => (window as unknown as { __mockHost: { args: { focus_stream: { scope?: string } } } }).__mockHost.args.focus_stream.scope)).toBe("folder:2");
});

test("reading from a column titles the drawer and records the column as the source", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await boot(page);
  await addColumn(page, "English");
  await openColumn(page, "English");
  const title = "sqlite-vec 在桌面端单库里的取舍：WAL、FTS5 与向量检索如何共存";
  const id = MOCK_FOCUS.articles.find((row) => row.title === title)!.id;
  await page.locator(".focus:not([hidden])").getByRole("article", { name: title }).click();
  const reader = page.locator(".drawer.reader");
  await expect(reader.locator(".reader-title")).toHaveText(title);
  await reader.getByRole("button", { name: "篇目", exact: true }).click();
  const drawer = page.getByRole("complementary", { name: "篇目" });
  await expect(drawer.locator(".reader-article-drawer-head strong")).toHaveText("English");
  await drawer.getByRole("button", { name: "收起篇目" }).click();
  await expect(drawer).toHaveCount(0);
  await reader.locator(".reader-back").click();
  await expect(reader).toHaveCount(0);
  await expect.poll(() => page.evaluate((articleId) => {
    const sessions = (window as unknown as { __mockHost: { signals: { sessions: { article_id: number; via: string }[] } } }).__mockHost.signals.sessions;
    return sessions.filter((row) => row.article_id === articleId).map((row) => row.via);
  }, id), { timeout: 8_000 }).toEqual(["focus_column"]);
});

test("mark read in a column only touches that folder", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await boot(page);
  await addColumn(page, "English");
  await openColumn(page, "English");
  await page.locator(".focus:not([hidden])").getByRole("button", { name: "全部已读" }).click();
  await expect.poll(() => page.evaluate(() => (window as unknown as { __mockHost: { args: { mark_scope_read?: { scope: string } } } }).__mockHost.args.mark_scope_read?.scope)).toBe("folder:2");
  const reads = await page.evaluate(() => {
    const rows = (window as unknown as { __mockHost: { state: { focus: { articles: { id: number; read: boolean }[] } } } }).__mockHost.state.focus.articles;
    return Object.fromEntries(rows.filter((row) => row.id > 0).map((row) => [row.id, row.read]));
  });
  expect(reads[1]).toBe(true);
  expect(reads[2]).toBe(true);
  expect(reads[4]).toBe(false);
});

test("main focus and a column keep their own scroll positions", async ({ page }) => {
  await page.setViewportSize({ width: 1200, height: 520 });
  await boot(page, { focus: longFocus() });
  await expect(page.getByText("栏目长文 1", { exact: true })).toBeVisible();
  await addColumn(page, "English");
  const focusTop = await scrollVisible(page, 0.55);
  await openColumn(page, "English");
  await expect(page.locator(".focus:not([hidden])").getByText("栏目长文 1", { exact: true })).toBeVisible();
  const columnTop = await scrollVisible(page, 0.4);
  expect(columnTop).not.toBe(focusTop);
  await page.getByRole("button", { name: "今日焦点", exact: true }).click();
  await expect.poll(() => visibleScroll(page)).toBe(focusTop);
  await openColumn(page, "English");
  await expect.poll(() => visibleScroll(page)).toBe(columnTop);
});

test("moves a column up and removes it", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await boot(page);
  await addColumn(page, "中文");
  await addColumn(page, "English");
  await expect.poll(() => columnSetting(page)).toBe("folder:1,folder:2");
  const english = page.locator(".nav-column", { hasText: "English" });
  await english.hover();
  await english.getByRole("button", { name: "English栏目菜单" }).click();
  await page.getByRole("menuitem", { name: "上移" }).click();
  await expect.poll(() => columnSetting(page)).toBe("folder:2,folder:1");
  await expect(page.locator(".nav-column-name")).toHaveText(["English", "中文"]);
  await english.hover();
  await english.getByRole("button", { name: "English栏目菜单" }).click();
  await page.getByRole("menuitem", { name: "移出焦点栏目" }).click();
  await expect.poll(() => columnSetting(page)).toBe("folder:1");
  await expect(page.locator(".nav-column-name")).toHaveText(["中文"]);
});

test("sidebar columns work at 560 width", async ({ page }) => {
  await page.setViewportSize({ width: 560, height: 800 });
  await boot(page);
  await addColumn(page, "English");
  await openColumn(page, "English");
  await expect(page.locator(".focus:not([hidden]) .focus-masthead h1")).toContainText("English");
  const row = page.locator(".nav-column", { hasText: "English" });
  await row.hover();
  await row.getByRole("button", { name: "English栏目菜单" }).click();
  await expect(page.getByRole("menuitem", { name: "移出焦点栏目" })).toBeVisible();
});
