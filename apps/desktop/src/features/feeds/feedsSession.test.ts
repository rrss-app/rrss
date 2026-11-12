import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { FeedsPage } from "../../lib/api";
import { createApiStub, type ApiStub } from "../../test/api";
import { deferred } from "../../test/deferred";
import { focusRow } from "../../test/fixtures/reader";
import { flushUpdates } from "../../test/react";
import { createFeedsSession, type FeedsSession } from "./feedsSession";

const box = vi.hoisted(() => ({ api: null as ApiStub | null }));
vi.mock("../../lib/api", async () => {
  const { createApiProxy } = await import("../../test/api");
  return { api: createApiProxy(() => box.api) };
});

const pageOf = (ids: number[], extra: Partial<FeedsPage> = {}): FeedsPage => ({
  articles: ids.map((id) => focusRow(id, `文章 ${id}`)),
  has_more: false, next_cursor: null, hidden_count: 0, scope: "all", ...extra,
});
const schema = (scope = "all", listScope = "all") => ({
  specs: [],
  values: [{ id: "feeds_scope", value: scope }, { id: "feeds_list_scope", value: listScope }],
});
const emptyCounts = { total: 0, feeds: [], folders: [] };

let session: FeedsSession;
beforeEach(() => { box.api = createApiStub(); session = createFeedsSession(vi.fn()); });
afterEach(() => { box.api?.assertNoUnexpectedCalls(); });

it("appends the next cursor page without repeating ids, and a second loadMore shares that request", async () => {
  const second = deferred<FeedsPage>();
  const feedsPage = vi.fn()
    .mockResolvedValueOnce(pageOf([3, 2], { has_more: true, next_cursor: { at: 2, id: 2 } }))
    .mockImplementationOnce(() => second.promise);
  box.api!.handle("settingsSchema", () => schema());
  box.api!.handle("feedsPage", (scope, listScope, cursor) => feedsPage(scope, listScope, cursor));
  box.api!.handle("feedCounts", () => emptyCounts);
  session.start();
  await flushUpdates();
  const pending = session.loadMore();
  expect(session.loadMore()).toBe(pending);
  await flushUpdates();
  expect(feedsPage).toHaveBeenCalledTimes(2);
  expect(feedsPage.mock.calls[1][2]).toEqual({ at: 2, id: 2 });
  second.resolve(pageOf([2, 1]));
  await pending;
  expect(session.getSnapshot().articles.map((row) => row.id)).toEqual([3, 2, 1]);
  expect(session.getSnapshot().loadingMore).toBe(false);
});

it("drops a late page from the scope that was just left", async () => {
  const stale = deferred<FeedsPage>();
  const feedsPage = vi.fn()
    .mockImplementationOnce(() => stale.promise)
    .mockResolvedValueOnce(pageOf([9], { scope: "feed:9" }));
  box.api!.handle("settingsSchema", () => schema());
  box.api!.handle("feedsPage", (scope, listScope, cursor) => feedsPage(scope, listScope, cursor));
  box.api!.handle("feedCounts", () => emptyCounts);
  box.api!.handle("setSetting", async (id, value) => ({ id, value }));
  session.start();
  await flushUpdates();
  const switched = session.setScope("feed:9");
  stale.resolve(pageOf([1], { scope: "all" }));
  await switched;
  await flushUpdates();
  expect(session.getSnapshot().scope).toBe("feed:9");
  expect(session.getSnapshot().articles.map((row) => row.id)).toEqual([9]);
  expect(feedsPage.mock.calls[1][0]).toBe("feed:9");
});

