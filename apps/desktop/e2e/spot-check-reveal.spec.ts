import type { Page } from "@playwright/test";
import { expect, test } from "./fixtures";
import { installMockHost, MOCK_FOCUS, MOCK_RULES } from "./mockHost";
import { shot } from "./shots";
import type { ArticleView } from "../src/bindings";

const spot: ArticleView = { ...MOCK_FOCUS.articles[1], score: -15, spot_check: true, hide_reasons: [
  { rule_id: 1, dimension: "title_kw", snippet: "融资", code: "legacy_text", params: { text: "标题含『融资』", prefix: null }, weight: -5 },
  { rule_id: 2, dimension: "title_kw", snippet: "招聘", code: "legacy_text", params: { text: "标题含『招聘』", prefix: null }, weight: -10 },
] };
async function setup(page: Page, details = true) {
  await installMockHost(page, { focus: { ...MOCK_FOCUS, ranking: { ...MOCK_FOCUS.ranking, show_details: details },
    articles: [MOCK_FOCUS.articles[0], spot, MOCK_FOCUS.articles[2]] },
    rules: { ...MOCK_RULES, rules: MOCK_RULES.rules.map(r => ({ ...r, enabled: true })) } });
  await page.goto("/");
  const card = page.getByRole("article", { name: spot.title, exact: true });
  await expect(card).toBeVisible();
  await card.hover();
  return { card, reveal: card.locator(".spot-check-reveal") };
}
const args = (page: Page, command: string) => page.evaluate(name => (window as any).__mockHost.args[name], command);
const rules = (page: Page) => page.evaluate(() => (window as any).__mockHost.state.rules.rules);

test("spot check like reveals all causes, disables the strongest rule, undoes and measures the taller row", async ({ page }) => {
  const { card, reveal } = await setup(page);
  await card.getByRole("button", { name: "赞", exact: true }).click();
  await expect(reveal).toContainText("标题含『融资』（-5）");
  await expect(reveal).toContainText("标题含『招聘』（-10）");
  await expect(reveal).toContainText("总分 -15，隐藏阈值 -10");
  await expect(card.locator(".score")).toHaveText("-15");
  await expect.poll(async () => {
    const a = await card.boundingBox(), b = await page.getByRole("article").nth(2).boundingBox();
    return !!a && !!b && b.y >= a.y + a.height - 1;
  }).toBe(true);
  await shot(page, "e2e/screenshots/r1-reveal-liked.png");
  await reveal.getByRole("button", { name: "停用这条规则" }).click();
  await expect(reveal).toContainText("已停用「标题含『招聘』」");
  expect(await args(page, "set_rule_action")).toEqual({ id: 2, action: 2 });
  expect((await rules(page)).find((r: any) => r.id === 2).enabled).toBe(false);
  await reveal.getByRole("button", { name: "撤销" }).click();
  await expect(reveal.getByRole("button", { name: "停用这条规则" })).toBeVisible();
  expect(await args(page, "set_rule_action")).toEqual({ id: 2, action: 1 });
  expect((await rules(page)).find((r: any) => r.id === 2).enabled).toBe(true);
  await page.locator(".toolbar").getByRole("button", { name: "刷新", exact: true }).click();
  await expect(reveal).toHaveCount(0);
});

test("halve and source allowance each keep an undo, and keep unchanged has none", async ({ page }) => {
  const { card, reveal } = await setup(page);
  await card.getByRole("button", { name: "赞", exact: true }).click();
  await reveal.getByRole("button", { name: "权重减半" }).click();
  await expect(reveal).toContainText("权重减半");
  expect((await args(page, "update_rule")).spec).toMatchObject({ polarity: "dislike", pattern: "招聘", weight: 8 });
  await reveal.getByRole("button", { name: "撤销" }).click();
  await expect(reveal.getByRole("button", { name: "权重减半" })).toBeVisible();
  expect((await args(page, "update_rule")).spec.weight).toBe(15);
  await reveal.getByRole("button", { name: `对 ${spot.feed_title} 放行` }).click();
  await expect(reveal).toContainText(`已对「${spot.feed_title}」放行`);
  expect(await args(page, "allow_rule_for_feed")).toEqual({ ruleId: 2, feedId: spot.feed_id });
  const added = (await rules(page)).find((r: any) => r.id === 99);
  expect(added).toMatchObject({ polarity: "like", scope_kind: "feed", scope_id: spot.feed_id });
  await reveal.getByRole("button", { name: "撤销" }).click();
  await expect(reveal.getByRole("button", { name: "保持不变" })).toBeVisible();
  expect(await args(page, "discard_rule")).toEqual({ id: 99 });
  expect((await rules(page)).find((r: any) => r.id === 99)).toBeUndefined();
  await reveal.getByRole("button", { name: "保持不变" }).click();
  await expect(reveal).toContainText("已保持不变");
  await expect(reveal.getByRole("button", { name: "撤销" })).toHaveCount(0);
});

test("spot check dislike records hidden-correct and never offers rule changes", async ({ page }) => {
  const { card, reveal } = await setup(page);
  await card.getByRole("button", { name: "踩", exact: true }).click();
  await expect(reveal).toContainText("看来规则没判错。");
  await expect(reveal).toContainText("这次反馈记为隐藏正确。");
  await expect(reveal.getByRole("button")).toHaveCount(0);
  await shot(page, "e2e/screenshots/r1-reveal-disliked.png");
});

test("opening a spot reveals on return, and liking with details off stays blind", async ({ page }) => {
  const { card, reveal } = await setup(page, false);
  await card.getByRole("button", { name: "赞", exact: true }).click();
  await expect.poll(() => args(page, "rate_article")).toEqual({ id: spot.id, up: true });
  await expect(reveal).toHaveCount(0);
  await page.locator(".toolbar").getByRole("button", { name: "显示", exact: true }).click();
  await page.getByRole("menuitemcheckbox", { name: "推荐详情", exact: true }).click();
  await page.keyboard.press("Escape");
  await expect(reveal).toBeVisible();
  await page.locator(".toolbar").getByRole("button", { name: "刷新", exact: true }).click();
  await expect(reveal).toHaveCount(0);
  await card.click();
  await expect(page.locator(".drawer.reader")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(reveal).toContainText("这是一次抽查");
  await expect(reveal.getByRole("button", { name: "停用这条规则" })).toBeVisible();
});
