import { describe, expect, it } from "vitest";
import { formatNotice } from "./notice";

describe("formatNotice", () => {
  it("hides invalid JSON model dump", () => {
    const dumped = `invalid JSON: It looks like you've shared the announcement for Armbian 26.8.\n\nHow can I help?`;
    expect(formatNotice("摘要生成失败：", dumped)).toBe(
      "摘要生成失败：模型没有返回可用的 JSON，请换模型或重试",
    );
  });

  it("clips long errors", () => {
    const out = formatNotice("刷新失败：", "x".repeat(200));
    expect(out.startsWith("刷新失败：")).toBe(true);
    expect(out.length).toBeLessThan(160);
    expect(out.endsWith("…")).toBe(true);
  });

  it("humanizes database lock", () => {
    expect(formatNotice("刷新失败：", "database is locked")).toBe(
      "刷新失败：数据库正忙，请稍后重试",
    );
  });
});
