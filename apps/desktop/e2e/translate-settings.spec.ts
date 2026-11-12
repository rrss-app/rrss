import { expect, test } from "./fixtures";
import { installMockHost, MOCK_TRANSLATE, settingsTab } from "./mockHost";
import { shot } from "./shots";

// 复用现有模拟宿主，仅在本用例覆盖翻译写入/可控失败，不扩展全局夹具。
test.beforeEach(async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.evaluate(({ translate }) => {
    const host = (window as any).__TAURI_INTERNALS__;
    const original = host.invoke;
    const state = {
      translate, saves: 0, tests: 0, reads: 0, saveFail: false, testFail: false, holdSave: false,
      releaseSave: (() => {}) as () => void,
    };
    (window as any).__translationTest = state;
    host.invoke = async (command: string, args: Record<string, any> = {}) => {
      switch (command) {
        case "translate_page": state.reads++; return structuredClone(state.translate);
        case "save_translate_settings":
          state.saves++;
          if (state.saveFail) throw new Error("database is locked");
          if (state.holdSave) await new Promise<void>(resolve => { state.releaseSave = resolve; });
          await original(command, args);
          Object.assign(state.translate, await original("translate_page"));
          return;
        case "test_translate":
          state.tests++;
          if (state.testFail) throw new Error("service offline");
          return "测试译文";
        case "save_model_bindings": {
          const saved = await original(command, args);
          return saved;
        }
        default: return original(command, args);
      }
    };
  }, { translate: MOCK_TRANSLATE });
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await settingsTab(page, "translate").click();
  await expect(page.getByLabel("语言代码", { exact: true })).toHaveValue("zh");
});

test("translation drafts guard category, back, Escape and browser history until discarded", async ({ page }) => {
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  const nav = settings.getByRole("navigation", { name: "设置分类" });
  await settings.locator("summary").filter({ hasText: /^其他语言$/ }).click();
  const language = page.getByLabel("语言代码", { exact: true });
  const confirm = page.getByRole("alertdialog", { name: "翻译设置有未保存的修改" });
  await language.fill("fr");
  await settingsTab(nav, "rules").click();
  await expect(confirm).toBeVisible();
  await confirm.getByRole("button", { name: "继续编辑" }).click();
  await expect(language).toHaveValue("fr");

  await settings.getByRole("button", { name: "返回阅读", exact: true }).click();
  await expect(confirm).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(confirm).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(confirm).toBeVisible();
  await confirm.getByRole("button", { name: "继续编辑" }).click();

  await page.goBack();
  await expect(confirm).toBeVisible();
  await shot(page, "../../test-results/f4-1-translate/dirty-leave.png");
  await confirm.getByRole("button", { name: /^不保存/ }).click();
  await expect(settings).toHaveCount(0);
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await expect(language).toHaveValue("zh");
  await settings.locator("summary").filter({ hasText: /^其他语言$/ }).click();

  await language.fill("fr");
  await page.getByRole("button", { name: "保存", exact: true }).click();
  await expect(page.getByText("翻译设置已保存。", { exact: true })).toBeVisible();
  await settingsTab(nav, "rules").click();
  await expect(confirm).toHaveCount(0);
  await settingsTab(nav, "translate").click();
  await expect(language).toHaveValue("fr");
  await expect(page.getByText(/有未保存的修改/)).toHaveCount(0);
  await shot(page, "../../test-results/f4-1-translate/saved-reentry.png");
});

