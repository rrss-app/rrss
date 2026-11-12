import { expect, test } from "./fixtures";
import { installMockHost, MOCK_READER } from "./mockHost";
import { shot } from "./shots";

test("reader shows an official cover card without a remote media src", async ({ page }) => {
  await page.route("http://rrss.localhost/media/**", async (route) => {
    await route.fulfill({
      path: "../../testdata/render/cover.png",
      contentType: "image/png",
    });
  });
  const openUrl = "https://www.bilibili.com/video/BV1xx411c7mD/";
  await installMockHost(page, {
    reader: {
      ...MOCK_READER,
      title: "示例视频文",
      summary: "",
      reasons: [],
      truncated: false,
      document: {
        v: 1,
        blocks: [
          { t: "p", inlines: [{ t: "t", text: "正文里的官方视频。" }] },
          { t: "embed", kind: "bilibili", url: openUrl, title: "示例" },
        ],
      },
      embed_covers: [{
        url: openUrl,
        open_url: openUrl,
        site: "bilibili",
        title: "示例视频",
        author: "示例作者",
        duration_secs: 482,
        cover_href: "http://rrss.localhost/media/9",
        failed: false,
      }],
    },
  });
  await page.setViewportSize({ width: 1100, height: 800 });
  await page.goto("/");
  await page.getByRole("article").first().click();
  const reader = page.locator(".drawer.reader");
  await expect(reader.getByText("正文里的官方视频。")).toBeVisible();
  const card = reader.locator(".embed-cover");
  await expect(card).toBeVisible();
  await expect(card.locator(".embed-cover-title")).toHaveText("示例视频");
  await expect(card.locator(".embed-cover-meta")).toHaveText("哔哩哔哩 · 示例作者 · 8:02");
  await expect(card.locator("img")).toHaveAttribute("src", "http://rrss.localhost/media/9");
  await expect.poll(() => card.locator("img").evaluate((img: HTMLImageElement) => img.complete && img.naturalWidth > 0)).toBe(true);
  await expect(reader.locator("iframe, video, audio")).toHaveCount(0);
  await shot(page, "../../test-results/embed-cover-card.png");
});
