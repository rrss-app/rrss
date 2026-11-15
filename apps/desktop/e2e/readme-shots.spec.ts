import { execFileSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import { expect, test, type Page } from "./fixtures";
import { installMockHost, settingsTab } from "./mockHost";
import { readmeDemo, type DemoLanguage } from "./readmeDemo";

// README 截图：只在更新 README 时运行，CI 与日常 e2e 跳过。
//   RRSS_README_SHOTS=1 pnpm --dir apps/desktop test:e2e e2e/readme-shots.spec.ts --grep zh-CN
//   RRSS_README_SHOTS=1 pnpm --dir apps/desktop test:e2e e2e/readme-shots.spec.ts --grep "\\ben\\b"
// 每种语言 9 张，按语言分开跑（本地 e2e 一次最多 10 条）。中文界面与中文演示数据写到 doc/images/，
// 英文写到 doc/images/en/（README.en 用）。
// 数据是演示用的模拟宿主内容，不调用模型、不碰日常库。PNG 先写到 test-results/readme-shots/，
// 再用 Python Pillow 转成无损 WebP（体积约为 PNG 的四成）。
test.skip(!process.env.RRSS_README_SHOTS, "只在更新 README 截图时运行");
test.use({ viewport: { width: 1280, height: 800 }, deviceScaleFactor: 2 });

const PNG_DIR = "../../test-results/readme-shots";
const OUT: Record<DemoLanguage, string> = { "zh-CN": "../../doc/images", en: "../../doc/images/en" };
const TO_WEBP = "import sys; from PIL import Image; Image.open(sys.argv[1]).save(sys.argv[2], 'WEBP', lossless=True, method=6)";

for (const language of ["zh-CN", "en"] as const) {
  const demo = readmeDemo(language);
  const { ui } = demo;

  test.describe(language, () => {
    async function open(page: Page, theme: "light" | "dark" = "light") {
      await page.emulateMedia({ reducedMotion: "reduce", colorScheme: theme });
      await installMockHost(page, demo.host);
      await page.goto("/");
      await page.evaluate(value => { document.documentElement.dataset.theme = value; }, theme);
      await expect(page.getByRole("article").first()).toBeVisible();
    }

    async function save(page: Page, name: string) {
      // 左缘是阅读器篇目抽屉的热区，鼠标放到右下角，避免唤出抽屉或悬停态。
      const { width, height } = page.viewportSize()!;
      await page.mouse.move(width - 1, height - 1);
      await page.waitForTimeout(150);
      const png = `${PNG_DIR}/${language}/${name}.png`;
      mkdirSync(`${PNG_DIR}/${language}`, { recursive: true });
      mkdirSync(OUT[language], { recursive: true });
      await page.screenshot({ path: png, animations: "disabled" });
      execFileSync("python3", ["-c", TO_WEBP, png, `${OUT[language]}/${name}.webp`]);
    }

    for (const theme of ["light", "dark"] as const) {
      test(`focus ${theme}`, async ({ page }) => {
        await open(page, theme);
        await save(page, `focus-${theme}`);
      });
    }

    test("cluster summary", async ({ page }) => {
      await open(page);
      const focus = page.locator(".focus");
      await focus.locator(".cluster-head button.cluster").click();
      await expect(focus.locator(".cluster-overview")).toContainText(demo.overviewCheck);
      // 组面板上沿放到工具栏下方，上一张卡片整个滚出视口，不留半行标题。
      await focus.locator(".cluster-panel").evaluate(el => {
        let scroller = el.parentElement;
        while (scroller && !/(auto|scroll)/.test(getComputedStyle(scroller).overflowY)) scroller = scroller.parentElement;
        (scroller ?? document.scrollingElement!).scrollBy(0, el.getBoundingClientRect().top - 72);
      });
      await save(page, "cluster");
    });

    test("reader", async ({ page }) => {
      await open(page);
      await page.getByRole("article").first().click();
      await expect(page.locator(".drawer.reader .reader-summary")).toBeVisible();
      await save(page, "reader");
    });

    test("digest", async ({ page }) => {
      await open(page, "dark");
      await page.getByRole("button", { name: ui.digest, exact: true }).click();
      await expect(page.locator(".digest-date")).not.toBeEmpty();
      await save(page, "digest");
    });

    test("chat", async ({ page }) => {
      await open(page);
      await page.getByRole("button", { name: demo.firstThread }).click();
      await expect(page.locator(".bubble.assistant").last()).toBeVisible();
      await save(page, "chat");
    });

    test("feeds", async ({ page }) => {
      await open(page);
      await page.locator(".sidebar").getByRole("button", { name: ui.feeds }).click();
      await expect(page.getByRole("tree", { name: ui.feedTree })).toBeVisible();
      for (const id of [1, 2]) await page.locator(`.feeds-row[data-key="folder:${id}"] .feeds-twist`).click();
      await save(page, "feeds");
    });

    test("search", async ({ page }) => {
      await open(page);
      await page.locator(".sidebar").getByRole("button", { name: new RegExp(`^${ui.search}`) }).click();
      const search = page.getByRole("dialog", { name: ui.searchDialog });
      await search.getByRole("searchbox").fill("SQLite");
      await expect(search.getByRole("option").first()).toBeVisible();
      await save(page, "search");
    });

    test("rules", async ({ page }) => {
      await open(page);
      await page.getByRole("button", { name: ui.settings, exact: true }).click();
      const settings = page.getByRole("dialog", { name: ui.settings, exact: true });
      await settingsTab(settings, "rules").click();
      await expect(settings.getByText(ui.suggestedRules)).toBeVisible();
      await save(page, "rules");
    });
  });
}
