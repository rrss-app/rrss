import { afterEach, expect, it, vi } from "vitest";
import { FocusScreen } from "./FocusScreen";
import { createFocusSession } from "./focusSession";
import { api } from "../../lib/api";
import { focusRow, TWO_ARTICLE_PAGE } from "../../test/fixtures/reader";
import { createReactHarness, flushUpdates } from "../../test/react";

const surfaces = vi.hoisted(() => ({ seen: [] as string[] }));
vi.mock("../../lib/useExposureTracker", () => ({
  useExposureTracker: ({ surface }: { surface: string }) => {
    surfaces.seen.push(surface);
    return () => {};
  },
}));

const harness = createReactHarness();
afterEach(() => { harness.cleanup(); vi.restoreAllMocks(); surfaces.seen.length = 0; });

const header = { ...focusRow(7, "同题"), row_kind: 1, cluster_id: 7, cluster_size: 2, cluster_label: "同题" };
const rep = { ...focusRow(5, "代表"), cluster_id: 7 };

it("sends scope and focus_column exposure for a column session, and omits scope for the main session", async () => {
  const focus = vi.spyOn(api, "focus").mockImplementation(async (_expanded, opts) => ({
    ...TWO_ARTICLE_PAGE, has_more: !opts?.more, articles: opts?.more ? [focusRow(8, "续页")] : [header, rep],
  }));
  const members = vi.spyOn(api, "focusClusterMembers").mockResolvedValue([rep, { ...focusRow(9, "同簇"), cluster_id: 7 }]);
  const mark = vi.spyOn(api, "markScopeRead").mockResolvedValue([5]);
  vi.spyOn(api, "feeds").mockResolvedValue({ folders: [], uncategorized: [], folder_combo: [] });
  vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(800);
  vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(1024);
  const notify = vi.fn();
  const main = createFocusSession(notify);
  const column = createFocusSession(notify, api.feeds, async () => {}, { scope: "folder:2" });
  main.start();
  await flushUpdates();
  await main.loadMore();
  const mainCalls = focus.mock.calls.length;
  expect(mainCalls).toBeGreaterThan(0);
  expect(focus.mock.calls.every((call) => call[1]?.scope === undefined)).toBe(true);
  expect(focus.mock.calls.at(-1)?.[1]).toMatchObject({ more: true });
  expect(focus.mock.calls.at(-1)?.[1]).not.toHaveProperty("scope");

  column.start();
  await flushUpdates();
  await column.loadMore();
  expect(focus.mock.calls.slice(mainCalls).every((call) => call[1]?.scope === "folder:2")).toBe(true);
  expect(focus.mock.calls.at(-1)?.[1]).toMatchObject({ more: true, scope: "folder:2" });
  await column.setExpanded(7);
  expect(members).toHaveBeenCalledWith(7, "folder:2");
  await column.markAllRead();
  expect(mark).toHaveBeenCalledWith("folder:2");
  await column.setFeedScope("folder:3");
  expect(focus.mock.calls.at(-1)?.[1]).toMatchObject({ scope: "folder:3" });
  expect(focus.mock.calls.slice(0, mainCalls).every((call) => call[1]?.scope === undefined)).toBe(true);

  const screen = {
    active: true, shortcuts: true, onOpen: vi.fn(), onCite: vi.fn(), onRate: vi.fn(),
    onOpenDigest: vi.fn(), onOpenSearch: vi.fn(), onOpenFeed: vi.fn(), notify,
  };
  harness.mount(<FocusScreen session={main} {...screen} />);
  harness.mount(<FocusScreen session={column} {...screen} surface="focus_column" column={{ scope: "folder:2", name: "English" }} />);
  expect(surfaces.seen).toContain("focus");
  expect(surfaces.seen).toContain("focus_column");
  expect(surfaces.seen.filter((surface) => surface !== "focus" && surface !== "focus_column")).toEqual([]);
});
