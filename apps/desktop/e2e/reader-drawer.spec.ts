import type { Page } from "@playwright/test";
import { expect, test } from "./fixtures";
import { installMockHost, MOCK_FOCUS, MOCK_READER, type MockHostOptions } from "./mockHost";
import { shot } from "./shots";

const READERS = {
  1: { ...MOCK_READER, id: 1, title: "sqlite-vec 在桌面端单库里的取舍：WAL、FTS5 与向量检索如何共存" },
  2: { ...MOCK_READER, id: 2, title: "Tauri 2 的 IPC 命令默认在主线程执行" },
  3: { ...MOCK_READER, id: 3, title: "WebKitGTK 2.46 发布说明" },
  4: { ...MOCK_READER, id: 4, title: "我用旧平板做了一块桌面信息屏" },
  5: { ...MOCK_READER, id: 5, title: "React 19 编译器在中大型应用里的实际收益" },
  6: { ...MOCK_READER, id: 6, title: "编译器 1.97.1 点发行说明" },
};

async function theme(page: Page, value: "light" | "dark") {
  await page.evaluate((next) => { document.documentElement.dataset.theme = next; }, value);
  await expect(page.locator("html")).toHaveCSS("color-scheme", value);
}

async function openReader(page: Page, options: MockHostOptions = {}) {
  await installMockHost(page, { readers: READERS, ...options });
  await page.goto("/");
  await page.getByRole("article").first().click();
  await expect(page.locator(".drawer.reader")).toBeVisible();
}

test("drawer follows navigation, keyboard selection, shared scope and return focus", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openReader(page);
  const reader = page.locator(".drawer.reader");
  await page.keyboard.press("l");
  const drawer = page.locator(".reader-article-drawer");
  await expect(drawer).toBeVisible();
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("ArrowDown");
  await expect(drawer.locator('[data-reader-drawer-row="3"] [aria-current="true"]')).toHaveCount(0);
  await page.keyboard.press("Enter");
  await expect(reader.locator(".reader-title")).toContainText("WebKitGTK");
  await page.keyboard.press("l");
  await expect(drawer.locator('[data-reader-drawer-row="3"] [aria-current="true"]')).toBeVisible();
  await drawer.getByRole("button", { name: "未读" }).click();
  await expect(drawer.getByRole("button", { name: "未读" })).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator(".main").getByRole("button", { name: "未读", exact: true })).toHaveAttribute("data-on", "1");
  await drawer.getByRole("button", { name: "全部" }).click();
  await page.keyboard.press("Escape");
  await expect(drawer).toBeHidden();
  await page.keyboard.press("Escape");
  await expect(reader).toHaveCount(0);
  await expect(page.locator(".main .toolbar")).toBeVisible();
  await expect(page.locator(".article.focused")).toContainText("WebKitGTK");
});

test("edge hover needs a pause, and drawer shots cover both themes", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openReader(page);
  const edge = page.locator(".reader-drawer-edge");
  // 掠过：同一任务内进入又离开（停留 0ms），延迟到期后也不打开；不靠墙钟计时，负载下不误判。
  await edge.evaluate((el) => {
    el.dispatchEvent(new MouseEvent("mouseover", { bubbles: true, relatedTarget: document.body }));
    el.dispatchEvent(new MouseEvent("mouseout", { bubbles: true, relatedTarget: document.body }));
  });
  await page.waitForTimeout(600);
  await expect(page.locator(".reader-article-drawer")).toHaveCount(0);
  // 顶部避让：阅读区顶部 96px 内不感应，停在左缘靠近「返回」处 1s 也不打开。
  const stage = await page.locator(".reader-stage").boundingBox();
  const box = await edge.boundingBox();
  expect(box!.y - stage!.y).toBeGreaterThanOrEqual(96);
  await page.mouse.move(stage!.x + 6, stage!.y + 60);
  await page.waitForTimeout(1000);
  await expect(page.locator(".reader-article-drawer")).toHaveCount(0);
  // 停留：超过延迟后打开。
  await edge.hover();
  await expect(page.locator(".reader-article-drawer")).toBeVisible();
  await shot(page, "test-results/reader-drawer/drawer-1280-light.png");
  await theme(page, "dark");
  await shot(page, "test-results/reader-drawer/drawer-1280-dark.png");
  await theme(page, "light");
  await page.setViewportSize({ width: 880, height: 800 });
  await expect(page.locator(".reader-article-drawer")).toBeVisible();
  await shot(page, "test-results/reader-drawer/drawer-880-light.png");
  await theme(page, "dark");
  await shot(page, "test-results/reader-drawer/drawer-880-dark.png");
});

