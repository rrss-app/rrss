import { expect, test } from "./fixtures";
import { installMockHost, settingsTab } from "./mockHost";
import { shot } from "./shots";

test("settings contains keyboard focus and restores nested search and confirmation", async ({ page }) => {
  await page.goto("/");
  const opener = page.getByRole("button", { name: "设置", exact: true });
  await opener.click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  const back = settings.getByRole("button", { name: "返回阅读", exact: true });
  await expect(back).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  expect(await settings.evaluate(el => el.contains(document.activeElement))).toBe(true);
  await page.keyboard.press("Tab");
  await expect(back).toBeFocused();

  await page.keyboard.press("Control+k");
  const search = page.getByRole("dialog", { name: "搜索文章" });
  await expect(search.getByRole("searchbox")).toBeFocused();
  // Tab 在搜索浮层内循环（输入框、筛选），不回到被压住的设置。
  await page.keyboard.press("Tab");
  expect(await search.evaluate(el => el.contains(document.activeElement))).toBe(true);
  await page.keyboard.press("Shift+Tab");
  await expect(search.getByRole("searchbox")).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(search).toHaveCount(0);
  await expect(back).toBeFocused();

  await settings.locator(".list-row").filter({ hasText: "闲聊广场" }).getByRole("button", { name: "编辑" }).click();
  await expect(settings.getByLabel("订阅名称", { exact: true })).toBeVisible();
  const remove = settings.getByRole("button", { name: "删除", exact: true });
  await remove.click();
  const confirm = page.getByRole("alertdialog", { name: "删除订阅" });
  await expect(confirm.getByRole("button", { name: "取消" })).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(confirm.getByRole("button", { name: "确认" })).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(confirm).toHaveCount(0);
  await expect(remove).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(settings).toHaveCount(0);
  await expect(opener).toBeFocused();
});

test("search results receive actual mouse clicks above the dismiss backdrop", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "搜索", exact: true }).click();
  const search = page.getByRole("dialog", { name: "搜索文章" });
  await search.getByRole("searchbox").fill("旧平板");
  const result = search.getByRole("option").first();
  await expect(result).toBeVisible();
  await result.click();
  await expect(page.locator(".reader-stage")).toBeVisible();
  await expect(search).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(page.locator(".reader-stage")).toHaveCount(0);
});

test("settings remains readable at narrow widths and resets scroll between categories", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  const navigation = settings.getByRole("navigation", { name: "设置分类" });
  for (const width of [1280, 880, 560]) {
    await page.setViewportSize({ width, height: 640 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme);
      await settingsTab(navigation, "subscribe").click();
      await expect(settings.getByLabel("订阅地址", { exact: true })).toBeVisible();
      expect(await settings.locator(".drawer-body").evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
      await shot(page, `../../test-results/settings-polish/preview-subscribe-${width}-${theme}.png`);
    }
  }
  await settings.locator(".drawer-body").evaluate(el => { el.scrollTop = el.scrollHeight; });
  await expect(settingsTab(navigation, "model")).toBeInViewport();
  await settingsTab(navigation, "model").click();
  await expect(settings.getByRole("heading", { name: "模型", exact: true, level: 3 })).toBeInViewport();
  expect(await settings.locator(".drawer-body").evaluate(el => el.scrollTop)).toBe(0);
});

// 离开守卫的全部出口（分类、返回、Esc、浏览器后退）由 translate-settings 覆盖；这里只测接入与本页差异。
test("F4.4 blank threshold is a guarded draft and discarding restores the saved value", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  const nav = settings.getByRole("navigation", { name: "设置分类" });
  await settingsTab(nav, "rules").click();
  const threshold = settings.getByRole("spinbutton", { name: "隐藏阈值", exact: true });
  await expect(threshold).toHaveValue("-10");
  await threshold.fill("");
  const confirm = page.getByRole("alertdialog", { name: "规则设置有未保存的修改" });
  await settingsTab(nav, "subscribe").click();
  await expect(confirm).toBeVisible();
  await confirm.getByRole("button", { name: "继续编辑" }).click();
  await expect(threshold).toHaveValue("");
  await expect(settings.getByText("请填写有效的隐藏阈值，不能为空。", { exact: true })).toBeVisible();
  await settings.getByRole("button", { name: "返回阅读", exact: true }).click();
  await expect(confirm).toBeVisible();
  await shot(page, "../../test-results/f4-4-rules/threshold-dirty-leave.png");
  await confirm.getByRole("button", { name: /^不保存/ }).click();
  await expect(settings).toHaveCount(0);
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await expect(threshold).toHaveValue("-10");
  await page.keyboard.press("Escape");
  await expect(settings).toHaveCount(0);
});

