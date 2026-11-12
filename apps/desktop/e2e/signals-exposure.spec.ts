import { expect, test } from "./fixtures";
import { installMockHost, MOCK_FOCUS } from "./mockHost";
import { settleExposures, settled } from "./exposure";
import type { ExposureInput } from "../src/bindings";

test("focus records only sustained visible articles, with article positions and no duplicates", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 720 });
  await page.emulateMedia({ reducedMotion: "reduce" });
  const articles = Array.from({ length: 48 }, (_, i) => ({
    ...MOCK_FOCUS.articles[0], id: i + 1, title: `曝光文章 ${i + 1}`,
    summary: "焦点卡片的摘要只在持续可见时计入曝光。".repeat(4),
  }));
  // 首行合辑不算文章，也不占文章 position。
  const cluster = { ...articles[0], id: 9000, row_kind: 1, cluster_id: 9000, cluster_size: 2, title: "合辑标题" };
  const rows = [cluster, ...articles];
  await installMockHost(page, { focus: { ...MOCK_FOCUS, articles: rows } });
  await page.clock.install();
  await page.goto("/");
  const focus = page.locator(".focus");
  await expect(focus.getByRole("article").first()).toBeVisible();
  const visibleIndices = await focus.locator(".scroll").evaluate(root => {
    const viewport = root.getBoundingClientRect();
    return [...root.querySelectorAll<HTMLElement>(".article-list > [data-index]")].filter(row => {
      const rect = row.getBoundingClientRect();
      const width = Math.max(0, Math.min(rect.right, viewport.right) - Math.max(rect.left, viewport.left));
      const height = Math.max(0, Math.min(rect.bottom, viewport.bottom) - Math.max(rect.top, viewport.top));
      return rect.width * rect.height > 0 && width * height / (rect.width * rect.height) >= 0.5;
    }).map(row => Number(row.dataset.index));
  });
  const expected = visibleIndices.filter(index => rows[index].row_kind === 0)
    .map(index => ({ article_id: rows[index].id, surface: "focus", position: index - 1 }));
  expect(expected.length).toBeGreaterThan(0);
  expect(expected[0].position).toBe(0);
  expect(expected.length).toBeLessThan(articles.length);

  const exposures = () => page.evaluate<ExposureInput[]>(() => (window as any).__mockHost.signals.exposures);
  const recorded = async () => (await exposures()).map(({ article_id, surface, position }) => ({ article_id, surface, position }));
  await expect.poll(settled(page, recorded)).toEqual(expected);
  const first = await exposures();
  expect(new Set(first.map(row => row.article_id)).size).toBe(first.length);
  expect(first.some(row => row.article_id === articles.at(-1)!.id || row.article_id === cluster.id)).toBe(false);

  // 重新加载同一列表不能重复登记已曝光文章。
  await focus.getByRole("button", { name: "刷新", exact: true }).click();
  await expect(focus.getByRole("button", { name: "刷新", exact: true })).toBeEnabled();
  await settleExposures(page);
  expect(await recorded()).toEqual(expected);
});