test("scrim keeps one tint under the pointer, and closing slides the drawer out before it unmounts", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openReader(page);
  await page.keyboard.press("l");
  const drawer = page.locator(".reader-article-drawer"), scrim = page.locator(".reader-drawer-scrim");
  await expect(drawer).toBeVisible();
  const tint = () => scrim.evaluate(el => getComputedStyle(el).backgroundColor);
  await drawer.locator(".reader-article-drawer-main").first().hover();
  const idle = await tint();
  await page.mouse.move(900, 400);
  expect(await tint()).toBe(idle);
  // 退场期间还在：带收起标记、不可交互，播完才卸载。退场只有 180ms，轮询断言在负载下可能
  // 第一次就落在卸载之后；改在页面里用 MutationObserver 记下每次变化时的状态，不靠墙钟。
  await page.evaluate(() => {
    const seen: string[] = [];
    (window as any).__drawerExit = seen;
    new MutationObserver(() => {
      const aside = document.querySelector(".reader-article-drawer"), cover = document.querySelector(".reader-drawer-scrim");
      const state = !aside ? "gone"
        : aside.getAttribute("data-closing") === "true" ? `closing:${cover ? getComputedStyle(cover).pointerEvents : "no-scrim"}`
        : null;
      if (state && seen[seen.length - 1] !== state) seen.push(state);
    }).observe(document.body, { subtree: true, childList: true, attributes: true, attributeFilter: ["data-closing"] });
  });
  await scrim.click({ position: { x: 600, y: 400 } });
  await expect(drawer).toHaveCount(0);
  await expect(scrim).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__drawerExit)).toEqual(["closing:none", "gone"]);
});

test("pinned drawer uses the reading area width and Escape returns to the current article", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openReader(page);
  const stage = page.locator(".reader-stage");
  await page.keyboard.press("l");
  await expect.poll(() => stage.evaluate((node) => node.clientWidth)).toBeGreaterThanOrEqual(1100);
  await page.locator(".reader-article-drawer").getByRole("button", { name: "钉住", exact: true }).click();
  await expect(stage).toHaveAttribute("data-drawer-pinned", "true");
  await shot(page, "test-results/reader-drawer/pinned-1280-light.png");
  await page.setViewportSize({ width: 880, height: 800 });
  await expect.poll(() => stage.evaluate((node) => node.clientWidth)).toBeLessThan(1100);
  await expect(stage).not.toHaveAttribute("data-drawer-pinned", "true");
  await expect(page.locator(".reader-article-drawer")).toBeVisible();
  await expect(page.getByRole("button", { name: "钉住", exact: true })).toHaveCount(0);
  await shot(page, "test-results/reader-drawer/pinned-880-light.png");
  await page.keyboard.press("Escape");
  await expect(page.locator(".reader-article-drawer")).toHaveCount(0);
  await expect(page.locator(".drawer.reader")).toBeVisible();
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.keyboard.press("l");
  await expect(stage).toHaveAttribute("data-drawer-pinned", "true");
  await page.keyboard.press("Escape");
  await expect(page.locator(".drawer.reader")).toHaveCount(0);
  await expect(page.locator(".article.focused")).toContainText("sqlite-vec");
  await page.locator(".article.focused").click();
  await expect(page.locator(".drawer.reader")).toBeVisible();
  await page.keyboard.press("l");
  await expect(stage).toHaveAttribute("data-drawer-pinned", "true");
});