test("F4.4 retains saved threshold after a failed reread and retries only the read", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  // 本用例仅在统一 IPC 入口模拟阈值写入与规则回读，不扩展共享宿主夹具。
  await page.evaluate(() => {
    const host = (window as any).__TAURI_INTERNALS__;
    const original = host.invoke;
    const state = { threshold: -10, writes: 0, reads: 0, readFail: false };
    (window as any).__rulesTest = state;
    host.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      if (command === "set_setting" && args.id === "hide_threshold") {
        state.writes++; state.threshold = Number(args.value); state.readFail = true;
        return { id: args.id, value: args.value };
      }
      if (command === "rules_page") {
        state.reads++;
        if (state.readFail) throw new Error("模拟规则回读失败");
        return { ...await original(command, args), hide_threshold: state.threshold };
      }
      return original(command, args);
    };
  });
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  const nav = settings.getByRole("navigation", { name: "设置分类" });
  await settingsTab(nav, "rules").click();
  const threshold = settings.getByRole("spinbutton", { name: "隐藏阈值", exact: true });
  await expect(threshold).toHaveValue("-10");
  await page.evaluate(() => { (window as any).__rulesTest.reads = 0; });
  await threshold.fill("-25");
  await threshold.press("Tab");
  await expect(settings.getByText(/刚才的修改已保存，重新读取即可/)).toBeVisible();
  await expect(threshold).toHaveValue("-25");
  expect(await page.evaluate(() => (window as any).__rulesTest)).toEqual({ threshold: -25, writes: 1, reads: 1, readFail: true });
  await page.setViewportSize({ width: 880, height: 900 });
  await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
  await settings.locator(".drawer-body").evaluate(el => { el.scrollTop = 0; });
  await shot(page, "../../test-results/f4-4-rules/saved-reread-failed-dark.png");
  await page.evaluate(() => { (window as any).__rulesTest.readFail = false; });
  await settings.getByRole("button", { name: "重试读取", exact: true }).click();
  await expect(settings.getByRole("alert")).toHaveCount(0);
  await expect(threshold).toHaveValue("-25");
  expect(await page.evaluate(() => (window as any).__rulesTest)).toEqual({ threshold: -25, writes: 1, reads: 2, readFail: false });
  await settingsTab(nav, "subscribe").click();
  await settingsTab(nav, "rules").click();
  await expect(threshold).toHaveValue("-25");
  await page.keyboard.press("Escape");
  await expect(settings).toHaveCount(0);
});

test("F4.5 memory and interest drafts are guarded and discarding drops the editor", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  const nav = settings.getByRole("navigation", { name: "设置分类" });
  await settingsTab(nav, "memory").click();
  await settings.getByRole("button", { name: "编辑", exact: true }).click();
  const content = settings.getByLabel("记忆内容", { exact: true });
  await content.fill("只保留本机的记忆草稿");
  const confirm = page.getByRole("alertdialog", { name: "记忆与兴趣有未保存的修改" });
  await settingsTab(nav, "diagnostics").click();
  await expect(confirm).toBeVisible();
  await confirm.getByRole("button", { name: "继续编辑" }).click();
  await expect(content).toHaveValue("只保留本机的记忆草稿");
  await settings.getByRole("button", { name: "返回阅读", exact: true }).click();
  await expect(confirm).toBeVisible();
  await shot(page, "../../test-results/f4-5-settings/memory-dirty-leave.png");
  await confirm.getByRole("button", { name: /^不保存/ }).click();
  await expect(settings).toHaveCount(0);
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await expect(content).toHaveCount(0);
  await settings.getByRole("group", { name: "记忆设置分类" }).getByRole("button", { name: /^兴趣/ }).click();
  await settings.getByLabel("新兴趣标签", { exact: true }).fill("离线阅读");
  await page.setViewportSize({ width: 880, height: 720 });
  await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
  expect(await settings.locator(".settings-body").evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
  await shot(page, "../../test-results/f4-5-settings/interest-draft-dark.png");
  await page.keyboard.press("Escape");
  await expect(confirm).toBeVisible();
  await confirm.getByRole("button", { name: /^不保存/ }).click();
});

