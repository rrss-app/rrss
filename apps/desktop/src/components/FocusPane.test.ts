import { describe, expect, it } from "vitest";
import { PREVIEW_FEEDS, PREVIEW_FOCUS } from "../lib/preview";
import { flattenFeeds } from "../lib/focusUtils";
import { emptyIcon, focusEmptyState } from "./FocusPane";

describe("focus empty state while feed metadata is unavailable", () => {
  it("does not show a no-subscriptions message above existing articles", () => {
    expect(focusEmptyState(PREVIEW_FOCUS, []).message).toBe("");
  });
  it("still explains a genuinely empty library", () => {
    expect(focusEmptyState({ ...PREVIEW_FOCUS, articles: [] }, []).message).toContain("还没有订阅");
  });
});

describe("focus empty state icon", () => {
  const feeds = flattenFeeds(PREVIEW_FEEDS);
  it("points at subscriptions when there are none", () => {
    expect(emptyIcon({ ...PREVIEW_FOCUS, articles: [] }, [])).toBe("rss");
  });
  it("matches the list scope otherwise", () => {
    expect(emptyIcon({ ...PREVIEW_FOCUS, articles: [], list_scope: "starred" }, feeds)).toBe("star");
    expect(emptyIcon({ ...PREVIEW_FOCUS, articles: [], list_scope: "unread" }, feeds)).toBe("checkAll");
    expect(emptyIcon({ ...PREVIEW_FOCUS, articles: [], list_scope: "all" }, feeds)).toBe("inbox");
  });
});
