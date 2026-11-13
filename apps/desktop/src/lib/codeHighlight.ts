/** 语法高亮的结果：纯文本或带语义色的一段；拼起来与原文逐字相同，复制、划选与引用仍用原文。 */
export type CodeToken = string | { kind: string; children: CodeToken[] };

/** 超大的代码块不高亮：既慢，也没多少人逐行读。 */
const MAX_CHARS = 30_000;
const MAX_LINES = 1_500;
const CACHE_SIZE = 64;

type Engine = typeof import("./codeHighlightEngine");
let engine: Promise<Engine> | null = null;
const cache = new Map<string, CodeToken[] | null>();

function keyOf(text: string, lang: string | null) {
  return `${lang ?? ""}\u0000${text}`;
}

export function highlightable(text: string): boolean {
  if (!text.trim() || text.length > MAX_CHARS) return false;
  let lines = 1;
  for (let i = text.indexOf("\n"); i >= 0; i = text.indexOf("\n", i + 1)) if (++lines > MAX_LINES) return false;
  return true;
}

/** 已经算过的结果（含「不上色」的 null）；没算过返回 undefined。 */
export function cachedHighlight(text: string, lang: string | null): CodeToken[] | null | undefined {
  return cache.get(keyOf(text, lang));
}

/** 按需加载语法包后高亮；加载或高亮失败都当作不上色。 */
export async function highlightCode(text: string, lang: string | null): Promise<CodeToken[] | null> {
  const key = keyOf(text, lang);
  if (cache.has(key)) return cache.get(key) ?? null;
  let tokens: CodeToken[] | null = null;
  if (highlightable(text)) {
    try {
      engine ??= import("./codeHighlightEngine");
      tokens = (await engine).highlight(text, lang?.trim().toLowerCase() || null);
    } catch {
      tokens = null;
    }
  }
  if (cache.size >= CACHE_SIZE) cache.delete(cache.keys().next().value as string);
  cache.set(key, tokens);
  return tokens;
}
