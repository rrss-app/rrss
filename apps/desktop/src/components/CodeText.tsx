import { useEffect, useState, type ReactNode } from "react";
import { cachedHighlight, highlightCode, type CodeToken } from "../lib/codeHighlight";

function render(tokens: CodeToken[], prefix = ""): ReactNode[] {
  return tokens.map((token, index) => typeof token === "string"
    ? token
    : <span key={`${prefix}${index}`} className={`code-${token.kind}`}>{render(token.children, `${prefix}${index}.`)}</span>);
}

/**
 * 代码块正文。先按原文显示，语法包加载完再换成带语义色的 span；文字逐字不变，
 * 所以划选偏移、复制与引用都不受影响。没有标注又猜不准、或代码太长时保持原文。
 */
export function CodeText({ text, lang }: { text: string; lang?: string | null }) {
  const language = lang || null;
  const [result, setResult] = useState(() => ({ text, language, tokens: cachedHighlight(text, language) }));
  const current = result.text === text && result.language === language ? result.tokens : cachedHighlight(text, language);

  useEffect(() => {
    if (cachedHighlight(text, language) !== undefined) return;
    let live = true;
    void highlightCode(text, language).then((tokens) => { if (live) setResult({ text, language, tokens }); });
    return () => { live = false; };
  }, [text, language]);

  return <code>{current ? render(current) : text}</code>;
}
