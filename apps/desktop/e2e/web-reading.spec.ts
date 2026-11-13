import { expect, test } from "./fixtures";
import { installMockHost, settingsTab, MOCK_MESSAGES, MOCK_THREADS } from "./mockHost";
import { readerView, TWO_ARTICLE_PAGE } from "../src/test/fixtures/reader";
import { shot } from "./shots";

test("外链阅读、收录与失败重试保留返回路径", async ({ page }) => {
  const url = "https://redirect.example.test/story";
  const links = [url, "https://web.example.test/waf", "https://web.example.test/empty"];
  await installMockHost(page, {
    focus: TWO_ARTICLE_PAGE,
    readers: {
      1: readerView(1, { document: { v: 1, blocks: links.map((href, i) => ({ t: "p", inlines: [{ t: "a", href, children: [{ t: "t", text: `外链 ${i}` }] }] })) } }),
      10: readerView(10, { title: "网页文章", url }),
    },
    webLinks: {
      [url]: { article_id: 10, temporary: true, actual_host: "web.example.test", actual_url: "http://web.example.test/story", layer: "UserRule", text_chars: 1842, image_count: 3 },
      [links[1]]: { kind: "Waf", message: "HTTP 403" },
      [links[2]]: { kind: "Empty", message: "empty" },
    },
  });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  await page.getByRole("article", { name: "甲文", exact: true }).click();
  const reader = page.locator(".drawer.reader");
  await reader.getByRole("link", { name: "外链 0", exact: true }).click();
  await expect(reader.locator(".reader-title")).toHaveText("网页文章");
  await expect(reader.locator(".web-origin")).toContainText("临时打开 · 不在库里");
  await expect(reader.locator(".web-origin")).toContainText("按你的站点规则抽取（web.example.test） · 1,842 字 · 3 张图");
  await expect(reader.locator(".web-host")).toHaveText("web.example.test");
  await expect(reader.locator(".web-host")).toHaveAttribute("data-tone", "warn");
  await expect(reader.getByRole("button", { name: "原网页", exact: true })).toBeEnabled();
  await expect(reader.getByRole("note")).toContainText("RRSS 只做了抽取和排版");
  await shot(page, "test-results/web-reading/first-tip.png");
  await reader.getByRole("checkbox", { name: "不再显示" }).check();
  await reader.getByRole("button", { name: "知道了", exact: true }).click();
  await expect(reader.getByRole("note")).toHaveCount(0);
  await reader.getByRole("button", { name: "返回", exact: true }).click();
  await expect(reader).toBeVisible();
  await expect(reader.locator(".reader-title")).toHaveText("甲文");
  await reader.getByRole("link", { name: "外链 0", exact: true }).click();
  await expect(reader.locator(".reader-title")).toHaveText("网页文章");
  await shot(page, "test-results/web-reading/temporary.png");
  await expect(reader.getByRole("note")).toHaveCount(0);
  await reader.locator("[data-reader-document] p").first().click({ button: "right" });
  await shot(page, "test-results/web-reading/collect-menu.png");
  await page.getByRole("menu", { name: "本篇操作" }).getByRole("menuitem", { name: "收录", exact: true }).click();
  await expect(reader.locator(".web-origin")).toContainText("已收录到库");
  await expect(reader.getByRole("button", { name: "已收录", exact: true })).toBeDisabled();
  await page.setViewportSize({ width: 560, height: 800 });
  await page.evaluate(() => document.documentElement.dataset.theme = "dark");
  await shot(page, "test-results/web-reading/collected-narrow-dark.png");
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.evaluate(() => document.documentElement.dataset.theme = "light");
  await reader.getByRole("button", { name: "返回", exact: true }).click();
  await expect(reader.locator(".reader-title")).toHaveText("甲文");
  for (const [i, primary] of [[1, "打开原网页通过验证"], [2, "打开原网页"]] as const) {
    await reader.getByRole("link", { name: `外链 ${i}`, exact: true }).click();
    await expect(reader.getByRole("button", { name: primary, exact: true })).toBeEnabled();
    await expect(reader.locator(".web-failure").getByRole("button", { name: "在系统浏览器打开", exact: true })).toBeEnabled();
    await expect(reader.getByRole("button", { name: "收录", exact: true })).toHaveCount(0);
    if (i === 1) await shot(page, "test-results/web-reading/waf.png");
    const opens = () => page.evaluate(() => (window as any).__mockHost.calls.filter((call: string) => call === "open_web_link").length as number);
    const before = await opens();
    await reader.getByRole("button", { name: "重试", exact: true }).click();
    await expect.poll(opens).toBe(before + 1);
    await expect(reader.getByRole("button", { name: primary, exact: true })).toBeEnabled();
    // 失败页按钮和浏览器后退都恢复来源文章。
    if (i === 1) await reader.getByRole("button", { name: "返回", exact: true }).click();
    else await page.goBack();
    await expect(reader.locator(".reader-title")).toHaveText("甲文");
  }
  await page.keyboard.press("Escape");
  await expect(reader).not.toBeVisible();
});

