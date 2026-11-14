import { expect, it } from "vitest";
import codes from "./uiCodes.json" with { type: "json" };
import errors from "../locales/zh-CN/errors.json" with { type: "json" };
import type { NetworkMessage } from "../bindings";

it("every exported error code and network message has a source-language translation", () => {
  const translations: Record<string, unknown> = errors;
  expect(codes.errors.filter(code => typeof translations[code] !== "string" || !translations[code])).toEqual([]);
  const messages: Record<NetworkMessage, string> = errors.network_message;
  expect(Object.values(messages).every(message => typeof message === "string" && message.length > 0)).toBe(true);
});
