import { mcpFixture } from "../src/test/fixtures/settings";
import { expect, test, type Locator, type Page } from "./fixtures";
import { installMockHost, settingsTab } from "./mockHost";
import { shot } from "./shots";

// 复用共享模拟宿主：状态经 `__mockHost.state` 调整，事件经 `__mockHost.emit` 推送。
const OUT = "../../test-results/assistant-settings";

/** L1 折叠组的标题按钮；按标题全文匹配，避免撞上「工具 xx」之类的行内开关。 */
function fold(scope: Locator, title: string) {
  return scope.locator(".settings-fold-head").filter({ has: scope.page().locator("strong", { hasText: new RegExp(`^${title}$`) }) });
}

async function openAssistant(page: Page) {
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  await settingsTab(settings, "assistant").click();
  await expect(settings.getByRole("heading", { name: "对话助手", exact: true })).toBeVisible();
  return settings;
}

/** 对话模型说明必须横排。选择框若按 width:100% 占满行，左侧文字会被挤成一字一行。 */
async function expectChatModelLabelHorizontal(settings: Locator) {
  const label = settings.locator("section[aria-labelledby=assistant-abilities] .settings-option > div:first-child").first();
  const box = await label.boundingBox();
  expect(box, "对话模型说明应可见").toBeTruthy();
  expect(box!.width, "对话模型说明应横排，不能被选择框挤成窄列").toBeGreaterThan(160);
  expect(box!.width).toBeGreaterThan(box!.height);
}

test("the assistant page is one column with a status line, folds and the engine section at the bottom", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  const settings = await openAssistant(page);
  await expectChatModelLabelHorizontal(settings);
  await expect(settings.getByRole("status").filter({ hasText: "引擎就绪Pi 0.87.1" })).toBeVisible();
  await expect(settings.getByRole("button", { name: "启动 Pi" })).toHaveCount(0);
  const engine = settings.locator("section[aria-labelledby=assistant-engine]");
  await expect(engine.getByRole("button", { name: "重启引擎", exact: true })).toBeVisible();
  for (const name of ["技能", "外部工具"]) {
    const head = fold(settings, name);
    await head.click();
    await expect(head).toHaveAttribute("aria-expanded", "true");
  }
  // 思考强度跟着模型走，这里只读。
  await expect(settings.getByText("Sonnet · low")).toBeVisible();
  for (const width of [1280, 560]) {
    await page.setViewportSize({ width, height: 720 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate((t) => { document.documentElement.dataset.theme = t; }, theme);
      expect(await settings.locator(".drawer-body").evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
      await expectChatModelLabelHorizontal(settings);
      await shot(page, `${OUT}/page-${width}-${theme}.png`);
    }
  }
  await engine.getByRole("button", { name: "重启引擎", exact: true }).click();
  await expect(settings.getByText("对话引擎已重启。", { exact: true })).toBeVisible();
});

test("a missing engine offers one download action and a missing model points to the model page", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.evaluate(() => {
    const state = (window as any).__mockHost.state;
    state.probe = { ...state.probe, version: "", version_state: "missing" };
  });
  const settings = await openAssistant(page);
  const status = settings.getByRole("alert").filter({ hasText: "未找到对话引擎" });
  await expect(status).toBeVisible();
  await shot(page, `${OUT}/missing-1280-light.png`);
  await expect(settings.locator(".engine-install-note")).toContainText("不影响你电脑上已有的 Pi");
  await page.evaluate(() => { (window as any).__mockHost.holdInstall = true; });
  await status.getByRole("button", { name: "下载对话引擎", exact: true }).click();
  // 下载在后台：状态行显示进度和安装位置，设置页不被锁住，可以直接离开。
  const progress = settings.locator(".engine-status");
  await expect(progress).toHaveAttribute("aria-busy", "true");
  await page.evaluate(() => (window as any).__mockHost.progressInstall(24 * 1024 * 1024));
  await expect(progress).toContainText("正在下载对话引擎 24.0 MB / 48.0 MB");
  await expect(progress.getByRole("progressbar")).toHaveAttribute("value", "50");
  await expect(settings.locator(".engine-install-note")).toContainText("/home/user/.local/share/rrss/pi-pkg");
  await shot(page, `${OUT}/installing-1280-light.png`);
  await page.keyboard.press("Escape");
  await expect(settings).toHaveCount(0);
  await page.evaluate(() => (window as any).__mockHost.finishInstall());
  await expect(page.locator(".toast")).toContainText("对话引擎已就绪");
  const reopened = await openAssistant(page);
  await expect(reopened.getByRole("status").filter({ hasText: "引擎就绪" })).toBeVisible();
});