test("x rates once, hides the row and opens the next article", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openReader(page);
  const reader = page.locator(".drawer.reader");
  await page.keyboard.press("x");
  await expect(reader.locator(".reader-title")).toContainText("Tauri 2");
  await expect.poll(() => page.evaluate(() => (window as unknown as { __mockHost: { calls: string[] } }).__mockHost.calls.filter((c) => c === "rate_article").length)).toBe(1);
  await page.keyboard.press("Escape");
  await expect(page.locator(".article").filter({ hasText: "sqlite-vec" })).toHaveCount(0);
});

test("disliking another drawer row keeps the current article", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await openReader(page);
  const reader = page.locator(".drawer.reader");
  await page.keyboard.press("l");
  const row = page.locator('[data-reader-drawer-row="2"]');
  await row.hover();
  await row.getByRole("button", { name: "踩「Tauri 2 的 IPC 命令默认在主线程执行」" }).click();
  await expect(reader.locator(".reader-title")).toContainText("sqlite-vec");
  await expect(page.locator('[data-reader-drawer-row="1"] [aria-current="true"]')).toBeVisible();
  await expect(page.locator('[data-reader-drawer-row="2"]')).toHaveCount(0);
  await expect(page.locator(".reader-article-drawer-row.selected")).toHaveAttribute("data-reader-drawer-row", "3");
  await expect.poll(() => page.evaluate(() => {
    const host = (window as unknown as { __mockHost: { calls: string[]; args: Record<string, { id: number; up: boolean }> } }).__mockHost;
    return { count: host.calls.filter((c) => c === "rate_article").length, args: host.args.rate_article };
  })).toEqual({ count: 1, args: { id: 2, up: false } });
  await expect(reader).toBeVisible();
});

test("digest readers do not expose the drawer entry", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "简报", exact: true }).click();
  await page.getByRole("button", { name: "来源 1：《我用旧平板做了一块桌面信息屏》，闲聊广场" }).click();
  await expect(page.locator(".drawer.reader")).toBeVisible();
  await expect(page.locator(".drawer.reader").getByRole("button", { name: "篇目" })).toHaveCount(0);
  await expect(page.locator(".reader-position")).toHaveCount(0);
  await expect(page.locator(".reader-next")).toHaveCount(0);
});

// 未读范围：踩后与阅读期间的列表重读都不能把正在读的文章从导航里拿掉。
const UNREAD_IDS = MOCK_FOCUS.articles.filter((row) => row.row_kind === 0).map((row) => row.id);
const UNREAD_FOCUS = { ...MOCK_FOCUS, list_scope: "unread", articles: MOCK_FOCUS.articles.map((row) => ({ ...row, read: false })) };
const UNREAD_READERS = Object.fromEntries(UNREAD_IDS.map((id) => [id, { ...MOCK_READER, id, title: `第 ${id} 篇` }]));
const calls = (page: Page, name: string) => page.evaluate((command) =>
  (window as unknown as { __mockHost: { calls: string[] } }).__mockHost.calls.filter((c) => c === command).length, name);

test("x keeps working in the unread scope and skips rule suggestions", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await installMockHost(page, { readers: UNREAD_READERS, focus: UNREAD_FOCUS });
  await page.goto("/");
  await page.getByRole("article").first().click();
  const title = page.locator(".drawer.reader .reader-title");
  await expect(title).toHaveText(`第 ${UNREAD_IDS[0]} 篇`);
  for (const next of UNREAD_IDS.slice(1, 4)) {
    await page.keyboard.press("x");
    await expect(title).toHaveText(`第 ${next} 篇`);
  }
  await expect.poll(() => calls(page, "rate_article")).toBe(3);
  expect(await calls(page, "training_features")).toBe(0);
  await page.keyboard.press("j");
  await expect(title).toHaveText(`第 ${UNREAD_IDS[4]} 篇`);
  await page.keyboard.press("Escape");
  await expect(page.locator(".drawer.reader")).toHaveCount(0);
  for (const id of UNREAD_IDS.slice(0, 5)) await expect(page.locator(".article").filter({ hasText: READERS_TITLE(id) })).toHaveCount(0);
  await expect(page.locator(".article.focused")).toContainText(READERS_TITLE(UNREAD_IDS[5]));
});

