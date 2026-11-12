import { useSyncExternalStore } from "react";
import type { CitationView } from "../../lib/api";

export type Draft = { text: string; citations: CitationView[] };
export const EMPTY_DRAFT: Draft = { text: "", citations: [] };

type Listener = () => void;

/**
 * 输入草稿 store：按上下文（每条会话 / 每期简报）保留文字与引用；异步回调只更新发起操作的上下文。
 * 草稿只在内存中，不写浏览器持久存储。
 */
export type DraftStore = {
  get: (context: string) => Draft;
  subscribe: (context: string, listener: Listener) => () => void;
  setText: (context: string, text: string) => void;
  setCitations: (context: string, update: (current: CitationView[]) => CitationView[]) => void;
  addCitations: (context: string, incoming: CitationView[]) => void;
  clear: (context: string) => void;
  move: (from: string, to: string) => void;
  consume: (context: string, snapshot: Draft) => void;
  /** 发送失败：把已清掉的文字与引用放回去，排在发送期间新输入的内容前面。 */
  restore: (context: string, snapshot: Draft) => void;
  threadContext: (id: number) => string;
  bindThread: (id: number, context: string) => void;
};

export function createDraftStore(): DraftStore {
  const drafts = new Map<string, Draft>();
  const threadContexts = new Map<number, string>();
  const listeners = new Map<string, Set<Listener>>();

  const write = (context: string, next: Draft) => {
    const current = drafts.get(context) ?? EMPTY_DRAFT;
    if (next.text === current.text && next.citations === current.citations) return;
    drafts.set(context, next);
    for (const listener of [...(listeners.get(context) ?? [])]) listener();
  };

  const store: DraftStore = {
    get: (context) => drafts.get(context) ?? EMPTY_DRAFT,
    subscribe(context, listener) {
      let set = listeners.get(context);
      if (!set) {
        set = new Set();
        listeners.set(context, set);
      }
      set.add(listener);
      return () => {
        set!.delete(listener);
        if (set!.size === 0) listeners.delete(context);
      };
    },
    setText(context, text) {
      write(context, { ...store.get(context), text });
    },
    setCitations(context, update) {
      const current = store.get(context);
      write(context, { ...current, citations: update(current.citations) });
    },
    addCitations(context, incoming) {
      if (incoming.length === 0) return;
      store.setCitations(context, (existing) => [...existing, ...incoming]);
    },
    threadContext: id => threadContexts.get(id) ?? (id > 0 ? `chat:${id}` : "chat"),
    bindThread(id, context) {
      if (id <= 0) return;
      store.move(store.threadContext(id), context);
      threadContexts.set(id, context);
    },
    move(from, to) {
      if (from === to) return;
      const source = store.get(from), target = store.get(to);
      write(to, {
        text: [target.text, source.text].filter(Boolean).join("\n"),
        citations: [...target.citations, ...source.citations.filter(c => c.kind !== "article" || !target.citations.some(t => t.kind === "article" && t.article_id === c.article_id))],
      });
      store.clear(from);
    },
    consume(context, snapshot) {
      const current = store.get(context);
      write(context, {
        text: current.text === snapshot.text ? "" : current.text,
        citations: current.citations.filter(c => !snapshot.citations.includes(c)),
      });
    },
    restore(context, snapshot) {
      const current = store.get(context);
      write(context, {
        text: [snapshot.text, current.text].filter(Boolean).join("\n"),
        citations: [...snapshot.citations, ...current.citations.filter(c => !snapshot.citations.includes(c))],
      });
    },
    clear(context) {
      write(context, EMPTY_DRAFT);
    },
  };
  return store;
}

export function useDraft(store: DraftStore, context: string): Draft {
  return useSyncExternalStore(
    (listener) => store.subscribe(context, listener),
    () => store.get(context),
    () => store.get(context),
  );
}
