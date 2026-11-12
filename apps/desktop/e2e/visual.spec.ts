import { expect, test, type Page } from "./fixtures";
import { installMockHost, settingsTab } from "./mockHost";
import { shot } from "./shots";

// 带数据的模拟宿主截图夹具：每个主视图在 1280 / 880 亮暗下无横向溢出，并核对阶段 15 的关键钩子。
// 截图只在审阅时生成（`RRSS_SHOTS=1`），断言每次都跑。
const OUT = "../../test-results/visual-overhaul";
const SIZES = [
  { width: 1280, height: 800 },
  { width: 880, height: 640 },
];

async function noOverflow(page: Page) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true);
  expect(await page.evaluate(() => document.body.scrollWidth <= window.innerWidth + 1)).toBe(true);
}

async function shoot(page: Page, name: string, opts: { fullPage?: boolean } = {}) {
  for (const size of SIZES) {
    await page.setViewportSize(size);
    for (const theme of ["light", "dark"]) {
      await page.evaluate((t) => { document.documentElement.dataset.theme = t; }, theme);
      // 左缘是阅读器篇目抽屉的悬停热区，鼠标停在右下角，避免截图时唤出抽屉。
      await page.mouse.move(size.width - 1, size.height - 1);
      await noOverflow(page);
      await shot(page, `${OUT}/${name}-${size.width}-${theme}.png`, { fullPage: opts.fullPage });
    }
  }
  await page.setViewportSize(SIZES[0]);
  await page.evaluate(() => { document.documentElement.dataset.theme = "light"; });
}

test.beforeEach(async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
});

test("focus: toolbar groups, card hover, digest lede and refresh retry", async ({ page }) => {
  await installMockHost(page, { refreshFails: true });
  await page.goto("/");
  await expect(page.getByRole("article").first()).toBeVisible();
  const toolbar = page.locator(".toolbar");
  await expect(toolbar.getByRole("group", { name: "评分过滤" })).toBeVisible();
  await expect(toolbar.getByRole("group", { name: "阅读范围" })).toBeVisible();
  await expect(toolbar.locator(".tab").filter({ hasText: /^全部$/ })).toHaveCount(0);
  await expect(page.locator(".focus-lede")).toContainText("今日简报");
  await shoot(page, "focus");
  for (const theme of ["light", "dark"]) {
    await page.evaluate(t => { document.documentElement.dataset.theme = t; }, theme);
    await page.locator('.nav-item[data-on="0"]').first().hover();
    await shot(page, `${OUT}/sidebar-hover-1280-${theme}.png`);
  }
  await page.evaluate(() => { document.documentElement.dataset.theme = "light"; });

  const card = page.getByRole("article").first();
  await page.mouse.move(0, 0);
  await expect(card.locator(".acts")).toHaveCSS("opacity", "0");
  await card.hover();
  await expect(card.locator(".acts")).toHaveCSS("opacity", "1");
  await shot(page, `${OUT}/focus-hover-1280-light.png`);

  await page.getByRole("button", { name: "刷新", exact: true }).click();
  const toast = page.locator(".toast");
  await expect(toast).toContainText("失败");
  // 失败源按名字点出来，不只给个数。
  await expect(toast).toContainText("闲聊广场");
  await expect(toast.getByRole("button", { name: "重试" })).toBeVisible();
  await shot(page, `${OUT}/focus-toast-1280-light.png`);

  await page.setViewportSize({ width: 720, height: 600 });
  await expect(toolbar.getByRole("button", { name: "刷新", exact: true })).toBeVisible();
  await noOverflow(page);
  await shot(page, `${OUT}/focus-720-light.png`);
});

test("sidebar search entry opens the palette", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "搜索文章" }).click();
  const search = page.getByRole("dialog", { name: "搜索文章" });
  await expect(search.getByRole("searchbox")).toBeFocused();
  await search.getByRole("searchbox").fill("tauri");
  await expect(search.getByRole("option").first()).toContainText("Tauri 2");
  await shot(page, `${OUT}/search-1280-light.png`);
  await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
  await shot(page, `${OUT}/search-1280-dark.png`);
});