test("安全拦截独立于首次提示，设置默认浏览器后仍需确认", async ({ page }) => {
  const href = "https://xn--pple-43d.com/story";
  await installMockHost(page, {
    focus: TWO_ARTICLE_PAGE, settingValues: { web_first_tip: "0" },
    readers: { 1: readerView(1, { document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "a", href, children: [{ t: "t", text: "apple.com" }] }] }] } }) },
    linkSafety: { [href]: { actual_host: "аpple.com", warnings: [{ kind: "MixedScript", unicode: "аpple.com", punycode: "xn--pple-43d.com" }, { kind: "HostMismatch", text_host: "apple.com" }] } },
  });
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  await settingsTab(settings, "web").click();
  await settings.getByRole("button", { name: "点击外部链接", exact: true }).click();
  const menu = page.getByRole("menu", { name: "点击外部链接" });
  await menu.getByRole("menuitemradio", { name: "打开原网页", exact: true }).click();
  await expect(settings.getByRole("button", { name: "点击外部链接", exact: true })).toHaveText("打开原网页");
  await settings.getByRole("button", { name: "点击外部链接", exact: true }).click();
  await menu.getByRole("menuitemradio", { name: "在系统浏览器打开", exact: true }).click();
  await expect(settings.getByRole("button", { name: "点击外部链接", exact: true })).toHaveText("在系统浏览器打开");
  await page.keyboard.press("Escape");
  await page.getByRole("article", { name: "甲文", exact: true }).click();
  const reader = page.locator(".drawer.reader");
  await reader.getByRole("link", { name: "apple.com", exact: true }).click();
  const warning = page.getByRole("dialog", { name: "链接安全提示" });
  await expect(warning).toContainText("аpple.com");
  await expect(warning).toContainText("xn--pple-43d.com");
  await expect(warning).toContainText("apple.com");
  const calls = () => page.evaluate(() => (window as any).__mockHost.calls as string[]);
  expect(await calls()).not.toContain("open_web_link");
  expect(await calls()).not.toContain("plugin:opener|open_url");
  await shot(page, "test-results/web-reading/safety.png");
  await warning.getByRole("button", { name: "不打开", exact: true }).click();
  await expect(reader.locator(".reader-title")).toHaveText("甲文");
  await reader.getByRole("link", { name: "apple.com", exact: true }).click();
  await warning.getByRole("button", { name: "仍然打开", exact: true }).click();
  await expect.poll(calls).toContain("plugin:opener|open_url");
  expect(await calls()).not.toContain("open_web_link");
  await expect(reader.locator(".reader-title")).toHaveText("甲文");
  await reader.getByRole("button", { name: "返回", exact: true }).click();
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await settings.getByRole("button", { name: "重新显示首次提示", exact: true }).click();
  await expect(settings.getByRole("status")).toContainText("下次打开阅读视图或原网页");
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.args.set_setting)).toEqual({ id: "web_original_tip", value: "1" });
  const js = settings.getByRole("switch", { name: "允许 JavaScript", exact: true });
  await js.click();
  await expect(js).toHaveAttribute("aria-checked", "false");
  await settings.getByRole("button", { name: "立即清空", exact: true }).click();
  const clear = page.getByRole("alertdialog", { name: "立即清空原网页数据？", exact: true });
  expect(await calls()).not.toContain("clear_web_data");
  await clear.getByRole("button", { name: "取消", exact: true }).click();
  await page.evaluate(() => { (window as any).__mockHost.clearWebDataError = "模拟清理失败"; });
  await settings.getByRole("button", { name: "立即清空", exact: true }).click();
  await clear.getByRole("button", { name: "立即清空", exact: true }).click();
  await expect(settings.getByRole("status")).toContainText("模拟清理失败");
  await page.evaluate(() => { (window as any).__mockHost.clearWebDataError = ""; });
  await settings.getByRole("button", { name: "立即清空", exact: true }).click();
  await clear.getByRole("button", { name: "立即清空", exact: true }).click();
  await expect(settings.getByRole("status")).toContainText("原网页数据已清空");
  await shot(page, "test-results/web-reading/settings.png");

});

