import type { ReactNode } from "react";

import { normalizeSearchTerm } from "../../components/searchTermHighlight";

const foldAscii = (text: string) => text.replace(/[A-Z]/g, (c) => c.toLowerCase());

/** 把文本里每处命中包进 `<mark>`；不拼 HTML。 */
export function Highlight({ text, query }: { text: string; query: string }) {
  const term = normalizeSearchTerm(query);
  if (!term) return <>{text}</>;
  const folded = foldAscii(text);
  const parts: ReactNode[] = [];
  let last = 0;
  for (let at = folded.indexOf(term); at >= 0; at = folded.indexOf(term, at + term.length)) {
    if (at > last) parts.push(text.slice(last, at));
    parts.push(<mark key={at}>{text.slice(at, at + term.length)}</mark>);
    last = at + term.length;
  }
  if (last < text.length) parts.push(text.slice(last));
  return <>{parts}</>;
}

/**
 * 命中词离段首太远时，从它前面不远处开始显示（前加省略号），让三行之内看得到命中处。
 * 只裁开头一刀，不拼接多个片段。
 */
export function focusOnMatch(text: string, query: string, lead = 40): string {
  const term = normalizeSearchTerm(query);
  const at = term ? foldAscii(text).indexOf(term) : -1;
  if (at < 0) return text;
  const before = Array.from(text.slice(0, at));
  if (before.length <= lead + 20) return text;
  let head = before.slice(before.length - lead).join("");
  // 西文从词边界开始，不切半个单词。
  const space = head.indexOf(" ");
  if (space >= 0 && space < 16) head = head.slice(space + 1);
  return `…${head}${text.slice(at)}`;
}
