import { expect, test, type Page } from "./fixtures";
import { installMockHost, settingsTab } from "./mockHost";
import { readerReturnFixture } from "../src/test/fixtures/reader";

// 后台驻留：关闭先拦下，按设置「关闭窗口时」询问 / 隐藏到托盘 / 退出；从托盘唤回时焦点直接采用最新内容。
type HostWindow = { closed: boolean; hidden: boolean; trayLabels: unknown; closeChoices: Array<{ choice: string; remember: boolean }> };
const hostWindow = (page: Page) => page.evaluate(() => (window as any).__mockHost.window as HostWindow);

test("closing asks once, remembers hiding to the tray, and restoring adopts the latest focus", async ({ page }) => {
  await page.setViewportSize({ width: 1200, height: 560 });
  const fixture = readerReturnFixture(24);
  await installMockHost(page, { focus: fixture.focus });
  await page.goto("/");
  await expect(page.getByRole("article").first()).toBeVisible();
  // 托盘菜单文字由前端交给宿主。
  await expect.poll(async () => (await hostWindow(page)).trayLabels).not.toBeNull();

  const closeKey = page.locator(".window-controls .window-close");
  const dialog = page.getByRole("alertdialog");
  await closeKey.click();
  await expect(dialog).toBeVisible();
  await expect(dialog.locator("[data-close-tray]")).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  expect(await hostWindow(page)).toMatchObject({ closed: false, hidden: false, closeChoices: [] });

  await closeKey.click();
  await dialog.getByRole("checkbox").check();
  await dialog.locator("[data-close-tray]").click();
  await expect(dialog).toHaveCount(0);
  expect(await hostWindow(page)).toMatchObject({ hidden: true, closeChoices: [{ choice: "tray", remember: true }] });

  // 记住之后不再询问。
  await page.evaluate(() => { (window as any).__mockHost.window.hidden = false; });
  await closeKey.click();
  await expect.poll(async () => (await hostWindow(page)).hidden).toBe(true);
  await expect(dialog).toHaveCount(0);

  // 在托盘期间抓到新文章；唤回时直接采用并回到顶部，不显示「有 N 篇新内容」。
  const scroll = page.getByTestId("focus-scroll");
  await scroll.evaluate((el) => { el.scrollTop = 1200; });
  await page.evaluate((row) => {
    const host = (window as any).__mockHost;
    host.state.focus.articles.unshift(row);
    host.emit("window:restored", null);
  }, { ...fixture.focus.articles[0], id: 900, title: "Arrived while in the tray" });
  await expect(page.getByRole("article", { name: "Arrived while in the tray", exact: true })).toBeVisible();
  await expect(scroll).toHaveJSProperty("scrollTop", 0);
  await expect(page.getByTestId("focus-new-content")).toHaveCount(0);

  // 设置里显示记住的选择，可以改回「每次询问」。
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  await settingsTab(settings.getByRole("navigation", { name: "设置分类" }), "advanced").click();
  const choices = settings.getByRole("region", { name: "窗口" }).locator(".chip-btn");
  await expect(choices).toHaveCount(3);
  await expect(choices.nth(1)).toHaveAttribute("aria-pressed", "true");
  await choices.nth(0).click();
  await expect(choices.nth(0)).toHaveAttribute("aria-pressed", "true");
});
