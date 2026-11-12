import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { api } from "../../lib/api";
import { deferred } from "../../test/deferred";
import { flushUpdates } from "../../test/react";
import { focusRow, TWO_ARTICLE_PAGE } from "../../test/fixtures/reader";
import { ruleFixture, rulesPageFixture } from "../../test/fixtures/settings";
import { createFocusSession, type FocusSession } from "./focusSession";

let session: FocusSession;
beforeEach(() => {
  session = createFocusSession(vi.fn());
  vi.spyOn(api, "feeds").mockResolvedValue({ folders: [], uncategorized: [], folder_combo: [] });
});
afterEach(() => { session.stop(); vi.restoreAllMocks(); });

it("retains spot explanations across pagination, locks writes and retains undo after a failed undo", async () => {
  const changed = vi.fn(async () => {});
  session = createFocusSession(vi.fn(), api.feeds, changed);
  const spot = { ...focusRow(1, "抽查"), spot_check: true, score: -10,
    hide_reasons: [{ rule_id: 2, dimension: "title_kw", snippet: "招聘", label: "标题含招聘", weight: -10 }] };
  vi.spyOn(api, "focus").mockResolvedValue({ ...TWO_ARTICLE_PAGE, articles: [spot], has_more: true });
  vi.spyOn(api, "rules").mockResolvedValue(rulesPageFixture({ rules: [ruleFixture({ id: 2, polarity: "dislike", pattern: "招聘" })] }));
  const pending = deferred<void>();
  const write = vi.spyOn(api, "setRuleAction").mockImplementationOnce(() => pending.promise).mockRejectedValueOnce(new Error("offline")).mockResolvedValue(undefined);
  session.start(); await flushUpdates();
  session.revealSpotCheck(1, true);
  await session.loadMore();
  expect(session.getSnapshot().spotReviews[1].article.score).toBe(-10);
  const action = session.decideSpotCheck(1, "disable");
  await flushUpdates();
  await session.decideSpotCheck(1, "disable");
  expect(write).toHaveBeenCalledTimes(1);
  pending.resolve(); await action;
  expect(session.getSnapshot().spotReviews[1].undo).toBeDefined();
  await session.decideSpotCheck(1, "undo");
  expect(session.getSnapshot().spotBusy).toBe(false);
  expect(session.getSnapshot().spotReviews[1].undo).toBeDefined();
  await session.decideSpotCheck(1, "undo");
  expect(write).toHaveBeenLastCalledWith(2, 1);
  expect(session.getSnapshot().spotReviews[1].undo).toBeUndefined();
  expect(changed).toHaveBeenCalledTimes(2);
});

it("keeps the new range locked when an old page settles, and retries with real exclusions", async () => {
  const initial = { ...TWO_ARTICLE_PAGE, articles: [focusRow(1, "旧范围")], has_more: true };
  const next = { ...initial, articles: [{ ...focusRow(2, "新范围"), cluster_id: 9 }] };
  const oldMore = deferred<typeof initial>();
  const newMore = deferred<typeof next>();
  const focus = vi.spyOn(api, "focus").mockResolvedValueOnce(initial).mockImplementationOnce(() => oldMore.promise)
    .mockResolvedValueOnce(next).mockImplementationOnce(() => newMore.promise)
    .mockResolvedValueOnce({ ...next, articles: [focusRow(3, "续页")], has_more: false });
  session.start(); await flushUpdates();
  const oldRequest = session.loadMore();
  await session.reload();
  const newRequest = session.loadMore();
  oldMore.resolve({ ...initial, articles: [focusRow(4, "旧续页")] }); await oldRequest;
  expect(session.getSnapshot().loadingMore).toBe(true);
  expect(session.loadMore()).toBe(newRequest);
  expect(focus).toHaveBeenCalledTimes(4);
  newMore.reject(new Error("offline")); await newRequest;
  expect(session.getSnapshot().moreError).toContain("offline");
  expect(session.getSnapshot().page?.articles.map((row) => row.id)).toEqual([2]);
  await session.loadMore();
  expect(focus).toHaveBeenLastCalledWith(null, { excludeIds: [2], excludeClusterIds: [9], more: true });
  expect(session.getSnapshot().page?.articles.map((row) => row.id)).toEqual([2, 3]);
  expect(session.getSnapshot().moreError).toBe("");
});