test("引用链接添加整篇 chip；抽取失败只通知，不换篇或添加引用", async ({ page }) => {
  const good = "https://web.example.test/good", bad = "https://web.example.test/bad";
  await installMockHost(page, {
    focus: TWO_ARTICLE_PAGE,
    messages: { 7: [{ ...MOCK_MESSAGES[7][1], document: { v: 1, blocks: [good, bad].map((href, i) => ({ t: "p", inlines: [{ t: "a", href, children: [{ t: "t", text: `引用目标 ${i}` }] }] })) } }] },
    readers: {
      10: readerView(10, { title: "引用目标文章", url: good }),
    },
    webLinks: {
      [good]: { article_id: 10, temporary: true, actual_host: "web.example.test", actual_url: good, layer: "General", text_chars: 300, image_count: 0 },
      [bad]: { kind: "Waf", message: "challenge" },
    },
  });
  await page.goto("/");
  await page.getByRole("button", { name: MOCK_THREADS[0].title, exact: true }).click();
  const reader = page.locator(".drawer.reader");
  for (const i of [0, 1]) {
    await page.locator(".bubble.assistant").getByRole("link", { name: `引用目标 ${i}`, exact: true }).click({ button: "right" });
    const menu = page.getByRole("menu", { name: "链接操作" });
    await expect(menu.getByRole("menuitem")).toHaveCount(5);
    await expect(menu.getByRole("menuitem", { name: "打开原网页", exact: true })).toBeEnabled();
    await menu.getByRole("menuitem", { name: "引用链接", exact: true }).click();
    if (i === 0) await expect(page.getByRole("region", { name: "引用托盘" })).toContainText("引用目标文章");
    else await expect(page.locator(".toast")).toContainText("引用链接失败：站点有访问防护");
    await expect(reader).toHaveCount(0);
    await expect(page.locator(".reader-citation-chip")).toHaveCount(1);
    await expect(page.getByRole("textbox", { name: "针对引用提问" })).toHaveValue("");
  }
  const args = await page.evaluate(() => (window as any).__mockHost.args);
  expect(args.cite_article).toEqual({ id: 10 });
  expect(args.open_article).toBeUndefined();
  expect(args.link_safety).toEqual({ href: bad, linkText: "引用目标 1" });
});


test("顶栏原网页首次确认后嵌入阅读区", async ({ page }) => {
  const url = "https://web.example.test/story";
  await installMockHost(page, {
    focus: TWO_ARTICLE_PAGE, settingValues: { web_first_tip: "0" },
    readers: { 1: readerView(1, { document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "a", href: url, children: [{ t: "t", text: "测试网页" }] }] }] } }), 10: readerView(10, { url }) },
    webLinks: { [url]: { article_id: 10, temporary: true, actual_host: "web.example.test", actual_url: url, layer: "General", text_chars: 300, image_count: 0 } },
  });
  await page.goto("/");
  await page.getByRole("article", { name: "甲文", exact: true }).click();
  await page.getByRole("link", { name: "测试网页", exact: true }).click();
  await page.locator(".drawer.reader").getByRole("button", { name: "原网页", exact: true }).click();
  const scene = page.getByRole("dialog", { name: "原网页", exact: true });
  await expect(scene.getByRole("note")).toContainText("不要在里面输入密码、验证码或支付信息");
  expect(await page.evaluate(() => (window as any).__mockHost.calls)).not.toContain("open_web_view");
  await shot(page, "test-results/web-reading/original-tip.png");
  await scene.getByRole("checkbox", { name: "不再显示" }).check();
  await scene.getByRole("button", { name: "打开原网页", exact: true }).click();
  await expect(scene.locator(".web-embed-slot")).toBeVisible();
  await expect(scene.getByRole("region", { name: "原网页伴随卡片" })).toHaveCount(0);
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.calls.includes("open_web_view"))).toBe(true);
});

