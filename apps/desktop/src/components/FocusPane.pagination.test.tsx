import { act, useState } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { FocusPane } from "./FocusPane";
import { createReactHarness } from "../test/react";
import { TWO_ARTICLE_PAGE } from "../test/fixtures/reader";

const harness = createReactHarness();
afterEach(() => { harness.cleanup(); vi.restoreAllMocks(); });

it("resets scroll and permits another page when scope changes with the same row count", () => {
  const loadMore = vi.fn();
  let changeScope: () => void = () => {};
  function Screen() {
    const [scope, setScope] = useState("all");
    changeScope = () => setScope("starred");
    return <FocusPane page={{ ...TWO_ARTICLE_PAGE, list_scope: scope, has_more: true }}
      feeds={[]} expandedCluster={null} onLoadMore={loadMore}
      onFilter={() => {}} onExpand={() => {}} onRanking={() => {}}
      onRefresh={() => {}} onOpen={() => {}} onCite={() => {}} onRate={() => {}} />;
  }
  const { host } = harness.mount(<Screen />);
  expect(loadMore).toHaveBeenCalledTimes(1);
  const scroll = host.querySelector<HTMLElement>(".scroll")!;
  scroll.scrollTop = 500;
  act(changeScope);
  expect(loadMore).toHaveBeenCalledTimes(2);
  expect(scroll.scrollTop).toBe(0);
});

it("keeps a hidden pane from paging or resetting scroll, then restores and rechecks on show", () => {
  const loadMore = vi.fn();
  let setVisible: (visible: boolean) => void = () => {};
  let changeScope: () => void = () => {};
  function Screen() {
    const [visible, show] = useState(true);
    const [scope, setScope] = useState("all");
    setVisible = show;
    changeScope = () => setScope("starred");
    return <FocusPane visible={visible} page={{ ...TWO_ARTICLE_PAGE, list_scope: scope, has_more: true }}
      feeds={[]} expandedCluster={null} onLoadMore={loadMore}
      onFilter={() => {}} onExpand={() => {}} onRanking={() => {}}
      onRefresh={() => {}} onOpen={() => {}} onCite={() => {}} onRate={() => {}} />;
  }
  const { host } = harness.mount(<Screen />);
  expect(loadMore).toHaveBeenCalledTimes(1);
  const pane = host.querySelector<HTMLElement>(".focus")!;
  const scroll = host.querySelector<HTMLElement>(".scroll")!;
  scroll.scrollTop = 300;
  act(() => { scroll.dispatchEvent(new Event("scroll")); });

  act(() => setVisible(false));
  expect(pane.hidden).toBe(true);
  expect(pane.hasAttribute("inert")).toBe(true);
  // 隐藏后宿主读到 0 高度、迟到的滚动事件都不能续页或改写记下的位置。
  scroll.scrollTop = 0;
  act(() => { scroll.dispatchEvent(new Event("scroll")); });
  expect(loadMore).toHaveBeenCalledTimes(1);
  act(() => setVisible(true));
  expect(pane.hidden).toBe(false);
  expect(scroll.scrollTop).toBe(300);
  expect(loadMore).toHaveBeenCalledTimes(1);

  act(() => setVisible(false));
  act(changeScope);
  expect(scroll.scrollTop).toBe(300);
  expect(loadMore).toHaveBeenCalledTimes(1);
  act(() => setVisible(true));
  expect(scroll.scrollTop).toBe(0);
  expect(loadMore).toHaveBeenCalledTimes(2);
});
