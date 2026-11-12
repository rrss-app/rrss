import { expect, test } from "./fixtures";
import { installMockHost, MOCK_FOCUS, MOCK_READER } from "./mockHost";
import { settled } from "./exposure";
import type { ExposureInput } from "../src/bindings";

test("drawer and search record sustained article exposures with their own positions", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.emulateMedia({ reducedMotion: "reduce" });
  const articles = Array.from({ length: 48 }, (_, i) => ({
    ...MOCK_FOCUS.articles[0], id: i + 1, title: `普通文章 ${i + 1}`,
    summary: "把列表后面的文章留在焦点首屏之外。".repeat(6),
  }));
  const searchArticles = articles.slice(-2).map(row => ({ ...row, title: `搜索曝光 ${row.id}` }));
  const cluster = { ...articles[0], id: 9000, row_kind: 1, cluster_id: 9000, cluster_label: "曝光合辑", cluster_size: 2 };
  const members = [71, 72].map(id => ({ ...articles[0], id, title: `抽屉曝光 ${id}`, cluster_id: cluster.cluster_id }));
  // 合辑成员起初折叠，搜索文章在长列表末尾，两组都不会被焦点首屏抢先曝光。
  await installMockHost(page, {
    focus: { ...MOCK_FOCUS, articles: [cluster, ...articles.slice(0, -2), ...searchArticles], has_more: false },
    clusterArticles: { [cluster.cluster_id]: members },
    readers: { 1: { ...MOCK_READER, id: 1, title: articles[0].title } },
  });
  await page.clock.install();
  await page.goto("/");
  await expect(page.getByRole("article").first()).toBeVisible();
  const exposures = () => page.evaluate<ExposureInput[]>(() => (window as any).__mockHost.signals.exposures);
  const recorded = async (ids: number[]) => (await exposures()).filter(row => ids.includes(row.article_id))
    .map(({ article_id, surface, position }) => ({ article_id, surface, position }));

  await page.getByRole("article").first().click();
  await expect(page.locator(".drawer.reader")).toBeVisible();
  await page.keyboard.press("l");
  const drawer = page.locator(".reader-article-drawer");
  await expect(drawer).toBeVisible();
  await drawer.getByRole("button", { name: /合辑 · 曝光合辑/ }).click();
  for (const row of members) await expect(drawer.locator(`[data-reader-drawer-row="${row.id}"]`)).toBeInViewport({ ratio: 0.5 });
  for (const row of searchArticles) await expect(drawer.locator(`[data-reader-drawer-row="${row.id}"]`)).not.toBeInViewport();
  await expect.poll(settled(page, () => recorded(members.map(row => row.id)))).toEqual(
    members.map((row, position) => ({ article_id: row.id, surface: "drawer", position })),
  );
  expect((await exposures()).some(row => row.article_id === cluster.id)).toBe(false);
  expect(await recorded(searchArticles.map(row => row.id))).toEqual([]);

  await page.keyboard.press("Control+k");
  const search = page.getByRole("dialog", { name: "搜索文章" });
  await search.getByRole("searchbox").fill("搜索曝光");
  await expect(search.getByRole("option")).toHaveCount(searchArticles.length);
  for (const row of searchArticles) await expect(search.getByRole("option").filter({ hasText: row.title })).toBeInViewport({ ratio: 0.5 });
  await expect.poll(settled(page, () => recorded(searchArticles.map(row => row.id)))).toEqual(
    searchArticles.map((row, position) => ({ article_id: row.id, surface: "search", position })),
  );
  expect((await exposures()).some(row => row.article_id === cluster.id)).toBe(false);
});
