import { expect, test } from "./fixtures";
import { installMockHost, settingsTab } from "./mockHost";
import { shot } from "./shots";

test("diagnostics shows the log, switches level and exports; storage clears logs", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  const nav = settings.getByRole("navigation", { name: "设置分类" });
  const host = () => page.evaluate(() => (window as any).__mockHost as { calls: string[]; args: Record<string, any> });
  const count = async (command: string) => (await host()).calls.filter((call) => call === command).length;

  await settingsTab(nav, "diagnostics").click();
  const card = settings.getByRole("region", { name: "运行日志" });
  await expect(card).toContainText("/home/user/.local/share/rrss/logs");
  await expect(card).toContainText("正在记录");
  await expect(card).toContainText("当前文件 2.4 MB · 全部日志 7.8 MB");
  await expect(card).not.toContainText("RRSS_LOG");

  const detailed = card.getByRole("button", { name: "详细", exact: true });
  await detailed.click();
  await expect(detailed).toHaveAttribute("aria-pressed", "true");
  await expect(card).toContainText("重启后恢复默认");
  expect((await host()).args.log_set_level).toEqual({ detailed: true });

  await card.getByRole("button", { name: "导出诊断包" }).click();
  const confirm = page.getByRole("alertdialog", { name: "导出诊断包" });
  await expect(confirm).toContainText("不含数据库、设置和密钥");
  await confirm.getByRole("button", { name: "选择保存位置", exact: true }).click();
  await expect(card.getByRole("status").filter({ hasText: "诊断包已保存" })).toBeVisible();
  expect(await count("log_export")).toBe(1);
  for (const width of [1280, 560]) {
    await page.setViewportSize({ width, height: 900 });
    expect(await settings.locator(".drawer-body").evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
    await shot(page, `../../test-results/diagnostics-log/diagnostics-${width}.png`);
  }
  await page.setViewportSize({ width: 1280, height: 900 });

  await settingsTab(nav, "storage").click();
  const logs = settings.locator('[data-storage-item="logs"]');
  await expect(logs.locator(".storage-size")).toHaveText("7.8 MB");
  await logs.getByRole("button", { name: "清空", exact: true }).click();
  const clear = page.getByRole("alertdialog", { name: "清空运行日志？" });
  await expect(clear).toContainText("之后继续记录新的日志");
  await clear.getByRole("button", { name: "确认", exact: true }).click();
  await expect(logs.locator(".storage-size")).toHaveText("0");
  expect((await host()).args.storage_clean).toEqual({ action: "logs" });
});
