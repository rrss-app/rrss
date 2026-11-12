import { expect, test } from "./fixtures";
import { installMockHost } from "./mockHost";
import { readerReturnFixture } from "../src/test/fixtures/reader";
import type { ReadSessionInput } from "../src/bindings";

test("focus reading writes one session after scrolling and returning", async ({ page }) => {
  const fixture = readerReturnFixture(2);
  await installMockHost(page, fixture);
  await page.goto("/");
  await page.getByRole("article", { name: fixture.readers[1].title, exact: true }).click();
  const reader = page.locator(".drawer.reader"), body = reader.locator(".drawer-body");
  await expect(reader.locator(".reader-title")).toHaveText(fixture.readers[1].title);
  await body.hover();
  await page.mouse.wheel(0, 600);
  await expect.poll(() => body.evaluate(element => element.scrollTop)).toBeGreaterThan(0);
  await reader.locator(".reader-back").click();
  await expect(reader).toHaveCount(0);
  await expect.poll(async () => page.evaluate(() => {
    const sessions = (window as any).__mockHost.signals.sessions as ReadSessionInput[];
    const rows = sessions.filter(session => session.article_id === 1);
    return rows.length === 1 && rows[0].via === "focus" && rows[0].ended_by === "back"
      && rows[0].total_chars > 0 && rows[0].active_ms > 0;
  }), { timeout: 8_000 }).toBe(true);
});
