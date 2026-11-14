import { expect, it } from "vitest";
import type { UiError } from "../../bindings";
import { i18n } from "../../i18n";
import { formatFeedError } from "./feedHealth";

it("keeps the old HTTP display when reading text or a new code record", () => {
  for (const status of [401, 403, 404, 429, 502, 503, 504, 0, 500]) {
    const old: UiError = { code: "legacy_text", params: {}, detail: `HTTP ${status}: fixture` };
    const coded: UiError = { code: "feed_refresh_http", params: { status }, detail: "stored feed error" };
    expect(formatFeedError(old)).toBe(formatFeedError(coded));
    expect(old.detail).toBe(`HTTP ${status}: fixture`);
  }
  expect(formatFeedError({ code: "legacy_text", params: {}, detail: "database is locked" }))
    .toBe(i18n.t("database_busy", { ns: "errors" }));
  expect(formatFeedError({ code: "legacy_text", params: {}, detail: "fixture failure" })).toBe("fixture failure");
  expect(formatFeedError({ code: "legacy_text", params: {}, detail: "" })).toBe(i18n.t("feed_refresh_failed", { ns: "errors" }));
  expect(formatFeedError({ code: "legacy_text", params: {}, detail: "HTTP 99999: fixture" })).toBe("HTTP 99999: fixture");
});
