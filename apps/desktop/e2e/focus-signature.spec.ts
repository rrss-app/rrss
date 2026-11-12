import { expect, test } from "./fixtures";
import { installMockHost, MOCK_FOCUS } from "./mockHost";
import type { FocusPage } from "../src/bindings";

test.use({ timezoneId: "Asia/Shanghai" });

test("focus masthead: digest entry, first unread headline and measured responsive column", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.clock.setFixedTime(new Date("2026-09-30T16:30:00Z"));
  const articles = Array.from({ length: 48 }, (_, i) => ({
    ...MOCK_FOCUS.articles[0], id: i + 1, title: `焦点文章 ${i + 1}：桌面端单库与向量检索的取舍`,
    summary: "保留本地数据与现有查询，让版心跟随窗口变化。".repeat(5),
  }));
  await installMockHost(page, { focus: { ...MOCK_FOCUS, articles }, focusPageSize: 12 });
  await page.goto("/");
  const focus = page.locator(".focus");
  const lede = focus.locator(".focus-lede");
  const headline = focus.locator("[data-headline] .article");
  const scroll = focus.locator(".scroll");
  await expect(focus.locator(".focus-kicker")).toHaveText("十月一日 · 星期四");
  await expect(headline).toHaveCount(1);
  await expect(headline).toHaveAttribute("aria-label", articles[0].title);
  await expect(lede).toContainText(MOCK_FOCUS.digest_lede);
  await lede.click();
  await expect(page.locator(".digest-date")).toBeVisible();
  await expect(focus).toBeHidden();
  await page.getByRole("button", { name: "今日焦点", exact: true }).click();

  for (const width of [960, 1440, 1920]) {
    await page.setViewportSize({ width, height: 900 });
    await expect.poll(async () => focus.evaluate(root => {
      const pane = root.getBoundingClientRect();
      const expected = Math.min(1040, Math.max(720, pane.width * 0.8), pane.width - 48);
      const rects = [".focus-masthead", ".focus-lede", ".article-list"].map(selector => root.querySelector(selector)!.getBoundingClientRect());
      // 工具栏铺满主栏（兼作标题栏），内容从版心左缘开始，右侧可能再给窗口三键让出一段。
      const bar = root.querySelector<HTMLElement>(".toolbar")!;
      const style = getComputedStyle(bar);
      const barBox = bar.getBoundingClientRect();
      const barLeft = barBox.left + parseFloat(style.paddingLeft);
      const barRight = barBox.right - parseFloat(style.paddingRight);
      return rects.every(rect => Math.abs(rect.width - expected) < 1 && rect.left - pane.left >= 23.5 && pane.right - rect.right >= 23.5
        && Math.abs(rect.left - rects[0].left) < 1)
        && Math.abs(barLeft - rects[0].left) < 1 && barRight <= rects[0].right + 0.5;
    })).toBe(true);
    // 实测后的每一行紧接上一行；窗口改宽也不能用旧估高覆盖头条。
    await expect.poll(async () => focus.locator(".article-list > div").evaluateAll(nodes => {
      const rects = nodes.map(node => node.getBoundingClientRect());
      return rects.slice(1).every((rect, i) => Math.abs(rect.top - rects[i].bottom) < 1);
    })).toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  }

  // 页眉和导语栏随列表滚走，不钉在顶部；头条紧贴在导语栏下方。
  const masthead = focus.locator(".focus-masthead");
  const top = await masthead.evaluate(el => el.getBoundingClientRect().top);
  expect(await focus.evaluate(root => Math.abs(root.querySelector(".article-list > div")!.getBoundingClientRect().top
    - root.querySelector(".focus-head")!.getBoundingClientRect().bottom) < 1)).toBe(true);
  await scroll.evaluate(el => { el.scrollTop = 200; });
  await expect.poll(() => masthead.evaluate(el => el.getBoundingClientRect().top)).toBeLessThan(top - 150);
  await scroll.evaluate(el => { el.scrollTop = 0; });

  // 续页与虚拟窗口的首行均不得成为另一条头条。
  const initialHeight = await scroll.evaluate(el => el.scrollHeight);
  await scroll.evaluate(el => { el.scrollTop = el.scrollHeight - el.clientHeight; });
  await expect.poll(() => page.evaluate(() => {
    const host = (window as any).__mockHost;
    return host.calls.filter((call: string) => call === "focus_stream").length;
  })).toBeGreaterThan(1);
  await expect.poll(() => scroll.evaluate(el => el.scrollHeight)).toBeGreaterThan(initialHeight);
  await scroll.evaluate(el => { el.scrollTop = el.scrollHeight - el.clientHeight; });
  await expect(headline).toHaveCount(0);
  await scroll.evaluate(el => { el.scrollTop = 0; });
  await expect(headline).toHaveAttribute("aria-label", articles[0].title);

  async function change(patch: Partial<FocusPage>) {
    await page.evaluate(patch => { Object.assign((window as any).__mockHost.state.focus, patch); }, patch);
    await focus.getByRole("button", { name: "刷新", exact: true }).click();
    await expect(focus.getByRole("button", { name: "刷新", exact: true })).toBeEnabled();
  }
  await change({ articles: [{ ...articles[0], read: true }, ...articles.slice(1)], digest_lede: "" });
  await expect(headline).toHaveCount(0); // 第二行虽未读，也不能越过已读首行。
  await expect(lede).toContainText("阅读简报");
  await expect(lede.locator(".focus-lede-text > span")).toHaveCount(0);
  await change({ articles: [{ ...articles[0], row_kind: 1, cluster_id: 90, cluster_size: 2 }, ...articles.slice(1)], digest_banner: "" });
  await expect(headline).toHaveCount(0); // 首行是聚类时也不跳过它。
  await expect(lede).toHaveCount(0);
  await change({ articles });
  await expect(headline).toHaveAttribute("aria-label", articles[0].title);
});
