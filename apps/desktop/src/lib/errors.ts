import type { NetworkMessage, UiError } from "../bindings";
import { i18n } from "../i18n";
import codes from "./uiCodes.json" with { type: "json" };

/** A newer host may reject with a code this UI does not know yet. */
export type UiErrorEnvelope = {
  code: string;
  params: Record<string, string | number>;
  detail: string;
};

export function asUiError(error: unknown): UiErrorEnvelope {
  try {
    if (error && typeof error === "object" && "code" in error && typeof error.code === "string") {
      const detail = "detail" in error && typeof error.detail === "string" ? error.detail : "";
      const params: UiErrorEnvelope["params"] = {};
      if ("params" in error && error.params && typeof error.params === "object") {
        for (const [name, value] of Object.entries(error.params)) {
          if (typeof value === "string" || typeof value === "number") params[name] = value;
        }
      }
      return { code: error.code, params, detail };
    }
    const detail = error instanceof Error ? error.message
      : error && typeof error === "object" && "message" in error && typeof error.message === "string" ? error.message
      : String(error);
    return { code: "legacy_text", params: {}, detail };
  } catch {
    return { code: "internal", params: {}, detail: "" };
  }
}

const ERROR_CODES = new Set<string>(codes.errors);
type ErrorKey = UiError["code"];

export function formatError(error: unknown): string {
  // Existing command-specific errors own their message projection. Do not run
  // FulltextError, TranslationCommandError or ModelBindingError through legacy
  // notice humanization just because they have no UI code.
  try {
    if (error && typeof error === "object" && !(error instanceof Error)
      && !("code" in error && typeof error.code === "string")
      && "message" in error && typeof error.message === "string") return error.message;
  } catch { /* asUiError supplies the unreadable-input fallback. */ }
  const value = asUiError(error);
  if (value.code === "legacy_text") {
    const detail = value.detail.replace(/^Error:\s*/, "").trim();
    // Preserve the two existing notice projections while old hosts / commands
    // still send strings. They disappear with the legacy transport path.
    if (/invalid JSON/i.test(detail)) return i18n.t("notice.invalidJson", { ns: "shell" });
    if (/database is locked|database lock|sqlite_busy/i.test(detail)) return i18n.t("database_busy", { ns: "errors" });
    return i18n.t("legacy_text", { ns: "errors", detail });
  }
  if (ERROR_CODES.has(value.code) && i18n.exists(value.code, { ns: "errors" })) {
    const params = { ...value.params };
    if (value.code.startsWith("network_")) {
      const key = `network_message.${params.message}` as `network_message.${NetworkMessage}`;
      params.message = i18n.exists(key, { ns: "errors" })
        ? String(i18n.t(key, { ns: "errors" })) : i18n.t("network_message.other", { ns: "errors" });
    }
    // Parameters are interpolation data, rather than i18next control options.
    return i18n.t(value.code as ErrorKey, { ns: "errors", replace: params });
  }
  return value.detail.trim()
    ? i18n.t("unknown_detail", { ns: "errors", detail: value.detail.trim() })
    : i18n.t("unknown", { ns: "errors" });
}

export function isProxyError(error: unknown): boolean {
  const { code, params } = asUiError(error);
  return code.startsWith("network_proxy_") || code.startsWith("proxy_")
    || (code === "legacy_text" && params.source === "network_proxy");
}
