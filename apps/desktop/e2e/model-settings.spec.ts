import { expect, test, type Page } from "./fixtures";
import { installMockHost, MOCK_AI, settingsTab } from "./mockHost";
import { shot } from "./shots";

// 模型页（阶段 19-7）：连接 → 添加模型 → 各用途；管理模型弹出页；连接测试与离开确认。
const OUT = "../../test-results/model-settings";

async function openModels(page: Page) {
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  await settingsTab(settings, "model").click();
  await expect(settings.getByRole("heading", { name: "模型", exact: true, level: 3 })).toBeVisible();
  return settings;
}

async function shots(page: Page, name: string, fullPage = false) {
  for (const width of [1280, 560]) {
    await page.setViewportSize({ width, height: 760 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate((t) => { document.documentElement.dataset.theme = t; }, theme);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true);
      const overflow = await page.locator(".settings-drawer .drawer-body").evaluate((el) => {
        const right = el.getBoundingClientRect().right;
        return [...el.querySelectorAll<HTMLElement>("*")].filter(node => node.getBoundingClientRect().right > right + 1)
          .map(node => `${node.className}: ${node.getBoundingClientRect().width}`);
      });
      expect(overflow, `settings body at ${width}px / ${theme}`).toEqual([]);
      await shot(page, `${OUT}/${name}-${width}-${theme}.png`, { fullPage });
    }
  }
  await page.setViewportSize({ width: 1280, height: 760 });
}

const lastArgs = (page: Page, command: string) => page.evaluate((c) => (window as any).__mockHost.args[c]?.args, command);

test("a new connection goes straight to picking its models", async ({ page }) => {
  await installMockHost(page);
  const settings = await openModels(page);
  await settings.getByRole("button", { name: "添加连接", exact: true }).first().click();
  const sheet = page.getByRole("dialog", { name: "添加连接", exact: true });
  await sheet.getByRole("button", { name: "DeepSeek", exact: true }).click();
  await sheet.getByLabel("API Key", { exact: true }).fill("sk-fake-e2e");
  await sheet.getByRole("button", { name: "测试连接", exact: true }).click();
  await expect(sheet.getByText("连接成功 · 42 ms · 返回 7 个模型")).toBeVisible();
  await shots(page, "add-connection");
  await sheet.getByRole("button", { name: "保存并选模型", exact: true }).click();
  await expect(sheet).toHaveCount(0);
  expect((await lastArgs(page, "save_ai_settings")).provider).toMatchObject({
    id: 0, kind: "deepseek", base_url: "https://api.deepseek.com/v1", api_key: "sk-fake-e2e" });

  const add = page.getByRole("dialog", { name: "添加模型", exact: true });
  await expect(add).toContainText("已保存连接「DeepSeek」。勾选要用的模型。");
  await expect(add.getByRole("button", { name: "DeepSeek", exact: true })).toHaveAttribute("aria-pressed", "true");
  await add.getByRole("button", { name: "获取模型列表", exact: true }).click();
  await expect(add).toContainText("已更新，共 3 个模型。");
  await add.getByRole("checkbox", { name: /gpt-4o-mini/ }).check();
  await add.getByLabel("手填模型 id").fill("deepseek-reasoner");
  await add.getByRole("button", { name: "加入勾选", exact: true }).click();
  await shots(page, "add-models");
  await expect(add).toContainText("将加入 2 个、移出 0 个");
  await add.getByRole("button", { name: "应用", exact: true }).click();
  await expect(add).toHaveCount(0);
  expect(await lastArgs(page, "save_model_pool")).toEqual({
    add: [{ provider_id: 3, model: "gpt-4o-mini" }, { provider_id: 3, model: "deepseek-reasoner" }], remove: [] });
  await expect(settings.getByRole("status").filter({ hasText: "已加入 2 个、移出 0 个模型。" })).toBeVisible();
  await expect(settings.locator(".connection-row")).toHaveCount(3);
  await expect(settings.getByRole("button", { name: "个模型 7", exact: true })).toBeVisible();
});

