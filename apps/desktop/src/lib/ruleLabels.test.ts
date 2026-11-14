import { describe, expect, it } from "vitest";
import type { RuleEffect, RuleView } from "../bindings";
import { i18n } from "../i18n";
import { ruleConditionLabel, formatRuleSummary } from "./ruleLabels";
import { ruleFixture } from "../test/fixtures/settings";

const effects = { boost: true, hide: true, lower: true, none: true } satisfies Record<RuleEffect, true>;
const dimensions = ["author", "tag", "title_kw", "title_re", "url_phrase", "url_re", "body_phrase", "body_re"];

describe("rule display ids", () => {
  it("has condition copy in both consumers and preserves content patterns", () => {
    const pattern = "literal.* < & / sample";
    for (const ns of ["focus", "settingsContent"] as const) for (const dimension of dimensions) {
      expect(i18n.exists(`ruleConditions.${dimension}`, { ns }), `${ns}:${dimension}`).toBe(true);
      expect(ruleConditionLabel({ dimension, pattern }, ns)).toContain(pattern);
    }
  });

  it("formats every effect and scope without inventing a scope name", () => {
    for (const effect of Object.keys(effects) as RuleEffect[]) for (const scope_kind of ["global", "feed", "folder"]) {
      expect(i18n.exists(`rules.effect.${effect}`, { ns: "settingsContent" })).toBe(true);
      expect(i18n.exists(`rules.summaryScope.${scope_kind}`, { ns: "settingsContent" })).toBe(true);
      const summary: RuleView["summary"] = { code: "rule_summary", params: { effect } };
      const base = ruleFixture({ summary, scope_kind, scope_name: "" });
      expect(formatRuleSummary(base)).not.toContain("{{");
      expect(formatRuleSummary(base)).not.toContain("ruleConditions.");
      const named = formatRuleSummary({ ...base, scope_name: "Example source" });
      if (scope_kind === "global") expect(named).toBe(formatRuleSummary(base));
      else expect(named).toContain("Example source");
    }
  });
});