test("a configuration problem points to the model page", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.evaluate(() => {
    const state = (window as any).__mockHost.state;
    state.probe = { ...state.probe, config_error: "还没有可用的对话模型：先在模型页添加连接" };
  });
  const settings = await openAssistant(page);
  await settings.getByRole("alert").getByRole("button", { name: "去设置对话模型", exact: true }).click();
  await expect(settings.getByRole("heading", { name: "模型", exact: true, level: 3 })).toBeVisible();
});

test("services and skills are edited in dialogs, and an edited path guards leaving", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  const settings = await openAssistant(page);
  if (await fold(settings, "外部工具").getAttribute("aria-expanded") !== "true") await fold(settings, "外部工具").click();
  await settings.getByRole("button", { name: "编辑服务 filesystem", exact: true }).click();
  const sheet = page.getByRole("dialog", { name: "编辑外部工具", exact: true });
  // 启动命令一行填完，引号里的空格保留。
  await sheet.getByLabel("启动命令", { exact: true }).fill('npx -y server "path with spaces"');
  await shot(page, `${OUT}/mcp-dialog-1280-light.png`);
  await sheet.getByRole("button", { name: "保存", exact: true }).click();
  await expect(sheet).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__mockHost.args.upsert_mcp)).toMatchObject({ input: { id: 1, name: "filesystem", command: "npx", args: ["-y", "server", "path with spaces"] } });

  if (await fold(settings, "外部工具").getAttribute("aria-expanded") !== "true") await fold(settings, "外部工具").click();
  await settings.getByRole("button", { name: "编辑服务 filesystem", exact: true }).click();
  await expect(sheet.getByLabel("启动命令", { exact: true })).toHaveValue('npx -y server "path with spaces"');
  await sheet.getByRole("button", { name: "取消", exact: true }).click();

  await fold(settings, "技能").click();
  await settings.getByRole("button", { name: "新建技能", exact: true }).click();
  const skill = page.getByRole("dialog", { name: "新建技能", exact: true });
  await skill.getByLabel("名称", { exact: true }).fill("weekly-notes");
  await skill.getByLabel("什么时候用", { exact: true }).fill("整理每周笔记时");
  await page.keyboard.press("Escape");
  const leave = page.getByRole("alertdialog", { name: "新建技能有未保存的修改" });
  await expect(leave).toBeVisible();
  await leave.getByRole("button", { name: "继续编辑", exact: true }).click();
  await skill.getByRole("button", { name: "保存", exact: true }).click();
  await expect(skill).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__mockHost.args.save_skill)).toEqual({ draft: { name: "weekly-notes", description: "整理每周笔记时", body: "" } });
});

test("the chat asks before reading a web page and the answer reaches the host", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "深度对话", exact: true }).click();
  await page.evaluate(() => (window as any).__mockHost.emit("chat:confirm", {
    thread_id: 1, id: "fetch-1", title: "读取网页", message: "助手想读取 https://example.test/post", server: "", tool: "", args: "", can_remember: false, timeout_ms: 120000,
  }));
  const bar = page.getByRole("alertdialog", { name: "读取网页" });
  await expect(bar).toContainText("2 分钟内不处理会自动拒绝");
  await shot(page, `${OUT}/confirm-bar-1280-light.png`);
  await bar.getByRole("button", { name: "允许", exact: true }).click();
  await expect(bar).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__mockHost.args.confirm_fetch)).toEqual({ id: "fetch-1", ok: true, remember: false });
});

