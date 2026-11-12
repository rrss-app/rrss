import { test, expect } from "./fixtures";
import { installMockHost, MOCK_FEEDS, MOCK_FOCUS } from "./mockHost";

test("adding continues while feeds load; background updates preserve drafts and failed subscriptions", async ({ page }) => {
  await installMockHost(page, { feeds: { ...MOCK_FEEDS, uncategorized: [] } });
  await page.goto("/");
  // Visit the timeline so its retained snapshot must also be invalidated.
  await page.locator(".sidebar").getByRole("button", { name: /^订阅/ }).click();
  await page.getByRole("button", { name: "设置", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  const input = settings.getByRole("textbox", { name: "订阅地址", exact: true });
  const add = settings.getByRole("button", { name: "添加", exact: true });
  await input.fill("https://first.test/rss");
  await add.click();
  await expect(input).toBeEnabled();
  await expect(input).toHaveValue("");
  await input.fill("https://second.test/rss");
  await add.click();
  await expect(input).toHaveValue("");
  // Keep an unfinished third address while the first two complete.
  await input.fill("https://draft.test/rss");
  const article = { ...MOCK_FOCUS.articles.find(row => row.row_kind === 0)!, id: 9001,
    title: "新订阅的后台文章", feed_id: 4, feed_title: "后台完成的来源", read: false };
  await page.evaluate(article => {
    const host = (window as any).__mockHost;
    const [first, second] = host.state.feeds.uncategorized;
    first.title = "后台完成的来源"; first.empty = false;
    second.error = "订阅请求超时";
    second.health = "failing"; second.health_label = "抓取失败"; second.health_detail = second.error;
    host.state.focus.articles.unshift(article);
    // Deliver only the overflow event: per-source events must not mask a
    // missing global-invalidation handler. Queries restore both sources.
    host.emit("feed:updated", { run_id: "burst", feed_id: null, generation: null,
      status: "invalidate", inserted: 0, error: null, derived_degraded: false });
  }, article);
  await expect(settings.getByText("后台完成的来源", { exact: true })).toBeVisible();
  // A burst of per-source events still settles in at most two tree reads.
  await page.evaluate(() => {
    const host = (window as any).__mockHost;
    const [first] = host.state.feeds.uncategorized;
    host.feedReadBaseline = host.calls.filter((call: string) => call === "feed_tree").length;
    first.title = "突发更新后的来源";
    for (let i = 0; i < 12; ++i) host.emit("feed:updated", { feed_id: first.id, inserted: 1, error: null });
  });
  await expect(settings.getByText("突发更新后的来源", { exact: true })).toBeVisible();
  expect(await page.evaluate(() => {
    const host = (window as any).__mockHost;
    return host.calls.filter((call: string) => call === "feed_tree").length - host.feedReadBaseline;
  })).toBeLessThanOrEqual(2);
  await settings.getByRole("button", { name: "抓取失败", exact: true }).click();
  await expect(settings.getByText("订阅请求超时", { exact: true })).toBeVisible();
  await expect(input).toHaveValue("https://draft.test/rss");
  expect(await page.evaluate(() => (window as any).__mockHost.state.feeds.uncategorized.length)).toBe(2);
  await input.fill("");
  await settings.getByRole("button", { name: "返回阅读", exact: true }).click();
  await expect(page.getByText("新订阅的后台文章", { exact: true }).first()).toBeVisible();
});