test("F4.5 diagnostics blocks leaving during maintenance and retries only failed reads", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  // 仅本用例拦截诊断命令；不扩展共享模拟宿主，也不调用真实向量服务。
  await page.evaluate(() => {
    const host = (window as any).__TAURI_INTERNALS__;
    const original = host.invoke;
    const state = { calls: 0, readFail: false, release: (() => {}) as () => void };
    (window as any).__diagTest = state;
    host.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      if (command === "reset_failed_embeds") {
        state.calls++;
        await new Promise<void>(resolve => { state.release = resolve; });
        state.readFail = true;
        return null;
      }
      if (command === "diagnostics" && state.readFail) throw new Error("模拟诊断读取失败");
      return original(command, args);
    };
  });
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  const nav = settings.getByRole("navigation", { name: "设置分类" });
  await settingsTab(nav, "diagnostics").click();
  const reset = settings.getByRole("button", { name: "重置失败计数", exact: true });
  await reset.click();
  // 处理中不换文字，只转圈。
  await expect(reset).toHaveAttribute("aria-busy", "true");
  await expect(reset).toBeDisabled();
  await settingsTab(nav, "memory").click();
  await expect(page.locator(".toast")).toContainText("诊断维护");
  await settings.getByRole("button", { name: "返回阅读", exact: true }).click();
  await page.keyboard.press("Escape");
  await page.goBack();
  await expect(settings.getByRole("heading", { name: "本机状态与排查", exact: true })).toBeVisible();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  await page.evaluate(() => (window as any).__diagTest.release());
  await expect(settings.getByText(/维护已经执行，刷新状态即可/)).toBeVisible();
  await expect(settings.locator(".diag-list")).not.toContainText("未能读取");
  await shot(page, "../../test-results/f4-5-settings/diagnostics-read-failed.png");
  await page.evaluate(() => { (window as any).__diagTest.readFail = false; });
  await settings.getByRole("button", { name: "刷新状态", exact: true }).click();
  await expect(settings.getByRole("alert")).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__diagTest.calls)).toBe(1);
  await settingsTab(nav, "memory").click();
  await expect(settings.getByRole("heading", { name: "记忆", exact: true })).toBeVisible();
});

