import { expect, test } from "./fixtures";
import { shot } from "./shots";
import { installMockHost } from "./mockHost";
import { digestSignatureFixtures } from "../src/test/fixtures/digest";

test("focus actions remain reachable without competing with titles", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/");
  const first = page.getByRole("article").first();
  const neutral = page.getByRole("article").filter({ hasText: "编译器 1.97.1" });
  await expect(first).toBeVisible();
  await expect(neutral.locator(".score")).toHaveCount(0);
  await page.mouse.move(0, 0);
  await expect(first.locator(".acts")).toHaveCSS("opacity", "0");
  await first.focus();
  await expect(first.locator(".acts")).toHaveCSS("opacity", "1");
  await expect(first.getByRole("button", { name: "引用", exact: true })).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(page.locator(".drawer.reader")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.locator(".drawer.reader")).toHaveCount(0);
  for (const width of [1280, 880]) {
    await page.setViewportSize({ width, height: width === 880 ? 560 : 800 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate(t => { document.documentElement.dataset.theme = t; }, theme);
      await page.mouse.move(0, 0);
      await expect(page.locator("html")).toHaveCSS("color-scheme", theme);
      expect(await page.evaluate(() => document.body.scrollWidth <= innerWidth)).toBe(true);
      await shot(page, `../../test-results/reader-polish-20260917/focus-${width}-${theme}.png`);
    }
  }
});

test("digest aligns text and follow-up with identifiable sources", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: "简报", exact: true }).click();
  const source = page.getByRole("button", { name: "来源 2：《编译器 1.97.1 点发行说明》，Compiler Notes" });
  await expect(source).toBeVisible();
  await source.hover();
  await expect(page.getByRole("tooltip")).toContainText("Compiler Notes");
  for (const width of [1600, 880]) {
    await page.setViewportSize({ width, height: width === 880 ? 560 : 900 });
    const column = await page.locator(".digest-doc > .reader-column").boundingBox();
    // 没有追问时不渲染「当期追问」区；输入卡与阅读版心对齐，输入行仍是单行高度。
    await expect(page.locator(".digest-discussion")).toHaveCount(0);
    const composer = await page.locator(".composer-card").boundingBox();
    expect(column!.width).toBeLessThanOrEqual(760);
    expect(Math.abs(column!.x - composer!.x)).toBeLessThan(2);
    expect(Math.abs(column!.width - composer!.width)).toBeLessThan(2);
    expect((await page.locator(".composer .input-row").boundingBox())!.height).toBeLessThan(56);
    for (const theme of ["light", "dark"]) {
      await page.evaluate(t => { document.documentElement.dataset.theme = t; }, theme);
      await shot(page, `../../test-results/reader-polish-20260917/digest-${width}-${theme}.png`);
    }
  }
  await source.click();
  await expect(page.locator(".drawer.reader")).toBeVisible();
  await page.getByRole("button", { name: "返回", exact: true }).click();
  await expect(page.locator(".digest")).toBeVisible();
});

test.describe("digest signature", () => {
  test.use({ timezoneId: "Asia/Shanghai" });

  test("local weekday, container date rail and closing paragraphs preserve source navigation", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await installMockHost(page, { digests: digestSignatureFixtures() });
    await page.goto("/");
    await page.getByRole("button", { name: "简报", exact: true }).click();
    const digest = page.locator(".digest"), head = digest.locator(".digest-doc-head");
    const prose = digest.locator("[data-digest-prose]");
    await expect(head.locator(".digest-doc-weekday")).toHaveText("星期六");
    await expect(head).toHaveCSS("position", "absolute");
    const column = (await digest.locator(".digest-doc > .reader-column").boundingBox())!;
    const date = (await head.boundingBox())!;
    expect(date.x + date.width).toBeLessThan(column.x);
    const discussion = (await digest.locator(".digest-discussion").boundingBox())!;
    expect(discussion.x).toBeCloseTo(column.x, 0);
    expect(discussion.width).toBeCloseTo(column.width, 0);
    await expect(digest.locator(".digest-discussion [data-reader-document]")).not.toHaveAttribute("data-digest-prose");

    // 同一窗口改变主栏宽度，证明按容器而不是 viewport 回退。
    await digest.evaluate(el => { el.style.width = "1167px"; });
    await expect(head).toHaveCSS("position", "static");
    const above = (await head.boundingBox())!, text = (await prose.boundingBox())!;
    expect(above.y + above.height).toBeLessThan(text.y);
    expect(above.x).toBeCloseTo(text.x, 0);
    await digest.evaluate(el => { el.style.width = "1168px"; });
    await expect(head).toHaveCSS("position", "absolute");
    await digest.evaluate(el => { el.style.removeProperty("width"); });

    // 已有编号原样保留；无序条目按所在小节的次序编号，每个小标题下重新从 1 数。
    await expect(prose.locator(".digest-list-number")).toHaveText(["7", "2", "12", "2"]);
    await expect(prose.locator(".digest-closing-label")).toHaveCount(1);
    await expect(prose.locator(".digest-closing")).toHaveText(["小结收尾第一段", "收尾第二段"]);
    for (const id of [1, 4]) await expect(prose.locator(`[data-block-id="${id}"]`)).not.toHaveClass(/digest-closing/);
    expect(await prose.locator("[data-block-id]").evaluateAll(els => els.map(el => el.getAttribute("data-block-id"))))
      .toEqual(Array.from({ length: 10 }, (_, i) => String(i)));
    // 外观文字不进入来源块的摘录。
    expect(await prose.locator('[data-block-id="2"]').evaluate(el => {
      const clone = el.cloneNode(true) as Element;
      clone.querySelectorAll("[data-selection-ignore]").forEach(node => node.remove());
      return clone.textContent;
    })).toBe("保留已有条目编号");
    await prose.locator('[data-block-id="2"] .digest-ref').first().click();
    await expect(page.locator(".drawer.reader")).toBeVisible();
    expect(await page.evaluate(() => (window as any).__mockHost.args.open_article.id)).toBe(1);
    await expect(page.locator(".drawer.reader [data-reader-document]")).not.toHaveAttribute("data-digest-prose");
    await page.getByRole("button", { name: "返回", exact: true }).click();
    await expect(prose).toBeVisible();

    await digest.getByRole("button", { name: "上期", exact: true }).click();
    await expect(prose).toContainText("最后就是列表");
    await expect(prose.locator(".digest-closing-label")).toHaveCount(0);
    await digest.getByRole("button", { name: "上期", exact: true }).click();
    await expect(prose).toContainText("只有普通段落");
    await expect(prose.locator(".digest-closing-label")).toHaveCount(0);
    await digest.getByRole("button", { name: "周报", exact: true }).click();
    await expect(head.locator(".digest-doc-date")).toHaveText("9月28日 – 10月4日");
    await expect(head.locator(".digest-doc-weekday")).toHaveCount(0);
    await expect(prose.locator(".digest-closing")).toHaveCount(2);
    // 日期范围仍在栏内，周报按同一收尾规则处理。
    expect(await head.evaluate(el => el.scrollWidth <= el.clientWidth)).toBe(true);
    await digest.getByRole("button", { name: "上期", exact: true }).click();
    await expect(prose).toContainText("另一节的普通段落");
    await expect(prose.locator(".digest-closing-label")).toHaveCount(0);
  });
});
