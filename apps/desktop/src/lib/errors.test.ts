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

  it("keeps the message of existing command-specific errors verbatim", () => {
    const message = "  Error: invalid JSON: fixture output\n";
    for (const kind of ["Network", "invalid_output", "other"]) {
      const error = { kind, message };
      expect(asUiError(error)).toMatchObject({ code: "legacy_text", detail: message });
      expect(formatError(error)).toBe(message);
    }
  });

  it("proxy classification depends on codes and the typed legacy source", () => {
    expect(isProxyError({ code: "network_proxy_auth", params: { status: 407, message: "proxy_auth" }, detail: "fixture" } satisfies UiError)).toBe(true);
    expect(isProxyError({ code: "legacy_text", params: { source: "network_proxy" }, detail: "fixture" } satisfies UiError)).toBe(true);
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
  if (error.code === "legacy_text") return error.params.source;
  if (error.code === "schema_unsupported") return error.params.supported;
  // @ts-expect-error parameters require narrowing to the owning variant
  return error.params.status;
}
void _typeContract;
