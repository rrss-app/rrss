import { expect, test, type Page } from "./fixtures";
import { installMockHost, settingsTab } from "./mockHost";
import type { HiddenPage, RuleStatsPage } from "../src/bindings";

const args = (page: Page, command: string) =>
  page.evaluate((name) => (window as any).__mockHost.args[name], command);

const reason = (rule_id: number, label: string, weight: number) => ({ rule_id, dimension: "title_kw", snippet: label, weight,
  code: "legacy_text" as const, params: { text: label, prefix: null } });
const hidden: HiddenPage = {
  spot_liked_count: 1,
  articles: [
    { id: 501, title: "Zed 编辑器融资后公布路线图", feed_id: 3, feed_title: "Byte Harbor", published_at: Date.now() - 5 * 3_600_000, score: -15, spot_liked: true, reasons: [reason(2, "标题：招聘", -15)] },
    { id: 502, title: "一周招聘速览", feed_id: 1, feed_title: "闲聊广场", published_at: Date.now() - 3_600_000, score: -20, spot_liked: false, reasons: [reason(2, "标题：招聘", -15), reason(4, "URL：/sponsored/", -5)] },
  ],
  rule_counts: [{ rule_id: 2, code: "rule_condition_title_kw", params: { pattern: "招聘" }, count: 2 },
    { rule_id: 4, code: "legacy_text", params: { text: "URL 匹配『/sponsored/』", prefix: null }, count: 1 }],
};
const ruleStats: RuleStatsPage = {
  rules: [
    { rule_id: 1, hits_30d: 58, last_hit_at: Date.now() - 86_400_000, hidden_count: 0, advice: "none" },
    { rule_id: 2, hits_30d: 0, last_hit_at: null, hidden_count: 2, advice: "stale" },
  ],
  spot_check: { exposed: 60, liked: 1, disliked: 11, insufficient: false },
};

test("the display menu opens the read-only hidden list, filters by rule and releases one article", async ({ page }) => {
  await installMockHost(page, { hidden, ruleStats });
  await page.goto("/");
  await page.locator(".toolbar").getByRole("button", { name: "显示", exact: true }).click();
  const entry = page.getByRole("menuitem", { name: /被隐藏的文章/ });
  await expect(entry).toContainText("2");
  await entry.click();
  const dialog = page.getByRole("dialog", { name: "被隐藏的文章" });
  await expect(dialog.getByRole("listitem")).toHaveCount(2);
  await dialog.getByRole("button", { name: /URL 匹配『\/sponsored\/』/ }).click();
  await expect(dialog.getByRole("listitem")).toHaveCount(1);
  const row = dialog.getByRole("listitem").first();
  await expect(row).toContainText("总分 -20");
  await row.getByRole("button", { name: "放行 一周招聘速览" }).click();
  await page.getByRole("menuitem", { name: "只放行这一篇" }).click();
  await expect(row).toContainText("已放行，会出现在焦点里。");
  expect(await args(page, "release_article")).toEqual({ id: 502 });
  await dialog.getByRole("button", { name: "关闭", exact: true }).click();
  await expect(dialog).toHaveCount(0);
});

test("the rules page shows rule effects, the spot-check rate and the articles liked during spot checks", async ({ page }) => {
  await installMockHost(page, { hidden, ruleStats });
  await page.goto("/");
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await settingsTab(page, "rules").click();
  const stale = page.locator('[data-rule-id="2"]');
  await expect(stale).toContainText("近 30 天命中 0 篇");
  await expect(stale).toContainText("经它隐藏 2 篇");
  await expect(stale).toContainText("90 天没有命中，可以清理");
  await expect(page.locator('[data-rule-id="1"]')).not.toContainText("可以清理");

  await page.getByRole("button", { name: "5%", exact: true }).click();
  await expect.poll(() => args(page, "set_setting")).toMatchObject({ id: "hidden_spot_check_rate", value: "5" });
  await expect(page.getByText(/近 30 天放回 60 篇：你打开或赞了 1 篇，踩了 11 篇。\s*估计误杀约 2%/)).toBeVisible();

  await page.getByRole("button", { name: "查看你赞过的 1 篇" }).click();
  const dialog = page.getByRole("dialog", { name: "被隐藏的文章" });
  await expect(dialog.getByRole("listitem")).toHaveCount(1);
  await expect(dialog.getByRole("listitem")).toContainText("抽查时放回焦点，你打开或赞了它。");
});
