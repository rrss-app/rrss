import { type ComponentProps } from "react";
import { afterEach, expect, it, vi } from "vitest";
import { FocusPane } from "./FocusPane";
import { PREVIEW_FEEDS, PREVIEW_FOCUS } from "../lib/preview";
import { flattenFeeds } from "../lib/focusUtils";
import { createReactHarness } from "../test/react";
import { click } from "../test/fixtures/shell";

const harness = createReactHarness();
afterEach(() => { harness.cleanup(); vi.restoreAllMocks(); });
/** 打开工具栏的下拉菜单，读它的条目（菜单渲染在 body 上）。 */
const menuText = (host: HTMLElement, label: string) => {
  if (!document.querySelector(`[role="menu"][aria-label="${label}"]`)) click(host.querySelector(`button[aria-label="${label}"]`));
  return document.querySelector(`[role="menu"][aria-label="${label}"]`)?.textContent ?? "";
};
const props: ComponentProps<typeof FocusPane> = {
  page: PREVIEW_FOCUS, feeds: flattenFeeds(PREVIEW_FEEDS), expandedCluster: null,
  onFilter: () => {}, onExpand: () => {}, onRanking: () => {}, onRefresh: () => {},
  onOpen: () => {}, onCite: () => {}, onRate: () => {},
};
it("separates filter and list scope and explains unavailable ranking", () => {
  const { host } = harness.mount(<FocusPane {...props} />);
  const tabs = [...host.querySelectorAll(".toolbar .tab")].map((el) => el.textContent?.trim());
  expect(tabs).not.toContain("全部");
  expect(tabs).toContain("所有文章");
  expect(tabs).toContain("全部条目");
  expect(host.querySelector('[aria-label="评分过滤"]')).toBeTruthy();
  expect(host.querySelector('[aria-label="阅读范围"]')).toBeTruthy();
  expect(host.textContent).not.toContain("今日 token");
  expect(host.textContent).toContain("兴趣未就绪");
  expect(host.textContent).toContain("收起噪声源 · 闲聊广场");
  expect(menuText(host, "显示")).toContain("兴趣加分（未就绪）");
});
it("offers all items rather than refresh for an empty starred scope", () => {
  const onListScope = vi.fn();
  const { host } = harness.mount(<FocusPane {...props} page={{ ...PREVIEW_FOCUS, articles: [], list_scope: "starred" }} onListScope={onListScope} />);
  expect(host.querySelector(".empty")?.textContent).toContain("还没有收藏");
  expect(host.querySelector(".empty")?.textContent).not.toContain("刷新");
  click(host.querySelector(".empty button"));
  expect(onListScope).toHaveBeenCalledWith("all");
});
it("opens the digest from its lede", () => {
  const onOpenDigest = vi.fn();
  const { host } = harness.mount(<FocusPane {...props} onOpenDigest={onOpenDigest} />);
  expect(host.querySelector(".focus-lede")?.textContent).toContain("今日简报");
  click(host.querySelector(".focus-lede"));
  expect(onOpenDigest).toHaveBeenCalledOnce();
});
