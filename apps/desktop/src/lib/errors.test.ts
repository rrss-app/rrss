import { describe, expect, it } from "vitest";
import type { UiError } from "../bindings";
import { i18n } from "../i18n";
import { asUiError, formatError, isProxyError } from "./errors";

describe("UI error transport", () => {
  it("normalizes old strings and Error instances without imposing a new input contract", () => {
    expect(asUiError("fixture failure")).toEqual({ code: "legacy_text", params: {}, detail: "fixture failure" });
    expect(asUiError(new Error("fixture failure"))).toMatchObject({ code: "legacy_text", detail: "fixture failure" });
    expect(formatError("fixture failure")).toBe("fixture failure");
    expect(asUiError({ code: "future_failure", params: null, detail: "fixture" })).toMatchObject({ code: "future_failure", params: {}, detail: "fixture" });
    const unreadable = { get code() { throw new Error("fixture getter"); } };
    expect(asUiError(unreadable).code).toBe("internal");
  });

  it("uses code and typed parameters rather than the diagnostic text", () => {
    const error: UiError = { code: "network_http", params: { status: 503, message: "http" }, detail: "diagnostic canary" };
    expect(formatError(error)).toBe(`HTTP 503: ${i18n.t("network_message.http", { ns: "errors" })}`);
    expect(formatError(error)).toContain("503");
    expect(formatError(error)).not.toContain("canary");
    expect(formatError(error)).not.toContain("$t(");
    expect(formatError({ ...error, params: { ...error.params, ns: "shell", returnDetails: "true" } })).toBe(formatError(error));
  });

  it("unknown codes get a general explanation and diagnostic detail", () => {
    expect(formatError({ code: "future_failure", detail: "fixture detail" })).toBe(i18n.t("unknown_detail", { ns: "errors", detail: "fixture detail" }));
    expect(formatError({ code: "future_failure" })).toBe(i18n.t("unknown", { ns: "errors" }));
  });

  it("localizes known task IDs while retaining user-provided task names", () => {
    const error = { code: "task_model_unset", params: { task: "focus_summary", label: "id" }, detail: "diagnostic canary" } satisfies UiError;
    expect(formatError(error)).toBe(i18n.t("task_model_unset", { ns: "errors", task: i18n.t("model_task.focus_summary", { ns: "errors" }) }));
    const named = { code: "task_model_unset", params: { task: "fixture task", label: "name" }, detail: "diagnostic canary" } satisfies UiError;
    expect(formatError(named)).toBe(i18n.t("task_model_unset", { ns: "errors", task: "fixture task" }));
    expect(formatError(named)).not.toContain("canary");
  });

  it("preserves setting parameters as data when formatting validation failures", () => {
    const error = { code: "setting_option_invalid", params: { value: "fixture", options: "off / on" }, detail: "diagnostic canary" } satisfies UiError;
    expect(formatError(error)).toBe(i18n.t("setting_option_invalid", { ns: "errors", replace: error.params }));
    expect(formatError({ ...error, params: { ...error.params, lng: "en", ns: "shell" } })).toBe(formatError(error));
    expect(formatError(error)).not.toContain("canary");
  });

  it("keeps the message of existing command-specific errors verbatim", () => {
    const message = "  Error: invalid JSON: fixture output\n";
    for (const kind of ["Network", "invalid_output", "other"]) {
      const error = { kind, message };
      expect(asUiError(error)).toMatchObject({ code: "legacy_text", detail: message });
      expect(formatError(error)).toBe(message);
    }
  });

  it("proxy classification depends on domain codes", () => {
    expect(isProxyError({ code: "network_proxy_auth", params: { status: 407, message: "proxy_auth" }, detail: "fixture" } satisfies UiError)).toBe(true);
    expect(isProxyError({ code: "proxy_custom_url_missing", detail: "fixture" } satisfies UiError)).toBe(true);
    expect(isProxyError({ code: "legacy_text", params: {}, detail: "proxy fixture" } satisfies UiError)).toBe(false);
    expect(isProxyError({ code: "internal", detail: "proxy authentication fixture" } satisfies UiError)).toBe(false);
    expect(isProxyError("proxy authentication fixture")).toBe(false);
  });
});

function _typeContract(error: UiError) {
  if (error.code === "network_http") {
    const status: number = error.params.status;
    // @ts-expect-error this domain has no name parameter
    void error.params.name;
    return status;
  }
  if (error.code === "legacy_text") return error.params;
  if (error.code === "schema_unsupported") return error.params.supported;
  // @ts-expect-error parameters require narrowing to the owning variant
  return error.params.status;
}
void _typeContract;

// Typed dialogue failures render through the same code boundary as invoke errors.
describe("chat error codes", () => {
  it("renders operational parameters and explicit diagnostic suffixes", () => {
    const code = "chat_model_mismatch";
    expect(formatError({ code, params: { expected: "fixture-model" }, detail: "canary" }))
      .toBe(i18n.t(code, { ns: "errors", expected: "fixture-model", actual: i18n.t("chat_model_unselected", { ns: "errors" }) }));
    expect(formatError({ code: "chat_reply_save_failed", detail: "disk full" }))
      .toBe(i18n.t("chat_reply_save_failed", { ns: "errors", detail: "disk full" }));
    expect(formatError({ code: "chat_handshake_timeout", params: { step: "state" }, detail: "canary" }))
      .toBe(i18n.t("chat_handshake_timeout", { ns: "errors", step: i18n.t("chat_handshake_step.state", { ns: "errors" }) }));
  });
});

it("keeps the Pi download stage and HTTP status without exposing service bodies", () => {
  const error = { code: "pi_download_failed", params: { stage: "asset_status", status: 407, message: "other" }, detail: "body canary" };
  expect(formatError(error)).toBe(i18n.t("pi_download_failed", { ns: "errors", stage: i18n.t("pi_download_stage.asset_status", { ns: "errors" }), status: 407, message: i18n.t("network_message.other", { ns: "errors" }) }));
  expect(formatError(error)).not.toContain("canary");
});

it("formats MCP process timing from numeric transport values", () => {
  expect(formatError({ code: "mcp_process_exit", params: { elapsed_ms: 1200, status: 2 }, detail: "fixture" }))
    .toBe(i18n.t("mcp_process_exit", { ns: "errors", seconds: "1.2", status: 2 }));
});

it("renders job HTTP and model validation codes without provider response data", () => {
  for (const [code, params] of [["job_model_http", { status: 503 }], ["job_model_proxy_auth", { status: 407 }], ["job_model_invalid_json", {}]] as const) {
    expect(formatError({ code, params, detail: "private response canary" })).toBe(i18n.t(code, { ns: "errors", replace: params }));
    expect(formatError({ code, params, detail: "private response canary" })).not.toContain("canary");
  }
});

it("formats typed causes inside chat failure prefixes", () => {
  const cause = { code: "schema_too_old", params: { found: 1, min: 38 }, detail: "diagnostic canary" } satisfies UiError;
  const error = { code: "chat_session_save_failed", detail: "diagnostic canary", cause } satisfies UiError;
  expect(formatError(error)).toBe(i18n.t("chat_session_save_failed", { ns: "errors", detail: formatError(cause) }));
  expect(formatError(error)).not.toContain("canary");
  expect(asUiError(error).cause?.code).toBe(cause.code);
});
