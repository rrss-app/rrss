import { useSyncExternalStore } from "react";
import { api, type ClusterSummaryView } from "../../lib/api";
import { formatNotice } from "../../lib/notice";

/** 一组的综述：读回的视图、是否在读 / 在生成、最近一次失败。 */
export type ClusterSummaryEntry = {
  view: ClusterSummaryView | null;
  loading: boolean;
  generating: boolean;
  error: string;
};

const EMPTY: ClusterSummaryEntry = { view: null, loading: false, generating: false, error: "" };

/**
 * 相近报道综述（AI 规格 · 相近报道综述）。按簇保存读回的状态；只在用户点「汇总 / 生成综述 / 更新综述」时生成。
 * 换范围时整体作废，迟到的结果不写回。
 */
export function createClusterSummaries(scope: () => string | undefined, onGenerated?: (clusterId: number) => void) {
  let entries: ReadonlyMap<number, ClusterSummaryEntry> = new Map();
  let epoch = 0;
  /** 每组最近一次请求的序号：读回晚于生成结果时不覆盖。 */
  let counter = 0;
  const latest = new Map<number, number>();
  const listeners = new Set<() => void>();
  const set = (id: number, patch: Partial<ClusterSummaryEntry>) => {
    const next = new Map(entries);
    next.set(id, { ...(entries.get(id) ?? EMPTY), ...patch });
    entries = next;
    listeners.forEach((listener) => listener());
  };
  const call = <T>(read: (scope?: string) => Promise<T>) => {
    const value = scope();
    return value ? read(value) : read();
  };

  async function load(id: number): Promise<ClusterSummaryView | null> {
    const mine = epoch;
    const ticket = ++counter;
    latest.set(id, ticket);
    set(id, { loading: true });
    try {
      const view = await call((s) => api.clusterSummary(id, s));
      // 换了范围，或之后又发过读回 / 生成：这次的结果作废。
      if (mine !== epoch || latest.get(id) !== ticket) return null;
      set(id, { view, loading: false });
      return view;
    } catch (err) {
      if (mine === epoch && latest.get(id) === ticket) set(id, { loading: false, error: formatNotice("读取综述失败：", err) });
      return null;
    }
  }

  async function generate(id: number) {
    if (entries.get(id)?.generating) return;
    const mine = epoch;
    latest.set(id, ++counter);
    set(id, { generating: true, loading: false, error: "" });
    try {
      const view = await call((s) => api.generateClusterSummary(id, s));
      if (mine !== epoch) return;
      // 生成的结果总比之前发出的读回新。
      latest.set(id, ++counter);
      set(id, { view, generating: false });
      onGenerated?.(id);
    } catch (err) {
      if (mine === epoch) set(id, { generating: false, error: formatNotice("生成综述失败：", err) });
    }
  }

  return {
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
    getAll: () => entries,
    get: (id: number): ClusterSummaryEntry => entries.get(id) ?? EMPTY,
    load,
    generate,
    /** 组头「汇总」：还没生成且模型可用就直接生成；已有综述只读回最新状态。 */
    async summarize(id: number) {
      const view = entries.get(id)?.view ?? await load(id);
      if (view?.state === "none" && view.model_ready) await generate(id);
    },
    /** 换范围或重读焦点：旧结果作废。 */
    reset() {
      epoch += 1;
      entries = new Map();
      listeners.forEach((listener) => listener());
    },
  };
}

export type ClusterSummaries = ReturnType<typeof createClusterSummaries>;

export function useClusterSummaries(store: ClusterSummaries): ReadonlyMap<number, ClusterSummaryEntry> {
  return useSyncExternalStore(store.subscribe, store.getAll, store.getAll);
}

/** 综述正文里只认 `**粗体**`，其余按纯文本；不拼 HTML。 */
export function boldSegments(text: string): { text: string; bold: boolean }[] {
  const out: { text: string; bold: boolean }[] = [];
  const pattern = /\*\*(.+?)\*\*/g;
  let last = 0;
  for (const match of text.matchAll(pattern)) {
    const at = match.index ?? 0;
    if (at > last) out.push({ text: text.slice(last, at), bold: false });
    out.push({ text: match[1], bold: true });
    last = at + match[0].length;
  }
  if (last < text.length) out.push({ text: text.slice(last), bold: false });
  return out;
}
