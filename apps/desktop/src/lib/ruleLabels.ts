import type { FeatureView, RuleView } from "../bindings";
import { i18n } from "../i18n";

export function ruleConditionLabel(feature: Pick<FeatureView, "dimension" | "pattern">, ns: "settingsContent" | "focus" = "settingsContent"): string {
  const key = `ruleConditions.${feature.dimension}`;
  return i18n.exists(key, { ns }) ? String(i18n.t(key as never, { ns, pattern: feature.pattern })) : feature.pattern;
}

export function formatRuleSummary(rule: RuleView): string {
  const text = (key: string, replace: Record<string, unknown> = {}) => String(i18n.t(key as never, { ns: "settingsContent", replace }));
  const prefix = rule.scope_kind !== "global" && rule.scope_name ? text(`rules.summaryPrefix.${rule.scope_kind}`, { name: rule.scope_name }) : "";
  return text("rules.summary", {
    prefix, condition: ruleConditionLabel({ dimension: rule.dimension_id, pattern: rule.pattern }),
    effect: text(`rules.effect.${rule.summary.params.effect}`), scope: text(`rules.summaryScope.${rule.scope_kind}`),
  });
}
