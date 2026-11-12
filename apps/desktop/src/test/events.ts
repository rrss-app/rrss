import type { SchedulerSnapshot, FeedUpdated, WebEscape, WebVerifyStateEvent, TranslationEvent, ChatContextEvent, ChatConfirmEvent, ChatErrorEvent, ChatStatusEvent, ChatStreamEvent, EmbedCoverEvent, MediaReadyEvent } from "../bindings";
import { deferred, type Deferred } from "./deferred";

export type EventName = "scheduler:updated" | "feed:updated" | "web:verify-state" | "web:navigated" | "web:closed" | "web:escape" | "web:download-blocked" | "chat:start" | "chat:delta" | "chat:done" | "chat:error" | "chat:status" | "chat:confirm" | "chat:context" | "media:ready" | "embed:cover" | "translate:start" | "translate:delta" | "translate:done" | "translate:error";

export type EventMap = {
  "scheduler:updated": SchedulerSnapshot;
  "feed:updated": FeedUpdated;
  "web:verify-state": WebVerifyStateEvent;
  "web:navigated": { label: string; url: string; title: string };
  "web:closed": { label: string };
  "web:escape": WebEscape;
  "web:download-blocked": { label: string; url: string };
  "translate:start": TranslationEvent;
  "translate:delta": TranslationEvent;
  "translate:done": TranslationEvent;
  "translate:error": TranslationEvent;
  "chat:start": number;
  "chat:delta": ChatStreamEvent;
  "chat:done": number;
  "chat:error": ChatErrorEvent;
  "chat:status": ChatStatusEvent;
  "chat:confirm": ChatConfirmEvent;
  "chat:context": ChatContextEvent;
  "media:ready": MediaReadyEvent;
  "embed:cover": EmbedCoverEvent;
};

type Listener<K extends EventName> = { cb: (payload: EventMap[K]) => void; alive: boolean };

export type EventStub = {
  listen: <K extends EventName>(name: K, cb: (payload: EventMap[K]) => void) => Promise<() => void>;
  emit: <K extends EventName>(name: K, payload: EventMap[K]) => void;
  deferListen: (name: EventName) => Deferred<void>;
  onChatStart: (cb: (id: number) => void) => Promise<() => void>;
  onChatDelta: (cb: (snapshot: ChatStreamEvent) => void) => Promise<() => void>;
  onChatDone: (cb: (id: number) => void) => Promise<() => void>;
  onChatError: (cb: (event: ChatErrorEvent) => void) => Promise<() => void>;
  onChatStatus: (cb: (event: ChatStatusEvent) => void) => Promise<() => void>;
  onChatContext: (cb: (event: ChatContextEvent) => void) => Promise<() => void>;
  onChatConfirm: (cb: (event: ChatConfirmEvent) => void) => Promise<() => void>;
  onMediaReady: (cb: (payload: MediaReadyEvent) => void) => Promise<() => void>;
  onEmbedCover: (cb: (payload: EmbedCoverEvent) => void) => Promise<() => void>;
  activeCount: (name?: EventName) => number;
  readonly subscribeCount: number;
  readonly unsubscribeCount: number;
};