test("打开原网页后验证事件不关掉页面", async ({ page }) => {
  const url = "https://web.example.test/verified";
  const result = { label: "web-1", url, state: "Extracted" as const, title: "过盾后正文", article_id: 10, text_chars: 1842, image_count: 3, reason: null };
  await installMockHost(page, {
    focus: TWO_ARTICLE_PAGE, settingValues: { web_original_tip: "0", web_first_tip: "0" },
    readers: { 1: readerView(1, { document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "a", href: url, children: [{ t: "t", text: "受保护网页" }] }] }] } }), 10: readerView(10, { url, title: "过盾后正文" }) },
    webLinks: { [url]: { kind: "Waf", message: "HTTP 403" } },
    webExtractions: { [url]: result },
  });
  await page.goto("/");
  await page.getByRole("article", { name: "甲文", exact: true }).click();
  await page.getByRole("link", { name: "受保护网页", exact: true }).click();
  await page.getByRole("button", { name: "打开原网页通过验证", exact: true }).click();
  const scene = page.getByRole("dialog", { name: "原网页", exact: true });
  await expect(scene.locator(".web-embed-slot")).toBeVisible();
  await page.evaluate(result => (window as any).__mockHost.emit("web:verify-state", { ...result, state: "WaitingVerification", title: "Just a moment...", article_id: null }), result);
  await expect(scene.getByLabel("原网页抽取进度")).toContainText("等待验证");
  await page.evaluate(result => (window as any).__mockHost.emit("web:verify-state", result), result);
  await expect(scene.getByLabel("原网页抽取进度")).toContainText("1,842 字");
  await expect(scene.locator(".web-embed-slot")).toBeVisible();
  expect(await page.evaluate(() => Object.keys((window as any).__mockHost.webWindows))).toEqual(["web-1"]);
  expect(await page.evaluate(() => (window as any).__mockHost.calls.filter((call: string) => call === "close_web_view"))).toEqual([]);
  await scene.getByRole("button", { name: "切到阅读视图", exact: true }).click();
  await expect(scene).toHaveCount(0);
  await expect(page.locator(".reader-title")).toHaveText("过盾后正文");
});

