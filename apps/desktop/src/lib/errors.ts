import type zhErrors from "../locales/zh-CN/errors.json";
import type { NetworkMessage, UiError } from "../bindings";
import { i18n } from "../i18n";
import codes from "./uiCodes.json" with { type: "json" };

/** A newer host may reject with a code this UI does not know yet. */
export type UiErrorEnvelope = {
  code: string;
  params: Record<string, string | number>;
  detail: string;
  cause?: UiErrorEnvelope;
};

export function asUiError(error: unknown, depth = 0): UiErrorEnvelope {
  try {
    if (error && typeof error === "object" && "code" in error && typeof error.code === "string") {
      const detail = "detail" in error && typeof error.detail === "string" ? error.detail : "";
      const params: UiErrorEnvelope["params"] = {};
      if ("params" in error && error.params && typeof error.params === "object") {
        for (const [name, value] of Object.entries(error.params)) {
          if (typeof value === "string" || typeof value === "number") params[name] = value;
        }
      }
      const value: UiErrorEnvelope = { code: error.code, params, detail };
      if (depth < 8 && "cause" in error && error.cause) value.cause = asUiError(error.cause, depth + 1);
      return value;
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
type ModelTask = keyof typeof zhErrors.model_task;

export function formatError(error: unknown): string {
  if (error && typeof error === "object" && "error" in error && error.error) return formatError(error.error);
  // Existing command-specific errors own their message projection. Do not run
  // Legacy command envelopes through
  // notice humanization just because they have no UI code.
  try {
    if (error && typeof error === "object" && !(error instanceof Error)
      && !("code" in error && typeof error.code === "string")
      && "message" in error && typeof error.message === "string") {
      return error.message;
    }
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
    if ((value.code.startsWith("network_") || value.code === "pi_download_failed")) {
      const key = `network_message.${params.message}` as `network_message.${NetworkMessage}`;
      params.message = i18n.exists(key, { ns: "errors" })
        ? String(i18n.t(key, { ns: "errors" })) : i18n.t("network_message.other", { ns: "errors" });
    }
    if (value.code === "feed_host_limited") {
      const until = Number(params.until);
      params.until = params.until != null && Number.isFinite(until) && Number.isFinite(new Date(until).getTime())
        ? new Date(until).toISOString().replace(/\.000Z$/, "+00:00").replace(/Z$/, "+00:00")
        : i18n.t("feed_distant_deadline", { ns: "errors" });
    }
    if (value.code === "task_model_unset" && params.label === "id") {
      const key = `model_task.${params.task}` as `model_task.${ModelTask}`;
      if (i18n.exists(key, { ns: "errors" })) params.task = String(i18n.t(key, { ns: "errors" }));
    }
    if (value.code === "mcp_process_exit" || value.code === "mcp_process_killed") {
      params.seconds = (Number(params.elapsed_ms) / 1000).toFixed(1);
    }
    if (value.code === "pi_download_failed") {
      const stage = `pi_download_stage.${params.stage}` as "pi_download_stage.asset_fetch";
      if (i18n.exists(stage, { ns: "errors" })) params.stage = String(i18n.t(stage, { ns: "errors" }));
    }
    if (value.code.startsWith("chat_handshake_")) {
      const step = `chat_handshake_step.${params.step}` as "chat_handshake_step.state";
      if (i18n.exists(step, { ns: "errors" })) params.step = String(i18n.t(step, { ns: "errors" }));
    }
    if (value.code === "chat_model_mismatch" && params.actual === undefined) {
      params.actual = String(i18n.t("chat_model_unselected", { ns: "errors" }));
    }
    if (value.code === "chat_version_unverified" && params.version === "unknown") params.version = String(i18n.t("chat_version_unknown", { ns: "errors" }));
    if (value.code === "site_rule_selector_invalid") {
      const key = `site_rule_field.${params.field}` as "site_rule_field.body";
      if (i18n.exists(key, { ns: "errors" })) params.field = String(i18n.t(key, { ns: "errors" }));
    }
    // Parameters are interpolation data, rather than i18next control options.
    return i18n.t(value.code as ErrorKey, { ns: "errors", replace: { ...params, detail: value.cause ? formatError(value.cause) : value.detail } });
  }
  return value.detail.trim()
    ? i18n.t("unknown_detail", { ns: "errors", detail: value.detail.trim() })
    : i18n.t("unknown", { ns: "errors" });
}

export function formatMediaFailure(code: string): string {
  return ERROR_CODES.has(code) ? formatError({ code, params: {}, detail: "" }) : code;
}

export function isProxyError(error: unknown): boolean {
  const { code } = asUiError(error);
  return code.startsWith("network_proxy_") || code.startsWith("proxy_");
}
