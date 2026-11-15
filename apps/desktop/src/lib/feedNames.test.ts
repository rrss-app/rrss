import { describe, expect, it } from "vitest";
import { i18n } from "../i18n";
import { MANUAL_FEED_URL, nameBuiltInFeeds } from "./feedNames";

describe("built-in feed names", () => {
  it("names only the untitled manual source, at any depth", () => {
    const name = i18n.t("manualFeed", { ns: "feeds" });
    const page = nameBuiltInFeeds({
      articles: [{ id: 1, feed_title: "" }, { id: 2, feed_title: "Fixture Weekly" }],
      tree: { uncategorized: [{ id: 3, url: MANUAL_FEED_URL, title: "" }, { id: 4, url: "https://feed.example/rss", title: "" }] },
    });
    expect(page.articles.map((article) => article.feed_title)).toEqual([name, "Fixture Weekly"]);
    expect(page.tree.uncategorized.map((feed) => feed.title)).toEqual([name, ""]);
    expect(nameBuiltInFeeds(null)).toBeNull();
  });
});