test("原网页嵌在阅读区：菜单盖住时收起，Esc 回阅读视图，切回不重建", async ({ page }) => {
  const url = "https://web.example.test/story";
  await installMockHost(page, {
    focus: TWO_ARTICLE_PAGE, settingValues: { web_open_mode: "web", web_original_tip: "0", web_first_tip: "0" },
    readers: { 1: readerView(1, { document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "a", href: url, children: [{ t: "t", text: "测试网页" }] }] }] } }), 10: readerView(10, { url, title: "网页正文" }) },
    webLinks: { [url]: { article_id: 10, temporary: true, actual_host: "web.example.test", actual_url: url, layer: "General", text_chars: 300, image_count: 0 } },
  });
  const host = () => page.evaluate(() => {
    const mock = (window as any).__mockHost;
    return { opens: mock.calls.filter((call: string) => call === "open_web_view").length, bounds: mock.webViewBounds[0], visible: mock.webViewVisible.at(-1)?.visible };
  });
  await page.goto("/");
  await page.getByRole("article", { name: "甲文", exact: true }).click();
  await page.getByRole("link", { name: "测试网页", exact: true }).click();
  const scene = page.getByRole("dialog", { name: "原网页", exact: true });
  await expect(scene.locator(".web-embed-slot")).toBeVisible();
  await expect.poll(async () => (await host()).visible).toBe(true);
  const { bounds } = await host();
  expect(bounds.width).toBeGreaterThan(0);
  expect(bounds.height).toBeGreaterThan(0);

  await scene.getByRole("button", { name: "更多", exact: true }).click();
  const menu = page.getByRole("menu", { name: "原网页操作", exact: true });
  await expect(menu.getByRole("menuitem", { name: "关闭原网页", exact: true })).toBeVisible();
  await expect.poll(async () => (await host()).visible).toBe(false);
  await expect(scene.getByRole("status").filter({ hasText: "网页暂时收起" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(menu).toHaveCount(0);
  await expect.poll(async () => (await host()).visible).toBe(true);

  await page.evaluate(() => (window as any).__mockHost.emit("web:escape", { label: "web-1" }));
  await expect(scene).toHaveCount(0);
  await expect(page.locator(".reader-title")).toHaveText("网页正文");
  await expect.poll(async () => (await host()).visible).toBe(false);

  await page.locator(".drawer.reader").getByRole("button", { name: "原网页", exact: true }).click();
  await expect(scene.locator(".web-embed-slot")).toBeVisible();
  await expect.poll(async () => (await host()).visible).toBe(true);
  expect((await host()).opens).toBe(1);
});

test("代理拒绝可选阅读视图、浏览器或代理设置；拦截下载可交系统浏览器", async ({ page }) => {
  const url = "http://web.example.test/story";
  await installMockHost(page, {
    focus: TWO_ARTICLE_PAGE, settingValues: { web_open_mode: "web", web_original_tip: "0" },
    linkSafety: { [url]: { actual_host: "web.example.test", warnings: [{ kind: "NonHttps" }] } },
    webWindowError: "当前代理包含认证，原网页无法表达；请改用阅读视图或系统浏览器。",
    readers: { 1: readerView(1, { document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "a", href: url, children: [{ t: "t", text: "测试网页" }] }] }] } }) },
  });
  await page.goto("/");
  await page.getByRole("article", { name: "甲文", exact: true }).click();
  await page.getByRole("link", { name: "测试网页", exact: true }).click();
  const safety = page.getByRole("dialog", { name: "链接安全提示" });
  await expect(safety).toContainText("这个网页没有使用 HTTPS");
  expect(await page.evaluate(() => (window as any).__mockHost.calls)).not.toContain("open_web_view");
  await safety.getByRole("button", { name: "继续打开", exact: true }).click();
  const scene = page.getByRole("dialog", { name: "原网页", exact: true });
  await expect(scene.getByRole("alert")).toContainText("当前代理包含认证");
  await expect(scene.getByRole("button", { name: "改用阅读视图", exact: true })).toBeEnabled();
  await expect(scene.getByRole("button", { name: "在系统浏览器打开", exact: true }).last()).toBeEnabled();
  await scene.getByRole("button", { name: "查看代理设置", exact: true }).focus();
  await page.keyboard.press("Tab");
  await expect(scene.getByRole("button", { name: "返回", exact: true })).toBeFocused();
  await shot(page, "test-results/web-reading/proxy-refused.png");
  await scene.getByRole("button", { name: "查看代理设置", exact: true }).click();
  await expect(settingsTab(page.getByRole("dialog", { name: "设置", exact: true }), "advanced")).toHaveAttribute("aria-current", "page");
  await page.keyboard.press("Escape");
  await page.evaluate(() => { (window as any).__mockHost.webWindowError = ""; });
  await page.getByRole("link", { name: "测试网页", exact: true }).click();
  await safety.getByRole("button", { name: "继续打开", exact: true }).click();
  await expect(scene.locator(".web-embed-slot")).toBeVisible();
  await page.evaluate(() => (window as any).__mockHost.emit("web:download-blocked", { label: "web-1", url: "https://web.example.test/file.zip" }));
  const toast = page.locator(".toast");
  await expect(toast).toContainText("已拦截下载，可交给系统浏览器下载");
  await toast.getByRole("button", { name: "系统浏览器", exact: true }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.args["plugin:opener|open_url"])).toEqual({ url: "https://web.example.test/file.zip" });
});