test("storage page scans only when opened, confirms cleanup and fits narrow widths", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  const nav = settings.getByRole("navigation", { name: "设置分类" });
  await expect(settings.getByLabel("订阅地址", { exact: true })).toBeVisible();
  await page.waitForTimeout(1200);
  const scans = () => page.evaluate(() => (window as any).__mockHost.calls.filter((c: string) => c === "storage_page").length);
  expect(await scans()).toBe(0);

  await settingsTab(nav, "storage").click();
  const vectors = settings.locator('[data-storage-item="vectors"]');
  await expect(vectors).toContainText("198 MB");
  // 开发模式 StrictMode 会挂载两次，第一次的读取被取消后重发；这里只要求打开前没有预载。
  expect(await scans()).toBeGreaterThanOrEqual(1);
  for (const width of [1280, 560]) {
    await page.setViewportSize({ width, height: 900 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme);
      expect(await settings.locator(".drawer-body").evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
      await shot(page, `../../test-results/storage/storage-${width}-${theme}.png`);
    }
  }
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.evaluate(() => { document.documentElement.dataset.theme = "light"; });

  const beforeClean = await scans();
  await settings.locator('[data-storage-item="ai_cache"]').getByRole("button", { name: "清空", exact: true }).click();
  const confirm = page.getByRole("alertdialog", { name: "清空 AI 缓存" });
  await expect(confirm).toContainText("可能产生费用");
  await shot(page, "../../test-results/storage/storage-confirm.png");
  await confirm.getByRole("button", { name: "确认", exact: true }).click();
  await expect(settings.getByRole("status").filter({ hasText: "已清理。" })).toBeVisible();
  const aiCache = settings.locator('[data-storage-item="ai_cache"]');
  await expect(aiCache.getByRole("button")).toHaveCount(0);
  await expect(aiCache.locator(".storage-size")).toHaveText("0");
  expect(await scans()).toBe(beforeClean + 1);

  const logs = settings.locator('[data-storage-item="recommendation_logs"]');
  await expect(logs).toContainText("18,000 条");
  await expect(logs.locator(".storage-size")).toHaveText("3.0 MB");
  await settings.locator(".storage-legend").getByRole("button", { name: "推荐日志", exact: true }).hover();
  await expect(settings.locator(".storage-readout")).toContainText(/推荐日志\s*3\.0 MB/);
  const cleans = () => page.evaluate(() => (window as any).__mockHost.calls.filter((c: string) => c === "storage_clean").length);
  const beforeLogs = await cleans();
  await logs.getByRole("button", { name: "清空", exact: true }).click();
  const logsConfirm = page.getByRole("alertdialog", { name: "清空推荐日志" });
  await expect(logsConfirm).toContainText("曝光、行为与阅读会话日志");
  await expect(logsConfirm).toContainText("文章、赞踩、收藏与阅读状态不受影响");
  await logsConfirm.getByRole("button", { name: "取消", exact: true }).click();
  expect(await cleans()).toBe(beforeLogs);
  await expect(logs.locator(".storage-size")).toHaveText("3.0 MB");
  const articles = await settings.locator('[data-storage-item="articles"]').textContent();
  const other = await settings.locator('[data-storage-item="other"]').textContent();
  await logs.getByRole("button", { name: "清空", exact: true }).click();
  await logsConfirm.getByRole("button", { name: "确认", exact: true }).click();
  await expect(logs).toContainText("0 条");
  await expect(logs.locator(".storage-size")).toHaveText("0");
  await expect(logs.getByRole("button")).toHaveCount(0);
  expect(await cleans()).toBe(beforeLogs + 1);
  expect(await scans()).toBe(beforeClean + 2);
  expect(await page.evaluate(() => (window as any).__mockHost.args.storage_clean)).toEqual({ action: "recommendation_logs" });
  await expect(settings.locator('[data-storage-item="articles"]')).toHaveText(articles!);
  await expect(settings.locator('[data-storage-item="other"]')).toHaveText(other!);
  await expect(settings.locator(".storage-legend").getByRole("button", { name: "推荐日志", exact: true })).toHaveCount(0);
});

test("storage composition highlights one part at a time and the data location can be revealed", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  // 首次进入：保留 / 备份不先于占用统计出现（否则统计回来后整片被推下去）。
  await page.evaluate(() => {
    const w = window as any;
    w.__storageOrder = [];
    new MutationObserver(() => {
      for (const sel of [".storage-total", ".retention-card"]) if (document.querySelector(sel) && !w.__storageOrder.includes(sel)) w.__storageOrder.push(sel);
    }).observe(document.body, { childList: true, subtree: true });
  });
  await settingsTab(settings, "storage").click();
  const wrap = settings.locator(".storage-composition-wrap");
  const readout = settings.locator(".storage-readout");
  await expect(readout).toContainText("指向色条或图例");
  expect(await page.evaluate(() => (window as any).__storageOrder)).toEqual([".storage-total", ".retention-card"]);
  // 快速扫过几项不连跳：指针停稳才换高亮，扫过的中间项不出现。
  const last = await wrap.evaluate((root) => {
    const w = window as any;
    w.__hot = [];
    new MutationObserver(() => w.__hot.push(root.getAttribute("data-hot"))).observe(root, { attributes: true, attributeFilter: ["data-hot"] });
    // 同一任务里依次进出四项（React 按 mouseout 的 relatedTarget 推出进入哪一项），停留都是 0ms。
    const items = [...root.querySelectorAll<HTMLElement>(".storage-legend > button")].slice(0, 4);
    items[0].dispatchEvent(new MouseEvent("mouseover", { bubbles: true, relatedTarget: document.body }));
    for (let i = 1; i < items.length; i++) items[i - 1].dispatchEvent(new MouseEvent("mouseout", { bubbles: true, relatedTarget: items[i] }));
    return items.at(-1)!.querySelector<HTMLElement>(".storage-swatch")!.dataset.part;
  });
  await expect.poll(() => page.evaluate(() => (window as any).__hot)).toEqual([last]);
  await page.mouse.move(5, 5);
  await wrap.dispatchEvent("mouseout", { relatedTarget: null });
  await expect(wrap).not.toHaveAttribute("data-hot");
  // 图例与色段联动：指到哪项只亮那项，读数给出大小与占比。
  await settings.locator(".storage-legend").getByRole("button", { name: "媒体" }).hover();
  await expect(wrap).toHaveAttribute("data-hot", "media");
  await expect(readout).toContainText(/媒体\s*[\d.]+ (MB|GB) · [\d.]+%/);
  // 其余色段退到背景（有淡出过渡，等到终值）。
  await expect(wrap.locator('.storage-composition > i[data-part="articles"]')).toHaveCSS("opacity", "0.18");
  await wrap.locator('.storage-composition > i[data-part="articles"]').hover();
  await expect(wrap).toHaveAttribute("data-hot", "articles");
  await page.mouse.move(5, 5);
  await expect(wrap).not.toHaveAttribute("data-hot");
  // 空闲单列为可回收空位，不再和「其他」并在一起。
  await expect(settings.locator(".storage-legend")).toContainText("可回收空位");
  await expect(settings.locator(".storage-legend")).not.toContainText("其他与空闲");
  // 数据位置在总量卡片底部；版本号不在存储页。
  const location = settings.locator(".storage-location");
  await expect(location.locator("code")).toHaveText("/home/user/.local/share/rrss");
  await location.getByRole("button", { name: "在文件夹中显示" }).click();
  expect(await page.evaluate(() => (window as any).__mockHost.args.backup_reveal)).toEqual({ path: "/home/user/.local/share/rrss" });
  await expect(settings.getByText(/版本 v\d+/)).toHaveCount(0);
  await expect(settings.locator(".storage-passive").first()).toBeVisible();
  expect(await settings.locator(".storage-passive").first().evaluate((el) => getComputedStyle(el, "::before").content)).toBe("none");
});