test("an online service is checked before saving, keeps its token masked, and opens only the chosen tools", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  const settings = await openAssistant(page);
  if (await fold(settings, "外部工具").getAttribute("aria-expanded") !== "true") await fold(settings, "外部工具").click();
  await settings.getByRole("button", { name: "添加服务", exact: true }).click();
  const sheet = page.getByRole("dialog", { name: "添加外部工具", exact: true });
  await sheet.getByRole("radio", { name: /在线服务/ }).click();
  await sheet.getByLabel("地址", { exact: true }).fill("https://mcp.example.test/mcp");
  // 名称不填时用按地址猜的名字（显示为占位示例）。
  await expect(sheet.getByLabel("名称", { exact: true })).toHaveAttribute("placeholder", "example");
  await sheet.getByLabel("名称", { exact: true }).fill("remote");
  await sheet.getByRole("group", { name: "登录" }).getByRole("button", { name: "访问令牌", exact: true }).click();
  const token = sheet.getByLabel("令牌", { exact: true });
  await token.fill("!literal-secret");
  await expect(token).toHaveAttribute("type", "password");
  await sheet.getByRole("button", { name: "显示令牌", exact: true }).click();
  await expect(token).toHaveAttribute("type", "text");
  await sheet.getByRole("button", { name: "遮盖令牌", exact: true }).click();
  // 检测用还没保存的内容；结果默认收起，展开后会改文件的默认不勾。
  await sheet.getByRole("button", { name: "检测连接", exact: true }).click();
  await expect(sheet.getByRole("status").filter({ hasText: "能连上 · 3 个工具" })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__mockHost.args.probe_mcp.input)).toMatchObject({ id: 0, transport: "http", auth: "token",
    headers: [{ name: "Authorization", value: "Bearer !literal-secret" }] });
  const tools = sheet.getByRole("list", { name: "开放给助手的工具" });
  await expect(tools).not.toBeVisible();
  await sheet.getByRole("button", { name: "查看工具", exact: true }).click();
  await expect(tools.getByRole("checkbox", { name: /read_file/ })).toBeChecked();
  await expect(tools.getByRole("checkbox", { name: /write_file/ })).not.toBeChecked();
  await expect(tools.locator("li").filter({ hasText: "write_file" })).toContainText("会改文件");
  for (const width of [1280, 560]) {
    await page.setViewportSize({ width, height: 720 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate((t) => { document.documentElement.dataset.theme = t; }, theme);
      expect(await sheet.evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
      await expect(sheet.getByRole("button", { name: "保存", exact: true })).toBeInViewport();
      await shot(page, `${OUT}/mcp-remote-${width}-${theme}.png`);
    }
  }
  await sheet.getByRole("button", { name: "保存", exact: true }).click();
  await expect(sheet).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__mockHost.args.upsert_mcp.input)).toMatchObject({ name: "remote", transport: "http", auth: "token",
    url: "https://mcp.example.test/mcp", headers: [{ name: "Authorization", value: "Bearer !literal-secret" }], include_tools: ["read_file", "list_directory"] });

  // 打开开关时自动检测一次，结果仍收起；勾上会改文件的工具后存成「全部开放」。
  const row = settings.getByRole("listitem").filter({ has: page.getByText("remote", { exact: true }) });
  const toggle = row.getByRole("button", { name: "服务 remote", exact: true });
  await expect(toggle).toHaveAttribute("aria-pressed", "false");
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  await expect(row.getByRole("status")).toHaveText(/^已连接 · 开放 2 \/ 3 个工具 · 0\.4 秒$/);
  await expect(row.getByRole("list", { name: "开放给助手的工具" })).not.toBeVisible();
  await row.getByRole("button", { name: "查看工具", exact: true }).click();
  await row.getByRole("checkbox", { name: /write_file/ }).check();
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.args.upsert_mcp.input.include_tools)).toEqual([]);
  await expect(row.getByRole("status")).toHaveText(/^已连接 · 开放 3 \/ 3 个工具/);
  await shot(page, `${OUT}/mcp-tools-560-dark.png`);
  // 重新检测时已展开的列表保持展开。
  await row.getByRole("button", { name: "检测 remote", exact: true }).click();
  await expect(row.getByRole("status")).toHaveText(/^已连接/);
  await expect(row.getByRole("list", { name: "开放给助手的工具" })).toBeVisible();

  await row.getByRole("button", { name: "编辑服务 remote", exact: true }).click();
  const edit = page.getByRole("dialog", { name: "编辑外部工具", exact: true });
  await expect(edit.getByLabel("令牌", { exact: true })).toHaveValue("");
  await expect(edit.getByLabel("令牌", { exact: true })).toHaveAttribute("placeholder", "已保存，留空不变");
  await edit.getByLabel("名称", { exact: true }).fill("remote-docs");
  await edit.getByRole("button", { name: "保存", exact: true }).click();
  await expect(edit).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__mockHost.args.upsert_mcp.input)).toMatchObject({ name: "remote-docs", headers: [{ name: "Authorization", value: "" }] });
});