it("stores the scope the page resolved to when the requested folder is gone", async () => {
  const setSetting = vi.fn(async (id: string, value: string) => ({ id, value }));
  box.api!.handle("settingsSchema", () => schema("folder:7", "unread"));
  box.api!.handle("feedsPage", () => pageOf([1, 2], { scope: "all" }));
  box.api!.handle("feedCounts", () => emptyCounts);
  box.api!.handle("setSetting", setSetting);
  session.start();
  await flushUpdates();
  expect(session.getSnapshot().scope).toBe("all");
  expect(session.getSnapshot().listScope).toBe("unread");
  expect(session.getSnapshot().articles.map((row) => row.id)).toEqual([1, 2]);
  expect(setSetting).toHaveBeenCalledWith("feeds_scope", "all");
});

it("marks the rows markScopeRead returns and refreshes counts", async () => {
  let counts = { total: 2, feeds: [{ id: 1, unread: 2 }], folders: [] as { id: number; unread: number }[] };
  let markedScope = "";
  box.api!.handle("settingsSchema", () => schema("folder:1"));
  box.api!.handle("feedsPage", () => pageOf([1, 2, 3], { scope: "folder:1" }));
  box.api!.handle("feedCounts", () => counts);
  box.api!.handle("markScopeRead", (scope) => {
    markedScope = scope;
    counts = { total: 0, feeds: [], folders: [] };
    return [1, 2];
  });
  session.start();
  await flushUpdates();
  const reads = box.api!.callCount("feedCounts");
  await session.markScopeRead();
  expect(markedScope).toBe("folder:1");
  expect(session.getSnapshot().articles.map((row) => row.read)).toEqual([true, true, false]);
  expect(box.api!.callCount("feedCounts")).toBe(reads + 1);
  expect(session.getSnapshot().counts).toEqual({ total: 0, feeds: [], folders: [] });
});

it("loads all when the settings read fails", async () => {
  const notify = vi.fn();
  session = createFeedsSession(notify);
  box.api!.handle("settingsSchema", () => { throw new Error("offline"); });
  const feedsPage = vi.fn((scope: string, listScope: string) => pageOf([8], { scope }));
  box.api!.handle("feedsPage", feedsPage);
  box.api!.handle("feedCounts", () => emptyCounts);
  session.start();
  await flushUpdates();
  expect(feedsPage).toHaveBeenCalledWith("all", "all", null);
  expect(session.getSnapshot().articles.map((row) => row.id)).toEqual([8]);
  expect(session.getSnapshot().scope).toBe("all");
  expect(session.getSnapshot().listScope).toBe("all");
  expect(notify).toHaveBeenCalledTimes(1);
});

it("loads the saved scope, list and counts once, and ignores later start calls", async () => {
  box.api!.handle("settingsSchema", () => schema("feed:4", "starred"));
  box.api!.handle("feedsPage", (scope, listScope) => {
    expect(listScope).toBe("starred");
    return pageOf([4], { scope });
  });
  box.api!.handle("feedCounts", () => emptyCounts);
  session.start();
  session.start();
  await flushUpdates();
  expect(box.api!.callCount("settingsSchema")).toBe(1);
  expect(box.api!.callCount("feedsPage")).toBe(1);
  expect(box.api!.callCount("feedCounts")).toBe(1);
  expect(session.getSnapshot().scope).toBe("feed:4");
  expect(session.getSnapshot().listScope).toBe("starred");
  session.start();
  await flushUpdates();
  expect(box.api!.callCount("feedsPage")).toBe(1);
});

it("entering from a card source before the first start keeps that feed and still reads the saved list scope", async () => {
  box.api!.handle("settingsSchema", () => schema("folder:7", "unread"));
  const feedsPage = vi.fn((scope: string) => pageOf([5], { scope }));
  box.api!.handle("feedsPage", feedsPage);
  box.api!.handle("feedCounts", () => emptyCounts);
  box.api!.handle("setSetting", async (id, value) => ({ id, value }));
  await session.setScope("feed:5");
  session.start();
  await flushUpdates();
  expect(feedsPage).toHaveBeenCalledTimes(1);
  expect(feedsPage).toHaveBeenCalledWith("feed:5", "unread", null);
  expect(session.getSnapshot().scope).toBe("feed:5");
  expect(session.getSnapshot().listScope).toBe("unread");
});