test("backup card writes a backup, checks a restore before asking and folds its extras", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  await settingsTab(settings, "storage").click();
  const card = settings.locator(".backup-card");
  await expect(card.getByRole("button", { name: "备份包含密钥", exact: true })).toHaveAttribute("aria-pressed", "false");
  await expect(card).toContainText("上次备份：9 月 20 日 22:40。");
  await expect(settings.locator('[data-storage-item="auto_backups"]')).toContainText("2.30 GB");

  await card.getByRole("button", { name: "立即备份…" }).click();
  await expect(card.locator(".backup-result")).toContainText("已保存到 /home/user/Documents/rrss-backup-2026-09-27-1432.zip · 1.19 GB");
  await expect(card).toContainText("上次备份：今天 14:32。");
  expect(await page.evaluate(() => (window as any).__mockHost.args.backup_create)).toEqual({ includeSecrets: false });
  await expect(card.locator(".backup-result")).toContainText("不含密钥");
  await card.getByRole("button", { name: "备份包含密钥", exact: true }).click();
  await card.getByRole("button", { name: "立即备份…" }).click();
  await expect(card.locator(".backup-result")).toContainText("包含密钥");
  expect(await page.evaluate(() => (window as any).__mockHost.args.backup_create)).toEqual({ includeSecrets: true });
  await card.getByRole("button", { name: "备份包含密钥", exact: true }).click();


  await card.locator('[data-backup-fold="auto"] > summary').click();
  await card.locator('[data-backup-fold="prefs"] > summary').click();
  await card.getByRole("button", { name: "每周", exact: true }).click();
  await expect(card.locator('[data-backup-fold="auto"] > summary')).toContainText("定时每周");
  for (const width of [1280, 560]) {
    await page.setViewportSize({ width, height: 900 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme);
      expect(await settings.locator(".drawer-body").evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
      await card.scrollIntoViewIfNeeded();
      await shot(card, `../../test-results/storage/backup-${width}-${theme}.png`);
    }
  }
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.evaluate(() => { document.documentElement.dataset.theme = "light"; });

  await card.locator(".backup-entry").first().getByRole("button", { name: "恢复" }).click();
  const confirm = page.getByRole("alertdialog", { name: "从备份恢复" });
  await expect(confirm).toContainText("rrss-upgrade-20260927-091200-000.zip 检查通过");
  await expect(confirm).toContainText("替换前会自动备份当前数据");
  await expect(confirm).toContainText("不含密钥，恢复后需重新填写");
  await shot(page, "../../test-results/storage/backup-restore-confirm.png");
  await confirm.getByRole("button", { name: "取消" }).click();
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.calls.includes("backup_cancel_restore"))).toBe(true);

  await card.getByRole("button", { name: "选择备份文件…" }).click();
  await page.getByRole("alertdialog", { name: "从备份恢复" }).getByRole("button", { name: "备份当前数据并重启恢复" }).click();
  await expect(card.getByRole("status").filter({ hasText: "正在备份当前数据" })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__mockHost.args.backup_restore)).toEqual({ path: "/home/user/Documents/rrss-backup-2026-09-20-2240.zip" });
  await expect(card.getByRole("button", { name: "立即备份…" })).toBeDisabled();
});

