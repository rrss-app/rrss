import { describe, expect, it } from "vitest";
import type { UiError } from "../bindings";
import { i18n } from "../i18n";
import {
  classifyAiError,
  explainAiError,
  redactAiText,
  secretsFromConnection,
  secretsFromUrl,
} from "./aiError";

describe("classifyAiError", () => {
  it("maps common HTTP and transport failures", () => {
    expect(classifyAiError("HTTP 401: invalid api key")).toBe("auth");
    expect(classifyAiError("403 Forbidden")).toBe("forbidden");
    expect(classifyAiError("HTTP 404: model not found")).toBe("notFound");
    expect(classifyAiError("429 too many requests")).toBe("rateLimit");
    expect(classifyAiError("error sending request: timed out")).toBe("timeout");
    expect(classifyAiError("connection refused (os error 111)")).toBe("network");
    expect(classifyAiError("HTTP 502: bad gateway")).toBe("server");
    expect(classifyAiError("something else")).toBe("other");
  });
});

describe("redactAiText", () => {
  it("redacts the current API key, URL userinfo, and query credentials", () => {
    const key = "sk-live-SUPERSECRET99";
    const url = "https://user:hunter2pass@api.example/v1?api_key=abcd1234token";
    const raw = `Authorization: Bearer ${key} url=${url} leftover ${key}`;
    const out = redactAiText(raw, secretsFromConnection(key, url));
    expect(out).not.toContain(key);
    expect(out).not.toContain("hunter2pass");
    expect(out).not.toContain("abcd1234token");
    expect(out).toContain("***");
  });

  it("redacts explicitly provided secrets even if they are short", () => {
    expect(redactAiText("ab failed", ["ab"])).toBe("*** failed");
  });
});

describe("explainAiError", () => {
  it("projects coded connection validation through its translation and hides internal detail", () => {
    const error = { code: "provider_url_invalid", detail: "diagnostic canary" } satisfies UiError;
    const view = explainAiError(error);
    expect(view.kind).toBe("other");
    expect(view.detail).toBe(i18n.t("provider_url_invalid", { ns: "errors" }));
    expect(view.detail).not.toContain("canary");
    const missing = explainAiError({ code: "provider_not_found", detail: "AI provider not found" } satisfies UiError);
    expect(missing.kind).toBe("notFound");
    expect(missing.detail).toBe(i18n.t("provider_not_found", { ns: "errors" }));
  });
  it("returns Chinese titles and redacted details", () => {
    const view = explainAiError(new Error("HTTP 401: key sk-ABCDEFGH12345678"), ["sk-ABCDEFGH12345678"]);
    expect(view.kind).toBe("auth");
    expect(view.title).toContain("API Key");
    expect(view.detail).not.toContain("sk-ABCDEFGH12345678");
  });
});

describe("secretsFromUrl", () => {
  it("picks userinfo and key-like query params", () => {
    expect(secretsFromUrl("https://alice:s3cretX@host/v1?token=tok_12345")).toEqual(
      expect.arrayContaining(["alice", "s3cretX", "tok_12345"]),
    );
  });
});
