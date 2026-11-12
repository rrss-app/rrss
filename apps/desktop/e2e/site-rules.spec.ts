import { expect, test } from "./fixtures";
import { installMockHost, settingsTab } from "./mockHost";
import { readerView, TWO_ARTICLE_PAGE } from "../src/test/fixtures/reader";
import { shot } from "./shots";

test("失败入口预填，新建规则测试、保存、停用和删除", async ({ page }) => {
  const url = "https://example.com/story";
  await installMockHost(page, {
    focus: TWO_ARTICLE_PAGE,
    readers: { 1: readerView(1, { document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "a", href: url, children: [{ t: "t", text: "测试网页" }] }] }] } }) },
    webLinks: { [url]: { kind: "Empty", message: "empty" } },
    siteRuleReport: { layer: "UserRule", title: "规则测试文章", author: "作者甲", date: "2026-10-18", text_chars: 2216, image_count: 1, paragraphs: ["正文第一段。", "正文第二段。", "正文第三段。"], stripped_elements: 3, elapsed_ms: 1200 },
  });
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/");
  await page.getByRole("article", { name: "甲文", exact: true }).click();
  await page.getByRole("link", { name: "测试网页", exact: true }).click();
  await page.getByRole("button", { name: "为 example.com 写一条规则", exact: true }).click();
  const settings = page.getByRole("dialog", { name: "设置", exact: true });
  await expect(settingsTab(settings, "web")).toHaveAttribute("aria-current", "page");
  const editor = page.getByRole("dialog", { name: "站点规则", exact: true });
  await expect(editor.getByLabel("域名", { exact: true })).toHaveValue("example.com");
  await expect(editor.getByLabel("测试链接", { exact: true })).toHaveValue(url);
  // The failure entry opens the same new-rule dialog as the list's button.
  await editor.getByRole("button", { name: "取消", exact: true }).click();
  await settings.getByRole("button", { name: "新建规则", exact: true }).click();
  await editor.getByLabel("域名", { exact: true }).fill("example.com");
  await editor.getByLabel("正文", { exact: true }).fill("article");
  await editor.getByLabel("删除", { exact: true }).fill(".related\n.ad");
  await editor.getByLabel("测试链接", { exact: true }).fill(url);
  await editor.getByRole("switch", { name: "在原网页禁用本站 JavaScript", exact: true }).click();
  await editor.getByRole("button", { name: "测试", exact: true }).click();
  expect(await page.evaluate(() => (window as any).__mockHost.args.test_site_rule.rule.javascript_disabled)).toBe(true);
  await expect(editor.getByRole("status")).toContainText("规则测试文章");
  await expect(editor.getByRole("status")).toContainText("正文第三段。");
  await shot(page, "test-results/site-rules/editor-report.png");
  await page.setViewportSize({ width: 560, height: 900 });
  await page.evaluate(() => document.documentElement.dataset.theme = "dark");
  await shot(page, "test-results/site-rules/editor-narrow-dark.png");
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.evaluate(() => document.documentElement.dataset.theme = "light");
  await editor.getByRole("button", { name: "保存", exact: true }).click();
  await expect(editor).toHaveCount(0);
  expect(await page.evaluate(() => (window as any).__mockHost.args.save_site_rule.rule.javascript_disabled)).toBe(true);
  const row = settings.locator(".site-rule-row").filter({ has: page.getByText("example.com", { exact: true }) });
  await expect(row).toBeVisible();
  await shot(page, "test-results/site-rules/list.png");
  await row.getByRole("button", { name: "编辑", exact: true }).click();
  await expect(editor.getByRole("switch", { name: "在原网页禁用本站 JavaScript", exact: true })).toHaveAttribute("aria-checked", "true");
  await editor.getByRole("button", { name: "取消", exact: true }).click();
  const enabled = row.getByRole("switch", { name: "启用 example.com" });
  await enabled.click();
  await expect(enabled).toHaveAttribute("aria-checked", "false");
  await row.getByRole("button", { name: "删除", exact: true }).click();
  await page.getByRole("alertdialog", { name: "删除站点规则？", exact: true }).getByRole("button", { name: "删除", exact: true }).click();
  await expect(row).toHaveCount(0);
});