test("uses are grouped, saved on pick, and the apply bar sets several groups in one write", async ({ page }) => {
  await installMockHost(page);
  const settings = await openModels(page);
  const group = (id: string) => settings.locator(`.use-row[data-group="${id}"]`);
  await expect(group("summary").getByRole("button", { name: "摘要的模型", exact: true })).toHaveText("2 种模型");
  await expect(group("translate")).toContainText("还没选模型，翻译暂停。");
  await expect(group("chat").getByRole("button", { name: "对话模型", exact: true })).toHaveText("Sonnet OpenRouter");
  await group("summary").getByRole("button", { name: /^细分/ }).click();
  await expect(group("summary").getByRole("button", { name: "焦点摘要的模型", exact: true })).toHaveText("llama3.2 本机 Ollama");
  await shots(page, "page", true);

  // 单项选即存。
  await group("translate").getByRole("button", { name: "翻译的模型", exact: true }).click();
  await page.getByRole("dialog", { name: "翻译的模型" }).getByRole("option", { name: /qwen2\.5:14b/ }).click();
  await expect(settings.getByRole("status").filter({ hasText: "已保存：本篇翻译改用 qwen2.5:14b。" })).toBeVisible();
  expect(await lastArgs(page, "save_model_bindings")).toEqual({ bindings: [{ task: "article_translate", provider_id: 1, model: "qwen2.5:14b" }] });
  await expect(group("translate")).not.toContainText("暂停");

  // 一键应用：默认勾摘要、简报、记忆整理。
  const bar = settings.getByRole("group", { name: "一键应用" });
  await bar.getByRole("button", { name: "一键应用的模型", exact: true }).click();
  await page.getByRole("dialog", { name: "一键应用的模型" }).getByRole("option", { name: /Sonnet/ }).click();
  await bar.getByRole("button", { name: /^应用到/ }).click();
  await expect.poll(async () => (await lastArgs(page, "save_model_bindings")).bindings)
    .toEqual([
      "focus_summary", "article_summary", "article_segment", "cluster_summary",
      "digest_daily", "digest_weekly", "digest_pick_daily", "digest_pick_weekly",
      "memory_candidate", "episode_compress",
    ].map((task) => ({ task, provider_id: 2, model: "anthropic/claude-sonnet-4" })));
  await expect(group("summary").getByRole("button", { name: "摘要的模型", exact: true })).toHaveText("Sonnet OpenRouter");

  // 已绑定向量库：未允许更换时不保存；打开开关后改选要确认重建。
  const pickEmbed = async () => {
    await group("embed").getByRole("button", { name: "嵌入的模型", exact: true }).click();
    await page.getByRole("dialog", { name: "嵌入的模型" }).getByRole("option", { name: /llama3\.2/ }).click();
  };
  const saves = await page.evaluate(() => (window as any).__mockHost.calls.filter((c: string) => c === "save_model_bindings").length);
  await pickEmbed();
  await expect(settings.getByRole("alert").filter({ hasText: "需要重建向量库" })).toBeVisible();
  const vectors = settings.locator("#vector-space");
  await expect(vectors).toContainText("nomic-embed-text（本机 Ollama）· 768 维 · 52,904 段");
  const allow = vectors.getByRole("button", { name: "允许更换嵌入模型" });
  await allow.click();
  await expect(allow).toHaveAttribute("aria-pressed", "true");
  await pickEmbed();
  const confirm = page.getByRole("alertdialog", { name: "更换嵌入模型并重建" });
  await expect(confirm).toContainText("需要重新生成 52,904 段");
  await confirm.getByRole("button", { name: "取消", exact: true }).click();
  expect(await page.evaluate(() => (window as any).__mockHost.calls.filter((c: string) => c === "save_model_bindings").length)).toBe(saves);
  await expect(group("embed").getByRole("button", { name: "嵌入的模型", exact: true })).toHaveText("nomic-embed-text 本机 Ollama");
});

