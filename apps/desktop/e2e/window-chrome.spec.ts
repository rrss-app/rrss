import { expect, test, type Locator, type Page } from "./fixtures";
import { installMockHost } from "./mockHost";

// 自绘标题栏：主窗口没有系统标题栏，三键固定在右上角；右上角放了内容的顶栏要给三键让位。
const controls = (page: Page) => page.locator(".window-controls");
const host = (page: Page) => page.evaluate(() => (window as any).__mockHost.window as { minimized: number; maximized: boolean; closed: boolean });

/** `bar` 里最靠右的可点元素不伸进三键的范围。 */
async function clearOfControls(page: Page, bar: Locator) {
  const keys = (await controls(page).boundingBox())!;
  const right = await bar.evaluate((el) => Math.max(...[...el.querySelectorAll("button, a, input")]
    .map((node) => node.getBoundingClientRect()).filter((box) => box.width > 0).map((box) => box.right)));
  expect(right).toBeLessThanOrEqual(keys.x);
}

test("the window keys minimize, maximize and restore, and close the window", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await installMockHost(page);
  await page.goto("/");
  const keys = controls(page);
  await keys.getByRole("button", { name: "最小化" }).click();
  expect((await host(page)).minimized).toBe(1);
  await keys.getByRole("button", { name: "最大化" }).click();
  // 最大化后按窗口实际状态换成「还原」，再按一次回到「最大化」。
  await expect(keys.getByRole("button", { name: "还原" })).toBeVisible();
  expect((await host(page)).maximized).toBe(true);
  await keys.getByRole("button", { name: "还原" }).click();
  await expect(keys.getByRole("button", { name: "最大化" })).toBeVisible();
  await keys.getByRole("button", { name: "关闭窗口" }).click();
  expect((await host(page)).closed).toBe(true);
});

for (const style of ["windows", "gnome"] as const) {
  test(`top bars leave room for the ${style} window keys`, async ({ page }) => {
    await page.setViewportSize({ width: 960, height: 760 });
    await installMockHost(page, { windowControls: style });
    await page.goto("/");
    await expect(controls(page)).toBeVisible();
    // 焦点工具栏的版心在窄窗口里会贴近右缘。
    await clearOfControls(page, page.locator(".focus > .toolbar"));
    await page.getByRole("button", { name: "简报", exact: true }).click();
    await clearOfControls(page, page.locator(".digest .toolbar"));
    await page.getByRole("button", { name: "今日焦点", exact: true }).click();
    await page.getByRole("article").first().click();
    await clearOfControls(page, page.locator(".drawer.reader .reader-head"));
    await page.getByRole("button", { name: "返回" }).first().click();
    // 设置没有固定顶栏：内容区上方留出拖动带，正文不从三键下面经过。
    await page.getByRole("button", { name: "设置" }).click();
    const body = (await page.locator(".settings-body").boundingBox())!;
    expect(body.y).toBeGreaterThanOrEqual((await controls(page).boundingBox())!.height);
  });
}
