import { describe, expect, it } from "vitest";
import { pseudoText, pseudoTree } from "./pseudo";

describe("pseudo-localization", () => {
  it("accents letters, pads about 40% and brackets, keeping interpolation and tags", () => {
    expect(pseudoText("Open {{name}} <b>now</b>")).toBe("[Öþéñ {{name}} <b>ñöŵ</b> ····]");
    expect(pseudoText("OK")).toBe("[ÖĶ]");
  });

  it("keeps joining spaces outside the brackets and leaves empty strings alone", () => {
    expect(pseudoText(", ")).toBe("[,] ");
    expect(pseudoText(" (added)")).toBe(" [(áððéð) ····]");
    expect(pseudoText("")).toBe("");
  });

  it("maps whole namespaces without changing their shape", () => {
    expect(pseudoTree({ a: { b_one: "1 item", b_other: "{{count}} items" } })).toEqual({
      a: { b_one: "[1 îţéɱ ···]", b_other: "[{{count}} îţéɱš ···]" },
    });
  });
});