test("manage models edits each model's own settings and removing a model in use asks for a replacement", async ({ page }) => {
  await installMockHost(page);
  const settings = await openModels(page);
  await settings.getByRole("button", { name: "能力未测 2", exact: true }).click();
  const sheet = page.getByRole("dialog", { name: "管理模型", exact: true });
  await expect(sheet.getByRole("list", { name: "加进来的模型" }).getByRole("button")).toHaveCount(2);
  await sheet.getByRole("button", { name: "全部", exact: true }).click();
  await sheet.getByRole("button", { name: /^Sonnet/ }).click();
  await sheet.locator(".settings-option").filter({ hasText: "思考强度" }).getByRole("button", { name: "high", exact: true }).click();
  await expect(sheet.getByRole("status")).toHaveText("已保存。");
  expect(await lastArgs(page, "update_pool_model")).toEqual({ provider_id: 2, model: "anthropic/claude-sonnet-4", thinking: "high" });
  await shots(page, "manage-models");

  await sheet.getByRole("button", { name: /^llama3\.2/ }).click();
  const tools = sheet.locator(".settings-option").filter({ hasText: "工具调用" });
  await expect(tools.getByRole("button", { name: "按测试（不支持）", exact: true })).toHaveAttribute("aria-pressed", "true");
  await tools.getByRole("button", { name: "支持", exact: true }).click();
  await expect(sheet.getByRole("button", { name: /^llama3\.2/ })).toContainText("能力受限");
  expect((await lastArgs(page, "update_pool_model")).overrides).toEqual({ tool_calling: true });
  await sheet.getByLabel("显示名称").fill("Llama 小");
  await sheet.getByLabel("显示名称").press("Enter");
  await expect(sheet.getByRole("button", { name: /^Llama 小/ })).toBeVisible();
  await sheet.getByRole("button", { name: "用于对话", exact: true }).click();
  expect((await lastArgs(page, "update_pool_model")).chat).toBe(true);

  await sheet.getByRole("button", { name: "从模型列表移出", exact: true }).click();
  const prompt = sheet.getByRole("group", { name: "改用哪个模型" });
  await expect(prompt).toContainText("焦点摘要");
  await prompt.getByRole("button", { name: "这些用途改用" }).click();
  await page.getByRole("menuitemradio", { name: "qwen2.5:14b（本机 Ollama）" }).click();
  await expect(prompt.getByRole("button", { name: "这些用途改用" })).toContainText("qwen2.5:14b");
  await prompt.getByRole("button", { name: "移出", exact: true }).click();
  await expect(sheet.getByRole("status")).toContainText("已移出 Llama 小，1 项用途改用替换模型。");
  expect(await lastArgs(page, "save_model_pool")).toEqual({ add: [], remove: [{ provider_id: 1, model: "llama3.2" }], replace: { provider_id: 1, model: "qwen2.5:14b" } });
  await sheet.getByRole("button", { name: "完成", exact: true }).click();
  await expect(sheet).toHaveCount(0);
  await expect(settings.getByRole("button", { name: "个模型 4", exact: true })).toBeVisible();
});

test("a slow connection test can be stopped and a failure never shows the key", async ({ page }) => {
  await installMockHost(page);
  await page.addInitScript(() => {
    const host = (window as any).__TAURI_INTERNALS__;
    const original = host.invoke;
    const probe = { finish: (_error?: string) => {} };
    (window as any).__probe = probe;
    host.invoke = (command: string, args: Record<string, unknown>) => command === "test_connection"
      ? new Promise((resolve) => { probe.finish = (error?: string) => resolve({ ok: !error, latency_ms: 9, models: error ? 0 : 3, error: error ?? "" }); })
      : original(command, args);
  });
  const settings = await openModels(page);
  const row = settings.locator(".connection-row").filter({ hasText: "OpenRouter" });
  await expect(row.locator(".connection-state")).toHaveAttribute("title", /通过 · 返回 14 个模型/);
  await row.getByRole("button", { name: "测试 OpenRouter", exact: true }).click();
  await expect(row.getByText("已等待 0 秒")).toBeVisible();
  await row.getByRole("button", { name: "停止等待", exact: true }).click();
  await expect(row.getByText(/已停止等待/)).toBeVisible();
  await page.evaluate(() => (window as any).__probe.finish());
  await expect(row.getByText(/已停止等待/)).toBeVisible();
  await row.getByRole("button", { name: "测试 OpenRouter", exact: true }).click();
  // 宿主拿库里的密钥去测，错误里的密钥由宿主抹掉（capabilities.rs 有单测）；界面只显示宿主给的文字。
  await page.evaluate(() => (window as any).__probe.finish("HTTP 401 Invalid API Key ***"));
  await expect(row.getByRole("alert")).toContainText("认证失败");
  await row.locator("summary").click();
  await expect(row.locator("pre")).toContainText("Invalid API Key ***");
  await expect(row.locator(".status-dot")).toHaveAttribute("data-state", "err");
});

test("a saved key shows only its last 4 characters; dialog edits survive history and Escape until discarded", async ({ page }) => {
  await installMockHost(page);
  const settings = await openModels(page);
  await settings.getByRole("button", { name: "编辑 OpenRouter", exact: true }).click();
  const sheet = page.getByRole("dialog", { name: "编辑连接", exact: true });
  await expect(sheet).toContainText("已保存 · fake");
  await expect(sheet.getByRole("textbox", { name: "API Key", exact: true })).toHaveCount(0);
  const name = sheet.getByLabel("名称", { exact: true });
  await expect(name).toBeFocused();
  await name.fill("未保存测试");
  // 弹层遮住导航；离开只能经浏览器历史或 Esc，二者都要守住草稿，并能直接保存。
  await page.goBack();
  const leave = page.getByRole("alertdialog", { name: "编辑连接有未保存的修改" });
  await expect(leave.getByRole("button", { name: "保存并离开", exact: true })).toBeVisible();
  await leave.getByRole("button", { name: "继续编辑" }).click();
  await expect(name).toHaveValue("未保存测试");
  await page.keyboard.press("Escape");
  const discard = page.getByRole("alertdialog", { name: "编辑连接有未保存的修改" });
  await discard.getByRole("button", { name: "不保存", exact: true }).click();
  await expect(sheet).toHaveCount(0);
  await expect(settings.getByRole("button", { name: "编辑 OpenRouter", exact: true })).toBeFocused();
  expect(await lastArgs(page, "save_ai_settings")).toBeUndefined();
});

