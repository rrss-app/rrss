import type { ReadSessionEndedBy, ReadSessionVia } from "../../bindings";
import { tableText, type Block, type Inline } from "../../lib/blockIr";
import { inlineText as plainInlineText } from "../../lib/readingTime";
import type { ReadSessionRecord } from "../../lib/signals";

const ENGLISH_CHARS_PER_MS = 230 * 6 / 60_000;
const CHINESE_CHARS_PER_MS = 400 / 60_000;
const CJK = /[\p{Script=Han}\p{Script=Hiragana}\p{Script=Katakana}\p{Script=Hangul}]/u;

type ReadingBlock = { id: number; chars: number; cjkChars?: number };
const inlineText = (nodes: Inline[]): string => plainInlineText(nodes, "\n");

/** 正文文字，不计 URL、控件、列表标记；码点计数排除空白，保留原块顺序。 */
export function readingBlocks(blocks: readonly Block[]): ReadingBlock[] {
  return blocks.map((block, id) => {
    const text = "inlines" in block ? inlineText(block.inlines) : block.t === "pre" ? block.text
      : block.t === "img" ? block.alt : block.t === "embed" ? block.title : block.t === "table" ? tableText(block, inlineText) : "";
    const chars = Array.from(text).filter(char => !/\s/u.test(char));
    return { id, chars: chars.length, cjkChars: chars.filter(char => CJK.test(char)).length };
  });
}

/** 无 IO / 时钟 / DOM；每次事件先结算旧状态对应的区间，再改变状态。 */
export function createReadingSession({ articleId, openedAt, via, blocks, translated, charsPerMs }: {
  articleId: number; openedAt: number; via: ReadSessionVia; blocks: ReadingBlock[];
  translated: boolean; charsPerMs?: { chinese?: number; english?: number };
}) {
  const totalChars = blocks.reduce((sum, block) => sum + block.chars, 0);
  const cjkChars = blocks.reduce((sum, block) => sum + (block.cjkChars ?? 0), 0);
  // 与 rrss-core/src/signals/mod.rs 一致：非空白文字中 CJK 字符占比 > 0.3。
  const chinese = totalChars > 0 && cjkChars / totalChars > 0.3;
  const personalSpeed = chinese ? charsPerMs?.chinese : charsPerMs?.english;
  const speed = personalSpeed !== undefined && Number.isFinite(personalSpeed) && personalSpeed > 0
    ? personalSpeed : chinese ? CHINESE_CHARS_PER_MS : ENGLISH_CHARS_PER_MS;
  const cap = Math.min(totalChars / speed * 3, 30 * 60_000);
  const byId = new Map(blocks.map((block, index) => [block.id, { ...block, index, dwell: 0, seen: false }]));
  let settledAt = openedAt, lastActivity = openedAt, active = false;
  let activeMs = 0, idleMs = 0, deepest = -1, reachedEnd = false;
  let visible = new Set<number>(), translatedOnce = translated;
  let result: ReadSessionRecord | undefined;

  const settle = (now: number) => {
    if (result) return;
    const until = Math.max(settledAt, now);
    const visibleChars = [...visible].reduce((sum, id) => sum + byId.get(id)!.chars, 0);
    const tolerance = Math.min(90_000, Math.max(15_000, visibleChars / speed));
    const effective = active ? Math.min(Math.max(0, cap - activeMs), Math.max(0, Math.min(until, lastActivity + tolerance) - settledAt)) : 0;
    activeMs += effective;
    idleMs += until - settledAt - effective;
    for (const id of visible) byId.get(id)!.dwell += effective;
    settledAt = until;
  };

  return {
    activity(now: number) { settle(now); if (!result) lastActivity = Math.max(settledAt, now); },
    setActive(on: boolean, now: number) { settle(now); if (!result) active = on; },
    setVisibleBlocks(ids: readonly number[], now: number) {
      settle(now);
      if (result) return;
      visible = new Set(ids.filter(id => byId.has(id)));
      for (const id of visible) {
        const block = byId.get(id)!;
        block.seen = true;
        deepest = Math.max(deepest, block.index);
      }
    },
    setTranslated(on: boolean, now: number) { settle(now); if (!result) translatedOnce ||= on; },
    markEnd() { if (!result) reachedEnd = true; },
    finish(now: number, endedBy: ReadSessionEndedBy): ReadSessionRecord {
      settle(now);
      if (!result) {
        let readChars = 0, skimmedChars = 0;
        for (const block of byId.values()) {
          if (!block.seen) continue;
          if (block.dwell >= 0.4 * block.chars / speed) readChars += block.chars;
          else skimmedChars += block.chars;
        }
        result = {
          article_id: articleId, opened_at: openedAt, via, ended_by: endedBy,
          active_ms: Math.round(activeMs), idle_ms: Math.round(idleMs),
          read_chars: readChars, skimmed_chars: skimmedChars, total_chars: totalChars,
          max_block: blocks[deepest]?.id ?? 0, reached_end: reachedEnd, translated: translatedOnce,
        };
      }
      return result;
    },
  };
}