test("a failed check folds its reason, and old SSE services can only be repointed", async ({ page }) => {
  await installMockHost(page);
  // 在页面载入前改好模拟宿主：设置数据可能在启动时就预载并缓存，载入后再改看不到。
  await page.addInitScript(() => {
    const host = (window as any).__mockHost;
    host.mcpProbe.filesystem = { ok: false, elapsed_ms: 1200, tools: [], error: "进程启动后 1.2 秒退出（退出码 1）。", hint: "多半是缺少环境变量，补上后再检测。",
      output: "Error: ALLOWED_DIRS environment variable is not set" };
    host.state.pi.mcp_servers.push({ id: 2, name: "notion", transport: "sse", command: "", args: [], url: "https://mcp.notion.example/sse",
      env: [], headers: [], auth: "none", logged_in: false, include_tools: [], enabled: false });
  });
  await page.goto("/");
  const settings = await openAssistant(page);
  if (await fold(settings, "外部工具").getAttribute("aria-expanded") !== "true") await fold(settings, "外部工具").click();
  const row = settings.getByRole("listitem").filter({ has: page.getByText("filesystem", { exact: true }) });
  await row.getByRole("button", { name: "检测 filesystem", exact: true }).click();
  await expect(row.getByRole("status")).toHaveText("连接失败 · 进程启动后 1.2 秒退出（退出码 1）");
  await row.getByRole("button", { name: "查看详情", exact: true }).click();
  await expect(row.locator(".mcp-error")).toContainText("多半是缺少环境变量");
  await expect(row.locator(".mcp-error pre")).toHaveText("Error: ALLOWED_DIRS environment variable is not set");

  const old = settings.getByRole("listitem").filter({ has: page.getByText("notion", { exact: true }) });
  await expect(old).toContainText("旧式 SSE");
  await expect(old.getByRole("button", { name: "服务 notion", exact: true })).toBeDisabled();
  await expect(old.getByRole("button", { name: "检测 notion", exact: true })).toHaveCount(0);
  await shot(page, `${OUT}/mcp-list-1280-light.png`);
  await old.getByRole("button", { name: "改填地址 notion", exact: true }).click();
  const edit = page.getByRole("dialog", { name: "编辑外部工具", exact: true });
  await expect(edit.getByRole("note")).toContainText("这个地址已经连不上");
  await expect(edit.getByRole("radio", { name: /在线服务/ })).toHaveAttribute("aria-checked", "true");
  await edit.getByRole("button", { name: "换成 …/mcp", exact: true }).click();
  await expect(edit.getByLabel("地址", { exact: true })).toHaveValue("https://mcp.notion.example/mcp");
  await edit.getByRole("button", { name: "保存", exact: true }).click();
  await expect(edit).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__mockHost.args.upsert_mcp.input)).toMatchObject({ id: 2, transport: "http", url: "https://mcp.notion.example/mcp" });
});

test("browser sign-in starts after saving, can be cancelled, and turns the service on when done", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  const settings = await openAssistant(page);
  if (await fold(settings, "外部工具").getAttribute("aria-expanded") !== "true") await fold(settings, "外部工具").click();
  await settings.getByRole("button", { name: "添加服务", exact: true }).click();
  const sheet = page.getByRole("dialog", { name: "添加外部工具", exact: true });
  await sheet.getByRole("radio", { name: /在线服务/ }).click();
  await sheet.getByLabel("地址", { exact: true }).fill("https://mcp.linear.app/mcp");
  await sheet.getByRole("group", { name: "登录" }).getByRole("button", { name: "浏览器登录", exact: true }).click();
  await expect(sheet.getByRole("button", { name: "检测连接", exact: true })).toHaveCount(0);
  await sheet.getByRole("button", { name: "保存并登录", exact: true }).click();
  await expect(sheet).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__mockHost.args.upsert_mcp.input)).toMatchObject({ name: "linear", auth: "oauth", headers: [] });
  const row = settings.getByRole("listitem").filter({ has: page.getByText("linear", { exact: true }) });
  await expect(row.getByRole("status")).toHaveText("等你在浏览器里完成登录…");
  await expect(row.getByRole("button", { name: "服务 linear", exact: true })).toBeDisabled();
  await row.getByRole("button", { name: "取消登录 linear", exact: true }).click();
  await expect(row.getByRole("status")).toHaveText("需要登录 · 在浏览器里授权后才能使用");
  await row.getByRole("button", { name: "登录 linear", exact: true }).click();
  await expect(row.getByRole("status")).toHaveText("等你在浏览器里完成登录…");
  const id = await page.evaluate(() => (window as any).__mockHost.args.login_mcp.id);
  await page.evaluate((id) => (window as any).__mockHost.finishMcpLogin(id), id);
  await expect(row.getByRole("button", { name: "服务 linear", exact: true })).toHaveAttribute("aria-pressed", "true");
  await expect(row.getByRole("status")).toHaveText(/^已连接 · 开放 3 \/ 3 个工具/);
});