test("原网页按当前地址回到阅读视图，窗口仍留着", async ({ page }) => {
  const url = "https://web.example.test/story", current = "https://web.example.test/current";
  await installMockHost(page, {
    focus: TWO_ARTICLE_PAGE, settingValues: { web_open_mode: "web", web_original_tip: "0", web_first_tip: "0" },
    readers: { 1: readerView(1, { document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "a", href: url, children: [{ t: "t", text: "测试网页" }] }] }] } }), 10: readerView(10, { url: current, title: "导航后正文" }) },
    webLinks: { [current]: { article_id: 10, temporary: true, actual_host: "web.example.test", actual_url: current, layer: "General", text_chars: 300, image_count: 0 } },
  });
  await page.goto("/");
  await page.getByRole("article", { name: "甲文", exact: true }).click();
  await page.getByRole("link", { name: "测试网页", exact: true }).click();
  const scene = page.getByRole("dialog", { name: "原网页", exact: true });
  await expect(scene.locator(".web-embed-slot")).toBeVisible();
  await page.evaluate(next => (window as any).__mockHost.emit("web:navigated", { label: "web-1", url: next, title: "后台导航" }), current);
  await scene.getByRole("button", { name: "阅读视图", exact: true }).click();
  await expect(scene).toHaveCount(0);
  await expect(page.locator(".reader-title")).toHaveText("导航后正文");
  expect(await page.evaluate(() => (window as any).__mockHost.args.open_web_link)).toEqual({ url: current });
  expect(await page.evaluate(() => Object.keys((window as any).__mockHost.webWindows))).toEqual(["web-1"]);
  expect(await page.evaluate(() => (window as any).__mockHost.calls.filter((call: string) => call === "close_web_view"))).toEqual([]);
});

test("网页阅读顶栏可翻译、可摘要，正文栏宽与普通文章相同", async ({ page }) => {
  const url = "https://web.example.test/measure";
  await installMockHost(page, {
    focus: TWO_ARTICLE_PAGE,
    settingValues: { web_first_tip: "0" },
    readers: {
      1: readerView(1, { document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "a", href: url, children: [{ t: "t", text: "量一量" }] }] }] } }),
      10: readerView(10, { title: "网页文章", url }),
    },
    webLinks: {
      [url]: { article_id: 10, temporary: true, actual_host: "web.example.test", actual_url: url, layer: "General", text_chars: 300, image_count: 0 },
    },
  });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  await page.getByRole("article", { name: "甲文", exact: true }).click();
  const reader = page.locator(".drawer.reader");
  const columnWidth = () => reader.locator(".reader-column").evaluate((el) => getComputedStyle(el).width);
  const articleWidth = await columnWidth();
  await reader.getByRole("link", { name: "量一量", exact: true }).click();
  await expect(reader.locator(".reader-title")).toHaveText("网页文章");
  const head = reader.locator(".reader-head");
  await expect(head).toHaveCSS("height", "48px");
  await expect(head.locator(".reader-scroll-line")).toHaveCount(1);
  await expect(head.locator(".web-label-long").first()).toHaveText("回到「甲文」");
  const translate = head.getByRole("button", { name: "翻译", exact: true });
  const summary = head.getByRole("button", { name: "生成本篇摘要", exact: true });
  await expect(translate).toBeEnabled();
  await expect(summary).toBeEnabled();
  await expect(head.getByRole("button", { name: "上一篇", exact: true })).toHaveCount(0);
  await expect(head.getByRole("button", { name: "收藏", exact: true })).toHaveCount(0);
  await expect(head.getByRole("button", { name: "查看原文", exact: true })).toHaveCount(0);
  expect(await columnWidth()).toBe(articleWidth);
  await head.locator(".reader-tools > summary").click();
  await expect(reader.getByRole("button", { name: "引用全文到对话", exact: true })).toBeVisible();
  await page.keyboard.press("Escape");
  await translate.click();
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.calls.includes("translate_article"))).toBe(true);
  expect(await page.evaluate(() => (window as any).__mockHost.args.translate_article.id)).toBe(10);
  await page.setViewportSize({ width: 560, height: 800 });
  await expect(head).toHaveCSS("height", "48px");
  await expect(head.locator(".web-label-long").first()).toBeHidden();
  await expect(head.locator(".web-label-short")).toBeVisible();
});