test("retention is off by default, previews after turning on and confirms before cleaning", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  await settingsTab(settings, "storage").click();
  const card = settings.locator(".retention-card");
  await expect(card.locator("summary")).toContainText("关闭");
  await card.locator("summary").click();
  await expect(card).toContainText("关闭时不清理任何文章");
  await card.getByRole("switch", { name: "保留文章正文" }).click();
  await expect(card.getByRole("spinbutton", { name: "保留天数", exact: true })).toHaveValue("90");
  await expect(card).toContainText("将清理 12,480 篇");
  await expect(card).toContainText("不会清理：已收藏的");
  for (const width of [1280, 560]) {
    await page.setViewportSize({ width, height: 900 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme);
      expect(await settings.locator(".drawer-body").evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
      await card.scrollIntoViewIfNeeded();
      await shot(card, `../../test-results/storage/retention-${width}-${theme}.png`);
    }
  }
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.evaluate(() => { document.documentElement.dataset.theme = "light"; });
  await card.getByRole("button", { name: "立即清理" }).click();
  const confirm = page.getByRole("alertdialog", { name: "立即清理旧文章正文" });
  await expect(confirm).toContainText("第一次清理，会先自动备份");
  await shot(page, "../../test-results/storage/retention-confirm.png");
  await confirm.getByRole("button", { name: "备份并清理" }).click();
  await expect(card.getByRole("status")).toContainText("已清理 12,480 篇");
  await expect(card).toContainText("目前没有要清理的文章");
});

test("subscribe page flags unhealthy feeds, filters them and confirms batch unsubscribe", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  const list = settings.locator(".subscribe-list");
  const bar = list.locator(".health-bar");
  await expect(bar).toContainText("3 个源需要处理");
  await list.getByRole("button", { name: "连续失败 12 次 · 9 天" }).click();
  await expect(list.locator(".feed-health-detail")).toContainText("最近错误：源暂时不可用（503）");
  await bar.getByRole("button", { name: "只看这些" }).click();
  await expect(list.locator(".feed-row")).toHaveCount(3);
  for (const width of [1280, 560]) {
    await page.setViewportSize({ width, height: 900 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme);
      expect(await settings.locator(".drawer-body").evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
      await shot(list, `../../test-results/storage/feed-health-${width}-${theme}.png`);
    }
  }
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.evaluate(() => { document.documentElement.dataset.theme = "light"; });

  await list.locator('[data-feed-health="stale"]').getByRole("button", { name: "忽略" }).click();
  await expect(bar).toContainText("2 个源需要处理");
  await bar.getByRole("button", { name: "退订长期失败的源…" }).click();
  const confirm = page.getByRole("alertdialog", { name: "退订长期失败的源" });
  await expect(confirm).toContainText("Gadget Wire Tech Weekly Digest Long Name — 连续失败 12 次 · 9 天");
  await confirm.getByRole("button", { name: "退订 1 个源" }).click();
  await expect(bar).toContainText("1 个源需要处理");
  expect(await page.evaluate(() => (window as any).__mockHost.args.unsubscribe_feeds)).toEqual({ ids: [4] });
});

