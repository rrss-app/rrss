import { useEffect, useReducer, useRef } from "react";
import type { SearchHitBlocksView } from "../../lib/api";
import { formatNotice } from "../../lib/notice";

export type HitBlocksStatus = "idle" | "loading" | "ready" | "error";
export type HitBlocks<T = SearchHitBlocksView> = { status: HitBlocksStatus; data: T | null; error: string };

export type HitBlocksOptions<T = SearchHitBlocksView> = {
  /** 浮层关闭时缓存与在途请求一起作废。 */
  active: boolean;
  /** 要看命中段的文章；null 表示没有选中行或不需要（窄档、只标题命中）。 */
  articleId: number | null;
  query: string;
  load: (articleId: number, query: string) => Promise<T>;
  /** 选中行停稳后才取，悬停扫过的行不发请求。 */
  delayMs?: number;
};

const IDLE = { status: "idle", data: null, error: "" } as const;
const LOADING = { status: "loading", data: null, error: "" } as const;
const keyOf = (articleId: number, query: string) => `${articleId}\u0000${query}`;

/**
 * 预览用的正文命中段：选中行停稳 `delayMs` 后取一次，按「文章 + 查询词」缓存在本次浮层会话里，
 * 来回悬停同一篇不重复请求；迟到的响应只进缓存，不覆盖当前选中行的状态。预览的阅读时长也按同样方式取。
 */
export function useHitBlocks<T = SearchHitBlocksView>({ active, articleId, query, load, delayMs = 140 }: HitBlocksOptions<T>): HitBlocks<T> & { peek: (articleId: number, query: string) => T | undefined } {
  const cache = useRef(new Map<string, T>());
  const failed = useRef(new Map<string, string>());
  const pending = useRef(new Set<string>());
  const session = useRef(0);
  const loadRef = useRef(load);
  loadRef.current = load;
  const [, bump] = useReducer((n: number) => n + 1, 0);
  const key = articleId == null || !query ? null : keyOf(articleId, query);
  /** 打开文章时直接带上已取到的命中段，阅读器不必再取。 */
  const peek = (id: number, q: string) => cache.current.get(keyOf(id, q));

  useEffect(() => {
    if (active) return;
    session.current += 1;
    cache.current.clear();
    failed.current.clear();
    pending.current.clear();
  }, [active]);

  useEffect(() => {
    if (!active || key == null || articleId == null) return;
    if (cache.current.has(key) || pending.current.has(key)) return;
    failed.current.delete(key);
    const own = session.current;
    const timer = window.setTimeout(() => {
      pending.current.add(key);
      Promise.resolve().then(() => loadRef.current(articleId, query)).then(
        (data) => { if (session.current === own) cache.current.set(key, data); },
        (err) => { if (session.current === own) failed.current.set(key, formatNotice("", err)); },
      ).finally(() => {
        if (session.current !== own) return;
        pending.current.delete(key);
        bump();
      });
    }, delayMs);
    return () => window.clearTimeout(timer);
  }, [active, key, articleId, query, delayMs]);

  if (!active || key == null) return { ...IDLE, peek };
  const data = cache.current.get(key);
  if (data !== undefined) return { status: "ready", data, error: "", peek };
  const error = failed.current.get(key);
  return error != null ? { status: "error", data: null, error, peek } : { ...LOADING, peek };
}
