import { expect, test, type Page } from "./fixtures";
import { installMockHost, MOCK_MESSAGES, MOCK_THREADS, type MockHostOptions } from "./mockHost";
import { readerReturnFixture } from "../src/test/fixtures/reader";

const fixture = readerReturnFixture(2);
const title = fixture.readers[2].title;
const question = "What did the field notes record?";
const threads = MOCK_THREADS.map((thread, index) => ({ ...thread, title: ["Morning station", "Evening station"][index] }));
const reader = (page: Page) => page.locator(".drawer.reader");
const pane = (page: Page) => page.locator(".chat-pane");
const bubble = (page: Page) => pane(page).locator(".bubble.user").filter({ hasText: question });
const composer = (page: Page) => page.getByRole("textbox", { name: "Chat message" });

async function setup(page: Page, options: MockHostOptions = {}) {
  await installMockHost(page, { ...fixture, threads: [], messages: {}, systemLanguage: "en", holdSend: true, ...options });
  await page.goto("/");
}

async function citeArticle(page: Page) {
  await reader(page).locator('summary[aria-label="More"]').click();
  await reader(page).getByRole("button", { name: "Cite the full article in chat", exact: true }).click();
  return page.getByRole("region", { name: "Citation tray" });
}

async function releaseSend(page: Page) {
  await page.evaluate(() => (window as any).__mockHost.releaseSend());
}

test("a new reader citation opens chat before acceptance or history and flows into the same reply", async ({ page }) => {
  await setup(page);
  await page.evaluate(() => { (window as any).__mockHost.holdHistory = true; });
  await page.getByRole("article", { name: title, exact: true }).click();
  const tray = await citeArticle(page);
  await tray.getByRole("textbox").fill(question);
  await tray.getByRole("button", { name: "Send", exact: true }).click();

  await expect(reader(page)).not.toBeVisible();
  await expect(bubble(page)).toBeVisible();
  await expect(bubble(page).getByRole("button", { name: `Open citation “${title}”` })).toBeVisible();
  await expect(page.locator(".sidebar .thread-row .thread-item")).toHaveCount(0);
  await expect(pane(page).locator(".chat-wait-label")).toBeVisible();
  await page.evaluate(() => {
    const host = (window as any).__mockHost;
    const step = { kind: "engine", id: "engine", start: true, tool: "", server: "", server_tool: "", detail: "", article_id: null, error: false };
    host.emit("chat:status", { thread_id: 0, notice_version: "", step });
  });
  await expect(pane(page).locator(".chat-wait-label")).toContainText(/start/i);
  await page.evaluate(() => {
    const host = (window as any).__mockHost;
    host.emit("chat:start", 401);
    host.emit("chat:delta", { thread_id: 401, text: "The first field observation.", document: null });
  });
  await expect(pane(page).locator(".bubble.assistant")).toContainText("The first field observation.");
  await expect(bubble(page)).toHaveCount(1);

  await page.getByRole("button", { name: "Today", exact: true }).click();
  await releaseSend(page);
  await expect(page.locator(".sidebar .thread-row .thread-item").filter({ hasText: question })).toBeVisible();
  await expect(page.getByRole("article", { name: title, exact: true })).toBeVisible();
  await expect(pane(page)).not.toBeVisible();
  await page.getByRole("button", { name: "Chat", exact: true }).click();
  await expect(bubble(page)).not.toHaveAttribute("data-pending", "");
  await expect(bubble(page)).toHaveCount(1);
  await page.evaluate(() => (window as any).__mockHost.releaseHistory());
  await expect(bubble(page)).toHaveAttribute("data-message-id", /\d+/);
  await expect(bubble(page)).toHaveCount(1);
});

test("a failed send restores the target draft after navigating away and retries in that thread", async ({ page }) => {
  await setup(page, { threads, sendFailures: 1, messages: {
    7: [{ ...MOCK_MESSAGES[7][0], text: "The earlier morning question.", citations: [] }],
  } });
  await page.getByRole("button", { name: "Morning station", exact: true }).click();
  await composer(page).fill("The saved morning draft.");
  await page.getByRole("button", { name: "Evening station", exact: true }).click();
  await composer(page).fill("An unrelated evening draft.");
  await page.getByRole("button", { name: "Today", exact: true }).click();
  await page.getByRole("article", { name: title, exact: true }).click();
  const tray = await citeArticle(page);
  await tray.getByRole("button", { name: "Send to New chat" }).click();
  await page.getByRole("menuitemradio", { name: "Morning station" }).click();
  await expect(tray.getByRole("textbox")).toHaveValue("The saved morning draft.");
  await tray.getByRole("textbox").fill(question);
  await tray.getByRole("button", { name: "Send", exact: true }).click();
  await expect(pane(page).getByText("The earlier morning question.")).toBeVisible();
  await expect(bubble(page)).toBeVisible();
  await page.getByRole("button", { name: "Evening station", exact: true }).click();
  await releaseSend(page);
  await expect(page.getByRole("heading", { name: "Evening station" })).toBeVisible();
  await expect(composer(page)).toHaveValue("An unrelated evening draft.");
  await page.getByRole("button", { name: "Morning station", exact: true }).click();
  await expect(composer(page)).toHaveValue(question);
  await expect(page.locator(".composer .citation-chip")).toHaveCount(1);
  await expect(bubble(page)).toHaveCount(0);
  await composer(page).press("Enter");
  await expect(bubble(page)).toBeVisible();
  await releaseSend(page);
  await expect(bubble(page)).toHaveAttribute("data-message-id", /\d+/);
  await expect(bubble(page)).toHaveCount(1);
});

test("a digest citation opens chat before resolving its anchor and retries in that discussion", async ({ page }) => {
  const cited = { kind: "article", article_id: 2, title, feed_title: "Fixture feed", url: "https://notes.example/field", excerpt: "", block_id: -1 };
  await setup(page, { sendFailures: 1, messages: { 301: [{ ...MOCK_MESSAGES[7][0], text: "The earlier digest follow-up.", citations: [cited] }] } });
  await page.getByRole("button", { name: "Digest", exact: true }).click();
  await page.locator(".digest-discussion .citation-open").click();
  const tray = await citeArticle(page);
  await tray.getByRole("textbox").fill(question);
  await page.evaluate(() => { (window as any).__mockHost.holdDigestThread = true; });
  await tray.getByRole("button", { name: "Send", exact: true }).click();
  await expect(reader(page)).not.toBeVisible();
  await expect(bubble(page)).toBeVisible();
  await page.evaluate(() => (window as any).__mockHost.releaseDigestThread());
  await expect(pane(page).getByText("The earlier digest follow-up.")).toBeVisible();
  await releaseSend(page);
  await expect(composer(page)).toHaveValue(question);
  await expect(pane(page).getByText("The earlier digest follow-up.")).toBeVisible();
  await composer(page).press("Enter");
  await expect(bubble(page)).toBeVisible();
  await releaseSend(page);
  await expect(bubble(page)).toHaveAttribute("data-message-id", /\d+/);
  await expect(bubble(page)).toHaveCount(1);
  await expect(page.locator(".sidebar .thread-row .thread-item")).toHaveCount(0);
  await page.getByRole("button", { name: "Digest", exact: true }).click();
  await expect(page.locator(".digest-discussion").getByText(question)).toBeVisible();
});