test("JSON import previews metadata, skips duplicates, and starts disabled", async ({ page }) => {
  await installMockHost(page);
  // 后端导入规则在 Rust 验证；这里只提供预览和成功写回的权威快照。
  const imported = mcpFixture({ id: 2, name: "docs", transport: "http", command: "", args: [], url: "https://example.test/mcp", headers: [{ name: "X-Key", has_value: true }], enabled: false });
  await page.addInitScript(({ imported }) => {
    const w = window as any;
    const original = w.__TAURI_INTERNALS__.invoke;
    w.__TAURI_INTERNALS__.invoke = async (command: string, args: any) => {
      if (["preview_mcp_import", "import_mcp"].includes(command)) {
        w.__mockHost.calls.push(command); w.__mockHost.args[command] = args;
        if (command === "import_mcp") w.__mockHost.state.pi.mcp_servers.push(imported);
        return [{ name: "filesystem", transport: "stdio", secret_count: 0, skipped: true }, { name: "docs", transport: "http", secret_count: 1, skipped: false }];
      }
      return original(command, args);
    };
  }, { imported });
  await page.goto("/");
  const settings = await openAssistant(page);
  if (await fold(settings, "外部工具").getAttribute("aria-expanded") !== "true") await fold(settings, "外部工具").click();
  await settings.getByRole("button", { name: "从 JSON 导入", exact: true }).click();
  const sheet = page.getByRole("dialog", { name: "从 JSON 导入外部工具", exact: true });
  const raw = '{"mcpServers":{"filesystem":{"command":"npx"},"docs":{"url":"https://example.test/mcp","headers":{"X-Key":"synthetic-secret"}}}}';
  const importButton = sheet.getByRole("button", { name: /^导入/ });
  await expect(importButton).toBeDisabled();
  await sheet.getByLabel("粘贴配置", { exact: true }).fill(raw);
  const preview = sheet.getByRole("list", { name: "将导入的服务" });
  await expect(preview.getByRole("listitem").filter({ hasText: "filesystem" })).toContainText("同名服务已存在，跳过");
  await expect(preview.getByRole("listitem").filter({ hasText: "docs" })).toContainText("1 个密钥项 · 将添加，默认关闭");
  // 改动文本后旧预览作废，停顿后重新预览，导入的是最新文本。
  await sheet.getByLabel("粘贴配置", { exact: true }).fill(raw + " ");
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.args.preview_mcp_import.json)).toBe(raw + " ");
  await expect(importButton).toHaveText("导入 1 个");
  await shot(page, `${OUT}/mcp-import-1280-light.png`);
  await importButton.click();
  await expect(sheet).toHaveCount(0);
  await expect(settings.getByRole("button", { name: "服务 docs", exact: true })).toHaveAttribute("aria-pressed", "false");
  await expect(settings.getByRole("button", { name: "编辑服务 filesystem", exact: true })).toHaveCount(1);
  expect(await page.evaluate(() => (window as any).__mockHost.args.import_mcp.json)).toBe(raw + " ");
});

test("restored MCP services name missing variables and accept replacements", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.evaluate(() => { (window as any).__mockHost.state.pi.mcp_servers[0].env = [{ name: "TOKEN", has_value: false }]; });
  const settings = await openAssistant(page);
  if (await fold(settings, "外部工具").getAttribute("aria-expanded") !== "true") await fold(settings, "外部工具").click();
  await expect(settings.getByText("需要重新填写：TOKEN", { exact: true })).toBeVisible();
  if (await fold(settings, "外部工具").getAttribute("aria-expanded") !== "true") await fold(settings, "外部工具").click();
  await settings.getByRole("button", { name: "编辑服务 filesystem", exact: true }).click();
  const sheet = page.getByRole("dialog", { name: "编辑外部工具", exact: true });
  await expect(sheet.getByLabel("环境变量值 1", { exact: true })).toHaveValue("");
  await sheet.getByLabel("环境变量值 1", { exact: true }).fill("synthetic-token");
  await sheet.getByRole("button", { name: "保存", exact: true }).click();
  await expect(sheet).toHaveCount(0);
  await expect(settings.getByText("需要重新填写：TOKEN", { exact: true })).toHaveCount(0);
});
