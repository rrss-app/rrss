import { describe, expect, it } from "vitest";
import { i18n } from "../../i18n";
import { chatStepLabel } from "./TurnProgress";

describe("chat step labels", () => {
  it("covers built-in tool ids and preserves unknown and external tool names", () => {
    for (const tool of ["search_articles", "list_focus", "list_feeds", "get_article", "get_digest", "search_memory", "fetch_url", "open_original", "explain_score", "mark_read", "star_article", "add_interest"]) {
      expect(i18n.exists(`steps.tools.${tool}`, { ns: "chat" }), tool).toBe(true);
    }
    const step = { kind: "tool" as const, tool: "weather", server: "", serverTool: "" };
    expect(chatStepLabel(step)).toContain("weather");
    expect(chatStepLabel({ ...step, server: "files", serverTool: "read_file" })).toContain("read_file");
  });

  it("resolves an existing step again when the interface language changes", async () => {
    const language = i18n.language;
    const step = { kind: "tool" as const, tool: "search_articles", server: "", serverTool: "" };
    const before = chatStepLabel(step);
    i18n.addResource("en", "chat", "steps.tools.search_articles", "Search local articles");
    try {
      await i18n.changeLanguage("en");
      expect(chatStepLabel(step)).toBe("Search local articles");
      expect(chatStepLabel(step)).not.toBe(before);
    } finally {
      await i18n.changeLanguage(language);
      i18n.removeResourceBundle("en", "chat");
    }
  });
});
