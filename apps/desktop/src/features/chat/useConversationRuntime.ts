import { formatError } from "../../lib/errors";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  api, onChatConfirm, onChatContext, onChatDelta, onChatDone, onChatError, onChatStart, onChatStatus,
  type ChatMessageView, type CitationView, type ContextUsageView, type MessagesPage, type ModelChoice, type ThreadView,
} from "../../lib/api";
import { trackEvent } from "../../lib/signals";
import { thinkingLabel } from "../../settings/modelChoices";
import { formatNotice } from "../../lib/notice";
import { i18n } from "../../i18n";
import { createActivityStore } from "./ChatActivity";
import { describeChatError, failureText } from "./ChatFailure";
import { createTurnStore } from "./turnTrace";
import {
  createConversationStore,
  threadSlot,
  useConversationStatus,
  type Conversation,
  type ConversationStore,
  type Outgoing,
} from "./conversationStore";

/**
 * 用最新一页覆盖当前窗口。当前窗口与新页有重叠（含 id 不早于新页第一条的消息）时，保留新页之前
 * 已加载的更早消息；没有重叠就整段换成新页，中间可能缺失的消息不能靠拼接假装连续。同一 id 不出现两次。
 */
export function mergeLatestPage(current: Conversation, page: MessagesPage): Pick<Conversation, "messages" | "hasMore"> {
  const first = page.messages[0];
  if (!first) return { messages: [], hasMore: false };
  const overlaps = current.messages.some((m) => m.id >= first.id);
  if (!overlaps) return { messages: page.messages, hasMore: page.has_more };
  const older = current.messages.filter((m) => m.id < first.id);
  return { messages: [...older, ...page.messages], hasMore: older.length ? current.hasMore : page.has_more };
}

const modelKey = threadSlot;

/** 已加载的历史里出现了这次发出的提问（id 大于发送时已知的最后一条）：撤下待发消息，由历史接替。 */
function settleOutgoing(store: ConversationStore, threadId: number, messages: ChatMessageView[]) {
  store.setOutgoing(threadSlot(threadId, ""), (current) => {
    // 不知道发送前历史的（after 为 0）还没被接受时无法判断，留到接受后的刷新。
    if (!current || (!current.accepted && current.after === 0)) return current;
    return messages.some((m) => m.role === "user" && m.id > current.after) ? null : current;
  });
}

/** 一条对话在输入区的选择：模型与思考强度，null 都表示跟随默认。 */
type ThreadOptions = { model: ModelChoice | null; thinking: string | null };
const optionsFor = (old: Record<string, ThreadOptions>, key: string): ThreadOptions => old[key] ?? { model: null, thinking: null };
/** 线程列表或简报追问记录带回的已保存选择与用量。 */
export type SavedThread = Pick<ThreadView, "chat_model"> & Partial<Pick<ThreadView, "chat_thinking" | "context">>;

export type ConversationRuntimeOptions = {
  /**
   * 没有会话位置可挂的错误（事件连接、历史加载、不知道线程的对话错误）。
   * 一轮对话的错误显示在该线程的回复位置（`store.getFailure`），不走这里。`fix` 指向能修好它的设置页。
   */
  onError: (message: string, fix?: "assistant" | "model") => void;
  onDone: () => void;
  /** 测试注入独立实例；缺省时每个宿主实例自建。 */
  store?: ConversationStore;
};

/**
 * 会话运行层：订阅宿主流式事件，把正文按线程写入 store；回复按线程归属更新，
 * 翻期、切页和迟到请求都不能改写另一个会话。停止生成只在宿主 `abort` 支持时才是真正取消。
 */
