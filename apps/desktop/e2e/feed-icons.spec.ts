import { readFileSync } from "node:fs";
import { test, expect } from "./fixtures";
import { installMockHost, settingsTab } from "./mockHost";
import { shot } from "./shots";

const icon = "http://rrss.localhost/media/9001";
const broken = "http://rrss.localhost/media/9002";

test("cached icons render, failures keep avatar size, and disabling falls back", async ({ page }) => {
  await installMockHost(page, { feedIconHrefs: { 1: icon, 3: broken } });
  await page.route(icon, route => route.fulfill({ contentType: "image/png", body: readFileSync("../../testdata/render/cover.png") }));
  let releaseBroken!: () => void;
  const held = new Promise<void>(resolve => { releaseBroken = resolve; });
  await page.route(broken, async route => { await held; await route.fulfill({ contentType: "image/png", body: "broken" }); });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/");
  const imageAvatar = page.locator(`.article .source-avatar:has(img[src="${icon}"])`).first();
  await expect(imageAvatar).toHaveAttribute("data-source-avatar", "image");
  await expect.poll(() => imageAvatar.locator("img").evaluate(el => (el as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
  const failingCard = page.locator(".article").filter({ hasText: "Byte Harbor" }).first();
  const failingAvatar = failingCard.locator(`.source-avatar:has(img[src="${broken}"])`);
  await expect(failingAvatar).toHaveAttribute("data-source-avatar", "image");
  const before = await failingAvatar.boundingBox();
  releaseBroken();
  const fallback = failingCard.locator(".source-avatar");
  await expect(fallback).toHaveAttribute("data-source-avatar", "initial");
  await expect(fallback.locator("img")).toHaveCount(0);
  const after = await fallback.boundingBox();
  expect(after?.width).toBe(before?.width);
  expect(after?.height).toBe(before?.height);

  for (const width of [1280, 560]) {
    await page.setViewportSize({ width, height: 800 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme);
      await shot(page, `../../test-results/worker/feed-icons/focus-${width}-${theme}.png`);
    }
  }
  await page.setViewportSize({ width: 1280, height: 800 });
  // 阅读器顶部与搜索结果用同一份图标，不退回首字。
  const iconTitle = await page.evaluate(() => (window as any).__mockHost.state.focus.articles
    .find((a: { feed_id: number; row_kind: number }) => a.feed_id === 1 && a.row_kind === 0).title as string);
  await page.getByRole("article", { name: iconTitle, exact: true }).click();
  const reader = page.locator(".drawer.reader");
  const readerIcon = reader.locator(`.reader-source .source-avatar img[src="${icon}"]`);
  await expect(readerIcon).toBeVisible();
  // 图标必须正好铺满头像框：阅读器正文的 img 规则曾把它挤出圆框。
  const [frame, image] = [await reader.locator(".reader-source .source-avatar").boundingBox(), await readerIcon.boundingBox()];
  for (const key of ["x", "y", "width", "height"] as const) expect(Math.abs(image![key] - frame![key])).toBeLessThan(0.5);
  await shot(reader.locator(".reader-source"), "../../test-results/feed-icons/reader-source.png");
  await page.keyboard.press("Escape");
  await expect(reader).toHaveCount(0);
  await page.keyboard.press("Control+k");
  const search = page.getByRole("dialog", { name: "搜索文章" });
  await search.getByRole("searchbox").fill(iconTitle.slice(0, 6));
  await expect(search.locator(`.source-avatar img[src="${icon}"]`).first()).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(search).toHaveCount(0);
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  await settingsTab(settings, "subscribe").click();
  const source = settings.locator(`.feed-row .source-avatar:has(img[src="${icon}"])`);
  await expect(source).toHaveAttribute("data-source-avatar", "image");
  await expect.poll(() => source.locator("img").evaluate(el => (el as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
  for (const width of [1280, 560]) {
    await page.setViewportSize({ width, height: 800 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme);
      await shot(page, `../../test-results/worker/feed-icons/subscribe-${width}-${theme}.png`);
    }
  }
  const toggle = settings.getByRole("button", { name: "显示网站图标" });
  await expect(toggle).toHaveAttribute("aria-pressed", "true");
  await toggle.click();
  await expect(toggle).toHaveAttribute("aria-pressed", "false");
  await expect(settings.locator('.source-avatar[data-source-avatar="image"]')).toHaveCount(0);
  await page.keyboard.press("Escape");
  await expect(page.locator('.article .source-avatar[data-source-avatar="image"]')).toHaveCount(0);
});
