import { expect, it } from "vitest";
import codes from "./uiCodes.json" with { type: "json" };
import reasons from "../locales/zh-CN/reasons.json" with { type: "json" };

it("every exported reason code has a source-language translation", () => {
  const translations: Record<string, unknown> = reasons;
  expect(codes.reasons.length).toBeGreaterThan(0);
  expect(codes.reasons.filter(code => typeof translations[code] !== "string" || !translations[code])).toEqual([]);
});
