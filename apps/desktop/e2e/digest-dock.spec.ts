import { expect, test, type Page } from "./fixtures";
import { installMockHost } from "./mockHost";
import { shot } from "./shots";

const OUT = "../../test-results/digest-dock";

async function shots(page: Page, name: string) {
  if (!process.env.RRSS_SHOTS) return;
  // 只为截图等台子的过渡播完；不截图时不等。
  await page.waitForTimeout(500);
  for (const theme of ["light", "dark"]) {
    await page.evaluate((t) => { document.documentElement.dataset.theme = t; }, theme);
    await shot(page, `${OUT}/${name}-${theme}.png`);
  }
  await page.evaluate(() => { document.documentElement.dataset.theme = "light"; });
}

test("the digest follow-up rests as a small dock and opens on hover, at the bottom and on /", async ({ page }) => {
  await installMockHost(page);
  // 视口矮一点，让这期简报需要滚动。
  await page.setViewportSize({ width: 1280, height: 520 });
  await page.goto("/");
  await page.getByRole("button", { name: "简报", exact: true }).click();
  await expect(page.locator(".digest-discussion .assistant")).toBeVisible();
  const composer = page.locator(".composer[data-dock]");
  const scroller = page.locator(".digest .scroll");
  await scroller.evaluate((el) => { el.scrollTop = 0; });

  // 空闲：只剩台子，模型按钮收走且不可聚焦。
  await expect(composer).toHaveAttribute("data-dock", "closed");
  await expect(composer.locator(".composer-dock-label")).toHaveText("追问本期 · 1 条");
  await expect(composer.locator(".composer-grow").first()).toHaveAttribute("inert", "");
  await expect(composer.locator(".composer-grow").first()).toHaveCSS("opacity", "0");
  await shots(page, "closed");

  // 悬停：停住才展开；离开后收回。
  await composer.locator(".input-row").hover();
  await expect(composer).toHaveAttribute("data-dock", "open");
  await expect(composer.locator(".composer-grow").first()).not.toHaveAttribute("inert", "");
  await expect(composer.locator(".composer-grow").first()).toHaveCSS("opacity", "1");
  await shots(page, "hover");
  await page.mouse.move(640, 120);
  await expect(composer).toHaveAttribute("data-dock", "closed");

  // 滚到底：展开，最后一段不被卡片压住。
  await scroller.evaluate((el) => { el.scrollTop = el.scrollHeight; });
  await expect(composer).toHaveAttribute("data-dock", "open");
  const last = await page.locator(".digest-discussion .assistant").last().boundingBox();
  const card = await composer.locator(".input-row").boundingBox();
  expect(last!.y + last!.height).toBeLessThanOrEqual(card!.y);
  await scroller.evaluate((el) => { el.scrollTop = 0; });
  await expect(composer).toHaveAttribute("data-dock", "closed");

  // 「/」聚焦输入并展开；有草稿时即使失焦也保持展开。
  await page.keyboard.press("/");
  const input = page.getByRole("textbox", { name: "追问当前简报" });
  await expect(input).toBeFocused();
  await expect(composer).toHaveAttribute("data-dock", "open");
  await input.fill("第二条的游标怎么做？");
  await page.locator(".digest-doc").click({ position: { x: 4, y: 4 } });
  await expect(input).not.toBeFocused();
  await expect(composer).toHaveAttribute("data-dock", "open");
  await input.fill("");
  await page.locator(".digest-doc").click({ position: { x: 4, y: 4 } });
  await page.mouse.move(640, 120);
  await expect(composer).toHaveAttribute("data-dock", "closed");
});

test("hovering at the edge of the dock's trigger band opens it once and keeps it open", async ({ page }) => {
  await installMockHost(page);
  await page.setViewportSize({ width: 1280, height: 520 });
  await page.goto("/");
  await page.getByRole("button", { name: "简报", exact: true }).click();
  await expect(page.locator(".digest-discussion .assistant")).toBeVisible();
  const composer = page.locator(".composer[data-dock]");
  await page.locator(".digest .scroll").evaluate((el) => { el.scrollTop = 0; });
  await expect(composer).toHaveAttribute("data-dock", "closed");
  await page.waitForTimeout(450);
  // 记下每次收起 / 展开，停在触发带上沿和下沿各试一次。
  await composer.evaluate((el) => {
    const flips: string[] = [];
    Object.assign(window, { __dockFlips: flips });
    new MutationObserver(() => flips.push(el.getAttribute("data-dock")!)).observe(el, { attributes: true, attributeFilter: ["data-dock"] });
  });
  const pill = (await composer.locator(".input-row").boundingBox())!;
  for (const y of [pill.y - 12, pill.y + pill.height + 16]) {
    await page.mouse.move(pill.x + pill.width / 2, y);
    await expect(composer).toHaveAttribute("data-dock", "open");
    await page.waitForTimeout(1200);
    await expect(composer).toHaveAttribute("data-dock", "open");
    expect(await page.evaluate(() => (window as any).__dockFlips.filter((v: string) => v === "closed").length)).toBe(0);
    // 移开后照常收起，下一轮从收起状态开始。
    await page.mouse.move(pill.x + pill.width / 2, 120);
    await expect(composer).toHaveAttribute("data-dock", "closed");
    await page.waitForTimeout(450);
    await page.evaluate(() => { (window as any).__dockFlips.length = 0; });
  }
});