it("ranking switches use existing stored keys and regroup only after a successful write", async () => {
  vi.spyOn(api, "focus").mockResolvedValue(TWO_ARTICLE_PAGE);
  vi.spyOn(api, "focusClusterMembers").mockResolvedValue([]);
  const write = vi.spyOn(api, "setSetting").mockImplementation(async (id, value) => ({ id, value }));
  session.start(); await flushUpdates();
  await session.setExpanded(9);
  for (const key of ["interest_boost", "rerank", "hide_noisy", "cluster_group"] as const) await session.setRanking(key, false);
  expect(write.mock.calls).toEqual([
    ["interest_boost_enabled", "0"], ["focus_rerank_enabled", "0"],
    ["focus_hide_noisy", "0"], ["cluster_group_enabled", "0"],
  ]);
  expect(session.getSnapshot().expandedCluster).toBeNull();
});

it("serializes host filters and queries only the final effective setting", async () => {
  let filter = "all";
  const first = deferred<void>();
  const second = deferred<void>();
  const write = vi.spyOn(api, "setSetting").mockImplementation(async (id, value) => {
    expect(id).toBe("focus_filter");
    await (value === "focus" ? first : second).promise;
    filter = value;
    return { id, value };
  });
  const read = vi.spyOn(api, "focus").mockImplementation(async () => ({ ...TWO_ARTICLE_PAGE, filter }));
  session.start(); await flushUpdates();
  const a = session.setFilter("focus");
  const b = session.setFilter("hide_faded");
  await flushUpdates();
  expect(write.mock.calls).toEqual([["focus_filter", "focus"]]);
  first.resolve(); await flushUpdates();
  expect(write.mock.calls).toEqual([["focus_filter", "focus"], ["focus_filter", "hide_faded"]]);
  expect(read).toHaveBeenCalledTimes(1);
  expect(session.getSnapshot().loading).toBe(true);
  second.resolve(); await Promise.all([a, b]);
  expect(read).toHaveBeenCalledTimes(2);
  expect(session.getSnapshot().page?.filter).toBe("hide_faded");
  expect(session.getSnapshot().loading).toBe(false);

  read.mockRejectedValueOnce(new Error("offline"));
  await session.reload();
  const callsAfterFailure = read.mock.calls.length;
  await session.loadMore();
  expect(read).toHaveBeenCalledTimes(callsAfterFailure);
  expect(session.getSnapshot().expandedCluster).toBeNull();
});

it("waits for the unread reload and chooses the surviving next row by identity", async () => {
  const initial = { ...TWO_ARTICLE_PAGE, list_scope: "unread", articles: [1, 2, 3, 4].map(id => focusRow(id, `文章 ${id}`)) };
  const reload = deferred<typeof initial>();
  vi.spyOn(api, "focus").mockResolvedValueOnce(initial).mockImplementationOnce(() => reload.promise);
  session.start(); await flushUpdates();
  session.setRead(1, true);
  session.setRead(2, true);
  const closed = session.readerClosed(2);
  expect(session.getSnapshot().readerReturn).toBeNull();
  reload.resolve({ ...initial, articles: initial.articles.slice(2) });
  await closed;
  expect(session.getSnapshot().readerReturn?.id).toBe(3);
});

it("a new scope or reader cancels a pending return without changing the new selection", async () => {
  const initial = { ...TWO_ARTICLE_PAGE, list_scope: "unread" };
  const old = deferred<typeof initial>();
  vi.spyOn(api, "focus").mockResolvedValueOnce(initial).mockImplementationOnce(() => old.promise).mockResolvedValue(TWO_ARTICLE_PAGE);
  session.start(); await flushUpdates();
  const closed = session.readerClosed(1);
  await flushUpdates();
  const superseded = session.reload();
  old.resolve(initial); await closed; await superseded;
  expect(session.getSnapshot().readerReturn).toBeNull();
  expect(session.getSnapshot().expandedCluster).toBeNull();
  const secondClose = session.readerClosed(2);
  session.readerOpened();
  await secondClose;
  expect(session.getSnapshot().readerReturn).toBeNull();
});

