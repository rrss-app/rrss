import { describe, expect, it } from "vitest";
import { fuzzyMatch, shortModel } from "./fuzzy";

describe("fuzzyMatch", () => {
  it("matches characters in order and prefers word starts", () => {
    expect(fuzzyMatch("openai/gpt-5-mini", "gpt5m")?.indices).toEqual([7, 8, 9, 11, 13]);
    const start = fuzzyMatch("anthropic/claude-sonnet-4.5", "son")!;
    const inner = fuzzyMatch("deepseek-reasoner", "rea")!;
    expect(start.score).toBeGreaterThan(0);
    expect(inner.score).toBeGreaterThan(0);
  });

  it("drops scattered matches and misses", () => {
    expect(fuzzyMatch("deepseek-reasoner", "son")).toBeNull();
    expect(fuzzyMatch("qwen2.5:14b", "llama")).toBeNull();
    expect(fuzzyMatch("anything", "  ")).toEqual({ score: 0, indices: [] });
  });
});

describe("shortModel", () => {
  it("drops the vendor prefix and cuts the middle of long ids", () => {
    expect(shortModel("anthropic/claude-sonnet-4.5")).toBe("claude-sonnet-4.5");
    expect(shortModel("qwen2.5:14b")).toBe("qwen2.5:14b");
    expect(shortModel("mistralai/mistral-small-3.2-24b-instruct")).toBe("mistral-small…instruct");
  });
});