test("automatic summaries explain the upgrade, confirm before turning on and set a budget when there is none", async ({ page }) => {
  await installMockHost(page, {
    settingValues: { ai_daily_budget_tokens: "50000" },
    ai: { auto_summary: { ...MOCK_AI.auto_summary, model_ready: true, cluster_model_ready: true, today: 12,
      notice: { budget_defaulted: false, generation_budget: 50000 } } },
  });
  const settings = await openModels(page);
  const settingWrites = () => page.evaluate(() => (window as any).__mockHost.calls.filter((c: string) => c === "set_setting").length);
  const lastSetting = () => page.evaluate(() => (window as any).__mockHost.args.set_setting);

  // 升级提示：分类栏「模型」带点；「查看自动任务」展开并定位到开关。
  await expect(settingsTab(settings, "model").locator(".settings-nav-dot")).toHaveCount(1);
  const notice = settings.locator(".auto-summary-notice");
  await expect(notice).toContainText("你设过的日预算 5 万 token 保留为「生成日预算」");
  await notice.getByRole("button", { name: "查看自动任务", exact: true }).click();
  const autoRow = settings.locator("#model-auto-summary");
  await expect(autoRow).toBeInViewport();
  const toggle = autoRow.getByRole("button", { name: "自动摘要", exact: true });
  await expect(toggle).toHaveAttribute("aria-pressed", "false");
  await expect(settings.getByRole("spinbutton", { name: "中性未读额外摘要条数（0 = 关）" })).toBeDisabled();

  // 开启要确认；取消不写入任何值。
  const before = await settingWrites();
  await toggle.click();
  let dialog = page.getByRole("dialog", { name: "开启自动摘要？", exact: true });
  await expect(dialog).toContainText("这是持续费用");
  await expect(dialog).toContainText("约够 3 篇 / 天");
  await dialog.getByRole("button", { name: "取消", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(await settingWrites()).toBe(before);
  await toggle.click();
  await dialog.getByRole("button", { name: "开启自动摘要", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(await lastSetting()).toEqual({ id: "focus_auto_summary", value: "1" });
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  await expect(autoRow).toContainText("今日已摘要 12 / 100 篇");
  // 开启后提示与分类栏的点一起消失。
  await expect(notice).toHaveCount(0);
  await expect(settingsTab(settings, "model").locator(".settings-nav-dot")).toHaveCount(0);
  await shots(page, "auto-summary-on", true);

  // 自动汇总不弹确认，点了就写。
  const cluster = settings.locator("#model-auto-cluster").getByRole("button", { name: "自动汇总相近报道", exact: true });
  await cluster.click();
  expect(await lastSetting()).toEqual({ id: "cluster_auto_summary", value: "1" });
  await expect(cluster).toHaveAttribute("aria-pressed", "true");
  await expect(settings.locator("#model-auto-cluster")).toContainText("今日已汇总 0 / 20 组");

  // 关掉自动摘要不需要确认；生成预算改成 0 后再开，弹窗里直接设预算。
  await toggle.click();
  expect(await lastSetting()).toEqual({ id: "focus_auto_summary", value: "0" });
  await settings.getByRole("button", { name: /^用量/ }).click();
  const budget = settings.getByLabel("生成日预算（万 token，0 = 不限额）");
  await budget.fill("0");
  await settings.getByRole("button", { name: "保存", exact: true }).click();
  await expect(settings.getByText(/现在不限额。建议设一个上限/)).toBeVisible();
  await toggle.click();
  dialog = page.getByRole("dialog", { name: "开启自动摘要？", exact: true });
  await expect(dialog).toContainText("还没有生成日预算");
  const draft = dialog.getByLabel("生成日预算（万 token）");
  await expect(draft).toHaveValue("20");
  await draft.fill("10");
  await expect(dialog).toContainText("扣除自动简报约 3.5 万、自动汇总约 4 万后约够 6 篇 / 天（估算）");
  await shots(page, "auto-summary-confirm-budget");
  await dialog.getByRole("button", { name: "设预算并开启", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  await expect(budget).toHaveValue("10");
});