it("return pagination failure stops without retrying or inventing a target", async () => {
  const initial = { ...TWO_ARTICLE_PAGE, list_scope: "unread", has_more: true };
  const read = vi.spyOn(api, "focus").mockResolvedValueOnce(initial)
    .mockResolvedValueOnce({ ...initial, articles: [] }).mockRejectedValueOnce(new Error("offline"));
  session.start(); await flushUpdates();
  session.setRead(1, true);
  await session.readerClosed(1);
  expect(read).toHaveBeenCalledTimes(3);
  expect(session.getSnapshot().readerReturn).toBeNull();
  expect(session.getSnapshot().moreError).toContain("offline");
});

it("prefers an unread successor not yet loaded over an earlier surviving row", async () => {
  const initial = { ...TWO_ARTICLE_PAGE, list_scope: "unread", has_more: true };
  vi.spyOn(api, "focus").mockResolvedValueOnce(initial)
    .mockResolvedValueOnce({ ...initial, articles: [focusRow(1, "甲文")] })
    .mockResolvedValueOnce({ ...initial, articles: [focusRow(3, "未加载的下一篇")], has_more: false });
  session.start(); await flushUpdates();
  session.setRead(2, true);
  await session.readerClosed(2);
  expect(session.getSnapshot().readerReturn?.id).toBe(3);
});

it("keeps the article being read in place when a reload drops it, then rereads on close", async () => {
  const rows = [1, 2, 3].map(id => focusRow(id, `文章 ${id}`));
  const initial = { ...TWO_ARTICLE_PAGE, list_scope: "unread", articles: rows };
  const read = vi.spyOn(api, "focus").mockResolvedValueOnce(initial)
    .mockResolvedValueOnce({ ...initial, articles: [rows[0], rows[2]] })
    .mockResolvedValueOnce({ ...initial, articles: [rows[0], rows[2]] });
  session.start(); await flushUpdates();
  session.readerAt(2);
  session.setRead(2, true);
  await session.reload();
  expect(session.getSnapshot().page?.articles.map(row => row.id)).toEqual([1, 2, 3]);
  await session.readerClosed(2);
  expect(read).toHaveBeenCalledTimes(3);
  expect(session.getSnapshot().page?.articles.map(row => row.id)).toEqual([1, 3]);
  expect(session.getSnapshot().readerReturn?.id).toBe(3);
});

it("hides rows disliked in the reader even when the reader was not opened from the list", async () => {
  const rows = [1, 2, 3].map(id => focusRow(id, `文章 ${id}`));
  vi.spyOn(api, "focus").mockResolvedValue({ ...TWO_ARTICLE_PAGE, articles: rows });
  session.start(); await flushUpdates();
  await session.readerClosed(null, [2]);
  expect(session.getSnapshot().page?.articles.map(row => row.id)).toEqual([1, 3]);
});