export function useConversationRuntime({ onError, onDone, store: injected }: ConversationRuntimeOptions) {
  const [store] = useState(() => injected ?? createConversationStore());
  const [activity] = useState(createActivityStore);
  // 回复位置的等待过程与收起后的摘要（同一时间只有一轮）。
  const [turns] = useState(() => createTurnStore());
  const status = useConversationStatus(store);
  const revisions = useRef(new Map<number, number>());
  const settled = useRef(new Set<number>());
  const olderInflight = useRef(new Set<number>());
  const outgoingSeq = useRef(0);
  const callbacks = useRef({ onError, onDone });
  callbacks.current = { onError, onDone };
  // 每条对话选定的模型与思考强度（阶段 19-6）：键是 `t<线程 id>`，还没发出第一条的新对话按输入框上下文区分。
  // 值里的 null 表示跟随默认；没有键表示还不知道（已有线程由列表或追问记录带入）。
  const [models, setModels] = useState<Record<string, ThreadOptions>>({});
  const modelsRef = useRef(models);
  modelsRef.current = models;
  // 每条线程上次已知的上下文用量；`chat:context` 按线程整份替换。
  const [contexts, setContexts] = useState<Record<number, ContextUsageView | null>>({});

  const hydrate = useCallback((id: number, messages: ChatMessageView[], hasMore = false) => {
    if (id <= 0) return;
    store.update(id, (current) => {
      // 翻期请求返回的旧快照不能覆盖刚刚收到的新回复，也不取消正在进行的刷新。
      if ((current.messages.at(-1)?.id ?? 0) > (messages.at(-1)?.id ?? 0)) return current;
      return { ...current, ...mergeLatestPage(current, { messages, has_more: hasMore }) };
    });
    settleOutgoing(store, id, messages);
  }, [store]);

  const loadMessages = useCallback(async (id: number, finished = false) => {
    if (id <= 0) return;
    const revision = (revisions.current.get(id) ?? 0) + 1;
    revisions.current.set(id, revision);
    const page = await api.messages(id);
    if (revisions.current.get(id) !== revision) return;
    store.update(id, (current) => {
      const stream = current.stream;
      const last = page.messages.at(-1);
      const persisted = !stream?.text || (last?.role === "assistant" && last.text === stream.text);
      // 落库失败时仍保留已收到的完整回复，不能因历史刷新而把可复制的内容清掉。
      return { ...mergeLatestPage(current, page), stream: finished && persisted ? null : stream };
    });
    settleOutgoing(store, id, page.messages);
  }, [store]);

  /** 向前加载一页更早的历史；同一线程同时只有一次在途，结果只在窗口起点未变时拼接。 */
  const loadOlder = useCallback(async (id: number) => {
    const current = store.get(id);
    const before = current.messages[0]?.id;
    if (id <= 0 || !current.hasMore || before === undefined || olderInflight.current.has(id)) return false;
    olderInflight.current.add(id);
    try {
      const page = await api.messages(id, before);
      let applied = false;
      store.update(id, (latest) => {
        if (latest.messages[0]?.id !== before) return latest;
        applied = true;
        const known = new Set(latest.messages.map((m) => m.id));
        const older = page.messages.filter((m) => m.id < before && !known.has(m.id));
        return { ...latest, messages: [...older, ...latest.messages], hasMore: page.has_more };
      });
      return applied;
    } finally {
      olderInflight.current.delete(id);
    }
  }, [store]);

  const release = useCallback((id: number) => store.release(id), [store]);

  useEffect(() => {
    let disposed = false;
    const unsubs: Array<() => void> = [];
    const subscribe = (promise: Promise<() => void>) => {
      void promise.then((unsub) => {
        if (disposed) unsub();
        else unsubs.push(unsub);
      }).catch((err) => {
        if (!disposed) callbacks.current.onError(formatNotice(i18n.t("runtime.eventsFailed", { ns: "chat" }), err));
      });
    };
    subscribe(onChatStart((id) => {
      turns.attach(id);
      store.setFailure(threadSlot(id, ""), () => null);
      settled.current.delete(id);
      revisions.current.set(id, (revisions.current.get(id) ?? 0) + 1);
      store.setStatus({ busyThreadId: id });
      store.update(id, (current) => (current.stream === null ? current : { ...current, stream: null }));
    }));
    subscribe(onChatDelta((snapshot) => {
      if (settled.current.has(snapshot.thread_id)) return;
      if (snapshot.text) turns.answering(snapshot.thread_id, snapshot.text.length);
      store.update(snapshot.thread_id, (current) => ({ ...current, stream: snapshot }));
    }));
    subscribe(onChatDone((id) => {
      settled.current.add(id);
      activity.set({ line: "", confirm: null });
      if (store.getStatus().busyThreadId === id) store.setStatus({ busyThreadId: null });
      // 回复落库读回后，把这一轮的过程摘要挂到回复上；没有回复时挂到这一轮的失败提示上（中止就不留）。
      const bindTrace = () => {
        const last = store.get(id).messages.at(-1);
        const trace = turns.finish(id, last?.role === "assistant" ? last.id : null);
        if (trace) store.setFailure(threadSlot(id, ""), (current) => (current && !current.trace ? { ...current, trace } : current));
      };
      void loadMessages(id, true).then(bindTrace, (err) => {
        bindTrace();
        callbacks.current.onError(formatNotice(i18n.t("runtime.replyLoadFailed", { ns: "chat" }), err));
      });
      callbacks.current.onDone();
    }));
    subscribe(onChatError((event) => {
      activity.set({ line: "", confirm: null });
      const failure = describeChatError(event);
      // 留在该线程的回复位置，直到下一轮开始；看不到线程时才退回通知。
      if (event.thread_id > 0) store.setFailure(threadSlot(event.thread_id, ""), () => failure);
      else callbacks.current.onError(failureText(failure), failure.fix);
    }));
    subscribe(onChatStatus((event) => {
      // 一步过程只进回复位置的等待过程，不进输入框上方的过程条。
      if (event.step) { turns.step(event.step, event.thread_id); return; }
      if (event.notice_version) activity.set({ notice: { text: event.notice ? formatError(event.notice) : "", version: event.notice_version } });
      else activity.set({ line: event.notice ? formatError(event.notice) : "" });
    }));
    subscribe(onChatConfirm((event) => {
      activity.set({ confirm: event });
      if (event.server) turns.ask(event.thread_id, event.server, event.tool);
    }));
    unsubs.push(activity.onAnswer((confirm, ok) => { if (confirm.server) turns.answer(confirm.thread_id, confirm.server, confirm.tool, ok); }));
    subscribe(onChatContext((event) => setContexts((old) => ({ ...old, [event.thread_id]: event.context }))));
    return () => {
      disposed = true;
      unsubs.forEach((unsub) => unsub());
    };
  }, [activity, loadMessages, store, turns]);

  /** 列表或追问记录带来的已保存选择与用量；本会话里改过的、事件已更新的不覆盖。 */
  const seedThread = useCallback((id: number, stored: SavedThread | null | undefined) => {
    if (id <= 0) return;
    setContexts((old) => (id in old ? old : { ...old, [id]: stored?.context ?? null }));
    if (`t${id}` in modelsRef.current) return;
    // 托盘查到简报线程后会立即发送；不等下一次渲染才能读到该线程的已存模型。
    const next = { ...modelsRef.current, [`t${id}`]: { model: stored?.chat_model ?? null, thinking: stored?.chat_thinking ?? null } };
    modelsRef.current = next;
    setModels(next);
  }, []);
  /** 对话中切换：只影响这条对话之后的轮次，随下一次发送记到线程上。 */
  const chooseModel = useCallback((id: number, context: string, choice: ModelChoice | null, label: string) => {
    const key = modelKey(id, context);
    setModels((old) => ({ ...old, [key]: { ...optionsFor(old, key), model: choice } }));
    if (id > 0) activity.set({ line: i18n.t("runtime.modelSwitched", { ns: "chat", label }) });
  }, [activity]);
  /** 思考强度同模型：null 跟随所用模型的设置。 */
  const chooseThinking = useCallback((id: number, context: string, level: string | null, label: string) => {
    const key = modelKey(id, context);
    setModels((old) => ({ ...old, [key]: { ...optionsFor(old, key), thinking: level } }));
    if (id > 0) activity.set({ line: level ? i18n.t("runtime.thinkingSet", { ns: "chat", level: thinkingLabel(level) }) : i18n.t("runtime.thinkingFollow", { ns: "chat", label }) });
  }, [activity]);
  /** 新对话每次从默认开始；上一次从这里发出、已归到新线程的消息不再留在「新对话」里。 */
  const resetNewModel = useCallback((context: string) => {
    store.setOutgoing(threadSlot(0, context), (current) => (current?.accepted ? null : current));
    setModels((old) => {
      if (!(`new:${context}` in old)) return old;
      const next = { ...old };
      delete next[`new:${context}`];
      return next;
    });
  }, [store]);
  const modelFor = useCallback((id: number, context: string) => models[modelKey(id, context)]?.model ?? null, [models]);
  const thinkingFor = useCallback((id: number, context: string) => models[modelKey(id, context)]?.thinking ?? null, [models]);
  const contextFor = useCallback((id: number) => (id > 0 ? contexts[id] ?? null : null), [contexts]);

  const sendMessage = useCallback(async (id: number, text: string, citations: CitationView[], digestId: number | null, context = "chat") => {
    const current = store.getStatus();
    if (current.sending || current.busyThreadId !== null) {
      throw new Error(i18n.t("runtime.busy", { ns: "chat" }));
    }
    store.setStatus({ sending: true });
    settled.current.clear();
    // 自己的消息立刻出现在末尾（发送中），不等引擎启动；失败撤回，由调用方把文字和引用放回输入框。
    const key = modelKey(id, context);
    const outgoing: Outgoing = {
      token: ++outgoingSeq.current, text, citations, accepted: false, threadId: 0,
      after: id > 0 ? store.get(id).messages.at(-1)?.id ?? 0 : 0,
    };
    const mine = (current: Outgoing | null) => current?.token === outgoing.token;
    store.setFailure(key, () => null);
    store.setOutgoing(key, () => outgoing);
    turns.begin(key, id);
    try {
      const options = modelsRef.current[key];
      const model = options?.model ?? null;
      let threadId: number;
      try {
        // 不知道这条线程的设置时不带思考强度（保持线程上已存的）；知道时连同「跟随模型」一起发。
        threadId = await api.sendMessage(id, text, citations, digestId, model, options ? options.thinking ?? "" : null);
      } catch (err) {
        store.setOutgoing(key, (current) => (mine(current) ? null : current));
        turns.abandon();
        throw err;
      }
      turns.attach(threadId);
      const accepted: Outgoing = { ...outgoing, accepted: true, threadId };
      store.setOutgoing(key, (current) => (mine(current) ? accepted : current));
      // 新线程：切过去之前历史可能还没读回，先把这条挂到新线程上。
      if (modelKey(threadId, context) !== key) store.setOutgoing(modelKey(threadId, context), (current) => current ?? accepted);
      for (const articleId of new Set(citations.map((c) => c.article_id).filter((a) => a > 0))) trackEvent(articleId, "ask");
      if (id <= 0) {
        setModels((old) => {
          const next = { ...old, [`t${threadId}`]: options ?? { model: null, thinking: null } };
          delete next[key];
          return next;
        });
      }
      if (!settled.current.has(threadId)) store.setStatus({ busyThreadId: threadId });
      // 发送已被接受后，即使历史加载失败，也不能把它当成发送失败再发一遍。
      await loadMessages(threadId, settled.current.has(threadId)).catch((err) => {
        callbacks.current.onError(formatNotice(i18n.t("runtime.messagesLoadFailed", { ns: "chat" }), err));
      });
      callbacks.current.onDone();
      return threadId;
    } catch (err) {
      store.setStatus({ busyThreadId: null });
      throw err;
    } finally {
      store.setStatus({ sending: false });
    }
  }, [loadMessages, store, turns]);

  return {
    store, activity, turns, hydrate, loadMessages, loadOlder, release, sendMessage, sending: status.sending, busyThreadId: status.busyThreadId,
    modelFor, thinkingFor, contextFor, seedThread, chooseModel, chooseThinking, resetNewModel,
  };
}
