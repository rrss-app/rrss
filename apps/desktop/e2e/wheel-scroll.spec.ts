import { expect, test, type Locator, type Page } from "./fixtures";
import { installMockHost, settingsTab } from "./mockHost";

/** 最近的、真有内容可滚的祖先；断言滚轮落到了它身上。 */
const scroller = (inner: Locator) => inner.evaluateHandle((el) => {
  let node = el.parentElement;
  while (node && !(node.scrollHeight > node.clientHeight + 1 && /auto|scroll/.test(getComputedStyle(node).overflowY))) node = node.parentElement;
  return node!;
});

async function wheelOver(page: Page, inner: Locator, deltaY: number) {
  const box = (await inner.boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.wheel(0, deltaY);
}

test("wheel over a code block without its own scrollbar keeps scrolling the article", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 600 });
  await installMockHost(page);
  await page.goto("/");
  await page.locator(".article").first().click();
  const code = page.locator(".drawer.reader [data-reader-document] pre").first();
  await code.scrollIntoViewIfNeeded();
  const body = await scroller(code);
  const before = await body.evaluate((el) => el.scrollTop);
  await wheelOver(page, code, 240);
  await expect.poll(() => body.evaluate((el) => el.scrollTop)).toBeGreaterThan(before);
});

test("wheel over a short inner box in settings scrolls the settings page", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 600 });
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  await settingsTab(settings, "assistant").click();
  // 助手说明的输入框是 overflow:auto，但内容很短、没有滚动条。
  const box = settings.locator("textarea").first();
  await box.scrollIntoViewIfNeeded();
  expect(await box.evaluate((el) => el.scrollHeight <= el.clientHeight + 1)).toBe(true);
  const page_ = await scroller(box);
  const before = await page_.evaluate((el) => el.scrollTop);
  await wheelOver(page, box, 200);
  await expect.poll(() => page_.evaluate((el) => el.scrollTop)).toBeGreaterThan(before);
});