it("keeps the open page when cluster members fail or arrive late", async () => {
  const header = { ...focusRow(7, "簇"), row_kind: 1, cluster_id: 7 };
  const rep = { ...focusRow(5, "代表"), cluster_id: 7 };
  const page = { ...TWO_ARTICLE_PAGE, articles: [header, rep], has_more: true };
  vi.spyOn(api, "focus").mockResolvedValue(page);
  const notify = vi.fn();
  session = createFocusSession(notify);
  vi.spyOn(api, "focusClusterMembers").mockRejectedValueOnce(new Error("offline"));
  session.start(); await flushUpdates();
  const revision = session.getSnapshot().revision;
  await session.setExpanded(7);
  expect(notify.mock.calls[0][0].message).toContain("offline");
  expect(session.getSnapshot().expandedCluster).toBeNull();
  expect(session.getSnapshot().revision).toBe(revision);
  expect(session.getSnapshot().page?.articles.map((row) => row.id)).toEqual([7, 5]);

  const late = deferred<ReturnType<typeof focusRow>[]>();
  vi.mocked(api.focusClusterMembers).mockImplementationOnce(() => late.promise);
  const pending = session.setExpanded(7);
  await session.setExpanded(null);
  late.resolve([{ ...focusRow(9, "迟到"), cluster_id: 7 }]);
  await pending;
  expect(session.getSnapshot().page?.articles.map((row) => row.id)).toEqual([7, 5]);
  expect(session.getSnapshot().expandedCluster).toBeNull();

  const duringReload = deferred<ReturnType<typeof focusRow>[]>();
  vi.mocked(api.focusClusterMembers).mockImplementationOnce(() => duringReload.promise);
  const expanding = session.setExpanded(7);
  await session.reload();
  duringReload.resolve([{ ...focusRow(9, "迟到"), cluster_id: 7 }]);
  await expanding;
  expect(session.getSnapshot().expandedCluster).toBeNull();
  expect(session.getSnapshot().page?.articles.map((row) => row.id)).toEqual([7, 5]);
});

it("expands only one cluster locally and excludes its members from the next page", async () => {
  const header = { ...focusRow(1, "簇一"), row_kind: 1, cluster_id: 1 };
  const first = { ...focusRow(11, "甲"), cluster_id: 1 };
  const other = { ...focusRow(2, "簇二"), row_kind: 1, cluster_id: 2 };
  const second = { ...focusRow(22, "乙"), cluster_id: 2 };
  const later = focusRow(8, "续页");
  const focus = vi.spyOn(api, "focus")
    .mockResolvedValueOnce({ ...TWO_ARTICLE_PAGE, articles: [header, first, other, second], has_more: true })
    .mockResolvedValueOnce({ ...TWO_ARTICLE_PAGE, articles: [later], has_more: true })
    .mockResolvedValue({ ...TWO_ARTICLE_PAGE, articles: [focusRow(30, "再续")], has_more: false });
  vi.spyOn(api, "focusClusterMembers").mockImplementation(async (id) => id === 1
    ? [first, { ...focusRow(12, "甲二"), cluster_id: 1 }]
    : [second, { ...focusRow(23, "乙二"), cluster_id: 2 }]);
  session.start(); await flushUpdates();
  const revision = session.getSnapshot().revision;
  await session.loadMore();
  await session.setExpanded(1);
  await session.setExpanded(2);
  expect(session.getSnapshot().revision).toBe(revision);
  expect(session.getSnapshot().expandedCluster).toBe(2);
  expect(session.getSnapshot().page?.has_more).toBe(true);
  expect(session.getSnapshot().page?.articles.map((row) => row.title)).toEqual(["簇一", "甲", "簇二", "乙", "乙二", "续页"]);
  await session.loadMore();
  expect(focus).toHaveBeenCalledTimes(3);
  expect(focus).toHaveBeenLastCalledWith(2, { excludeIds: [11, 22, 23, 8], excludeClusterIds: [1, 2], more: true });
});

it("pins the representative first, lists the rest newest first, and keeps that order after a reload", async () => {
  const header = { ...focusRow(1, "簇"), row_kind: 1, cluster_id: 1 };
  const rep = { ...focusRow(11, "代表"), cluster_id: 1, published_at: 200 };
  const old = { ...focusRow(12, "最早"), cluster_id: 1, published_at: 100 };
  const fresh = { ...focusRow(13, "最新"), cluster_id: 1, published_at: 300 };
  // 宿主按发布时间正序给成员，展开后的重读也一样。
  vi.spyOn(api, "focus").mockImplementation(async (expanded) => ({ ...TWO_ARTICLE_PAGE, articles: expanded === 1 ? [header, old, rep, fresh] : [header, rep] }));
  vi.spyOn(api, "focusClusterMembers").mockResolvedValue([old, rep, fresh]);
  session.start(); await flushUpdates();
  await session.setExpanded(1);
  expect(session.getSnapshot().page?.articles.map((row) => row.title)).toEqual(["簇", "代表", "最新", "最早"]);
  await session.reload();
  expect(session.getSnapshot().page?.articles.map((row) => row.title)).toEqual(["簇", "代表", "最新", "最早"]);
  await session.setExpanded(null);
  expect(session.getSnapshot().page?.articles.map((row) => row.title)).toEqual(["簇", "代表"]);
});

