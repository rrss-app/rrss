import { describe, expect, it } from "vitest";
import { i18n } from "../i18n";
import { REGISTRY } from "./registry";

// 注册表只记有哪些文案，文字在语言包里按键取；可选文案漏了类型检查发现不了，这里逐项核对键都在。
describe("settings registry copy", () => {
  it("has a language-pack entry for every field it declares", () => {
    const missing: string[] = [];
    const has = (key: string) => i18n.exists(key, { ns: "settingsSystem" }) || missing.push(key);
    for (const [id, shape] of Object.entries(REGISTRY)) {
      has(`registry.${id}.label`);
      for (const field of ["hint", "control", "on", "off"] as const) if (field in shape && shape[field as keyof typeof shape]) has(`registry.${id}.${field}`);
      if ("options" in shape) for (const value of shape.options as readonly string[]) has(`registry.${id}.options.${value}`);
    }
    expect(missing).toEqual([]);
  });
});