export function createEventStub(): EventStub {
  const listeners = new Map<EventName, Set<Listener<EventName>>>();
  const listenGates = new Map<EventName, Deferred<void>[]>();
  let subscribeCount = 0;
  let unsubscribeCount = 0;

  function listen<K extends EventName>(name: K, cb: (payload: EventMap[K]) => void): Promise<() => void> {
    subscribeCount += 1;
    const sub: Listener<K> = { cb, alive: true };
    const unlisten = () => {
      if (!sub.alive) return;
      sub.alive = false;
      listeners.get(name)?.delete(sub as Listener<EventName>);
      unsubscribeCount += 1;
    };
    const attach = () => {
      let set = listeners.get(name);
      if (!set) {
        set = new Set();
        listeners.set(name, set);
      }
      set.add(sub as Listener<EventName>);
      return unlisten;
    };
    const gate = listenGates.get(name)?.shift();
    if (!gate) return Promise.resolve(attach());
    return gate.promise.then(attach);
  }

  function emit<K extends EventName>(name: K, payload: EventMap[K]) {
    const set = listeners.get(name);
    if (!set) return;
    for (const sub of [...set]) {
      if (sub.alive) (sub.cb as (payload: EventMap[K]) => void)(payload);
    }
  }

  function deferListen(name: EventName): Deferred<void> {
    const pending = deferred<void>();
    const queue = listenGates.get(name) ?? [];
    queue.push(pending);
    listenGates.set(name, queue);
    return pending;
  }

  function activeCount(name?: EventName): number {
    if (name) {
      const set = listeners.get(name);
      if (!set) return 0;
      let n = 0;
      for (const sub of set) if (sub.alive) n += 1;
      return n;
    }
    let n = 0;
    for (const key of listeners.keys()) n += activeCount(key);
    return n;
  }

  return {
    listen,
    emit,
    deferListen,
    onChatStart: (cb) => listen("chat:start", cb),
    onChatDelta: (cb) => listen("chat:delta", cb),
    onChatDone: (cb) => listen("chat:done", cb),
    onChatError: (cb) => listen("chat:error", cb),
    onChatStatus: (cb) => listen("chat:status", cb),
    onChatContext: (cb) => listen("chat:context", cb),
    onChatConfirm: (cb) => listen("chat:confirm", cb),
    onMediaReady: (cb) => listen("media:ready", cb),
    onEmbedCover: (cb) => listen("embed:cover", cb),
    activeCount,
    get subscribeCount() {
      return subscribeCount;
    },
    get unsubscribeCount() {
      return unsubscribeCount;
    },
  };
}

export function eventListenerExports(getEvents: () => EventStub | null) {
  const events = () => {
    const stub = getEvents();
    if (!stub) throw new Error("event stub not installed");
    return stub;
  };
  return {
    onSchedulerUpdated: (cb: (payload: SchedulerSnapshot) => void) => events().listen("scheduler:updated", cb),
    onFeedUpdated: (cb: (payload: FeedUpdated) => void) => events().listen("feed:updated", cb),
    onWebVerifyState: (cb: (payload: WebVerifyStateEvent) => void) => events().listen("web:verify-state", cb),
    onWebNavigated: (cb: (payload: EventMap["web:navigated"]) => void) => events().listen("web:navigated", cb),
    onWebClosed: (cb: (payload: EventMap["web:closed"]) => void) => events().listen("web:closed", cb),
    onWebEscape: (cb: (payload: EventMap["web:escape"]) => void) => events().listen("web:escape", cb),
    onWebDownloadBlocked: (cb: (payload: EventMap["web:download-blocked"]) => void) => events().listen("web:download-blocked", cb),
    onTranslationEvent: async (cb: (payload: TranslationEvent) => void) => {
      const results = await Promise.allSettled((["translate:start", "translate:delta", "translate:done", "translate:error"] as const).map(name => events().listen(name, cb)));
      const dispose = () => { for (const result of results) if (result.status === "fulfilled") result.value(); };
      const failure = results.find(result => result.status === "rejected");
      if (failure?.status === "rejected") { dispose(); throw failure.reason; }
      return dispose;
    },
    onChatStart: (cb: (id: number) => void) => events().onChatStart(cb),
    onChatDelta: (cb: (snapshot: ChatStreamEvent) => void) => events().onChatDelta(cb),
    onChatDone: (cb: (id: number) => void) => events().onChatDone(cb),
    onChatError: (cb: (event: ChatErrorEvent) => void) => events().onChatError(cb),
    onChatStatus: (cb: (event: ChatStatusEvent) => void) => events().onChatStatus(cb),
    onChatContext: (cb: (event: ChatContextEvent) => void) => events().onChatContext(cb),
    onChatConfirm: (cb: (event: ChatConfirmEvent) => void) => events().onChatConfirm(cb),
    onMediaReady: (cb: (payload: MediaReadyEvent) => void) => events().onMediaReady(cb),
    onEmbedCover: (cb: (payload: EmbedCoverEvent) => void) => events().onEmbedCover(cb),
  };
}
