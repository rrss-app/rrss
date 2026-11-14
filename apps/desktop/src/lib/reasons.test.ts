import { afterEach, expect, it, vi } from "vitest";
import { i18n, SOURCE_LANGUAGE } from "../i18n";
import { formatReason } from "./reasons";

afterEach(async () => { vi.restoreAllMocks(); await i18n.changeLanguage(SOURCE_LANGUAGE); });

it("retains old preformatted content and formats legacy prefixes as reason codes", () => {
  const text = "older  stored sentence";
  expect(formatReason({ text })).toBe(text);
  expect(formatReason({ label: text })).toBe(text);
  expect(formatReason({ code: "legacy_text", params: { text, prefix: null } })).toBe(text);
  const translate = vi.spyOn(i18n, "t").mockReturnValue("rendered");
  formatReason({ code: "legacy_text", params: { text: "Rust", prefix: "title_re" } });
  expect(translate).toHaveBeenLastCalledWith("rule_title_re", expect.objectContaining({ ns: "reasons", replace: { text: "Rust", prefix: "title_re", pattern: "Rust" } }));
  formatReason({ code: "legacy_text", params: { text: "https://author.example/notes?q=topic", prefix: "rank_author_read" } });
  expect(translate).toHaveBeenLastCalledWith("rank_author", expect.objectContaining({ replace: {
    text: "https://author.example/notes?q=topic", prefix: "rank_author_read", pattern: "https://author.example/notes?q=topic", habit: "rendered",
  } }));
});

it("keeps enum localization separate from interpolation data and i18next options", () => {
  const translate = vi.spyOn(i18n, "t").mockReturnValue("habit word");
  formatReason({ code: "rank_author", params: { habit: "read", pattern: "Archive", ns: "errors", lng: "en", context: "skip" } });
  expect(translate).toHaveBeenCalledWith("habit.read", { ns: "reasons" });
  expect(translate).toHaveBeenLastCalledWith("rank_author", expect.objectContaining({
    ns: "reasons", context: undefined, replace: { habit: "habit word", pattern: "Archive", ns: "errors", lng: "en", context: "skip" },
  }));
  formatReason({ code: "rank_source", params: { habit: "skip" } });
  expect(translate).toHaveBeenLastCalledWith("rank_source", expect.objectContaining({ context: "skip", replace: { habit: "skip" } }));
});

it("formats raw counts in the current locale while preserving numeric plural selection", async () => {
  await i18n.changeLanguage("ar-EG-u-nu-arab");
  const translate = vi.spyOn(i18n, "t").mockReturnValue("rendered");
  formatReason({ code: "rank_coverage", params: { count: 9876 } });
  expect(translate).toHaveBeenLastCalledWith("rank_coverage", expect.objectContaining({ count: 9876, replace: { count: "٩٨٧٦" } }));
});

it("uses matched content for rule hits and gives future codes the common fallback", () => {
  const translate = vi.spyOn(i18n, "t").mockReturnValue("rendered");
  formatReason({ code: "rule_title_re", params: { pattern: "rust\\s+async" }, snippet: "Rust async" });
  expect(translate).toHaveBeenLastCalledWith("rule_title_re", expect.objectContaining({ replace: { pattern: "Rust async" } }));
  formatReason({ code: "future_reason", params: { pattern: "opaque" } });
  expect(translate).toHaveBeenLastCalledWith("unknown", { ns: "reasons" });
});
