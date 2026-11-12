import { act, useState } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { FocusScreen } from "./FocusScreen";
import { createFocusSession } from "./focusSession";
import { api } from "../../lib/api";
import { focusRow, TWO_ARTICLE_PAGE } from "../../test/fixtures/reader";
import { createReactHarness } from "../../test/react";
import { clickText, pressKey } from "../../test/fixtures/shell";

const harness = createReactHarness();
afterEach(() => { harness.cleanup(); vi.restoreAllMocks(); });
it("owns keyboard navigation, resets it on equal-size scope changes and retains the range while hidden", async () => {
  let scope = "all";
  const notify = vi.fn();
  const session = createFocusSession(notify);
  const onOpen = vi.fn();
  const onCite = vi.fn();
  const onRate = vi.fn();
  vi.spyOn(api, "feeds").mockResolvedValue({ folders: [], uncategorized: [], folder_combo: [] });
  vi.spyOn(api, "setSetting").mockImplementation(async (id, value) => { expect(id).toBe("list_scope"); scope = value; return { id, value }; });
  vi.spyOn(api, "focus").mockImplementation(async () => ({ ...TWO_ARTICLE_PAGE, list_scope: scope,
    articles: scope === "all" ? TWO_ARTICLE_PAGE.articles : [focusRow(3, "丙文"), focusRow(4, "丁文")] }));
  vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(800);
  vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(1024);
  function Screen() {
    const [active, setActive] = useState(true);
    return <><button onClick={() => setActive(!active)}>切页</button><FocusScreen session={session} active={active} shortcuts={active}
      onOpen={onOpen} onCite={onCite} onRate={onRate} onOpenDigest={() => {}} onOpenSearch={() => {}} onOpenFeed={() => {}} notify={notify} /></>;
  }
  const { host } = harness.mount(<Screen />);
  await harness.flush();
  pressKey("j"); pressKey("Enter");
  expect(onOpen).toHaveBeenLastCalledWith(1);
  clickText(host, "收藏"); await harness.flush();
  pressKey("Enter");
  expect(onOpen).toHaveBeenCalledTimes(1);
  pressKey("j"); pressKey("j"); pressKey("Enter");
  expect(onOpen).toHaveBeenLastCalledWith(4);
  const page = session.getSnapshot().page;
  clickText(host, "切页");
  pressKey("j"); pressKey("Enter");
  expect(onOpen).toHaveBeenCalledTimes(2);
  clickText(host, "切页");
  expect(session.getSnapshot().page).toBe(page);
  expect(host.querySelector(".focus")).not.toBeNull();

  // 阅读时保留当前行供上下篇定位，返回未读范围才重读移除已读项。
  const unread = { ...TWO_ARTICLE_PAGE, list_scope: "unread" };
  vi.mocked(api.focus).mockResolvedValueOnce(unread).mockResolvedValueOnce({ ...unread, articles: unread.articles.slice(1) });
  clickText(host, "未读"); await harness.flush();
  act(() => session.setRead(1, true)); await harness.flush();
  expect(session.findArticle(1)?.read).toBe(true);
  await act(async () => { await session.readerClosed(); }); await harness.flush();
  expect(session.getSnapshot().page?.articles.map((row) => row.id)).toEqual([2]);
});

it("keeps later pages and the scroll position when a cluster expands, then restores the representative", async () => {
  const rep = { ...focusRow(5, "代表文章"), cluster_id: 7 };
  const header = { ...focusRow(7, "同题"), row_kind: 1, cluster_id: 7, cluster_size: 2, cluster_label: "同题报道" };
  const later = focusRow(8, "续页文章");
  const page = { ...TWO_ARTICLE_PAGE, articles: [focusRow(6, "首页其他"), header, rep], has_more: true };
  const notify = vi.fn();
  const session = createFocusSession(notify);
  vi.spyOn(api, "feeds").mockResolvedValue({ folders: [], uncategorized: [], folder_combo: [] });
  const focus = vi.spyOn(api, "focus").mockImplementation(async (_cluster, opts) => opts?.more
    ? { ...TWO_ARTICLE_PAGE, articles: [later], has_more: false } : page);
  vi.spyOn(api, "focusClusterMembers").mockResolvedValue([rep, { ...focusRow(9, "同簇第二篇"), cluster_id: 7 }]);
  vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(800);
  vi.spyOn(HTMLElement.prototype, "offsetWidth", "get").mockReturnValue(1024);
  const { host } = harness.mount(<FocusScreen session={session} active shortcuts
    onOpen={() => {}} onCite={() => {}} onRate={() => {}} onOpenDigest={() => {}} onOpenSearch={() => {}} onOpenFeed={() => {}} notify={notify} />);
  await harness.flush();
  if (!session.getSnapshot().page?.articles.some((row) => row.id === later.id)) await session.loadMore();
  await harness.flush();
  expect(session.getSnapshot().page?.articles.map((row) => row.title)).toContain("续页文章");
  const scroll = host.querySelector<HTMLElement>(".scroll")!;
  scroll.scrollTop = 320;
  expect(host.textContent).toContain("续页文章");
  expect(scroll.scrollTop).toBe(320);
  await act(async () => { host.querySelector("button.cluster")!.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
  await harness.flush();
  expect(focus.mock.calls.filter((call) => !call[1]?.more)).toHaveLength(1);
  expect(scroll.scrollTop).toBe(320);
  expect(host.textContent).toContain("同簇第二篇");
  expect(host.textContent).toContain("续页文章");
  await act(async () => { host.querySelector("button.cluster")!.dispatchEvent(new MouseEvent("click", { bubbles: true })); });
  await harness.flush();
  expect(host.textContent).not.toContain("同簇第二篇");
  expect(host.textContent).toContain("代表文章");
  expect(host.textContent).toContain("续页文章");
  expect(session.getSnapshot().expandedCluster).toBeNull();
});
