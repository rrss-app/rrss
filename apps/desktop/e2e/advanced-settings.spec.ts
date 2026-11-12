import { expect, test, type Page } from "./fixtures";
import { installMockHost, settingsTab } from "./mockHost";
import { networkProxyFixture } from "../src/test/fixtures/settings";
import { shot } from "./shots";

const OUT = "../../test-results/advanced-settings";

async function openSettings(page: Page) {
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  return page.getByRole("dialog", { name: "设置", exact: true });
}

test("basic pages link to their advanced group; the advanced page fits 1280 and 560 in both themes", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  const settings = await openSettings(page);
  await settingsTab(settings, "model").click();
  await expect(settings.locator(".settings-fold-head", { hasText: "地址、协议" })).toHaveCount(0);
  await settings.getByRole("button", { name: "此页的高级设置 →" }).click();
  await expect(settings.getByRole("heading", { name: "高级", exact: true })).toBeVisible();
  await expect(settings.locator("#advanced-model")).toBeInViewport();

  await settingsTab(settings, "assistant").click();
  await expect(settings.getByText("路径与开发者工具", { exact: false })).toHaveCount(0);
  await settings.getByRole("button", { name: "此页的高级设置 →" }).click();
  await expect(settings.locator("#advanced-assistant")).toBeInViewport();

  for (const size of [{ width: 1280, height: 800 }, { width: 560, height: 640 }]) {
    await page.setViewportSize(size);
    for (const theme of ["light", "dark"]) {
      await page.evaluate((t) => { document.documentElement.dataset.theme = t; }, theme);
      expect(await settings.locator(".drawer-body").evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
      await settings.locator(".drawer-body").evaluate((el) => { el.scrollTop = 0; });
      await shot(page, `${OUT}/advanced-${size.width}-${theme}.png`);
      await settings.locator("#advanced-assistant").evaluate((el) => el.scrollIntoView({ block: "start" }));
      await shot(page, `${OUT}/advanced-engine-${size.width}-${theme}.png`);
    }
  }
});

