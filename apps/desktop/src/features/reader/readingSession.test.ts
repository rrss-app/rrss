import { describe, expect, it } from "vitest";
import type { Block } from "../../lib/blockIr";
import { createReadingSession, readingBlocks } from "./readingSession";

function clocked(blocks = [{ id: 0, chars: 1_000 }], charsPerMs = 0.01) {
  let now = 10_000;
  const model = createReadingSession({ articleId: 7, openedAt: now, via: "focus", blocks, translated: false, charsPerMs: { chinese: charsPerMs, english: charsPerMs } });
  model.setActive(true, now);
  return { model, now: () => now, advance: (ms: number) => { now += ms; }, finish: () => model.finish(now, "back") };
}

describe("reading session interval accounting", () => {
  it("excludes loss of focus from time and block dwell", () => {
    const c = clocked();
    c.model.setVisibleBlocks([0], c.now());
    c.advance(10_000); c.model.setActive(false, c.now());
    c.advance(60_000); c.model.setActive(true, c.now()); c.model.activity(c.now());
    c.advance(10_000);
    expect(c.finish()).toMatchObject({ opened_at: 10_000, active_ms: 20_000, idle_ms: 60_000, read_chars: 0, skimmed_chars: 1_000 });
  });

  it("counts only the tolerance when unattended and settles before renewed activity", () => {
    const c = clocked([{ id: 0, chars: 100 }, { id: 1, chars: 10_000 }]);
    c.model.setVisibleBlocks([0], c.now());
    c.advance(120_000); c.model.activity(c.now());
    c.advance(5_000);
    expect(c.finish()).toMatchObject({ active_ms: 20_000, idle_ms: 105_000, read_chars: 100 });
  });

  it("classifies quickly passed blocks as skimmed without counting them as read", () => {
    const blocks = Array.from({ length: 20 }, (_, id) => ({ id, chars: 1_000 }));
    const c = clocked(blocks);
    for (const block of blocks) {
      c.model.activity(c.now()); c.model.setVisibleBlocks([block.id], c.now()); c.advance(100);
    }
    expect(c.finish()).toMatchObject({ active_ms: 2_000, read_chars: 0, skimmed_chars: 20_000, total_chars: 20_000 });
  });

  it("allows a long stationary paragraph for up to 90 seconds", () => {
    const c = clocked();
    c.model.setVisibleBlocks([0], c.now()); c.advance(85_000);
    expect(c.finish()).toMatchObject({ active_ms: 85_000, idle_ms: 0, read_chars: 1_000 });
  });

  it("uses the sum of visible blocks and settles the previous viewport first", () => {
    const c = clocked([{ id: 0, chars: 200 }, { id: 1, chars: 400 }, { id: 2, chars: 1_000 }]);
    c.model.setVisibleBlocks([0, 1, 1, 999], c.now());
    c.advance(50_000); c.model.setVisibleBlocks([2], c.now());
    c.advance(40_000);
    expect(c.finish()).toMatchObject({ active_ms: 90_000, read_chars: 1_600, skimmed_chars: 0 });
  });

  it("caps at three estimated article durations and makes excess time idle", () => {
    const c = clocked([{ id: 0, chars: 10 }]);
    c.model.setVisibleBlocks([0], c.now()); c.advance(10_000);
    expect(c.finish()).toMatchObject({ active_ms: 3_000, idle_ms: 7_000, read_chars: 10 });
  });

  it("caps long articles at 30 minutes even with ongoing activity", () => {
    const c = clocked([{ id: 0, chars: 100_000 }]);
    c.model.setVisibleBlocks([0], c.now());
    for (let i = 0; i < 35; i++) { c.advance(60_000); c.model.activity(c.now()); }
    expect(c.finish()).toMatchObject({ active_ms: 1_800_000, idle_ms: 300_000, read_chars: 0, skimmed_chars: 100_000 });
  });

  it("keeps the translation flag after returning to original text", () => {
    const c = clocked();
    c.advance(1_000); c.model.setTranslated(true, c.now());
    c.advance(1_000); c.model.setTranslated(false, c.now());
    expect(c.finish().translated).toBe(true);
  });

  it("tracks depth in document order and marks the end only explicitly", () => {
    const c = clocked([{ id: 42, chars: 100 }, { id: 2, chars: 100 }, { id: 8, chars: 100 }]);
    c.model.setVisibleBlocks([8], c.now()); c.model.setVisibleBlocks([42], c.now());
    expect(c.finish()).toMatchObject({ max_block: 8, reached_end: false });
    const next = clocked(); next.model.markEnd();
    expect(next.finish().reached_end).toBe(true);
  });

  it("handles an empty document and freezes the finished summary", () => {
    const c = clocked([]); c.model.setVisibleBlocks([0], c.now()); c.advance(1_000);
    const result = c.finish();
    expect(result).toMatchObject({ total_chars: 0, active_ms: 0, idle_ms: 1_000, read_chars: 0, skimmed_chars: 0, max_block: 0 });
    c.advance(5_000); c.model.activity(c.now()); c.model.markEnd();
    expect(c.model.finish(c.now(), "next")).toBe(result);
  });
});

describe("visible IR text and default reading speeds", () => {
  it("counts Unicode code points without whitespace or URLs, including nested inline text", () => {
    const blocks: Block[] = [
      { t: "p", inlines: [{ t: "t", text: "中 文 \n" }, { t: "s", children: [{ t: "a", href: "https://example.test", children: [{ t: "c", text: "𠀀😀a" }] }] }, { t: "br" }] },
      { t: "pre", lang: "typescript", text: "a \t b" }, { t: "img", src: "image.jpg", alt: "图 示" },
      { t: "embed", kind: "video", url: "https://example.test", title: "title" }, { t: "hr" },
    ];
    expect(readingBlocks(blocks)).toEqual([
      { id: 0, chars: 5, cjkChars: 3 }, { id: 1, chars: 2, cjkChars: 0 },
      { id: 2, chars: 2, cjkChars: 2 }, { id: 3, chars: 5, cjkChars: 0 }, { id: 4, chars: 0, cjkChars: 0 },
    ]);
  });

  it.each([
    ["中文", 400, 400], ["English", 1_380, 0], ["exactly 30% CJK", 1_380, 414],
  ])("selects the personal speed or specified default for %s", (_, chars, cjkChars) => {
    for (const charsPerMs of [undefined, { chinese: 400 / 30_000, english: 1_380 / 30_000 }]) {
      const c = createReadingSession({ articleId: 1, openedAt: 0, via: "digest", blocks: [{ id: 0, chars, cjkChars }], translated: true, charsPerMs });
      c.setActive(true, 0); c.setVisibleBlocks([0], 0);
      const activeMs = charsPerMs ? 30_000 : 60_000;
      expect(c.finish(120_000, "close")).toMatchObject({ active_ms: activeMs, idle_ms: 120_000 - activeMs, read_chars: chars, translated: true });
    }
  });
});