test("disliking from the reader tools keeps J/K working in the unread scope", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await installMockHost(page, { readers: UNREAD_READERS, focus: UNREAD_FOCUS });
  await page.goto("/");
  await page.getByRole("article").first().click();
  const reader = page.locator(".drawer.reader");
  const title = reader.locator(".reader-title");
  await expect(title).toHaveText(`第 ${UNREAD_IDS[0]} 篇`);
  await reader.locator(".reader-tools > summary").click();
  await reader.locator(".reader-tools-panel").getByRole("button", { name: "踩", exact: true }).click();
  await expect.poll(() => calls(page, "training_features")).toBe(1);
  await expect.poll(() => calls(page, "focus_stream")).toBeGreaterThan(1);
  await page.keyboard.press("j");
  await expect(title).toHaveText(`第 ${UNREAD_IDS[1]} 篇`);
  await page.keyboard.press("k");
  await expect(title).toHaveText(`第 ${UNREAD_IDS[0]} 篇`);
});

function READERS_TITLE(id: number) {
  return MOCK_FOCUS.articles.find((row) => row.row_kind === 0 && row.id === id)!.title.slice(0, 12);
}

test("drawer clusters share focus expansion and support mouse and keyboard article navigation", async ({ page }) => {
  // 合辑与文章特意用相同的 ID，验证选中键不会串行。
  const article = { ...MOCK_FOCUS.articles[0], id: 1 };
  const cluster = { ...article, row_kind: 1, cluster_id: 1, cluster_label: "同题报道", cluster_size: 2 };
  const member = { ...article, id: 71, title: "合辑内第一篇", cluster_id: 1 };
  const member2 = { ...member, id: 72, title: "合辑内第二篇" };
  await openReader(page, {
    focus: { ...MOCK_FOCUS, articles: [article, cluster], has_more: false },
    clusterArticles: { 1: [member, member2] },
    readers: { 1: READERS[1], 71: { ...MOCK_READER, id: 71, title: member.title }, 72: { ...MOCK_READER, id: 72, title: member2.title } },
  });
  const reader = page.locator(".drawer.reader");
  const drawer = page.locator(".reader-article-drawer");
  await expect(reader.locator(".reader-position")).toHaveText("1 / 1");
  await page.keyboard.press("l");
  const group = drawer.getByRole("button", { name: /合辑 · 同题报道/ });
  await group.click();
  await expect(group).toHaveAttribute("aria-expanded", "true");
  await expect(reader.locator(".reader-position")).toHaveText("1 / 3");
  await expect(page.locator('.focus button.cluster')).toHaveAttribute("aria-expanded", "true");
  await group.click();
  await expect(group).toHaveAttribute("aria-expanded", "false");
  await expect(drawer.locator('[data-reader-drawer-row="71"]')).toHaveCount(0);
  // 焦点在合辑按钮上，方向键选中后 Enter 仍对选中行操作。
  await page.keyboard.press("ArrowUp");
  await page.keyboard.press("ArrowDown");
  await expect(drawer.locator('[data-reader-drawer-row="-1"]')).toHaveClass(/selected/);
  await page.keyboard.press("Enter");
  await expect(group).toHaveAttribute("aria-expanded", "true");
  await page.keyboard.press("Enter");
  await expect(group).toHaveAttribute("aria-expanded", "false");
  await page.keyboard.press("Enter");
  await expect(group).toHaveAttribute("aria-expanded", "true");
  await page.keyboard.press("ArrowDown");
  await page.keyboard.press("Enter");
  await expect(reader.locator(".reader-title")).toHaveText(member.title);
  await expect(reader.locator(".reader-position")).toHaveText("2 / 3");
  await page.keyboard.press("l");
  await drawer.getByRole("button", { name: `踩「${member2.title}」`, exact: true }).click();
  await expect(drawer.locator('[data-reader-drawer-row="72"]')).toHaveCount(0);
  await expect(reader.locator(".reader-title")).toHaveText(member.title);
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await expect(page.locator('.focus button.cluster')).toHaveAttribute("aria-expanded", "true");
  await expect(page.locator('.focus .article.focused')).toContainText(member.title);
});
