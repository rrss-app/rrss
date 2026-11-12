import { describe, expect, it } from "vitest";
import { parseDocument } from "./blockIr";

describe("block IR entry validation", () => {
  it("drops unknown or malformed nodes and keeps the rest", () => {
    const doc = parseDocument({
      v: 1,
      blocks: [
        { t: "p" },
        { t: "p", inlines: [{ t: "t", text: "a" }, { t: "nope" }, 3, { t: "a", href: 1, children: [] }, { t: "s", children: "x" }] },
        { t: "h", level: "3", inlines: [] },
        { t: "li", inlines: [{ t: "br" }] },
        { t: "img", alt: "no src" },
        { t: "pre", lang: 5, text: "x" },
        { t: "embed", url: "https://v.example", kind: "video" },
        { t: "unknown" },
        null,
        "p",
        { t: "hr", extra: true },
      ],
    });
    expect(doc.v).toBe(1);
    expect(doc.blocks).toEqual([
      { t: "p", inlines: [] },
      { t: "p", inlines: [{ t: "t", text: "a" }, { t: "s", children: [] }] },
      { t: "h", level: 1, inlines: [] },
      { t: "li", indent: 0, marker: "", inlines: [{ t: "br" }] },
      { t: "pre", lang: null, text: "x" },
      { t: "embed", kind: "video", url: "https://v.example", title: "" },
      { t: "hr" },
    ]);
  });

  it("degrades non-documents to an empty document", () => {
    expect(parseDocument(null).blocks).toEqual([]);
    expect(parseDocument("x").blocks).toEqual([]);
    expect(parseDocument({ v: 1, blocks: "nope" }).blocks).toEqual([]);
    expect(parseDocument([]).blocks).toEqual([]);
  });

  it("keeps tables aligned to the header and drops tables without one", () => {
    const text = (value: string) => [{ t: "t", text: value }];
    const doc = parseDocument({
      v: 1,
      blocks: [
        { t: "table", align: ["c", "x"], head: [text("项"), text("值")], rows: [[text("甲")], [text("乙"), text("2"), text("多余")], "bad"] },
        { t: "table", align: [], head: [], rows: [] },
      ],
    });
    expect(doc.blocks).toEqual([
      { t: "table", align: ["c", ""], head: [text("项"), text("值")], rows: [[text("甲"), []], [text("乙"), text("2")], [[], []]] },
    ]);
  });

  it("returns the same validated instance for the same payload", () => {
    const raw = { v: 1, blocks: [{ t: "p", inlines: [{ t: "t", text: "a" }] }] };
    const first = parseDocument(raw);
    expect(parseDocument(raw)).toBe(first);
    expect(parseDocument(first)).toBe(first);
    expect(parseDocument({ ...raw })).not.toBe(first);
  });
});
