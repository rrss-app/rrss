import { describe, expect, it } from "vitest";
import { highlightable, highlightCode, type CodeToken } from "./codeHighlight";

const flat = (tokens: CodeToken[]): string => tokens.map((t) => typeof t === "string" ? t : flat(t.children)).join("");
const kinds = (tokens: CodeToken[]): string[] => tokens.flatMap((t) => typeof t === "string" ? [] : [t.kind, ...kinds(t.children)]);

describe("code highlighting", () => {
  it("uses the labelled language and keeps the text unchanged", async () => {
    const text = "fn main() {\n    // greet\n    let name = \"rrss\";\n    println!(\"{}\", name);\n}";
    const tokens = await highlightCode(text, "rust");
    expect(tokens).not.toBeNull();
    expect(flat(tokens!)).toBe(text);
    expect(kinds(tokens!)).toEqual(expect.arrayContaining(["keyword", "string", "comment"]));
  });

  it("leaves an unknown label uncolored instead of guessing", async () => {
    expect(await highlightCode("let x = 1;", "not-a-language")).toBeNull();
  });

  it("guesses unlabelled code only when confident", async () => {
    const js = "const response = await fetch(url);\nif (!response.ok) throw new Error(\"bad status\");\nexport default response;";
    expect(await highlightCode(js, null)).not.toBeNull();
    expect(await highlightCode("{\n  \"name\": \"rrss\",\n  \"count\": 2\n}", null)).not.toBeNull();
    expect(await highlightCode("Install it, then run it. If it fails, check the logs and try again.", null)).toBeNull();
    expect(await highlightCode("src/\n├── main.rs\n└── lib.rs", null)).toBeNull();
  });

  it("skips very large blocks", () => {
    expect(highlightable("x\n".repeat(2_000))).toBe(false);
    expect(highlightable("a".repeat(40_000))).toBe(false);
    expect(highlightable("   ")).toBe(false);
    expect(highlightable("let x = 1;")).toBe(true);
  });
});