test("reader: summary, reasons, tools menu and rail", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("article").first().click();
  const reader = page.locator(".drawer.reader");
  await expect(reader.locator(".reader-title")).toBeVisible();
  await expect(reader.locator(".reader-summary")).toBeVisible();
  await shoot(page, "reader");
  await reader.locator(".reader-tools > summary").click();
  await expect(reader.getByRole("button", { name: "抓取全文" })).toBeVisible();
  const panel = await reader.locator(".reader-tools-panel").boundingBox();
  expect(panel!.x + panel!.width).toBeLessThanOrEqual(1280);
  for (const theme of ["light", "dark"]) {
    await page.evaluate(t => { document.documentElement.dataset.theme = t; }, theme);
    await shot(page, `${OUT}/reader-tools-1280-${theme}.png`);
  }
});

test("digest: pager, sources and follow-up thread", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "简报", exact: true }).click();
  await expect(page.locator(".digest-date")).toHaveText("9月19日");
  await expect(page.locator(".digest-discussion .assistant")).toBeVisible();
  await expect(page.locator(".composer textarea")).toBeVisible();
  await shoot(page, "digest", { fullPage: true });
  await page.getByRole("button", { name: "上期" }).click();
  await expect(page.locator(".digest-date")).toHaveText("9月18日");
  await expect(page.locator(".digest-discussion")).toHaveCount(0);
  await shot(page, `${OUT}/digest-empty-1280-light.png`);
  await page.locator(".composer textarea").focus();
  await shot(page, `${OUT}/digest-composer-focus-1280-light.png`);
});

test("chat: thread with citations and list reply", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "查证 Rust 1.97 的回归修复" }).click();
  await expect(page.locator(".bubble.assistant")).toBeVisible();
  await expect(page.locator(".bubble.user .citation-chip")).toBeVisible();
  await shoot(page, "chat");
  await page.getByRole("button", { name: "新对话" }).click();
  await expect(page.locator(".chat-empty")).toBeVisible();
  await shot(page, `${OUT}/chat-empty-1280-light.png`);
});

test("settings: every category with data, theme segment, sticky navigation", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  await expect(settings.locator(".settings-theme .segmented")).toBeVisible();
  await expect(settings.getByText("外观")).toBeVisible();
  const nav = settings.getByRole("navigation", { name: "设置分类" });
  // 按分类 id 遍历；截图也用 id 命名，改导航文字不影响用例。
  for (const [id, probe] of [
    ["subscribe", "我的订阅"],
    ["rules", "隐藏阈值"],
    ["memory", "本地优先"],
    ["model", "各用途"],
    ["translate", "阅读"],
    ["assistant", "模型与能力"],
    ["diagnostics", "索引维护"],
    ["storage", "空闲空间"],
    ["advanced", "对话引擎与安全"],
  ] as const) {
    await settingsTab(nav, id).click();
    await expect(settings.getByText(probe).first()).toBeVisible();
    await shoot(page, `settings-${id}`);
    await page.setViewportSize({ width: 560, height: 640 });
    await noOverflow(page);
    expect(await settings.locator(".drawer-body").evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
    await shot(page, `${OUT}/settings-${id}-560-light.png`);
    await page.setViewportSize(SIZES[0]);
  }
  await settingsTab(nav, "rules").click();
  await settings.locator(".drawer-body").evaluate((el) => { el.scrollTop = el.scrollHeight; });
  await expect(settingsTab(nav, "diagnostics")).toBeInViewport();
  await settingsTab(nav, "memory").click();
  await settings.getByRole("group", { name: "记忆设置分类" }).getByRole("button", { name: /^兴趣/ }).click();
  await expect(settings.getByRole("button", { name: "兴趣加分" })).toBeVisible();
  await shot(page, `${OUT}/settings-interest-1280-light.png`);
});
