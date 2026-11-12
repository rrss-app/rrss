import { useSyncExternalStore } from "react";
import type { ChatMessageView, ChatStreamEvent, CitationView } from "../../lib/api";
import type { Trace } from "./turnTrace";

export type Conversation = {
  /** 已加载的连续历史窗口（按 id 升序）；第一条之前是否还有更早的消息见 `hasMore`。 */
  messages: ChatMessageView[];
  stream: ChatStreamEvent | null;
  hasMore: boolean;
};
export const EMPTY_CONVERSATION: Conversation = { messages: [], stream: null, hasMore: false };

/**
 * 刚发出、还没出现在已加载历史里的自己的消息。发送中半透明；宿主接受后变实（`threadId` 记下归属线程），
 * 历史里出现 id 大于 `after` 的提问时撤下；发送失败撤回。只在内存里，不是业务数据。
 */
export type Outgoing = {
  token: number;
  text: string;
  citations: CitationView[];
  accepted: boolean;
  threadId: number;
  /** 发送时这条线程已知的最后一条消息 id。 */
  after: number;
};

/**
 * 一轮对话没能正常结束（模型服务、对话引擎或本地出错）：显示在回复位置，直到这里再发出一条或下一轮开始。
 * 只在内存里，不是业务数据。`fix` 指向能修好它的设置页；没有回复时 `trace` 留下这一轮做过的步骤。
 */
export type ChatFailure = {
  title: string;
  detail: string;
  fix?: "assistant" | "model";
  trace?: Trace;
};

/**
 * 一处会话位置的键：已有线程按 id，还没有线程的新对话按输入框上下文区分（对话「新对话」与各期简报追问互不相干）。
 * 模型选择与待发消息共用这个键。
 */
export const threadSlot = (threadId: number, context: string) => (threadId > 0 ? `t${threadId}` : `new:${context}`);

export type ConversationStatus = { busyThreadId: number | null; sending: boolean };
const IDLE_STATUS: ConversationStatus = { busyThreadId: null, sending: false };

type Listener = () => void;

/**
 * 会话 store：按线程持有正文快照并按线程通知订阅者。不是业务数据库——只装当前会话期间收到的
 * 消息与流式快照；每个测试 / 根壳实例各建一份。
 */
export type ConversationStore = {
  get: (threadId: number) => Conversation;
  subscribe: (threadId: number, listener: Listener) => () => void;
  /** 更新某线程；返回同一引用时不通知。 */
  update: (threadId: number, updater: (current: Conversation) => Conversation) => void;
  getStatus: () => ConversationStatus;
  subscribeStatus: (listener: Listener) => () => void;
  setStatus: (patch: Partial<ConversationStatus>) => void;
  /**
   * 回收非活动线程已落库的历史。仍有订阅者、正在回复或持有未落库流式正文的线程不回收；
   * 返回是否真正释放。
   */
  release: (threadId: number) => boolean;
  getOutgoing: (slot: string) => Outgoing | null;
  subscribeOutgoing: (slot: string, listener: Listener) => () => void;
  /** 返回同一引用时不通知。 */
  setOutgoing: (slot: string, updater: (current: Outgoing | null) => Outgoing | null) => void;
  getFailure: (slot: string) => ChatFailure | null;
  subscribeFailure: (slot: string, listener: Listener) => () => void;
  /** 返回同一引用时不通知。 */
  setFailure: (slot: string, updater: (current: ChatFailure | null) => ChatFailure | null) => void;
};

/** 按会话位置存一份值（待发消息、失败），只通知这一处的订阅者。 */
function createSlotValues<T>() {
  const values = new Map<string, T>();
  const listeners = new Map<string, Set<Listener>>();
  return {
    get: (slot: string) => values.get(slot) ?? null,
    subscribe: (slot: string, listener: Listener) => listen(listeners, slot, listener),
    set(slot: string, updater: (current: T | null) => T | null) {
      const current = values.get(slot) ?? null;
      const next = updater(current);
      if (next === current) return;
      if (next) values.set(slot, next);
      else values.delete(slot);
      for (const listener of [...(listeners.get(slot) ?? [])]) listener();
    },
  };
}

function listen<K>(map: Map<K, Set<Listener>>, key: K, listener: Listener) {
  let set = map.get(key);
  if (!set) {
    set = new Set();
    map.set(key, set);
  }
  set.add(listener);
  return () => {
    set!.delete(listener);
    if (set!.size === 0) map.delete(key);
  };
}

export function createConversationStore(): ConversationStore {
  const conversations = new Map<number, Conversation>();
  const listeners = new Map<number, Set<Listener>>();
  const statusListeners = new Set<Listener>();
  const outgoing = createSlotValues<Outgoing>();
  const failures = createSlotValues<ChatFailure>();
  let status = IDLE_STATUS;

  const notify = (threadId: number) => {
    for (const listener of [...(listeners.get(threadId) ?? [])]) listener();
  };

  return {
    get: (threadId) => conversations.get(threadId) ?? EMPTY_CONVERSATION,
    subscribe: (threadId, listener) => listen(listeners, threadId, listener),
    update(threadId, updater) {
      const current = conversations.get(threadId) ?? EMPTY_CONVERSATION;
      const next = updater(current);
      if (next === current) return;
      conversations.set(threadId, next);
      notify(threadId);
    },
    getStatus: () => status,
    subscribeStatus(listener) {
      statusListeners.add(listener);
      return () => { statusListeners.delete(listener); };
    },
    setStatus(patch) {
      const next = { ...status, ...patch };
      if (next.busyThreadId === status.busyThreadId && next.sending === status.sending) return;
      status = next;
      for (const listener of [...statusListeners]) listener();
    },
    release(threadId) {
      const current = conversations.get(threadId);
      if (!current) return false;
      if (listeners.has(threadId) || status.busyThreadId === threadId || current.stream !== null) return false;
      conversations.delete(threadId);
      return true;
    },
    getOutgoing: outgoing.get,
    subscribeOutgoing: outgoing.subscribe,
    setOutgoing: outgoing.set,
    getFailure: failures.get,
    subscribeFailure: failures.subscribe,
    setFailure: failures.set,
  };
}

/** 只订阅一个线程；其他线程的流式更新不会让调用方重渲染。 */
export function useConversation(store: ConversationStore, threadId: number): Conversation {
  return useSyncExternalStore(
    (listener) => store.subscribe(threadId, listener),
    () => store.get(threadId),
    () => store.get(threadId),
  );
}

export function useOutgoing(store: ConversationStore, slot: string): Outgoing | null {
  return useSyncExternalStore(
    (listener) => store.subscribeOutgoing(slot, listener),
    () => store.getOutgoing(slot),
    () => store.getOutgoing(slot),
  );
}

export function useFailure(store: ConversationStore, slot: string): ChatFailure | null {
  return useSyncExternalStore(
    (listener) => store.subscribeFailure(slot, listener),
    () => store.getFailure(slot),
    () => store.getFailure(slot),
  );
}

export function useConversationStatus(store: ConversationStore): ConversationStatus {
  return useSyncExternalStore(store.subscribeStatus, store.getStatus, store.getStatus);
}