it("returning to a viewed scope shows its cached rows and position at once, then keeps the loaded tail when the first page is unchanged", async () => {
  const back = deferred<FeedsPage>();
  const feedsPage = vi.fn()
    .mockResolvedValueOnce(pageOf([3, 2], { has_more: true, next_cursor: { at: 2, id: 2 } }))
    .mockResolvedValueOnce(pageOf([1], { has_more: false }))
    .mockResolvedValueOnce(pageOf([9], { scope: "feed:9" }))
    .mockImplementationOnce(() => back.promise);
  box.api!.handle("settingsSchema", () => schema());
  box.api!.handle("feedsPage", (scope, listScope, cursor) => feedsPage(scope, listScope, cursor));
  box.api!.handle("feedCounts", () => emptyCounts);
  box.api!.handle("setSetting", async (id, value) => ({ id, value }));
  session.start();
  await flushUpdates();
  await session.loadMore();
  session.noteScroll(420);
  await session.setScope("feed:9");
  expect(session.getSnapshot().restoreTop).toBe(0);
  void session.setScope("all");
  // 缓存先上屏：行、位置都在，核对请求还没回来。
  expect(session.getSnapshot()).toMatchObject({ loading: true, restoreTop: 420 });
  expect(session.getSnapshot().articles.map((row) => row.id)).toEqual([3, 2, 1]);
  back.resolve(pageOf([3, 2], { has_more: true, next_cursor: { at: 2, id: 2 } }));
  await flushUpdates();
  expect(session.getSnapshot().articles.map((row) => row.id)).toEqual([3, 2, 1]);
  expect(session.getSnapshot()).toMatchObject({ loading: false, hasMore: false, restoreTop: 420 });
});

it("a changed first page replaces the cached rows and returns to the top; read marks reach cached scopes", async () => {
  const feedsPage = vi.fn()
    .mockResolvedValueOnce(pageOf([3, 2]))
    .mockResolvedValueOnce(pageOf([9], { scope: "feed:9" }))
    .mockResolvedValueOnce(pageOf([4, 3, 2]));
  box.api!.handle("settingsSchema", () => schema());
  box.api!.handle("feedsPage", (scope, listScope, cursor) => feedsPage(scope, listScope, cursor));
  box.api!.handle("feedCounts", () => emptyCounts);
  box.api!.handle("setSetting", async (id, value) => ({ id, value }));
  session.start();
  await flushUpdates();
  session.noteScroll(300);
  await session.setScope("feed:9");
  await session.setRead(2, true);
  const switched = session.setScope("all");
  expect(session.getSnapshot().articles.find((row) => row.id === 2)?.read).toBe(true);
  const revision = session.getSnapshot().revision;
  await switched;
  expect(session.getSnapshot().articles.map((row) => row.id)).toEqual([4, 3, 2]);
  expect(session.getSnapshot().revision).toBe(revision + 1);
  expect(session.getSnapshot().restoreTop).toBe(0);
});

it("keeps the saved scope when an update arrives during initial settings loading", async () => {
  const settings = deferred<ReturnType<typeof schema>>();
  const feedsPage = vi.fn((scope: string) => pageOf([9], { scope }));
  box.api!.handle("settingsSchema", () => settings.promise);
  box.api!.handle("feedCounts", () => emptyCounts);
  box.api!.handle("feedsPage", feedsPage);
  session.start();
  const invalidated = session.invalidate();
  expect(feedsPage).not.toHaveBeenCalled();
  settings.resolve(schema("folder:7", "unread"));
  await invalidated;
  expect(session.getSnapshot().scope).toBe("folder:7");
  expect(session.getSnapshot().listScope).toBe("unread");
  expect(feedsPage.mock.calls.every(call => call[0] === "folder:7")).toBe(true);
});
