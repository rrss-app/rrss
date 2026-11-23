import { describe, expect, it } from "vitest";
import { applyStarToFocusPage, flattenFeeds, focusExcludes, mergeFocusPage, mergeLowerArrivals, promoteArrivals } from "./focusUtils";
import { PREVIEW_FEEDS, PREVIEW_FOCUS } from "./preview";
import { focusRow, TWO_ARTICLE_PAGE } from "../test/fixtures/reader";

describe("focus page transformations", () => {
  it("flattens uncategorized feeds before folder feeds", () => {
    expect(flattenFeeds(PREVIEW_FEEDS).map((feed) => feed.title)).toEqual(["gadgetwire", "闲聊广场", "Compiler Notes"]);
  });
  it("excludes articles and clusters when fetching another page", () => {
    expect(focusExcludes([
      { ...focusRow(11, "A"), cluster_id: 7 },
      { ...focusRow(7, "header"), row_kind: 1, cluster_id: 7 },
      focusRow(12, "B"),
    ])).toEqual({ excludeIds: [11, 12], excludeClusterIds: [7] });
  });
  it("appends only unseen rows and keeps the original banner and status", () => {
    const next = mergeFocusPage(PREVIEW_FOCUS, { ...PREVIEW_FOCUS, digest_banner: "should-not-replace", has_more: false,
      articles: [PREVIEW_FOCUS.articles[0], { ...PREVIEW_FOCUS.articles[0], id: 99, title: "下一页" }] });
    expect(next.digest_banner).toBe(PREVIEW_FOCUS.digest_banner);
    expect(next.has_more).toBe(false);
    expect(next.articles.map((row) => row.id)).toEqual([...PREVIEW_FOCUS.articles.map((row) => row.id), 99]);
  });
  it("inserts only the lower arrivals inside a partially visible group", () => {
    const member = (id: number) => ({ ...focusRow(id, String(id)), cluster_id: 7 });
    const header = { ...member(7), row_kind: 1 };
    const footer = { ...member(7), row_kind: 2 };
    const current = { ...TWO_ARTICLE_PAGE, articles: [header, member(1), member(2), footer] };
    const latest = { ...current, articles: [header, member(3), member(1), member(4), member(2), footer] };
    expect(mergeLowerArrivals(current, latest, new Set([3, 4]), 1).articles).toEqual([
      header, member(1), member(4), member(2), footer,
    ]);
  });
  it("places arrivals among shown rows only, so old rows that rose do not push them below the reading position", () => {
    const rows = [1, 2, 3, 4].map(id => focusRow(id, String(id)));
    const current = { ...TWO_ARTICLE_PAGE, articles: rows };
    // 20、21 是没显示过的旧文章：不插入，也不占位置。
    const latest = { ...current, articles: [rows[0], focusRow(20, "20"), focusRow(21, "21"), focusRow(9, "9"), ...rows.slice(1)] };
    expect(mergeLowerArrivals(current, latest, new Set([9]), 0).articles.map(row => row.id)).toEqual([1, 9, 2, 3, 4]);
    expect(mergeLowerArrivals(current, latest, new Set([9]), 1)).toBe(current);
  });
  it("moves new arrivals and their groups to the top and keeps the rest in the current order", () => {
    const member = (id: number) => ({ ...focusRow(id, String(id)), cluster_id: 7 });
    const header = { ...member(7), row_kind: 1 };
    const current = { ...TWO_ARTICLE_PAGE, articles: [focusRow(1, "1"), header, member(2), { ...focusRow(3, "3"), read: true }] };
    const latest = { ...current, articles: [focusRow(5, "旧"), { ...focusRow(3, "3"), read: false }, header, member(9), focusRow(1, "1")] };
    const next = promoteArrivals(current, latest, new Set([9]));
    expect(next.articles.map(row => [row.row_kind, row.id])).toEqual([[1, 7], [0, 9], [0, 1], [0, 3]]);
    expect(next.articles.at(-1)?.read).toBe(true);
  });
  it("removes an empty starred cluster including its header and more row, keeping other clusters", () => {
    const card = { ...focusRow(42, "唯一收藏"), cluster_id: 10, starred: true };
    const articles = [{ ...card, id: 10, row_kind: 1 }, card, { ...card, id: 10, row_kind: 2 }];
    const page = { ...TWO_ARTICLE_PAGE, list_scope: "starred", articles };
    expect(applyStarToFocusPage(page, 42, false).articles).toEqual([]);
    const other = [{ ...focusRow(2, "B"), cluster_id: 9, starred: true }, { ...focusRow(9, "簇B"), cluster_id: 9, row_kind: 1 }];
    expect(applyStarToFocusPage({ ...page, articles: [...articles, ...other] }, 42, false).articles).toEqual(other);
  });
});
