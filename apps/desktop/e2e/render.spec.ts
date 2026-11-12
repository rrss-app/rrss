import { expect, test } from "./fixtures";
import { shot } from "./shots";

test.describe("render fixtures", () => {
  test("CJK block IR renders in light and dark", async ({ page }) => {
    await page.goto("/#render");
    await expect(page.getByRole("heading", { name: "开馆日" })).toBeVisible();
    await expect(page.getByText("图书馆不是奢侈品")).toBeVisible();
    await shot(page, "e2e/screenshots/render-light.png", { fullPage: true });
    await page.evaluate(() => {
      document.documentElement.dataset.theme = "dark";
    });
    await shot(page, "e2e/screenshots/render-dark.png", { fullPage: true });
  });

  test("probe page CJK and IME field", async ({ page }) => {
    await page.goto("/#probe");
    await expect(page.getByRole("heading", { name: "WebView 探针" })).toBeVisible();
    await page.getByPlaceholder("用拼音/五笔输入").fill("图书馆");
    await expect(page.getByText("已输入：图书馆")).toBeVisible();
    await shot(page, "e2e/screenshots/probe.png", { fullPage: true });
  });
});