it("marks a group read in one write and updates only the rows the host changed, without undo", async () => {
  const notify = vi.fn();
  session = createFocusSession(notify);
  const header = { ...focusRow(1, "簇"), row_kind: 1, cluster_id: 1 };
  const members = [{ ...focusRow(11, "甲"), cluster_id: 1 }, { ...focusRow(12, "乙"), cluster_id: 1, read: true }, { ...focusRow(13, "丙"), cluster_id: 1 }];
  const outside = focusRow(2, "组外");
  vi.spyOn(api, "focus").mockResolvedValue({ ...TWO_ARTICLE_PAGE, articles: [header, ...members, outside] });
  const mark = vi.spyOn(api, "markClusterRead").mockResolvedValue([11, 13]);
  session.start(); await flushUpdates();
  await session.markGroupRead(1);
  expect(mark).toHaveBeenCalledExactlyOnceWith(1);
  const rows = session.getSnapshot().page!.articles;
  expect(rows.filter((row) => row.cluster_id === 1 && row.row_kind === 0).every((row) => row.read)).toBe(true);
  expect(rows.find((row) => row.id === 2)?.read).toBe(false);
  expect(notify).toHaveBeenLastCalledWith({ message: "已把这组 2 篇标为已读" });
});

it("returns next to the last article when it was disliked right before closing", async () => {
  const rows = [1, 2, 3].map(id => focusRow(id, `文章 ${id}`));
  vi.spyOn(api, "focus").mockResolvedValue({ ...TWO_ARTICLE_PAGE, articles: rows });
  session.start(); await flushUpdates();
  await session.readerClosed(3, [3]);
  expect(session.getSnapshot().page?.articles.map(row => row.id)).toEqual([1, 2]);
  expect(session.getSnapshot().readerReturn?.id).toBe(2);
});

it("background updates refresh shown rows in place and only count new articles until the user asks", async () => {
  const rows = [1, 2, 3].map(id => focusRow(id, `文章 ${id}`));
  const focus = vi.spyOn(api, "focus").mockResolvedValue({ ...TWO_ARTICLE_PAGE, articles: rows });
  session.start(); await flushUpdates();
  session.setRead(2, true);
  const revision = session.getSnapshot().revision;
  // 排序变了、第 1 篇有了摘要、来了一篇新文章；后台的旧已读状态不覆盖本地。
  focus.mockResolvedValueOnce({ ...TWO_ARTICLE_PAGE, articles: [focusRow(9, "新文章"), focusRow(3, "文章 3"), { ...rows[0], summary: "刚写好的摘要" }, rows[1]] });
  await session.backgroundChanged();
  const snapshot = session.getSnapshot();
  expect(snapshot.revision).toBe(revision);
  expect(snapshot.page?.articles.map(row => row.id)).toEqual([1, 2, 3]);
  expect(snapshot.page?.articles[0].summary).toBe("刚写好的摘要");
  expect(snapshot.page?.articles[1].read).toBe(true);
  expect(snapshot.pendingNew).toBe(1);
  focus.mockResolvedValueOnce({ ...TWO_ARTICLE_PAGE, articles: [focusRow(9, "新文章"), ...rows] });
  await session.reload();
  expect(session.getSnapshot().revision).toBe(revision + 1);
  expect(session.getSnapshot().pendingNew).toBe(0);
  expect(session.getSnapshot().page?.articles[0].id).toBe(9);
});
