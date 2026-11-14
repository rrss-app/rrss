import type { ReasonMessage, ReasonPrefix } from "../bindings";
import { i18n, locale } from "../i18n";
import codes from "./uiCodes.json" with { type: "json" };

const REASON_CODES = new Set<string>(codes.reasons);
const LEGACY_PREFIX: Record<ReasonPrefix, { code: ReasonMessage["code"]; habit?: "read" | "skip" }> = {
  author: { code: "rule_author" }, tag: { code: "rule_tag" }, title_kw: { code: "rule_title_kw" }, title_re: { code: "rule_title_re" },
  url_phrase: { code: "rule_url_phrase" }, url_re: { code: "rule_url_re" }, body_phrase: { code: "rule_body_phrase" }, body_re: { code: "rule_body_re" },
  interest: { code: "interest_match" },
  rank_title_kw_read: { code: "rank_title_kw", habit: "read" }, rank_title_kw_skip: { code: "rank_title_kw", habit: "skip" },
  rank_author_read: { code: "rank_author", habit: "read" }, rank_author_skip: { code: "rank_author", habit: "skip" },
  rank_tag_read: { code: "rank_tag", habit: "read" }, rank_tag_skip: { code: "rank_tag", habit: "skip" },
  rank_domain_read: { code: "rank_domain", habit: "read" }, rank_domain_skip: { code: "rank_domain", habit: "skip" },
  rank_affinity: { code: "rank_affinity" },
};

export function formatReasonNumber(value: number): string {
  return new Intl.NumberFormat(locale(), { useGrouping: false }).format(value);
}

/** Handles stored legacy content and future host codes without exposing a code as prose. */
export function formatReason(reason: unknown): string {
  if (!reason || typeof reason !== "object") return i18n.t("unknown", { ns: "reasons" });
  // Old preview/host envelopes used either label or text directly.
  if (!("code" in reason) || typeof reason.code !== "string") {
    if ("text" in reason && typeof reason.text === "string") return reason.text;
    if ("label" in reason && typeof reason.label === "string") return reason.label;
    return i18n.t("unknown", { ns: "reasons" });
  }
  let code = reason.code;
  const params: Record<string, string | number> = {};
  if ("params" in reason && reason.params && typeof reason.params === "object") {
    for (const [name, value] of Object.entries(reason.params)) {
      if (typeof value === "string" || typeof value === "number") params[name] = value;
    }
  }
  if (code === "legacy_text" && params.prefix && Object.hasOwn(LEGACY_PREFIX, params.prefix)) {
    const legacy = LEGACY_PREFIX[params.prefix as ReasonPrefix];
    code = legacy.code;
    params.pattern = params.text;
    if (legacy.habit) params.habit = legacy.habit;
  } else if (code.startsWith("rule_") && !code.startsWith("rule_condition_")
    && "snippet" in reason && typeof reason.snippet === "string") {
    // Matched content stays in its existing content field, outside safe parameters.
    // Author casing and regex captures must keep their original display.
    params.pattern = reason.snippet;
  }
  if (!REASON_CODES.has(code) || !i18n.exists(code, { ns: "reasons" })) {
    return i18n.t("unknown", { ns: "reasons" });
  }
  const habit = params.habit;
  if (code !== "rank_source" && (habit === "read" || habit === "skip")) {
    params.habit = i18n.t(`habit.${habit}`, { ns: "reasons" });
  }
  const count = typeof params.count === "number" ? params.count : undefined;
  if (count !== undefined) params.count = formatReasonNumber(count);
  // Data cannot override namespace, locale, context or other i18next options.
  return i18n.t(code as ReasonMessage["code"], {
    ns: "reasons", replace: params, count,
    context: code === "rank_source" && (habit === "read" || habit === "skip") ? habit : undefined,
  });
}
