import { expect, test } from "./fixtures";
import { installMockHost, MOCK_DIGESTS, MOCK_FOCUS } from "./mockHost";
import { settled } from "./exposure";
import type { ExposureInput } from "../src/bindings";

test("digest source lists record exposures only while the digest is topmost", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.emulateMedia({ reducedMotion: "reduce" });
  const articles = Array.from({ length: 48 }, (_, i) => ({
    ...MOCK_FOCUS.articles[0], id: i + 1, title: i === 47 ? "搜索曝光哨兵" : `普通文章 ${i + 1}`,
    summary: "让搜索哨兵留在焦点首屏之外。".repeat(6),
  }));
  const sources = MOCK_DIGESTS[0].source_index.slice(0, 2).map((source, i) => ({
    ...source, article_id: 71 + i, title: `简报曝光 ${71 + i}`,
  }));
  const digest = { ...MOCK_DIGESTS[0], source_index: sources,
    items: [{ block_id: 1, sources: [sources[1]] }], unlocated_sources: [],
  };
  await installMockHost(page, {
    focus: { ...MOCK_FOCUS, articles, has_more: false }, digests: [digest],
  });
  await page.clock.install();
  await page.goto("/");
  await page.getByRole("button", { name: "简报", exact: true }).click();
  const recorded = () => page.evaluate<ExposureInput[]>(() => (window as any).__mockHost.signals.exposures);
  const digestExposures = async () => (await recorded()).filter(row => sources.some(source => source.article_id === row.article_id))
    .map(({ article_id, surface, position }) => ({ article_id, surface, position }));

  // 从来源列表出现到搜索盖上这一段暂停时钟：机器再慢，简报来源也攒不够停留时间。
  // pauseAt 只能往后拨；留出余量，免得调用到达前页面时钟已走过目标。此时列表未展开，往前拨不产生曝光。
  await page.clock.pauseAt(await page.evaluate(() => Date.now() + 5000));
  await page.locator(".digest-source-index summary").click();
  for (const row of await page.locator(".digest-source-index li").all()) await expect(row).toBeInViewport({ ratio: 0.5 });
  await page.keyboard.press("Control+k");
  const search = page.getByRole("dialog", { name: "搜索文章" });
  await expect(search).toBeVisible();
  await page.clock.resume();
  await search.getByRole("searchbox").fill("搜索曝光哨兵");
  await expect(search.getByRole("option")).toHaveCount(1);
  // 上层搜索写出曝光，说明停留与批量写入都已走过；此时简报来源不能有曝光。
  await expect.poll(settled(page, async () => (await recorded()).filter(row => row.article_id === 48)))
    .toMatchObject([{ article_id: 48, surface: "search", position: 0 }]);
  expect(await digestExposures()).toEqual([]);

  await page.keyboard.press("Escape");
  await expect(search).toHaveCount(0);
  await page.mouse.move(10, 10);
  await expect.poll(settled(page, digestExposures)).toEqual([
    { article_id: 71, surface: "digest", position: 0 },
    { article_id: 72, surface: "digest", position: 1 },
  ]);
});
