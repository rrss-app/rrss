import { describe, expect, it } from "vitest";
import { mergeDocumentPatches, parseDocument } from "./blockIr";

describe("incremental reader IR", () => {
  it("validates changed blocks while preserving unaffected validated subtrees", () => {
    const source = parseDocument({ v: 1, blocks: [
      { t: "p", inlines: [{ t: "t", text: "first" }] },
      { t: "p", inlines: [{ t: "t", text: "second" }] },
    ] });
    const next = mergeDocumentPatches(source, [{ index: 1, ir: { t: "p", inlines: [{ t: "t", text: "第二段" }] } }]);
    expect(next.blocks[0]).toBe(source.blocks[0]);
    expect(next.blocks[1]).not.toBe(source.blocks[1]);
    expect(parseDocument(next)).toBe(next);
    expect(source.blocks[1]).toEqual({ t: "p", inlines: [{ t: "t", text: "second" }] });
  });

  it("rejects an entire malformed patch without shifting source block IDs", () => {
    const source = parseDocument({ v: 1, blocks: [{ t: "p", inlines: [{ t: "t", text: "source" }] }] });
    for (const patches of [
      [{ index: 1, ir: { t: "hr" } }],
      [{ index: 0, ir: { t: "script" } }],
      [{ index: 0, ir: { t: "p", inlines: [{ t: "t", text: 12 }] } }],
      [{ index: 0, ir: { t: "hr" } }, { index: 0, ir: { t: "hr" } }],
    ]) expect(() => mergeDocumentPatches(source, patches)).toThrow();
    expect(source.blocks).toHaveLength(1);
  });
});
