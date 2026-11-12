import { expect, test } from "./fixtures";
import { installMockHost } from "./mockHost";
import { readerReturnFixture } from "../src/test/fixtures/reader";

test("J/K shows the cached body and position before open returns, then reconciles without moving", async ({ page }) => {
  const fixture = readerReturnFixture(3);
  const first = fixture.readers[1];
  await installMockHost(page, { ...fixture, readerSequences: { 1: [first, { ...first, summary: "宿主核对后的摘要" }] } });
  await page.goto("/");
  await page.getByRole("article", { name: first.title, exact: true }).click();
  const reader = page.locator(".drawer.reader");
  const body = reader.locator(".drawer-body");
  const title = reader.locator(".reader-title");
  await expect(title).toHaveText(first.title);
  await body.evaluate(el => {
    const block = el.querySelector('[data-reader-document] > [data-block-id="8"]')!;
    el.scrollTop += block.getBoundingClientRect().top - el.getBoundingClientRect().top;
  });
  const offset = () => body.evaluate(el => {
    const block = el.querySelector('[data-reader-document] > [data-block-id="8"]')!;
    return Math.abs(block.getBoundingClientRect().top - el.getBoundingClientRect().top);
  });
  await expect.poll(offset).toBeLessThan(2);
  await page.keyboard.press("j");
  await expect(title).toHaveText(fixture.readers[2].title);
  const before = await page.evaluate(() => {
    const host = (window as any).__mockHost;
    host.holdOpen = true;
    return host.calls.filter((name: string) => name === "open_article").length;
  });
  await page.keyboard.press("k");
  await expect(title).toHaveText(first.title);
  await expect(reader.locator('[data-reader-document] > [data-block-id="8"]')).toContainText("Part 9.");
  await expect.poll(offset).toBeLessThan(2);
  await expect(reader).not.toContainText("宿主核对后的摘要");
  expect(await page.evaluate(() => (window as any).__mockHost.calls.filter((name: string) => name === "open_article").length)).toBe(before + 1);
  const element = await body.elementHandle();
  await page.evaluate(() => (window as any).__mockHost.releaseOpen());
  await expect(reader.locator(".reader-summary")).toContainText("宿主核对后的摘要");
  await expect.poll(offset).toBeLessThan(2);
  expect(await body.evaluate((el, original) => el === original, element)).toBe(true);
});

test("the next article is prefetched without marking it read, so J shows it before open returns", async ({ page }) => {
  const fixture = readerReturnFixture(3);
  const [first, second] = [fixture.readers[1], fixture.readers[2]];
  await installMockHost(page, fixture);
  await page.goto("/");
  await page.getByRole("article", { name: first.title, exact: true }).click();
  const reader = page.locator(".drawer.reader");
  await expect(reader.locator(".reader-title")).toHaveText(first.title);
  // 当前篇显示后取下一篇的只读快照：不走 open_article，列表里它仍是未读。
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.args.article_reader?.id)).toBe(second.id);
  await expect(page.getByRole("article", { name: second.title, exact: true })).toHaveClass(/\bunread\b/);
  await page.evaluate(() => { (window as any).__mockHost.holdOpen = true; });
  await page.keyboard.press("j");
  await expect(reader.locator(".reader-title")).toHaveText(second.title);
  await expect(page.locator("[data-reader-opening-skeleton]")).toHaveCount(0);
  await page.evaluate(() => (window as any).__mockHost.releaseOpen());
  await expect.poll(() => page.evaluate(() => (window as any).__mockHost.calls.filter((name: string) => name === "open_article").length)).toBe(2);
});
