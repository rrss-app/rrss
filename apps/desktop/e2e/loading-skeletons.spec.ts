import { expect, test, type Page } from "./fixtures";
import { installMockHost } from "./mockHost";

const OUT = "../../test-results/worker/loading-skeletons";
const focusSkeleton = (page: Page) => page.locator('[data-loading-skeleton="focus"]');
const digestSkeleton = (page: Page) => page.locator('[data-loading-skeleton="digest"]');

async function trackSkeletons(page: Page) {
  await page.addInitScript(() => {
    const seen: string[] = [];
    Object.assign(window, { __skeletonsSeen: seen });
    new MutationObserver((records) => {
      for (const record of records) for (const node of record.addedNodes) {
        if (!(node instanceof Element)) continue;
        const matches = [...node.querySelectorAll("[data-loading-skeleton]")];
        if (node.matches("[data-loading-skeleton]")) matches.unshift(node);
        for (const match of matches) seen.push(match.getAttribute("data-loading-skeleton")!);
      }
    }).observe(document, { childList: true, subtree: true });
  });
}

async function calls(page: Page, command: string) {
  return page.evaluate((cmd) => (window as any).__mockHost.calls.filter((c: string) => c === cmd).length, command);
}

async function shots(page: Page, name: string) {
  if (!process.env.RRSS_SHOTS) return;
  for (const width of [1280, 560]) {
    await page.setViewportSize({ width, height: 800 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate((next) => { document.documentElement.dataset.theme = next; }, theme);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: `${OUT}/${name}-${width}-${theme}.png`, animations: "disabled" });
    }
  }
}

async function pauseClock(page: Page) {
  await page.clock.install();
  await page.clock.pauseAt(new Date());
}

test("fast focus and digest reads never mount a skeleton", async ({ page }) => {
  await pauseClock(page);
  await installMockHost(page, { holdFocusPage: true, holdDigest: true });
  await trackSkeletons(page);
  await page.goto("/");
  await expect.poll(() => calls(page, "focus_stream")).toBe(1);
  await page.clock.runFor(100);
  await expect(focusSkeleton(page)).toHaveCount(0);
  await page.evaluate(() => {
    const host = (window as any).__mockHost;
    host.holdFocusPage = false;
    host.releaseFocusPage();
  });
  await expect(page.locator(".focus .article").first()).toBeVisible();
  await page.getByRole("button", { name: "简报", exact: true }).click();
  await expect.poll(() => calls(page, "digest_list")).toBeGreaterThan(0);
  await page.clock.runFor(100);
  await expect(digestSkeleton(page)).toHaveCount(0);
  await page.evaluate(() => {
    const host = (window as any).__mockHost;
    host.holdDigest = false;
    host.releaseDigest();
  });
  await expect(page.locator('.digest-doc[data-digest-id="101"]')).toBeVisible();
  await page.clock.runFor(500);
  expect(await page.evaluate(() => (window as any).__skeletonsSeen)).toEqual([]);
});

test("slow focus shows six same-shape cards after the delay and replaces them", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await pauseClock(page);
  await installMockHost(page, { holdFocusPage: true });
  await page.goto("/");
  await expect(page.locator('.focus .scroll[aria-busy="true"]')).toBeVisible();
  await page.clock.runFor(200);
  await expect(focusSkeleton(page)).toHaveCount(0);
  await page.clock.runFor(64);
  await expect(focusSkeleton(page)).toBeVisible();
  await expect(focusSkeleton(page).locator(".article-skeleton")).toHaveCount(6);
  await expect(focusSkeleton(page).locator("i").first()).toHaveCSS("animation-name", "none");
  await shots(page, "focus-loading");
  const masthead = await page.locator(".focus-masthead").boundingBox();
  await page.evaluate(() => {
    const host = (window as any).__mockHost;
    host.holdFocusPage = false;
    host.releaseFocusPage();
  });
  await expect(focusSkeleton(page)).toHaveCount(0);
  await expect(page.locator('.focus .scroll[aria-busy="false"]')).toBeVisible();
  await expect(page.locator(".focus .article").first()).toBeVisible();
  expect((await page.locator(".focus-masthead").boundingBox())?.y).toBe(masthead?.y);
  expect(await page.locator(".focus .scroll").evaluate((el) => el.scrollTop)).toBe(0);
});

test("refresh stays busy and keeps the old page until its replacement arrives", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  const card = page.locator(".focus .article").first();
  await expect(card).toBeVisible();
  const before = await card.getAttribute("aria-label");
  const replacement = `${before} · 更新`;
  await page.evaluate((title) => {
    const host = (window as any).__mockHost;
    host.holdFocusPage = true;
    host.state.focus = { ...host.state.focus, articles: host.state.focus.articles.map((row: { id: number }) => row.id === 1 ? { ...row, title } : row) };
  }, replacement);
  const refresh = page.getByRole("button", { name: "刷新", exact: true });
  await refresh.click();
  await expect(refresh).toHaveAttribute("aria-busy", "true");
  await expect(refresh).toBeDisabled();
  await expect.poll(() => calls(page, "focus_stream")).toBe(2);
  await expect(card).toHaveAttribute("aria-label", before!);
  await expect(focusSkeleton(page)).toHaveCount(0);
  await page.keyboard.press("r");
  expect(await calls(page, "refresh")).toBe(1);
  await page.evaluate(() => {
    const host = (window as any).__mockHost;
    host.holdFocusPage = false;
    host.releaseFocusPage();
  });
  await expect(card).toHaveAttribute("aria-label", replacement);
  await expect(refresh).toHaveAttribute("aria-busy", "false");
  await expect(refresh).toBeEnabled();
});

