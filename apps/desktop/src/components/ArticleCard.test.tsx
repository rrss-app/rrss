import { act } from "react";
import { afterEach, expect, it, vi } from "vitest";
import type { ArticleView } from "../lib/api";
import { ArticleCard } from "./ArticleCard";
import { focusRow } from "../test/fixtures/reader";
import { createReactHarness } from "../test/react";
import { click } from "../test/fixtures/shell";

const harness = createReactHarness();
afterEach(() => { harness.cleanup(); vi.restoreAllMocks(); });
const actions = { onCite: () => {}, onRate: () => {} };
function setup(patch: Partial<ArticleView> = {}) {
  const row = { ...focusRow(42, "测试文章"), score: 0, ...patch };
  const cb = { onOpen: vi.fn(), onCite: vi.fn(), onRate: vi.fn(), onStar: vi.fn() };
  const { host } = harness.mount(<ArticleCard row={row} {...cb} />);
  return { host, cb, row };
}

it("omits neutral score but keeps title, source and summary", () => {
  const { host, row } = setup({ summary: "一行摘要" });
  expect(host.querySelector(".score")).toBeNull();
  expect(host.querySelector("h3")?.textContent).toBe(row.title);
  expect(host.querySelector(".article-source")?.textContent).toBe(row.feed_title);
  expect(host.querySelector(".summary")?.textContent).toBe("一行摘要");
});
it("keeps meaningful scores and feedback actions", () => {
  const { host, cb, row } = setup({ score: 10 });
  expect(host.querySelector(".score")?.textContent).toBe("+10");
  click(host.querySelector('[aria-label="赞"]'));
  expect(cb.onRate).toHaveBeenCalledWith(row, true);
  expect(cb.onOpen).not.toHaveBeenCalled();
});
it("opens the focused article with Enter but does not reopen from a child button", () => {
  const { host, cb, row } = setup();
  const article = host.querySelector("article")!;
  expect(article.tabIndex).toBe(0);
  act(() => article.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true })));
  expect(cb.onOpen).toHaveBeenCalledOnce();
  expect(cb.onOpen).toHaveBeenCalledWith(row.id);
  click(host.querySelector(".star-btn"));
  expect(cb.onStar).toHaveBeenCalledWith(row);
  expect(cb.onOpen).toHaveBeenCalledOnce();
});
it("marks unread without a dot and opens the article", () => {
  const onOpen = vi.fn();
  const { host } = harness.mount(<ArticleCard row={focusRow(42, "测试文章")} onOpen={onOpen} {...actions} />);
  const article = host.querySelector("article");
  expect(article?.className).toContain("article unread");
  // 未读只靠标题字重与颜色区分，不画圆点。
  expect(article?.querySelector(".mark")).toBeNull();
  click(article);
  expect(onOpen).toHaveBeenCalledWith(42);
});
it("removes the unread class when read", () => {
  const { host } = harness.mount(<ArticleCard row={{ ...focusRow(42, "测试文章"), read: true }} onOpen={() => {}} {...actions} />);
  expect(host.querySelector("article")?.className).toBe("article");
});

it("shows the publish time in local relative wording and keeps it current", () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(2026, 8, 27, 15, 30, 0));
  try {
    const published = new Date(2026, 8, 27, 15, 25).getTime();
    const { host } = setup({ published_at: published });
    const time = host.querySelector<HTMLTimeElement>("time.article-time")!;
    expect(time.textContent).toBe("5 分钟前");
    expect(time.title).toBe("2026年9月27日 15:25");
    expect(time.dateTime).toBe(new Date(published).toISOString());
    // 保活的列表不重新拉取也要跟着时间走。
    act(() => { vi.advanceTimersByTime(60 * 60_000); });
    expect(time.textContent).toBe("1 小时前");
  } finally {
    vi.useRealTimers();
  }
});
it("omits the time separator when the publish time is unknown", () => {
  const { host } = setup({ published_at: null });
  expect(host.querySelector(".article-time")).toBeNull();
  expect(host.querySelector(".meta-sep")).toBeNull();
});
