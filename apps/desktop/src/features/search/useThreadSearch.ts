import { useEffect, useRef, useState } from "react";
import type { ThreadSearchHitView } from "../../lib/api";
import { formatNotice } from "../../lib/notice";

export type ThreadSearchStatus = "idle" | "loading" | "ready" | "error";
export type ThreadSearch = { status: ThreadSearchStatus; hits: ThreadSearchHitView[]; searched: string; error: string };

const IDLE: ThreadSearch = { status: "idle", hits: [], searched: "", error: "" };
/** 每条会话一条结果；浮层里只列前这么多条。 */
export const THREAD_SEARCH_LIMIT = 20;

/**
 * 搜会话（阶段 11 · N4 搜索对话）：和文章搜索同一个输入框、同样的防抖与最短长度，单独发请求；
 * 「对话 N」的数字在文章标签上也看得到。旧词与关闭后迟到的结果按代际丢弃，出错时保留上一批。
 */
export function useThreadSearch({ active, query, search, debounceMs = 180, minLength = 2 }: {
  active: boolean;
  query: string;
  search?: (query: string, limit: number) => Promise<ThreadSearchHitView[]>;
  debounceMs?: number;
  minLength?: number;
}): ThreadSearch {
  const [result, setResult] = useState<ThreadSearch>(IDLE);
  const searchRef = useRef(search);
  searchRef.current = search;
  const generation = useRef(0);
  const enabled = !!search;

  useEffect(() => {
    const q = query.trim();
    if (!active || !enabled || q.length < minLength) {
      generation.current += 1;
      setResult(IDLE);
      return;
    }
    const gen = ++generation.current;
    const timer = window.setTimeout(() => {
      setResult((cur) => ({ ...cur, status: "loading", error: "" }));
      Promise.resolve().then(() => searchRef.current!(q, THREAD_SEARCH_LIMIT)).then(
        (hits) => { if (generation.current === gen) setResult({ status: "ready", hits, searched: q, error: "" }); },
        (err) => { if (generation.current === gen) setResult((cur) => ({ ...cur, status: "error", error: formatNotice("", err) })); },
      );
    }, debounceMs);
    return () => window.clearTimeout(timer);
  }, [active, enabled, query, debounceMs, minLength]);

  return result;
}