test("proxy drafts save, keep failed drafts, warn on SOCKS for Pi, and guard leaving", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  const nav = settings.getByRole("navigation", { name: "设置分类" });
  await settingsTab(nav, "advanced").click();
  const group = settings.locator("#advanced-network-proxy");

  // 保存：全局切换为自定义代理，带认证与 no_proxy；用途覆盖 Pi 单独走 SOCKS。
  await group.getByRole("button", { name: "全局代理模式" }).click();
  await page.getByRole("menu", { name: "全局代理模式" }).getByRole("menuitemradio", { name: "自定义代理" }).click();
  await group.getByRole("textbox", { name: "全局代理地址" }).fill("http://proxy.example:8080");
  await group.getByRole("textbox", { name: "全局不代理地址" }).fill("internal.test");
  await group.getByRole("group", { name: "全局代理认证" }).getByRole("button", { name: "添加认证" }).click();
  await group.getByRole("textbox", { name: "全局代理用户名" }).fill("user");
  await group.getByLabel("全局代理密码", { exact: true }).fill("secret");
  await group.getByRole("button", { name: "按用途单独设置" }).click();
  await group.getByRole("button", { name: "Pi代理模式", exact: true }).click();
  await page.getByRole("menu", { name: "Pi代理模式", exact: true }).getByRole("menuitemradio", { name: "自定义代理" }).click();
  const piUrl = group.getByRole("textbox", { name: "Pi代理地址" });
  await piUrl.fill("socks5h://proxy.example:1080");
  await expect(group.getByText("对话能否走 SOCKS 代理")).toBeVisible();
  await group.getByRole("button", { name: "保存", exact: true }).click();
  await expect(group.getByText(/新的请求马上生效/)).toBeVisible();
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.args.save_network_proxy)).toMatchObject({
    args: {
      global: { mode: "custom", url: "http://proxy.example:8080", no_proxy: "internal.test", username: "user", password: "secret", clear_auth: false },
      pi: { mode: "custom", url: "socks5h://proxy.example:1080" },
    },
  });
  // 保存成功后视图脱敏：地址不再带认证，has_auth 只在状态里。
  await expect(group.getByLabel("全局代理密码", { exact: true })).toHaveCount(0);
  await expect(group.getByText("已设置，不回显")).toBeVisible();

  // 失败保存：草稿保留，可以重试。
  await page.evaluate(() => {
    const host = (window as any).__TAURI_INTERNALS__;
    const original = host.invoke;
    (window as any).__originalInvoke = original;
    host.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      if (command === "save_network_proxy") throw new Error("模拟代理保存失败");
      return original(command, args);
    };
  });
  await group.getByRole("textbox", { name: "全局代理地址" }).fill("http://proxy2.example:8080");
  await group.getByRole("button", { name: "保存", exact: true }).click();
  await expect(group.getByRole("alert")).toContainText("保存网络代理失败");
  await expect(group.getByRole("textbox", { name: "全局代理地址" })).toHaveValue("http://proxy2.example:8080");
  await page.evaluate(() => {
    (window as any).__TAURI_INTERNALS__.invoke = (window as any).__originalInvoke;
  });

  // 离开守卫：未保存的修改要先确认。
  const confirm = page.getByRole("alertdialog", { name: "网络代理有未保存的修改" });
  await settingsTab(nav, "rules").click();
  await expect(confirm).toBeVisible();
  await confirm.getByRole("button", { name: "继续编辑" }).click();
  await expect(group.getByRole("textbox", { name: "全局代理地址" })).toHaveValue("http://proxy2.example:8080");
  await settingsTab(nav, "rules").click();
  await expect(confirm).toBeVisible();
  await confirm.getByRole("button", { name: "保存并离开" }).click();
  await expect(settings.getByRole("heading", { name: "规则", exact: true, level: 3 })).toBeVisible();
  await settingsTab(nav, "advanced").click();
  // 保存成功后按宿主规范化后的地址回填（http 会补上结尾斜杠）。
  await expect(group.getByRole("textbox", { name: "全局代理地址" })).toHaveValue("http://proxy2.example:8080/");

  // Pi 跟随全局时按实际生效的规则提示 SOCKS；撤销后提示随草稿消失。
  await expect(group.getByText("对话能否走 SOCKS 代理")).toBeVisible();
  await group.getByRole("button", { name: "按用途单独设置" }).click();
  await group.getByRole("button", { name: "Pi代理模式", exact: true }).click();
  await page.getByRole("menu", { name: "Pi代理模式", exact: true }).getByRole("menuitemradio", { name: "跟随全局" }).click();
  await expect(group.getByText("对话能否走 SOCKS 代理")).toHaveCount(0);
  await group.getByRole("textbox", { name: "全局代理地址" }).fill("socks5h://proxy.example:1080");
  await expect(group.getByText("对话能否走 SOCKS 代理")).toBeVisible();
  await group.getByRole("button", { name: "撤销", exact: true }).click();
  await expect(group.getByRole("textbox", { name: "全局代理地址" })).toHaveValue("http://proxy2.example:8080/");

  // 亮暗与窄屏：展开用途覆盖后，代理分组不得横向溢出。
  for (const size of [{ width: 1280, height: 800 }, { width: 560, height: 640 }]) {
    await page.setViewportSize(size);
    for (const theme of ["light", "dark"]) {
      await page.evaluate((t) => { document.documentElement.dataset.theme = t; }, theme);
      if (await group.getByRole("button", { name: "按用途单独设置" }).getAttribute("aria-expanded") === "false") {
        await group.getByRole("button", { name: "按用途单独设置" }).click();
      }
      await expect(group.getByRole("textbox", { name: "Pi代理地址" })).toBeVisible();
      expect(await settings.locator(".drawer-body").evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
      await settings.locator("#advanced-network-proxy").evaluate((el) => { el.scrollIntoView({ block: "start" }); el.scrollTop = 0; });
      await shot(page, `${OUT}/proxy-${size.width}-${theme}.png`);
    }
  }
});

