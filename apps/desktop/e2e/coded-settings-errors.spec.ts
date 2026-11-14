import { expect, test } from "./fixtures";
import type { UiError } from "../src/bindings";
import { installMockHost, settingsTab } from "./mockHost";

test("coded connection validation retains the draft and hides diagnostic detail", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");

  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  await settingsTab(settings, "model").click();
  await settings.getByRole("button", { name: "添加连接", exact: true }).first().click();
  const editor = page.getByRole("dialog", { name: "添加连接", exact: true });
  await page.evaluate(() => {
    const host = (window as any).__TAURI_INTERNALS__;
    const original = host.invoke;
    host.invoke = async (command: string, args: unknown) => {
      if (command === "save_ai_settings") {
        const error = { code: "provider_url_invalid", detail: "diagnostic canary" } satisfies UiError;
        (window as any).__validationFailure = error;
        throw error;
      }
      return original(command, args);
    };
  });
  const advanced = editor.locator(".settings-dialog-advanced");
  if (await advanced.getAttribute("open") === null) await advanced.locator("summary").first().click();
  const address = editor.locator(".settings-dialog-advanced input").first();
  await address.fill("not-a-url");
  await editor.getByRole("button", { name: "保存并选模型", exact: true }).click();
  await expect(editor.getByRole("alert")).toBeVisible();
  expect(await page.evaluate(() => (window as any).__validationFailure)).toMatchObject({ code: "provider_url_invalid" });
  await editor.locator(".ai-error-details > summary").click();
  await expect(editor.locator(".ai-error-details pre")).not.toContainText("canary");
  await expect(address).toHaveValue("not-a-url");
  await expect(page.getByRole("dialog", { name: "添加模型", exact: true })).toHaveCount(0);
});

test("coded setting and rule validation retain both rule-page drafts", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");

  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  await settingsTab(settings, "rules").click();
  const threshold = settings.getByRole("spinbutton", { name: "隐藏阈值", exact: true });
  await expect(threshold).toHaveValue("-10");
  await page.evaluate(() => {
    const host = (window as any).__TAURI_INTERNALS__;
    const original = host.invoke;
    const failures: UiError[] = [];
    (window as any).__validationFailures = failures;
    host.invoke = async (command: string, args: Record<string, unknown> = {}) => {
      const error = command === "set_setting" && args.id === "hide_threshold"
        ? { code: "setting_out_of_range", params: { min: -100, max: 0 }, detail: "diagnostic canary" } satisfies UiError
        : command === "create_rule"
          ? { code: "rule_regex_invalid", detail: "diagnostic canary" } satisfies UiError : null;
      if (error) { failures.push(error); throw error; }
      return original(command, args);
    };
  });
  await threshold.fill("1");
  await threshold.press("Tab");
  await expect(settings.getByRole("alert")).toBeVisible();
  await expect(threshold).toHaveValue("1");
  expect(await page.evaluate(() => (window as any).__validationFailures.at(-1))).toMatchObject({
    code: "setting_out_of_range", params: { min: -100, max: 0 },
  });
  await expect(settings.getByRole("alert")).not.toContainText("canary");
  await settings.getByRole("button", { name: "匹配位置", exact: true }).click();
  await page.getByRole("menuitemradio", { name: "标题正则", exact: true }).click();
  const pattern = settings.getByRole("textbox", { name: "匹配内容", exact: true });
  await pattern.fill("[");
  await settings.getByRole("button", { name: "添加", exact: true }).click();
  await expect(settings.getByRole("alert")).toBeVisible();
  expect(await page.evaluate(() => (window as any).__validationFailures.at(-1))).toMatchObject({ code: "rule_regex_invalid" });
  await expect(pattern).toHaveValue("[");
  await expect(threshold).toHaveValue("1");
  await expect(settings.getByRole("alert")).not.toContainText("canary");
});