test("slow digest keeps one delayed document skeleton through list and body reads", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await pauseClock(page);
  await installMockHost(page, { holdDigest: true });
  await trackSkeletons(page);
  await page.goto("/");
  await page.getByRole("button", { name: "简报", exact: true }).click();
  await expect(page.locator('.digest .scroll[aria-busy="true"]')).toBeVisible();
  await page.clock.runFor(200);
  await expect(digestSkeleton(page)).toHaveCount(0);
  await page.clock.runFor(64);
  await expect(digestSkeleton(page)).toBeVisible();
  await expect(digestSkeleton(page).locator(".digest-skeleton-item")).toHaveCount(3);
  await expect(digestSkeleton(page).locator("i").first()).toHaveCSS("animation-name", "none");
  await shots(page, "digest-loading");
  await page.evaluate(() => (window as any).__mockHost.releaseDigest());
  await expect.poll(() => calls(page, "digest")).toBe(1);
  await expect(digestSkeleton(page)).toBeVisible();
  await page.evaluate(() => {
    const host = (window as any).__mockHost;
    host.holdDigest = false;
    host.releaseDigest();
  });
  await expect(digestSkeleton(page)).toHaveCount(0);
  await expect(page.locator('.digest-doc[data-digest-id="101"]')).toBeVisible();
  await expect(page.locator('.digest .scroll[aria-busy="false"]')).toBeVisible();
  expect(await page.evaluate(() => (window as any).__skeletonsSeen)).toEqual(["digest"]);
});

test("paging holds the previous issue and discussion until the new body arrives", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "简报", exact: true }).click();
  const previous = page.locator('.digest-doc[data-digest-id="101"]');
  await expect(previous).toBeVisible();
  await expect(page.locator(".digest-discussion")).toBeVisible();
  await page.evaluate(() => { (window as any).__mockHost.holdDigest = true; });
  await page.getByRole("button", { name: "上期", exact: true }).click();
  await expect.poll(() => calls(page, "digest")).toBe(2);
  await expect(page.locator('.digest .scroll[aria-busy="true"]')).toBeVisible();
  await expect(previous).toBeVisible();
  await expect(page.locator(".digest-discussion")).toBeVisible();
  await expect(digestSkeleton(page)).toHaveCount(0);
  await page.evaluate(() => {
    const host = (window as any).__mockHost;
    host.holdDigest = false;
    host.releaseDigest();
  });
  await expect(previous).toHaveCount(0);
  await expect(page.locator('.digest-doc[data-digest-id="100"]')).toBeVisible();
  await expect(page.locator('.digest .scroll[aria-busy="false"]')).toBeVisible();
  await expect(page.locator(".digest-discussion")).toHaveCount(0);
});

test("switching back to a seen period shows its issue and discussion at once without remounting", async ({ page }) => {
  await installMockHost(page);
  await page.goto("/");
  await page.getByRole("button", { name: "简报", exact: true }).click();
  const daily = page.locator('.digest-doc[data-digest-id="101"]');
  await expect(daily).toBeVisible();
  await expect(page.locator(".digest-discussion")).toBeVisible();
  await page.getByRole("button", { name: "周报", exact: true }).click();
  await expect(page.locator('.digest-doc[data-digest-id="200"]')).toBeVisible();
  // 后台核对全部挂起：看过的日报仍要立即出现，追问区不先卸载。
  await page.evaluate(() => { (window as any).__mockHost.holdDigest = true; });
  const listsBefore = await calls(page, "digest_list");
  await page.getByRole("button", { name: "日报", exact: true }).click();
  await expect(daily).toBeVisible();
  await expect(page.locator(".digest-discussion")).toBeVisible();
  await expect(page.locator(".digest-date")).toHaveText("9月19日");
  await expect(digestSkeleton(page)).toHaveCount(0);
  await expect.poll(() => calls(page, "digest_list")).toBe(listsBefore + 1);
  await daily.evaluate((node) => { (node as any).__kept = true; });
  await page.evaluate(() => {
    const host = (window as any).__mockHost;
    host.holdDigest = false;
    host.releaseDigest();
  });
  await expect.poll(() => calls(page, "digest")).toBeGreaterThanOrEqual(3);
  await expect(page.locator('.digest .scroll[aria-busy="false"]')).toBeVisible();
  expect(await daily.evaluate((node) => (node as any).__kept === true)).toBe(true);
});