test("system mode shows the detected proxy and the connection check reports each use", async ({ page }) => {
  await installMockHost(page, { networkProxy: networkProxyFixture({
    system: { http: "http://127.0.0.1:7890/", https: null, all: null, no_proxy: "corp.example", source: "Windows Internet 设置", pac: "已配置（未解析）" },
  }) });
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  await settingsTab(settings.getByRole("navigation", { name: "设置分类" }), "advanced").click();
  const group = settings.locator("#advanced-network-proxy");

  await group.getByRole("button", { name: "全局代理模式" }).click();
  await page.getByRole("menu", { name: "全局代理模式" }).getByRole("menuitemradio", { name: "系统代理" }).click();
  const detected = group.getByRole("group", { name: "检测到的系统代理" });
  await expect(detected).toContainText("Windows Internet 设置");
  await expect(detected).toContainText("http://127.0.0.1:7890/");
  await expect(detected).toContainText("PAC");

  // 内容网络单独走一个连不上的代理：只有这一行失败，其余跟随全局的照常。
  await group.getByRole("button", { name: "按用途单独设置" }).click();
  await group.getByRole("button", { name: "内容网络代理模式", exact: true }).click();
  await page.getByRole("menu", { name: "内容网络代理模式", exact: true }).getByRole("menuitemradio", { name: "自定义代理" }).click();
  await group.getByRole("textbox", { name: "内容网络代理地址" }).fill("http://unreachable.example:8080");
  await group.getByRole("button", { name: "测试连接" }).click();
  await expect(group.locator('[data-proxy-check="content"]')).toHaveAttribute("data-state", "failed");
  await expect(group.locator('[data-proxy-check="api"]')).toHaveAttribute("data-state", "ok");
  await expect(group.locator('[data-proxy-check="pi"]')).toHaveAttribute("data-state", "ok");
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.args.save_network_proxy)).toBeUndefined();
  for (const size of [{ width: 1280, height: 800 }, { width: 560, height: 640 }]) {
    await page.setViewportSize(size);
    expect(await settings.locator(".drawer-body").evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
    await group.locator(".proxy-check").evaluate((el) => el.scrollIntoView({ block: "center" }));
    await shot(page, `${OUT}/proxy-check-${size.width}.png`);
  }
});

test("path drafts are guarded, the generated config marks ignored keys, and own config is confirmed first", async ({ page }) => {
  const settings = await openSettings(page);
  await settingsTab(settings, "advanced").click();
  const engine = settings.locator("#advanced-assistant");

  await engine.getByRole("button", { name: "查看", exact: true }).click();
  const generated = page.getByRole("dialog", { name: "生成的配置" });
  await expect(generated).toContainText("已忽略：settings.cacheWarming");
  await shot(page, `${OUT}/generated-config-1280-light.png`);
  await page.keyboard.press("Escape");
  await expect(generated).toHaveCount(0);

  await engine.getByRole("button", { name: "使用我自己的 Pi 配置" }).click();
  const confirm = page.getByRole("alertdialog", { name: "使用我自己的 Pi 配置" });
  await confirm.getByRole("button", { name: "改用我的配置" }).click();
  await expect(engine.getByRole("button", { name: "使用我自己的 Pi 配置" })).toHaveAttribute("aria-pressed", "true");
  // 对话助手页的对话模型随之改为从 Pi 的模型里选。
  await settingsTab(settings, "assistant").click();
  await settings.getByRole("button", { name: "对话模型", exact: true }).click();
  const menu = page.getByRole("dialog", { name: "对话模型" });
  await expect(menu).toContainText("模型列表不用于对话");
  await menu.getByRole("button", { name: "读取可用模型" }).click();
  await menu.getByRole("option", { name: "ollama/qwen2.5:14b" }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.args.save_pi_config)).toEqual({ args: { own_model: "ollama/qwen2.5:14b" } });

  await settingsTab(settings, "advanced").click();
  await settings.getByRole("textbox", { name: "Pi 可执行文件路径" }).fill("/draft/pi");
  await settingsTab(settings, "rules").click();
  await expect(page.getByRole("alertdialog", { name: "对话引擎路径有未保存的修改" })).toBeVisible();
});
