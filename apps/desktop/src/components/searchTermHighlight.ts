import { useEffect } from "react";

const NAME = "rrss-search-term";
const DOCUMENT = "[data-reader-document]";

type HighlightRegistry = { set: (name: string, value: unknown) => void; delete: (name: string) => void };
type HighlightCtor = new (...ranges: Range[]) => unknown;

function registry(): { highlights: HighlightRegistry; Highlight: HighlightCtor } | null {
  const css = globalThis.CSS as unknown as { highlights?: HighlightRegistry } | undefined;
  const Highlight = (globalThis as { Highlight?: HighlightCtor }).Highlight;
  return css?.highlights && Highlight ? { highlights: css.highlights, Highlight } : null;
}

/** 与搜索一致：ASCII 不分大小写的字面子串；首尾配对引号按整句处理。 */
function normalize(query: string): string {
  const q = query.trim();
  const term = q.length >= 2 && q.startsWith('"') && q.endsWith('"') ? q.slice(1, -1).trim() : q;
  return term.replace(/[A-Z]/g, (c) => c.toLowerCase());
}

function rangesFor(root: Element, term: string): Range[] {
  const ranges: Range[] = [];
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const text = (node.nodeValue ?? "").replace(/[A-Z]/g, (c) => c.toLowerCase());
    for (let at = text.indexOf(term); at >= 0; at = text.indexOf(term, at + term.length)) {
      const range = document.createRange();
      range.setStart(node, at);
      range.setEnd(node, at + term.length);
      ranges.push(range);
    }
  }
  return ranges;
}

/**
 * 在正文里标出搜索词（CSS Highlight API，不改 DOM）。正文重绘时随之更新；
 * 不支持该 API 的环境只保留命中段高亮。
 */
export function useSearchTermHighlight(body: HTMLElement | null, query: string | undefined) {
  useEffect(() => {
    const api = registry();
    const term = query ? normalize(query) : "";
    if (!api || !body || term.length < 2) return;
    let frame: number | undefined;
    const paint = () => {
      frame = undefined;
      const root = body.querySelector(DOCUMENT);
      api.highlights.set(NAME, new api.Highlight(...(root ? rangesFor(root, term) : [])));
    };
    paint();
    const observer = new MutationObserver(() => { if (frame === undefined) frame = requestAnimationFrame(paint); });
    observer.observe(body, { childList: true, subtree: true, characterData: true });
    return () => {
      observer.disconnect();
      if (frame !== undefined) cancelAnimationFrame(frame);
      api.highlights.delete(NAME);
    };
  }, [body, query]);
}
