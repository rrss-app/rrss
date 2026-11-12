import { describe, expect, it } from "vitest";
import { sourceHue, sourceInitial } from "./sourceAvatar";

describe("sourceInitial", () => {
  it("uppercases a latin first letter and keeps CJK as-is", () => {
    expect(sourceInitial("rust blog")).toBe("R");
    expect(sourceInitial("  hacker news")).toBe("H");
    expect(sourceInitial("少数派")).toBe("少");
  });
  it("falls back to a dot for empty titles", () => {
    expect(sourceInitial("")).toBe("·");
    expect(sourceInitial("   ")).toBe("·");
  });
  it("handles astral characters without splitting surrogate pairs", () => {
    expect(sourceInitial("📰 Daily")).toBe("📰");
  });
});

describe("sourceHue", () => {
  it("is stable for the same title and within the hue circle", () => {
    const hue = sourceHue("Rust Blog");
    expect(hue).toBe(sourceHue("Rust Blog"));
    expect(hue).toBe(sourceHue("  Rust Blog  "));
    expect(hue).toBeGreaterThanOrEqual(0);
    expect(hue).toBeLessThan(360);
  });
  it("separates different titles", () => {
    expect(sourceHue("V2EX")).not.toBe(sourceHue("Hacker News"));
  });
});
