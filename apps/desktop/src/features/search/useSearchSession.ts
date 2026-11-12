import { useCallback, useEffect, useRef, useState } from "react";
import type { SearchHitView, SearchQuery } from "../../lib/api";
import { formatNotice } from "../../lib/notice";
import { DEFAULT_FILTERS, publishedSince, SEARCH_LIMIT, type SearchFilters } from "./searchFilters";

export type SearchStatus = "idle" | "short" | "loading" | "ready" | "error";

export type SearchSession = {
  query: string;
  setQuery: (query: string) => void;
  filters: SearchFilters;
  setFilters: (filters: SearchFilters) => void;
  status: SearchStatus;
  hits: SearchHitView[];
  /** 当前结果对应的查询词（已去首尾空白）；高亮与「没有找到」用它，不跟着输入框抢先变化。 */
  searched: string;
  error: string;
  retry: () => void;
};

export type SearchSessionOptions = {
  /** 会话关闭时查询、筛选、结果与在途请求一起作废。 */
  active: boolean;
  search: (query: SearchQuery) => Promise<SearchHitView[]>;
  debounceMs?: number;
  minLength?: number;
};

type Result = { status: SearchStatus; hits: SearchHitView[]; searched: string; error: string };
const IDLE: Result = { status: "idle", hits: [], searched: "", error: "" };
const SHORT: Result = { status: "short", hits: [], searched: "", error: "" };

/**
 * 搜索会话：防抖只由查询词与重试次数驱动，搜索函数变化或宿主其他状态更新不重置计时；
 * 改筛选与重试是显式动作，立即查询。每次请求带代际，旧词结果与关闭后迟到的结果都被丢弃；
 * 新结果到达前、以及出错时保留上一批结果。
 */
export function useSearchSession({ active, search, debounceMs = 180, minLength = 2 }: SearchSessionOptions): SearchSession {
  const [query, setQuery] = useState("");
  const [filters, setFilters] = useState<SearchFilters>(DEFAULT_FILTERS);
  const [attempt, setAttempt] = useState(0);
  const [result, setResult] = useState<Result>(IDLE);
  const searchRef = useRef(search);
  searchRef.current = search;
  const generation = useRef(0);
  const seenAttempt = useRef(0);
  const seenFilters = useRef(filters);

  useEffect(() => {
    if (!active) {
      generation.current += 1;
      setQuery("");
      setFilters(DEFAULT_FILTERS);
      seenFilters.current = DEFAULT_FILTERS;
      setAttempt(0);
      seenAttempt.current = 0;
      setResult(IDLE);
      return;
    }
    const q = query.trim();
    if (q.length < minLength) {
      generation.current += 1;
      seenFilters.current = filters;
      setResult(q.length === 0 ? IDLE : SHORT);
      return;
    }
    const gen = ++generation.current;
    const run = () => {
      setResult((cur) => ({ ...cur, status: "loading", error: "" }));
      const request: SearchQuery = {
        query: q, scope: filters.scope, feed_ids: filters.feedIds,
        published_since: publishedSince(filters.time), limit: SEARCH_LIMIT,
      };
      Promise.resolve().then(() => searchRef.current(request)).then(
        (hits) => { if (generation.current === gen) setResult({ status: "ready", hits, searched: q, error: "" }); },
        (err) => { if (generation.current === gen) setResult((cur) => ({ ...cur, status: "error", error: formatNotice("", err) })); },
      );
    };
    // 重试和改筛选是用户显式动作，不再等防抖。
    if (attempt !== seenAttempt.current || filters !== seenFilters.current) {
      seenAttempt.current = attempt;
      seenFilters.current = filters;
      run();
      return;
    }
    const timer = window.setTimeout(run, debounceMs);
    return () => window.clearTimeout(timer);
  }, [active, query, filters, attempt, debounceMs, minLength]);

  const retry = useCallback(() => setAttempt((n) => n + 1), []);

  return { query, setQuery, filters, setFilters, status: result.status, hits: result.hits, searched: result.searched, error: result.error, retry };
}
