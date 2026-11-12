import { expect, test } from "./fixtures";
import { installMockHost, MOCK_FOCUS } from "./mockHost";
import { shot } from "./shots";

const lastArgs = (page: import("@playwright/test").Page, command: string) =>
  page.evaluate((name) => (window as any).__mockHost.args[name], command);

test("toolbar dropdowns use the shared menu, and the keyboard-selected row has no outline box", async ({ page }) => {
  await installMockHost(page);
  await page.setViewportSize({ width: 1280, height: 800 });
  await page.goto("/");
  await expect(page.getByRole("article").first()).toBeVisible();
  const toolbar = page.locator(".toolbar");

  // 显示：多选菜单，点一项不关，写进注册表。
  await toolbar.getByRole("button", { name: "显示", exact: true }).click();
  const display = page.getByRole("menu", { name: "显示" });
  const cluster = display.getByRole("menuitemcheckbox", { name: "相近文章折叠" });
  const before = await cluster.getAttribute("aria-checked");
  await cluster.click();
  await expect(cluster).toHaveAttribute("aria-checked", before === "true" ? "false" : "true");
  expect(await lastArgs(page, "set_setting")).toMatchObject({ id: "cluster_group_enabled" });
  await page.keyboard.press("Escape");
  await expect(display).toHaveCount(0);

  // 选中行：没有描边框，版心外侧一条竖线。从阅读器返回后，当前篇成为选中行。
  const row = page.getByRole("article").first();
  await row.click();
  await expect(page.locator(".drawer.reader")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(row).toHaveClass(/focused/);
  await expect(row).toHaveCSS("outline-style", "none");
  expect(await row.evaluate((el) => getComputedStyle(el, "::before").width)).toBe("3px");
});

test("recommendation details default off, reveal ordinary reasons and keep spot checks blind", async ({ page }) => {
  const [first, second] = MOCK_FOCUS.articles;
  const reasons = [
    { component: "freshness", text: "按这个源的阅读节奏降低旧文优先级", points: -1 },
    { component: "source", text: "你常读这个源", points: 1 },
    { component: "coverage", text: "3 个源在报道", points: 2 },
    { component: "affinity", text: "和你常读的『Rust async』相似", points: 2 },
  ];
  await installMockHost(page, { focus: { ...MOCK_FOCUS, articles: [
    { ...first, starred: true, noisy: true, rank_reasons: reasons },
    { ...second, spot_check: true, score: -10, noisy: true, interest_label: first.interest_label, rank_reasons: reasons },
  ] } });
  await page.goto("/");
  const ordinary = page.getByRole("article", { name: first.title, exact: true });
  const spot = page.getByRole("article", { name: second.title, exact: true });
  const tag = ordinary.locator(".meta-tag.learned");
  await expect(ordinary).toBeVisible();
  await expect(spot).toBeVisible();
  await expect(ordinary.locator(".score, .meta-tag.learned, .meta-tag.interest")).toHaveCount(0);
  await expect(ordinary.getByText("噪声源", { exact: true })).toHaveCount(0);
  await expect(ordinary.getByText("已收藏", { exact: true })).toBeVisible();
  await expect(spot.getByText("抽查", { exact: true })).toHaveCount(0);
  await expect(spot.locator(".score, .meta-tag.learned, .meta-tag.interest")).toHaveCount(0);
  await shot(page, "e2e/screenshots/r1-details-off.png");

  await page.locator(".toolbar").getByRole("button", { name: "显示", exact: true }).click();
  const display = page.getByRole("menu", { name: "显示" });
  const details = display.getByRole("menuitemcheckbox", { name: "推荐详情", exact: true });
  await expect(details).toHaveAttribute("aria-checked", "false");
  await expect(display.getByRole("menuitemcheckbox").last()).toHaveText("推荐详情");
  await details.click();
  await expect(details).toHaveAttribute("aria-checked", "true");
  expect(await lastArgs(page, "set_setting")).toMatchObject({ id: "focus_show_details", value: "1" });
  await expect(ordinary.locator(".score")).toHaveText("+12");
  await expect(ordinary.locator(".meta-tag.interest")).toHaveText("本地优先");
  await expect(ordinary.getByText("噪声源", { exact: true })).toBeVisible();
  await expect(tag).toHaveText("3 个源在报道");
  await expect(tag).toHaveAttribute("title", /你常读这个源/);
  await expect(tag).toHaveAttribute("title", /和你常读的『Rust async』相似/);
  await expect(spot.getByText("抽查", { exact: true })).toHaveAttribute("title", "随机放回的一篇本该隐藏的文章，用来核对规则有没有误杀");
  await expect(spot.locator(".score, .meta-tag.learned, .meta-tag.interest")).toHaveCount(0);
  await expect(spot.getByText("噪声源", { exact: true })).toHaveCount(0);
  await page.keyboard.press("Escape");
  await shot(page, "e2e/screenshots/r1-details-on.png");

  await page.locator(".toolbar").getByRole("button", { name: "显示", exact: true }).click();
  const source = page.getByRole("menu", { name: "显示" }).getByRole("menuitemcheckbox", { name: "常读的源" });
  await expect(source).toHaveAttribute("aria-checked", "true");
  await source.click();
  await expect(source).toHaveAttribute("aria-checked", "false");
  expect(await lastArgs(page, "set_setting")).toMatchObject({ id: "rank_source_enabled", value: "0" });
  const affinity = page.getByRole("menu", { name: "显示" }).getByRole("menuitemcheckbox", { name: "和常读文章相似" });
  await expect(affinity).toHaveAttribute("aria-checked", "true");
  await affinity.click();
  await expect(affinity).toHaveAttribute("aria-checked", "false");
  expect(await lastArgs(page, "set_setting")).toMatchObject({ id: "rank_affinity_enabled", value: "0" });
  await details.click();
  await expect(details).toHaveAttribute("aria-checked", "false");
  expect(await lastArgs(page, "set_setting")).toMatchObject({ id: "focus_show_details", value: "0" });
  await expect(ordinary.locator(".score, .meta-tag.learned, .meta-tag.interest")).toHaveCount(0);
  await expect(spot.getByText("抽查", { exact: true })).toHaveCount(0);
  await expect(ordinary.getByText("已收藏", { exact: true })).toBeVisible();
});
