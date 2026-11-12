import { describe, expect, it } from "vitest";
import { applyStarToFocusPage, flattenFeeds, focusExcludes, mergeFocusPage } from "./focusUtils";
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
    const next = mergeFocusPage(PREVIEW_FOCUS, { ...PREVIEW_FOCUS, digest_banner: "should-not-replace", status: "", has_more: false,
      articles: [PREVIEW_FOCUS.articles[0], { ...PREVIEW_FOCUS.articles[0], id: 99, title: "下一页" }] });
    expect(next.digest_banner).toBe(PREVIEW_FOCUS.digest_banner);
    expect(next.status).toBe(PREVIEW_FOCUS.status);
    expect(next.has_more).toBe(false);
    expect(next.articles.map((row) => row.id)).toEqual([...PREVIEW_FOCUS.articles.map((row) => row.id), 99]);
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