test("settings fills the whole window with the category nav on the left", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  await expect(settings.getByLabel("订阅地址", { exact: true })).toBeVisible();
  await expect(page.locator('[aria-label="关闭设置"]')).toHaveCount(0);
  for (const width of [1440, 1280, 560]) {
    await page.setViewportSize({ width, height: 800 });
    const box = await settings.boundingBox();
    expect(box).toMatchObject({ x: 0, y: 0, width, height: 800 });
    if (width > 640) {
      const nav = await settings.getByRole("navigation", { name: "设置分类" }).boundingBox();
      expect(nav!.x).toBe(0);
    }
    for (const theme of ["light", "dark"]) {
      await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme);
      await shot(page, `../../test-results/settings-window/settings-${width}-${theme}.png`);
    }
  }
  await page.keyboard.press("Escape");
  await expect(settings).toHaveCount(0);
});

test("theme segments preserve click and native keyboard selection", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const group = page.locator(".settings-theme .segmented");
  const buttons = group.getByRole("button");
  await buttons.nth(1).click();
  await expect(buttons.nth(1)).toHaveAttribute("aria-pressed", "true");
  await expect(buttons.nth(0)).toHaveAttribute("aria-pressed", "false");
  // 这些是原生切换按钮，不是单选组：方向键不改变选择。
  await page.keyboard.press("ArrowRight");
  await expect(buttons.nth(1)).toHaveAttribute("aria-pressed", "true");
  await page.keyboard.press("Tab");
  await expect(buttons.nth(2)).toBeFocused();
  await page.keyboard.press("Space");
  await expect(buttons.nth(2)).toHaveAttribute("aria-pressed", "true");
  await expect(buttons.nth(1)).toHaveAttribute("aria-pressed", "false");
  await expect(group.locator(".segment-thumb")).toBeVisible();
});

test("settings groups follow body width and quiet actions remain keyboard reachable", async ({ page }) => {
  await installMockHost(page);
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  const body = settings.locator(".settings-body");
  const group = settings.locator(".subscribe-add");
  const layout = () => group.evaluate(el => {
    const head = el.querySelector(".settings-section-meta")!.getBoundingClientRect();
    const controls = el.querySelector(".settings-section-body")!.getBoundingClientRect();
    return { split: controls.left > head.right && Math.abs(controls.top - head.top) < 2,
      single: Math.abs(controls.left - head.left) < 2 && controls.top >= head.bottom };
  });
  // 固定窗口，只改变正文可用宽度，排除用 viewport media query 冒充容器查询。
  for (const width of [861, 859, 861]) {
    await body.evaluate((el, width) => {
      const shell = el.closest<HTMLElement>(".settings-shell")!;
      const sidebar = shell.querySelector<HTMLElement>(".settings-sidebar")!;
      sidebar.style.width = `${shell.clientWidth - width - (el.getBoundingClientRect().width - el.clientWidth)}px`;
    }, width);
    await expect.poll(() => body.evaluate(el => el.clientWidth)).toBe(width);
    await expect.poll(async () => (await layout())[width > 860 ? "split" : "single"]).toBe(true);
  }
  await settings.locator(".settings-sidebar").evaluate(el => { el.style.width = ""; });
  await settingsTab(settings, "model").click();
  const edit = settings.locator(".connection-row").last().getByRole("button", { name: /^编辑 / });
  await page.mouse.move(1439, 899);
  await expect(edit).toHaveCSS("opacity", "0");
  // 「添加连接」在列表之后：反向 Tab 依次经过末行收起的「测试」「编辑」。
  await settings.getByRole("button", { name: "添加连接", exact: true }).focus();
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Shift+Tab");
  await expect(edit).toBeFocused();
  await expect(edit).toHaveCSS("opacity", "1");
  await expect(edit).toHaveCSS("outline-style", "solid");
  await page.keyboard.press("Enter");
  const dialog = page.getByRole("dialog", { name: "编辑连接", exact: true });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "取消", exact: true }).click();

  const footer = settings.locator(".settings-nav-foot");
  await footer.getByRole("button", { name: "深色", exact: true }).click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await expect(footer).toContainText("数据只存在本机");
  await page.setViewportSize({ width: 560, height: 900 });
  const navItem = settingsTab(settings, "model");
  await expect.poll(() => navItem.evaluate(el => {
    const line = getComputedStyle(el, "::before");
    return line.height === "3px" && line.bottom === "0px" && parseFloat(line.width) > 3;
  })).toBe(true);
  await settings.locator(".settings-nav-head").getByRole("button", { name: "返回阅读", exact: true }).click();
  await expect(settings).toHaveCount(0);
});
