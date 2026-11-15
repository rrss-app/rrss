import { expect, test, type Page } from "./fixtures";
import { installMockHost, settingsTab } from "./mockHost";

// 多语言布局审阅截图：英文与伪本地化（en-XA，字母变形、加长约 40%、首尾加括号）各截主路径，
// 亮 / 暗 × 1280 / 560。只出图不断言，CI 与日常 e2e 跳过：
//   RRSS_I18N_SHOTS=1 pnpm --dir apps/desktop test:e2e e2e/i18n-shots.spec.ts
// 图写到 test-results/i18n-shots/<语言>/。没变形的文字是漏进语言包的硬编码，括号不全是被截断。
test.skip(!process.env.RRSS_I18N_SHOTS, "只在审阅多语言布局时运行");
test.setTimeout(240_000);

const OUT = "../../test-results/i18n-shots";
const SETTINGS = ["subscribe", "web", "rules", "memory", "model", "assistant", "translate", "diagnostics", "storage", "advanced"];

async function capture(page: Page, dir: string, name: string) {
  for (const width of [1280, 560]) {
    await page.setViewportSize({ width, height: 800 });
    for (const theme of ["light", "dark"]) {
      await page.evaluate((value) => { document.documentElement.dataset.theme = value; }, theme);
      await page.waitForTimeout(150);
      await page.screenshot({ path: `${OUT}/${dir}/${name}-${width}-${theme}.png`, animations: "disabled" });
    }
  }
  await page.setViewportSize({ width: 1280, height: 800 });
}

for (const language of ["en", "en-XA"]) {
  test(`main paths in ${language}`, async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await installMockHost(page, { systemLanguage: "en", settingValues: { ui_debug_mode: "1" } });
    await page.goto("/");
    await expect(page.locator(".focus")).toBeVisible();
    const openSettings = async () => {
      await page.locator(".sidebar-foot button").first().click();
      await expect(page.locator(".settings-drawer")).toBeVisible();
    };
    if (language !== "en") {
      await openSettings();
      await page.locator(".settings-nav-foot .menu-select").last().click();
      await page.getByRole("menuitemradio", { name: `${language} (pseudo)` }).click();
      await expect.poll(() => page.evaluate(() => document.documentElement.lang)).toBe(language);
      await page.keyboard.press("Escape");
    }

    await capture(page, language, "focus");
    await page.getByRole("article").first().click();
    await expect(page.locator(".drawer.reader")).toBeVisible();
    await capture(page, language, "reader");
    await page.keyboard.press("Escape");

    await page.keyboard.press("Control+k");
    await expect(page.locator(".command-palette")).toBeVisible();
    await page.locator(".command-palette input").first().fill("sqlite");
    await page.waitForTimeout(300);
    await capture(page, language, "search");
    await page.keyboard.press("Escape");

    await openSettings();
    for (const id of SETTINGS) {
      await settingsTab(page.locator(".settings-drawer"), id).click();
      await capture(page, language, `settings-${id}`);
    }
    await page.keyboard.press("Escape");

    const nav = page.locator(".sidebar .nav > button.nav-item:not(.nav-column)");
    for (const [index, name] of [[2, "digest"], [3, "chat"], [1, "feeds"]] as const) {
      await nav.nth(index).click();
      await page.waitForTimeout(300);
      await capture(page, language, name);
    }
  });
}