test("failed saves keep drafts and save-and-test blocks navigation without duplicate writes", async ({ page }) => {
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  const nav = settings.getByRole("navigation", { name: "设置分类" });
  await settings.locator("summary").filter({ hasText: /^其他语言$/ }).click();
  const language = page.getByLabel("语言代码", { exact: true });
  await language.fill("de");
  await page.evaluate(() => { (window as any).__translationTest.saveFail = true; });
  await page.getByRole("button", { name: "测试连接", exact: true }).click();
  await expect(settings.getByRole("alert")).toContainText("保存失败，未开始测试");
  await expect(language).toHaveValue("de");
  expect(await page.evaluate(() => (window as any).__translationTest.tests)).toBe(0);
  await settingsTab(nav, "rules").click();
  await page.getByRole("alertdialog").getByRole("button", { name: "继续编辑" }).click();

  await page.evaluate(() => { Object.assign((window as any).__translationTest, { saveFail: false, holdSave: true, testFail: true }); });
  const test = page.getByRole("button", { name: "测试连接", exact: true });
  await test.click();
  // 处理中按钮不换文字（不跳宽），只转圈。
  await expect(test).toHaveAttribute("aria-busy", "true");
  await expect(test).toBeDisabled();
  await settingsTab(nav, "rules").click();
  await expect(page.locator(".toast")).toContainText("正在保存或测试翻译设置");
  await settings.getByRole("button", { name: "返回阅读", exact: true }).click();
  await expect(settings).toBeVisible();
  await page.goBack();
  await expect(settings.getByRole("heading", { name: "翻译", exact: true })).toBeVisible();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
  await page.evaluate(() => (window as any).__translationTest.releaseSave());
  await expect(settings.getByRole("alert")).toContainText("设置已保存，但测试失败");
  await expect(language).toHaveValue("de");
  expect(await page.evaluate(() => [(window as any).__translationTest.saves, (window as any).__translationTest.tests])).toEqual([2, 1]);
  await expect(page.getByText(/有未保存的修改/)).toHaveCount(0);
  await page.setViewportSize({ width: 880, height: 720 });
  await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
  expect(await settings.locator(".settings-body").evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
  await shot(page, "../../test-results/f4-1-translate/saved-test-failed-dark.png");
  await settingsTab(nav, "rules").click();
  await expect(page.getByRole("alertdialog")).toHaveCount(0);
});

test("the translation model is picked right here and is the same use as on the model page", async ({ page }) => {
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  const model = settings.getByRole("button", { name: "翻译的模型", exact: true });
  await expect(model).toHaveText("未选择");
  await model.click();
  await page.getByRole("dialog", { name: "翻译的模型" }).getByRole("option", { name: /Sonnet/ }).click();
  await expect(settings.locator(".translate-model").getByRole("status")).toHaveText("已保存：翻译改用 Sonnet。");
  expect(await page.evaluate(() => (window as any).__mockHost.args.save_model_bindings.args))
    .toEqual({ bindings: [{ task: "article_translate", provider_id: 2, model: "anthropic/claude-sonnet-4" }] });
  await settingsTab(settings, "model").click();
  await expect(settings.locator('.use-row[data-group="translate"]').getByRole("button", { name: "翻译的模型", exact: true })).toHaveText("Sonnet OpenRouter");
  await settingsTab(settings, "translate").click();
  await expect(page.getByLabel("语言代码", { exact: true })).toHaveValue("zh");
});

test("saved translation keys stay at the host and replacement clears the input after saving", async ({ page }) => {
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  // 宿主侧已有密钥（如恢复备份后）；点选服务即保存并回读到它。
  await page.evaluate(() => {
    const w = window as any;
    const snapshot = { state: "set", hint: "9876" };
    w.__mockHost.state.translate.http_key = snapshot;
    w.__translationTest.translate.http_key = snapshot;
  });
  await settings.getByRole("button", { name: "HTTP 服务", exact: true }).click();
  await expect(settings.getByText("已保存 · 9876", { exact: true })).toBeVisible();
  // 点选即存成功不另报「已保存」。
  await expect(settings.locator(".translate-actions .form-status")).toHaveCount(0);
  await expect(settings.locator('input[aria-label="API Key（可选）"]')).toHaveCount(0);
  await settings.getByLabel("HTTP 服务地址", { exact: true }).fill("http://127.0.0.1:5050");
  await settings.getByRole("button", { name: "保存", exact: true }).click();
  await expect(settings.getByText("翻译设置已保存。", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => (window as any).__mockHost.args.save_translate_settings.args.http_key)).toBeUndefined();
  await expect(settings.getByText("已保存 · 9876", { exact: true })).toBeVisible();
  await settings.getByRole("button", { name: "更换API Key（可选）", exact: true }).click();
  await settings.getByLabel("API Key（可选）", { exact: true }).fill("synthetic-replacement-4321");
  await settings.getByRole("button", { name: "保存", exact: true }).click();
  await expect(settings.getByText("已保存 · synt…4321", { exact: true })).toBeVisible();
  await expect(settings.locator('input[aria-label="API Key（可选）"]')).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__translationTest.translate.http_key)).toEqual({ state: "set", hint: "synt…4321" });
  for (const width of [1280, 560]) {
    await page.setViewportSize({ width, height: 800 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate(t => { document.documentElement.dataset.theme = t; }, theme);
      expect(await settings.locator(".drawer-body").evaluate(el => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
      await shot(page, `../../test-results/translation-secrets/${width}-${theme}.png`);
    }
  }
  await page.evaluate(() => {
    const w = window as any;
    w.__mockHost.state.translate.http_key = { state: "missing", hint: "" };
    w.__translationTest.translate.http_key = { state: "missing", hint: "" };
  });
  await settings.getByRole("button", { name: "DeepL 官方", exact: true }).click();
  await settings.getByRole("button", { name: "HTTP 服务", exact: true }).click();
  await expect(settings.getByText("备份未包含此密钥，需要重新填写 API Key（可选）。", { exact: true })).toBeVisible();
});
