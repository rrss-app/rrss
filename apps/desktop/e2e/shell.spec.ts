import { expect, test } from "./fixtures";
import { PREVIEW_FEEDS, PREVIEW_FOCUS } from "../src/lib/preview";
import { installMockHost, settingsTab, MOCK_READER, MOCK_FOCUS } from "./mockHost";
import { libraryReaderFixture } from "../src/test/fixtures/reader";

test.describe("desktop shell", () => {
  test("mock host rejects unknown writes and known commands without a handler", async ({ page }) => {
    await installMockHost(page);
    await page.goto("/");
    const errors = await page.evaluate(async () => {
      const host = (window as any).__TAURI_INTERNALS__;
      const error = async (run: () => Promise<unknown>) => {
        try { await run(); return "accepted"; } catch (e) { return String(e); }
      };
      return [await error(() => host.invoke("set_unregistered_option", {})),
        await error(() => host.invoke("test_translate", {}))];
    });
    expect(errors[0]).toContain("unexpected command set_unregistered_option");
    expect(errors[1]).toContain("unexpected command test_translate");
  });

  test("the window itself never scrolls in any main view or window size", async ({ page }) => {
    await installMockHost(page);
    await page.goto("/");
    await expect(page.locator(".focus")).toBeVisible();
    // 窗口外壳不是滚动容器：怎么滚都留在原点（简报页输入区里隐藏的提示框曾把整页撑出 4px）。
    const tryScroll = () => page.evaluate(() => {
      const root = document.getElementById("root")!;
      window.scrollTo(9999, 9999);
      root.scrollTop = document.body.scrollTop = 9999;
      return [window.scrollX, window.scrollY, root.scrollTop, document.body.scrollTop];
    });
    for (const view of ["今日焦点", "简报", "深度对话"]) {
      await page.getByRole("button", { name: view, exact: true }).first().click();
      for (const size of [{ width: 1280, height: 800 }, { width: 880, height: 560 }, { width: 640, height: 360 }]) {
        await page.setViewportSize(size);
        expect(await tryScroll(), `${view} ${size.width}x${size.height}`).toEqual([0, 0, 0, 0]);
      }
    }
  });

  test("reader opens original, cites from menu, and returns", async ({ page }) => {
    // 保持本例的阅读与引用路径，网页阅读默认方式另有专项覆盖。
    await installMockHost(page, { reader: libraryReaderFixture(), settingValues: { web_open_mode: "browser" } });
    const opened: string[] = [];
    await page.exposeFunction("rrssOpenCapture", (url: string) => {
      opened.push(url);
    });
    await page.addInitScript(() => {
      const host = (window as any).__TAURI_INTERNALS__;
      const invoke = host.invoke;
      host.invoke = (command: string, args: any) => command === "plugin:opener|open_url"
        ? (window as any).rrssOpenCapture(args.url) : invoke(command, args);
      window.open = ((url?: string | URL) => {
        const href = String(url ?? "");
        (window as unknown as { rrssOpenCapture: (u: string) => void }).rrssOpenCapture(href);
        return null;
      }) as typeof window.open;
    });

    await page.goto("/");
    await page.getByRole("article").first().click();
    await expect(page.getByRole("heading", { name: "开馆日", level: 2 })).toBeVisible();
    const stage = page.locator(".drawer.reader");
    const frame = await stage.boundingBox();
    expect(frame).toBeTruthy();
    expect(frame!.width).toBeGreaterThan(1000);
    expect(frame!.width).toBe(page.viewportSize()!.width);
    await expect(page.getByRole("button", { name: "查看原文" })).toBeVisible();
    await expect(page.getByRole("button", { name: "生成本篇摘要" })).toBeVisible();
    await page.locator(".reader-tools > summary").click();
    await expect(page.getByRole("button", { name: "生成本篇摘要" })).toBeVisible();
    await expect(page.getByRole("button", { name: "翻译", exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "返回" }).first()).toBeVisible();

    await page.locator(".reader-tools > summary").click();
    const rail = page.locator(".reader-view-rail");
    await expect(rail).toBeVisible();
    await expect(rail.getByRole("radio", { name: "双语" })).toBeVisible();
    await rail.getByRole("radio", { name: "双语" }).click();
    await expect(page.locator(".reader-title")).toHaveText("Opening Day");
    await expect(page.getByRole("heading", { name: "开馆日", level: 2 })).toBeVisible();

    await page.locator(".drawer.reader .drawer-body").evaluate((el) => {
      el.scrollTop = el.scrollHeight;
    });
    await expect(rail.getByRole("radio", { name: "译文" })).toBeVisible();
    await rail.getByRole("radio", { name: "译文" }).click();
    await expect(rail.getByRole("radio", { name: "译文", checked: true })).toBeVisible();

    await page.getByRole("button", { name: "查看原文" }).click();
    await expect.poll(() => opened.at(-1)).toContain("example.test");

    await rail.getByRole("radio", { name: "双语" }).click();
    await page.getByRole("link", { name: "时刻表" }).click();
    await expect.poll(() => opened.at(-1)).toContain("library.example/hours");

    const paragraph = page.getByText("儿童区有矮书架");
    const box = await paragraph.boundingBox();
    expect(box).toBeTruthy();
    await page.mouse.move(box!.x + 4, box!.y + 8);
    await page.mouse.down();
    await page.mouse.move(box!.x + Math.min(box!.width - 8, 120), box!.y + 8);
    await page.mouse.up();
    const cite = page.locator(".sel-menu").getByRole("menuitem", { name: "引用" });
    await expect(cite).toBeVisible();
    await expect(page.locator(".composer .chip")).toHaveCount(0);
    await cite.click();
    await expect(page.getByRole("region", { name: "引用托盘" })).toBeVisible();
    await expect(page.locator(".reader-citation-chip")).toHaveCount(1);
    await page.getByRole("region", { name: "引用托盘" }).getByRole("button", { name: "清空" }).click();

    await page.getByRole("button", { name: "返回" }).first().click();
    await expect(page.getByRole("heading", { name: "开馆日", level: 2 })).toHaveCount(0);

    await page.getByRole("article").first().click();
    await expect(page.getByRole("heading", { name: "开馆日", level: 2 })).toBeVisible();
    await expect(page.locator(".reader-stage .overlay-dismiss")).toHaveCount(0);
    await page.locator(".drawer.reader .drawer-body").click({ position: { x: 16, y: 80 } });
    await expect(page.getByRole("heading", { name: "开馆日", level: 2 })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("heading", { name: "开馆日", level: 2 })).toHaveCount(0);

    await page.getByRole("article").first().click();
    await expect(page.getByRole("heading", { name: "开馆日", level: 2 })).toBeVisible();
    await page.evaluate(() => {
      window.dispatchEvent(new MouseEvent("mouseup", { button: 3, bubbles: true }));
    });
    await expect(page.getByRole("heading", { name: "开馆日", level: 2 })).toHaveCount(0);
  });

  test("keyboard opens article and returns", async ({ page }) => {
    await page.goto("/");
    await expect(page.getByRole("article").first()).toBeVisible();
    await page.keyboard.press("j");
    await page.keyboard.press("Enter");
    await expect(page.locator(".drawer.reader")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.locator(".drawer.reader")).toHaveCount(0);
  });

  test("search backend mock supports filters and read-only reader block hits", async ({ page }) => {
    await installMockHost(page);
    await page.goto("/");
    await expect(page.getByRole("article").first()).toBeVisible();
    const result = await page.evaluate(async () => {
      const modulePath = "/src/lib/api.ts";
      const { api } = await import(modulePath);
      const query = { query: '"sqlite"', scope: "all", feed_ids: [], published_since: null, limit: 20 };
      const all = await api.searchArticles(query);
      const unread = await api.searchArticles({ ...query, scope: "unread" });
      const starred = await api.searchArticles({ ...query, scope: "starred" });
      const feed = await api.searchArticles({ ...query, feed_ids: [2] });
      const time = await api.searchArticles({ ...query, published_since: Date.now() - 30 * 60_000 });
      const limited = await api.searchArticles({ ...query, limit: 1 });
      const blocks = await api.searchHitBlocks(1, '"SQLITE"');
      const snapshot = await api.reader(1);
      const focus = await api.focus();
      return { all, unread, starred, feed, time, limited, blocks, snapshot, focus };
    });
    expect(result.all.find((hit: any) => hit.id === 1)).toMatchObject({ read: false, starred: false, title_match: true, body_match: true });
    expect(result.all[0].snippet).not.toBe(MOCK_READER.summary);
    expect(result.unread.every((hit: any) => !hit.read)).toBe(true);
    expect(result.starred.map((hit: any) => hit.id)).toEqual(MOCK_FOCUS.articles.filter(a => a.row_kind === 0 && a.starred).map(a => a.id));
    expect(result.feed.map((hit: any) => hit.id)).toEqual(MOCK_FOCUS.articles.filter(a => a.row_kind === 0 && a.feed_id === 2 && a.score > -10).map(a => a.id));
    expect(result.time.map((hit: any) => hit.id)).toEqual([1]);
    expect(result.limited).toHaveLength(1);
    expect(result.blocks).toMatchObject({ source_hash: result.snapshot.source_hash, total: 8, title_match: true, hits: [{ block_id: 2, ordinal: 3, section: "单库的三个组件" }] });
    expect(result.blocks.hits[0].text).toContain("sqlite-vec");
    expect(result.focus.articles.find((a: any) => a.id === 1).read).toBe(false);
  });

  test("focus filters and remaining settings panes", async ({ page }) => {
    // 设置各页读失败时只显示重试（F4 不再用预览默认值冒充已保存配置），所以用完整模拟宿主给出每页数据。
    await installMockHost(page, { focus: PREVIEW_FOCUS, feeds: PREVIEW_FEEDS });
    await page.goto("/");
    await expect(page.getByRole("button", { name: "仅关注" })).toBeVisible();
    await expect(page.getByRole("button", { name: "隐藏弱化" })).toBeVisible();
    const displayMenu = page.getByRole("button", { name: "显示", exact: true });
    await expect(displayMenu).toHaveText("显示");
    await displayMenu.click();
    const display = page.getByRole("menu", { name: "显示" });
    await expect(display.getByRole("menuitemcheckbox", { name: "相近文章折叠" })).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(display).toHaveCount(0);
    // 相近报道只留标题行一个展开入口。
    await expect(page.getByRole("button", { name: "Rust 发行 · 2 篇 · 展开" })).toBeVisible();
    await expect(page.getByText("时间线")).toHaveCount(0);

    await page.getByRole("button", { name: "设置" }).click();
    const drawer = page.locator(".drawer");
    await settingsTab(drawer, "rules").click();
    await expect(drawer.getByRole("heading", { name: "新建规则", exact: true })).toBeVisible();
    await expect(drawer.getByLabel("匹配内容", { exact: true })).toBeVisible();
    await expect(drawer.getByRole("button", { name: "添加", exact: true })).toBeVisible();

    await settingsTab(drawer, "memory").click();
    await expect(drawer.locator(".memory-card").first()).toBeVisible();
    await expect(drawer.getByLabel("搜索记忆", { exact: true })).toBeVisible();
    await drawer.getByRole("group", { name: "记忆设置分类" }).getByRole("button", { name: /^兴趣/ }).click();
    await expect(drawer.getByRole("button", { name: "兴趣加分" })).toBeVisible();

    await settingsTab(drawer, "model").click();
    await expect(drawer.getByRole("button", { name: "添加连接" })).toBeVisible();

    await settingsTab(drawer, "translate").click();
    await expect(drawer.getByRole("heading", { name: "阅读", exact: true })).toBeVisible();
    await expect(drawer.getByText("目标语言", { exact: true })).toBeVisible();
    await expect(drawer.getByRole("button", { name: "测试连接" })).toBeVisible();
    await expect(drawer.getByRole("button", { name: "DeepLX" })).toBeVisible();

    await settingsTab(drawer, "assistant").click();
    await expect(drawer.getByRole("button", { name: "重启引擎", exact: true })).toBeVisible();
    await expect(drawer.getByRole("button", { name: /^工具 / }).first()).toBeVisible();

    await settingsTab(drawer, "diagnostics").click();
    await expect(drawer.getByRole("button", { name: "处理一批" })).toBeVisible();
    await expect(drawer.getByRole("button", { name: "重置失败计数" })).toBeVisible();
  });

  test("digest is a base view and survives settings", async ({ page }) => {
    await page.goto("/");
    await page.getByRole("button", { name: "简报", exact: true }).click();
    await expect(page.getByRole("button", { name: "生成本期" })).toBeVisible();
    await expect(page.getByRole("button", { name: "生成日报" })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "生成周报" })).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "今日三条主线" })).toBeVisible();
    await expect(page.locator(".overlay")).toHaveCount(0);
    await page.getByRole("button", { name: "周报", exact: true }).click();
    await expect(page.getByRole("heading", { name: "本周三条主线" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "今日三条主线" })).toHaveCount(0);
    await page.getByRole("button", { name: "日报", exact: true }).click();
    await expect(page.getByRole("heading", { name: "今日三条主线" })).toBeVisible();
    await page.getByRole("button", { name: "设置" }).click();
    await expect(page.getByPlaceholder("粘贴订阅地址，例如 https://example.com/feed.xml")).toBeVisible();
    await page.getByRole("button", { name: "返回" }).click();
    await expect(page.getByRole("button", { name: "生成本期" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "今日三条主线" })).toBeVisible();
    await expect(page.locator(".digest")).toBeVisible();
  });

  test("digest source chip opens reader and returns to digest", async ({ page }) => {
    await page.goto("/");
    await page.getByRole("button", { name: "简报", exact: true }).click();
    await page.getByRole("button", { name: "来源 1：《我用旧平板做了一块桌面信息屏》，闲聊广场" }).click();
    await expect(page.locator(".drawer.reader")).toBeVisible();
    await expect(page.locator(".digest")).toBeVisible();
    await expect(page.locator(".drawer.reader").getByRole("button", { name: "上一篇", exact: true })).toBeDisabled();
    await expect(page.locator(".drawer.reader").getByRole("button", { name: "下一篇", exact: true })).toBeDisabled();
    await page.getByRole("button", { name: "返回" }).first().click();
    await expect(page.locator(".drawer.reader")).toHaveCount(0);
    await expect(page.getByRole("heading", { name: "今日三条主线" })).toBeVisible();
  });
});
