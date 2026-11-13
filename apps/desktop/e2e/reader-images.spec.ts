import { readFileSync } from "node:fs";
import { expect, test } from "./fixtures";
import { installMockHost, MOCK_READER } from "./mockHost";

const para = (text: string) => ({ t: "p" as const, inlines: [{ t: "t" as const, text }] });
const image = (src: string) => ({ t: "img" as const, src, alt: "" });
const cover = readFileSync("../../testdata/render/cover.png");

test("sized images reserve their ratio before decoding, and the opened hit stays put while unsized ones arrive", async ({ page }) => {
  // 第一张带宿主读出的宽高；后两张读不出（模拟老缓存 / 未知格式），加载后才有高度。
  const document = { v: 1, blocks: [
    image("sized.png"), para("开头一段。"), image("loose-1.png"), image("loose-2.png"),
    ...Array.from({ length: 8 }, (_, i) => para(`中间第 ${i + 1} 段，把命中段推到首屏以外。`.repeat(4))),
    para("这里有缓存命中。"),
    ...Array.from({ length: 8 }, (_, i) => para(`后面第 ${i + 1} 段。`.repeat(6))),
  ] };
  const media = [
    { src: "sized.png", href: "/e2e-img/sized.png", width: 480, height: 240, error: "" },
    { src: "loose-1.png", href: "/e2e-img/loose-1.png", width: null, height: null, error: "" },
    { src: "loose-2.png", href: "/e2e-img/loose-2.png", width: null, height: null, error: "" },
  ];
  let release!: () => void;
  const gate = new Promise<void>(resolve => { release = resolve; });
  await page.route("**/e2e-img/**", async route => { await gate; await route.fulfill({ contentType: "image/png", body: cover }); });
  await installMockHost(page, { readers: { 1: { ...MOCK_READER, document, media } } });
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  await expect(page.getByRole("article").first()).toBeVisible();
  await page.keyboard.press("Control+k");
  const search = page.getByRole("dialog", { name: "搜索文章" });
  await search.getByRole("searchbox").fill("缓存");
  await search.getByRole("option").first().click();

  const stage = page.locator(".reader-stage");
  const hit = stage.locator('[data-block-id="12"]');
  await expect(hit).toHaveClass(/reader-search-hit/);
  // 还没解码：带尺寸的图已按 480×240 占位，不铺满正文列。
  const sized = stage.locator('[data-block-id="0"] img');
  expect(await sized.evaluate((img: HTMLImageElement) => img.complete)).toBe(false);
  const box = (await sized.boundingBox())!;
  expect(box.width).toBeCloseTo(480, 0);
  expect(box.height).toBeCloseTo(240, 0);

  const top = () => hit.evaluate(el => Math.round(el.getBoundingClientRect().top - el.closest(".drawer-body")!.getBoundingClientRect().top));
  const before = await top();
  release();
  await expect.poll(() => stage.locator("img").evaluateAll(list => list.every(img => (img as HTMLImageElement).complete && (img as HTMLImageElement).naturalWidth > 0))).toBe(true);
  // 两张没尺寸的图各长出 240px；用户还没滚动，命中段仍停在原处。
  await expect.poll(top).toBe(before);

  // 用户滚过之后，上方再变高也不把他拉回命中段。
  await hit.hover();
  await page.mouse.wheel(0, -300);
  await expect.poll(top).toBeGreaterThan(before + 200);
  const scrolled = await top();
  await stage.locator('[data-block-id="1"]').evaluate(el => { (el as HTMLElement).style.paddingTop = "100px"; });
  await expect.poll(top).toBe(scrolled + 100);
});
