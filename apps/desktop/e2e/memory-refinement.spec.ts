import { expect, test } from "./fixtures";
import { installMockHost, settingsTab } from "./mockHost";

const MEMORY_FIXTURE = {
  memories: [
    {
      id: 101,
      kind: "procedural",
      content: "Uses Windows 11; prefer Windows commands over bash.",
      pinned: false,
      pending: true,
      scope: "global",
      strength: "0.0",
      replaces_id: 1,
      replaces_content: "Uses Linux desktop.",
      replaces_pinned: true,
    },
    {
      id: 102,
      kind: "procedural",
      content: "Explain hardware concepts with plain analogies.",
      pinned: false,
      pending: true,
      scope: "global",
      strength: "0.0",
    },
    {
      id: 103,
      kind: "semantic",
      content: "Follows Rust and database internals.",
      pinned: false,
      pending: true,
      scope: "global",
      strength: "0.0",
    },
    {
      id: 1,
      kind: "semantic",
      content: "Uses Linux desktop.",
      pinned: true,
      pending: false,
      scope: "global",
      strength: "1.0",
    },
  ],
  interests: [],
  inject_preview: "",
};

test("memory update candidate shows comparison, replaces target in place, and hides interest button on procedural rules", async ({ page }) => {
  await installMockHost(page, { memory: structuredClone(MEMORY_FIXTURE) });
  await page.goto("/");
  await page.locator(".sidebar-foot button").first().click();
  const settings = page.locator(".settings-drawer");
  await expect(settings).toBeVisible();
  await settingsTab(settings, "memory").click();

  const updateCard = settings.locator(".memory-card.pending.is-update");
  await expect(updateCard).toBeVisible();
  await expect(updateCard.locator(".memory-diff-row.old")).toContainText("Uses Linux desktop.");
  await expect(updateCard.locator(".memory-diff-row.next")).toContainText("Uses Windows 11; prefer Windows commands over bash.");
  await expect(updateCard.locator(".memory-diff-note")).toBeVisible();

  // Procedural standalone candidate has no "Add as interest" button (only edit/ignore/confirm = 3 buttons);
  // semantic standalone candidate includes "Add as interest" (4 buttons).
  const ruleCard = settings.locator(".memory-card.pending").filter({ hasText: "Explain hardware concepts with plain analogies." });
  const factCard = settings.locator(".memory-card.pending").filter({ hasText: "Follows Rust and database internals." });
  await expect(ruleCard.locator(".memory-actions button")).toHaveCount(3);
  await expect(factCard.locator(".memory-actions button")).toHaveCount(4);

  // Confirm replacement: target is updated in place and keeps its pinned badge.
  await updateCard.locator(".memory-actions .btn.primary").click();
  await expect(settings.locator(".memory-card.pending.is-update")).toHaveCount(0);
  const replaced = settings.locator(".memory-card:not(.pending)").filter({ hasText: "Uses Windows 11; prefer Windows commands over bash." });
  await expect(replaced).toBeVisible();
  await expect(replaced.locator(".meta-tag.accent")).toBeVisible();
  await expect(settings.locator(".memory-card").filter({ hasText: "Uses Linux desktop." })).toHaveCount(0);
});

test("ignoring an update candidate keeps the original memory intact", async ({ page }) => {
  await installMockHost(page, { memory: structuredClone(MEMORY_FIXTURE) });
  await page.goto("/");
  await page.locator(".sidebar-foot button").first().click();
  const settings = page.locator(".settings-drawer");
  await expect(settings).toBeVisible();
  await settingsTab(settings, "memory").click();

  const updateCard = settings.locator(".memory-card.pending.is-update");
  await updateCard.locator(".memory-ignore").click();
  await expect(settings.locator(".memory-card.pending.is-update")).toHaveCount(0);
  await expect(settings.locator(".memory-card:not(.pending)").filter({ hasText: "Uses Linux desktop." })).toBeVisible();
});
