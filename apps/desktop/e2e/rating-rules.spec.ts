import { expect, test, type Page } from "./fixtures";
import { installMockHost, MOCK_RULES, MOCK_RULE_SUMMARIES, settingsTab } from "./mockHost";
import { shot } from "./shots";
import { trainingFeaturesFixture } from "../src/test/fixtures/settings";

const shots = "../../test-results/worker/rating-rules-copy";
const calls = (page: Page, command: string) => page.evaluate(cmd =>
  (window as unknown as { __mockHost: { calls: string[] } }).__mockHost.calls.filter(c => c === cmd).length, command);

async function appearances(page: Page, name: string, check?: () => Promise<void>) {
  for (const width of [1280, 560]) {
    await page.setViewportSize({ width, height: 800 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme);
      await expect(page.locator("html")).toHaveCSS("color-scheme", theme);
      await check?.();
      await shot(page, `${shots}/${name}-${width}-${theme}.png`);
    }
  }
}

// 文案是本路径的产品契约，只核对这一条反馈和后端返回的摘要，不依赖行内 DOM 文案拼接。
test("rating without features announces success, expires, and rules use host summaries", async ({ page }) => {
  await installMockHost(page);
  await page.clock.install();
  await page.goto("/");
  await page.getByRole("article").first().hover();
  await page.getByRole("article").first().getByRole("button", { name: "赞", exact: true }).click();
  const toast = page.locator('.toast[role="status"]');
  await expect(toast).toContainText("已记录偏好");
  await expect(page.locator(".training-sheet")).toHaveCount(0);
  await expect(toast.locator(".toast-action")).toHaveCount(0);
  await expect(toast).toHaveCount(0, { timeout: 4000 });

  await page.getByRole("article").first().click();
  const reader = page.locator(".drawer.reader");
  await reader.locator('summary[aria-label="更多"]').click();
  await reader.locator(".reader-tools-feedback").getByRole("button", { name: "赞", exact: true }).click();
  await expect(toast).toContainText("已记录偏好");
  await page.clock.pauseAt(new Date());
  await appearances(page, "rating-toast");
  await page.clock.resume();
  await expect(toast).toHaveCount(0, { timeout: 4000 });
  await reader.locator('summary[aria-label="更多"]').click();
  await expect(reader.locator('.reader-tools-feedback button[aria-pressed="true"]')).toHaveCount(1);
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await expect(reader).toHaveCount(0);
  await page.getByRole("button", { name: "设置", exact: true }).click();
  await settingsTab(page, "rules").click();
  const rules = page.locator(".rules-pane");
  for (const rule of MOCK_RULES.rules.map(rule => ({ ...rule, summary: MOCK_RULE_SUMMARIES[rule.id] }))) {
    const row = rules.locator(`[data-rule-id="${rule.id}"]`);
    await expect(row).toHaveAttribute("data-pending", String(rule.pending));
    await expect(row.locator(".rule-summary")).toHaveText(rule.summary);
    await expect(row.locator(".rule-score")).not.toContainText(/\d/);
  }
  await appearances(page, "rules", () => rules.locator(".rule-list").last().scrollIntoViewIfNeeded());
});

test("reader training stays below the middle and above the citation tray until confirmation", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  await installMockHost(page, { trainingFeatures: trainingFeaturesFixture() });
  await page.goto("/");
  await page.getByRole("article").first().click();
  const reader = page.locator(".drawer.reader");
  await reader.locator('summary[aria-label="更多"]').click();
  await reader.locator(".reader-tools-feedback").getByRole("button", { name: "赞", exact: true }).click();
  const training = page.locator('.training-sheet[role="dialog"]');
  await expect(training).toBeVisible();
  await expect(page.locator(".toast")).toHaveCount(0);
  await expect(training).toBeFocused();
  await page.keyboard.press("Escape");
  await expect(training).toHaveCount(0);
  await reader.locator('summary[aria-label="更多"]').click();
  await reader.getByRole("button", { name: "引用全文到对话", exact: true }).click();
  const tray = page.locator(".reader-citation-tray");
  await tray.getByRole("textbox").fill("这条引用先保留");
  await reader.locator('summary[aria-label="更多"]').click();
  const before = await calls(page, "focus_stream");
  await reader.locator(".reader-tools-feedback").getByRole("button", { name: "踩", exact: true }).click();
  await expect(training).toBeVisible();
  await expect(page.locator(".training-stage")).toHaveAttribute("data-reader", "true");
  expect(await calls(page, "focus_stream")).toBe(before);
  await page.keyboard.press("Tab");
  await expect(training.getByRole("button", { name: "关闭", exact: true })).toBeFocused();
  await appearances(page, "reader-training", async () => {
    await expect.poll(async () => {
      const panel = await training.boundingBox(), dock = await tray.boundingBox(), region = await reader.boundingBox();
      return !!panel && !!dock && !!region && panel.y >= region.y + region.height / 2
        && panel.y + panel.height <= dock.y - 8 && panel.x >= region.x && panel.x + panel.width <= region.x + region.width;
    }).toBe(true);
  });
  await training.getByRole("button", { name: "仅此源", exact: true }).first().click();
  await expect(training).toHaveCount(0);
  await expect.poll(() => calls(page, "focus_stream")).toBe(before + 1);
  expect(await page.evaluate(() => (window as unknown as { __mockHost: { args: Record<string, unknown> } }).__mockHost.args.confirm_feature))
    .toMatchObject({ articleId: 1, dislike: true, feedScope: true, dimension: "title_kw", pattern: "融资" });
  await expect(tray.getByRole("textbox")).toHaveValue("这条引用先保留");
  await reader.locator('summary[aria-label="更多"]').click();
  await expect(reader.locator(".reader-tools-feedback").getByRole("button", { name: "已踩", exact: true })).toHaveAttribute("aria-pressed", "true");
});
