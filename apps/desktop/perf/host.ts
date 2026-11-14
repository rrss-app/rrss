import { modelPoolFixture, poolEntryFixture, bindingsFixture } from "../src/test/fixtures/settings";
import { applyFeedTreeOrder } from "../src/test/treeOrder";
import { digestThreadFixture } from "../src/test/fixtures/chat";
import type { Page } from "@playwright/test";
import type {
  UiError,
  AiPage,
  ArticleView,
  ChatMessageView,
  DiagnosticsPage,
  DigestView,
  FeedTree,
  MediaRef,
  MemoryPage,
  PiPage,
  RankingSettings,
  ReaderView,
  RulesPage,
  ThreadView,
  TranslatePage,
} from "../src/bindings";
import type { GeneratedScene } from "./generate";
import "./window";

/** 与 `rrss-app::queries::chat` 的历史窗口常量一致：默认一页与上限。 */
export const MESSAGE_PAGE_DEFAULT = 40;
export const MESSAGE_PAGE_MAX = 200;

export type IpcCallMeta = {
  method: string;
  requestBytes: number;
  responseBytes: number;
  durationMs: number;
};

export type HostAudit = {
  unexpected: string[];
  ipc: IpcCallMeta[];
  streamEmits: number;
  listedCount: number;
};

export type HostPayload = {
  pageSize: number;
  streamSnapshotCount: number;
  streamIntervalMs: number;
  longArticleId: number;
  longThreadId: number;
  digestThreadId: number;
  articles: ArticleView[];
  longReader: ReaderView;
  shortDocument: unknown;
  threads: ThreadView[];
  messages: Record<string, ChatMessageView[]>;
  feeds: FeedTree;
  /** `applyFeedTreeOrder` 的源码，工厂在页面里 eval，不能引用外部绑定。 */
  feedTreeOrder: string;
  digests: DigestView[];
  digestMessages: ChatMessageView[];
  ranking: RankingSettings;
  filter: string;
  listScope: string;
  digestBanner: string;
  ai: AiPage;
  pool: import("../src/bindings").ModelPoolView;
  bindings: import("../src/bindings").ModelBindingsView;
  digestThread: import("../src/bindings").DigestThreadView;
  rules: RulesPage;
  memory: MemoryPage;
  pi: PiPage;
  diag: DiagnosticsPage;
  translate: TranslatePage;
};

export type StreamEmitMeta = {
  seq: number;
  emittedAt: number;
  textLength: number;
};

export type PerfHost = {
  invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown>;
  listedCount: () => number;
  hasMore: () => boolean;
  streamLog: () => StreamEmitMeta[];
  streamDone: () => boolean;
  audit: () => HostAudit;
  dispose: () => void;
  transformCallback: (cb: (event: unknown) => void, once?: boolean) => number;
  unregisterListener: (event: string, eventId: number) => void;
};

type HostState = {
  articles: ArticleView[];
  threads: ThreadView[];
  messages: Record<string, ChatMessageView[]>;
  ranking: RankingSettings;
  filter: string;
  listScope: string;
  theme: string;
  nextThreadId: number;
  nextMessageId: number;
  streamTimer: ReturnType<typeof setTimeout> | null;
  streamAborted: boolean;
  listedIds: number[];
};

export function sceneToHostPayload(scene: GeneratedScene): HostPayload {
  const messages: Record<string, ChatMessageView[]> = {};
  for (const [id, rows] of Object.entries(scene.messages)) {
    messages[id] = JSON.parse(JSON.stringify(rows)) as ChatMessageView[];
  }
  return {
    pageSize: scene.spec.pageSize,
    streamSnapshotCount: scene.spec.streamSnapshotCount,
    streamIntervalMs: scene.spec.streamIntervalMs,
    longArticleId: scene.longArticleId,
    longThreadId: scene.longThreadId,
    digestThreadId: scene.digestThreadId,
    articles: JSON.parse(JSON.stringify(scene.articles)) as ArticleView[],
    longReader: JSON.parse(JSON.stringify(scene.longReader)) as ReaderView,
    shortDocument: JSON.parse(JSON.stringify(scene.shortDocument)) as unknown,
    threads: JSON.parse(JSON.stringify(scene.threads)) as ThreadView[],
    messages,
    feeds: JSON.parse(JSON.stringify(scene.feeds)) as FeedTree,
    feedTreeOrder: applyFeedTreeOrder.toString(),
    digests: JSON.parse(JSON.stringify(scene.digests)) as DigestView[],
    digestMessages: JSON.parse(JSON.stringify(scene.digestMessages)) as ChatMessageView[],
    ranking: JSON.parse(JSON.stringify(scene.ranking)) as RankingSettings,
    filter: scene.filter,
    listScope: scene.listScope,
    digestBanner: scene.digestBanner,
    ai: JSON.parse(JSON.stringify(scene.ai)) as AiPage,
    pool: modelPoolFixture({ pool: [poolEntryFixture({ chat: true, engine_context_window: 32000 })] }),
    bindings: bindingsFixture({ tasks: [{ task: "chat", title: "对话", provider_id: 1, model: "fixture-model" }] }),
    digestThread: digestThreadFixture({thread_id: scene.digestThreadId, messages: scene.digestMessages}),
    rules: JSON.parse(JSON.stringify(scene.rules)) as RulesPage,
    memory: JSON.parse(JSON.stringify(scene.memory)) as MemoryPage,
    pi: JSON.parse(JSON.stringify(scene.pi)) as PiPage,
    diag: JSON.parse(JSON.stringify(scene.diag)) as DiagnosticsPage,
    translate: JSON.parse(JSON.stringify(scene.translate)) as TranslatePage,
  };
}

/**
 * 自包含工厂：toString() 后可在页面 eval，禁止引用外部绑定。
 * 返回快照一律 JSON 深拷贝，避免调用方改写内部状态。
 */
export function createPerfHostFactory(payload: HostPayload): PerfHost {
  function clone<T>(value: T): T {
    return JSON.parse(JSON.stringify(value)) as T;
  }
  function bytes(value: unknown): number {
    try {
      return new TextEncoder().encode(JSON.stringify(value)).length;
    } catch {
      return 0;
    }
  }
  function asRecord(value: unknown): Record<string, unknown> {
    return value !== null && typeof value === "object" ? (value as Record<string, unknown>) : {};
  }
  function asNumber(value: unknown, fallback = 0): number {
    return typeof value === "number" && Number.isFinite(value) ? value : fallback;
  }
  function asBool(value: unknown, fallback = false): boolean {
    return typeof value === "boolean" ? value : fallback;
  }
  function asString(value: unknown, fallback = ""): string {
    return typeof value === "string" ? value : fallback;
  }
  function asNumList(value: unknown): number[] {
    return Array.isArray(value) ? value.filter((item): item is number => typeof item === "number") : [];
  }
  function pick(obj: Record<string, unknown>, camel: string, snake: string): unknown {
    return obj[camel] !== undefined ? obj[camel] : obj[snake];
  }

  const focusColumns = { value: "" };
  const state: HostState = {
    articles: clone(payload.articles),
    threads: clone(payload.threads),
    messages: clone(payload.messages),
    ranking: clone(payload.ranking),
    filter: payload.filter,
    listScope: payload.listScope,
    theme: payload.ranking.theme || "light",
    nextThreadId: Math.max(payload.longThreadId, payload.digestThreadId) + 1,
    nextMessageId: 10_000,
    streamTimer: null,
    streamAborted: false,
    listedIds: [],
  };

  const ipc: IpcCallMeta[] = [];
  const unexpected: string[] = [];
  let streamEmits = 0;
  let lastHasMore = false;
  let streamCompleted = false;
  const streamLog: Array<{ seq: number; emittedAt: number; textLength: number }> = [];
  const listeners = new Map<string, Set<number>>();
  const callbacks = new Map<number, (event: unknown) => void>();
  let callbackId = 0;

  function transformCallback(cb: (event: unknown) => void, once = false): number {
    const id = ++callbackId;
    callbacks.set(id, (event: unknown) => {
      if (once) callbacks.delete(id);
      cb(event);
    });
    return id;
  }

  function unregisterListener(event: string, eventId: number): void {
    listeners.get(event)?.delete(eventId);
  }

  function emit(event: string, payloadValue: unknown): void {
    const ids = listeners.get(event);
    if (!ids) return;
    for (const id of [...ids]) {
      const cb = callbacks.get(id);
      if (cb) cb({ event, id, payload: payloadValue });
    }
  }

  function listedCount(): number {
    return state.listedIds.length;
  }

  function hasMore(): boolean {
    return lastHasMore;
  }

  function streamLogSnapshot(): Array<{ seq: number; emittedAt: number; textLength: number }> {
    return streamLog.map((row) => ({ seq: row.seq, emittedAt: row.emittedAt, textLength: row.textLength }));
  }

  function streamDone(): boolean {
    return streamCompleted;
  }

  function articleById(id: number): ArticleView | undefined {
    return state.articles.find((row) => row.id === id);
  }

  /** 焦点栏目只认文件夹（含子文件夹）。单个源与无法识别的范围按全部订阅。 */
  function scopeFeedIds(scope: string): Set<number> | null {
    const folderMatch = /^folder:(-?\d+)$/.exec(scope);
    if (!folderMatch) return null;
    const folders = payload.feeds.folders;
    const index = folders.findIndex((item) => item.id === Number(folderMatch[1]));
    if (index < 0) return null;
    const base = folders[index].indent;
    const ids = new Set<number>();
    for (let i = index; i < folders.length; i++) {
      if (i > index && folders[i].indent <= base) break;
      for (const item of folders[i].feeds) ids.add(item.id);
    }
    return ids;
  }

  function paginate(args: Record<string, unknown>): {
    articles: ArticleView[];
    digest_banner: string;
    digest_lede: string;
    filter: string;
    status: string;
    ranking: RankingSettings;
    has_more: boolean;
    list_scope: string;
  } {
    const feedIdRaw = pick(args, "feedId", "feed_id");
    const feedId = feedIdRaw === null || feedIdRaw === undefined ? null : asNumber(feedIdRaw, 0) || null;
    const columnIds = scopeFeedIds(asString(pick(args, "scope", "scope")));
    const excludeIds = new Set(asNumList(pick(args, "excludeIds", "exclude_ids")));
    const rows = state.articles.filter((row) => {
      if (row.row_kind !== 0) return false;
      if (excludeIds.has(row.id)) return false;
      if (feedId !== null && row.feed_id !== feedId) return false;
      if (columnIds && !columnIds.has(row.feed_id)) return false;
      if (state.listScope === "starred") return row.starred;
      if (state.listScope === "unread") return !row.read;
      return true;
    });
    const pageSize = payload.pageSize;
    if (excludeIds.size === 0) state.listedIds = [];
    const has_more = rows.length > pageSize;
    lastHasMore = has_more;
    const pageRows = clone(has_more ? rows.slice(0, pageSize) : rows);
    for (const row of pageRows) {
      if (!state.listedIds.includes(row.id)) state.listedIds.push(row.id);
    }
    return {
      articles: pageRows,
      digest_banner: columnIds ? "" : payload.digestBanner,
      digest_lede: "",
      filter: state.filter,
      status: "",
      ranking: clone(state.ranking),
      has_more,
      list_scope: state.listScope,
    };
  }

  function readerFor(id: number, markRead: boolean): ReaderView {
    const row = articleById(id);
    if (!row) throw { code: "article_not_found", detail: "article not found" } satisfies UiError;
    if (markRead) row.read = true;
    if (id === payload.longArticleId) {
      const view = clone(payload.longReader);
      view.starred = row.starred;
      return view;
    }
    return {
      id,
      source_hash: `perf-${id}`,
      title: row.title,
      meta: `${row.feed_title} · 固定时刻 ${new Date(row.published_at ?? 0).toISOString().slice(5, 16).replace("T", " ")}`,
      feed_icon_href: "",
      url: row.url,
      summary: row.summary,
      score: row.score,
      feedback: 0,
      document: clone(payload.shortDocument),
      reasons: [],
      media: [],
      translated_title: "",
      translated_document: null,
      translation_ready: false,
      translation_note: "",
      truncated: false, pruned: false, summary_stale: false,
      starred: row.starred,
      embed_covers: [],
    };
  }

  function clearStream(): void {
    if (state.streamTimer !== null) {
      clearTimeout(state.streamTimer);
      state.streamTimer = null;
    }
  }

  function startStream(threadId: number): void {
    clearStream();
    state.streamAborted = false;
    streamCompleted = false;
    streamLog.length = 0;
    streamEmits = 0;
    const count = payload.streamSnapshotCount;
    const interval = payload.streamIntervalMs;
    const full = "合成流式完整快照。".repeat(Math.max(8, count));
    const unit = Math.max(1, Math.ceil(full.length / count));
    let step = 0;
    emit("chat:start", threadId);
    const tick = () => {
      if (state.streamAborted) return;
      step += 1;
      const text = full.slice(0, Math.min(full.length, step * unit));
      const snapshot = {
        thread_id: threadId,
        text,
        document: { v: 1, blocks: [{ t: "p", inlines: [{ t: "t", text }] }] },
      };
      streamEmits += 1;
      streamLog.push({
        seq: step,
        emittedAt: typeof performance !== "undefined" ? performance.now() : Date.now(),
        textLength: text.length,
      });
      emit("chat:delta", clone(snapshot));
      if (step < count) {
        state.streamTimer = setTimeout(tick, interval);
        return;
      }
      const key = String(threadId);
      const history = state.messages[key] ?? [];
      history.push({
        id: ++state.nextMessageId,
        role: "assistant",
        text: full,
        streaming: false,
        cite_article_id: 0,
        cite_title: "",
        citations: [],
        document: clone(snapshot.document),
      });
      state.messages[key] = history;
      state.streamTimer = null;
      streamCompleted = true;
      const thread = state.threads.find(t => t.id === threadId);
      const context = {used: Math.ceil(history.reduce((sum,m)=>sum+m.text.length,0)/3),window: thread ? contextWindow(thread) : 32000,at:Date.now()};
      if (thread) thread.context = context;
      emit("chat:context",{thread_id:threadId,context});
      emit("chat:done", threadId);
    };
    state.streamTimer = setTimeout(tick, 0);
  }

  function contextWindow(thread: ThreadView) {
    const selected = thread.chat_model ?? payload.bindings.tasks.find(t => t.task === "chat");
    const entry = payload.pool.pool.find(e => e.provider_id === selected?.provider_id && e.model === selected?.model);
    return entry?.context_window ?? entry?.engine_context_window ?? 32000;
  }

  const applyFeedTreeOrder = eval(`(${payload.feedTreeOrder})`) as (
    tree: FeedTree,
    op: "move" | "restore",
    args: { items?: unknown; parentFolderId?: number | null; before?: string | null; layout?: unknown },
  ) => { tree: FeedTree; layout: unknown };
  function handle(command: string, rawArgs: Record<string, unknown>): unknown {
    switch (command) {
      case "scheduler_status":
        return { run_id: "perf", version: 0, operations: [], background_requested: 0, background_completed: 0, background: [], stopped: false, feed_pause: null };

      case "plugin:event|listen": {
        const event = asString(rawArgs.event);
        const handler = asNumber(rawArgs.handler);
        const ids = listeners.get(event) ?? new Set<number>();
        ids.add(handler);
        listeners.set(event, ids);
        return handler;
      }
      case "plugin:event|unlisten": {
        unregisterListener(asString(rawArgs.event), asNumber(pick(rawArgs, "eventId", "event_id")));
        return null;
      }
      case "settings_schema":
        return {
          specs: [{ id: "focus_columns", domain: "reader", level: "l0", kind: { type: "text", max: 160 }, default: "", apply: "immediate", affects_pi: false }],
          values: [{ id: "focus_columns", value: focusColumns.value }, { id: "feeds_scope", value: "all" }, { id: "feeds_list_scope", value: "all" }],
        };
      case "focus_stream":
        return paginate(rawArgs);
      case "focus_cluster_members":
        return [];
      case "mark_cluster_read":
        return [];
      case "cluster_summary":
      case "generate_cluster_summary":
        return { cluster_id: Number(rawArgs.clusterId ?? 0), state: "none", model_ready: false, estimate_tokens: 0, overview: "",
          additions: [], conflicts: [], basis: 0, generated_at: null, fresh: [], members: [] };
      case "feed_tree":
        return clone(payload.feeds);
      case "move_tree_items": {
        const moved = applyFeedTreeOrder(payload.feeds, "move", {
          items: rawArgs.items,
          parentFolderId: (pick(rawArgs, "parentFolderId", "parent_folder_id") as number | null | undefined) ?? null,
          before: (rawArgs.before as string | null | undefined) ?? null,
        });
        payload.feeds = moved.tree;
        return moved.layout;
      }
      case "restore_tree_layout": {
        payload.feeds = applyFeedTreeOrder(payload.feeds, "restore", { layout: rawArgs.layout }).tree;
        return null;
      }
      case "threads":
        return clone(state.threads);
      case "thread_list":
        return clone({ folders: [], threads: state.threads, archived_count: 0 });
      case "archived_threads":
        return [];
      case "messages": {
        const threadId = asNumber(pick(rawArgs, "threadId", "thread_id"));
        const beforeRaw = pick(rawArgs, "beforeId", "before_id");
        const before = beforeRaw == null ? null : asNumber(beforeRaw);
        const limitRaw = pick(rawArgs, "limit", "limit");
        // 与 rrss-app 的窗口常量一致（工厂在页面内 eval，不能引用模块常量）。
        const pageDefault = 40;
        const pageMax = 200;
        const limit = limitRaw == null || asNumber(limitRaw) <= 0 ? pageDefault : Math.min(asNumber(limitRaw), pageMax);
        const all = (state.messages[String(threadId)] ?? []).filter((m) => before === null || m.id < before);
        const window = all.slice(Math.max(0, all.length - limit));
        return { messages: clone(window), has_more: all.length > limit };
      }
      case "new_thread": {
        const id = state.nextThreadId++;
        state.threads = [{ id, title: "新对话", chat_model: null, chat_thinking: null, context: null, folder_id: null, pinned: false, updated_at: 0, archived_at: null }, ...state.threads];
        state.messages[String(id)] = [];
        return id;
      }
      case "send_message": {
        const envelope = asRecord(rawArgs.args ?? rawArgs);
        const requested = asNumber(pick(envelope, "threadId", "thread_id"));
        let threadId = requested;
        if (threadId <= 0) {
          threadId = state.nextThreadId++;
          state.threads = [{ id: threadId, title: "合成新会话", chat_model: null, chat_thinking: null, context: null, folder_id: null, pinned: false, updated_at: 0, archived_at: null }, ...state.threads];
        }
        const thread = state.threads.find(t => t.id === threadId);
        if (thread) {
          thread.chat_model = (envelope.model ?? null) as ThreadView["chat_model"];
          if (envelope.thinking != null) thread.chat_thinking = String(envelope.thinking) || null;
          if (thread.context) thread.context = { ...thread.context, window: contextWindow(thread) };
        }
        const key = String(threadId);
        const history = state.messages[key] ?? [];
        history.push({
          id: ++state.nextMessageId,
          role: "user",
          text: asString(pick(envelope, "text", "text")) || "合成提问",
          streaming: false,
          cite_article_id: 0,
          cite_title: "",
          citations: [],
          document: null,
        });
        state.messages[key] = history;
        startStream(threadId);
        return threadId;
      }
      case "abort_generation": {
        state.streamAborted = true;
        clearStream();
        emit("chat:error", { thread_id: 0, message: "已停止生成" });
        return null;
      }
      case "article_reader":
        return readerFor(asNumber(rawArgs.id), false);
      case "open_article":
        return readerFor(asNumber(rawArgs.id), true);
      case "related_articles":
        return [];
      case "prefetch_images": {
        const id = asNumber(pick(rawArgs, "articleId", "article_id"));
        if (id === payload.longArticleId) return clone(payload.longReader.media);
        return [] as MediaRef[];
      }
      case "retry_image":
        return {
          src: asString(pick(rawArgs, "url", "url")),
          media_id: 1,
          href: "rrss://localhost/media/1",
          width: null,
          height: null,
          error: "",
        };
      case "digest_list": {
        const period = asString(rawArgs.period, "daily");
        return clone(payload.digests.filter((row) => row.period === period).map((row) => ({
          id: row.id, period: row.period, period_label: row.period_label, period_start: row.period_start, date_label: row.date_label,
        })));
      }
      case "digest": {
        const id = asNumber(rawArgs.id);
        const row = payload.digests.find((item) => item.id === id);
        if (!row) throw { code: "digest_not_found", detail: "digest not found" } satisfies UiError;
        return clone(row);
      }
      case "digest_thread": {
        const digestId = asNumber(pick(rawArgs, "digestId", "digest_id"));
        if (digestId === 101) {
          const thread = state.threads.find(t => t.id === payload.digestThreadId);
          return clone({...payload.digestThread,chat_model:thread?.chat_model ?? null,chat_thinking:thread?.chat_thinking ?? null,context:thread?.context ?? null});
        }
        return { thread_id: 0, chat_model: null, chat_thinking: null, context: null, messages: [], has_more: false };
      }
      case "rules_page":
        return clone(payload.rules);
      case "memory_page":
        return clone(payload.memory);
      case "model_pool": return clone(payload.pool);
      case "model_bindings": return clone(payload.bindings);
      case "ai_page":
        return clone({ ...payload.ai, selected_id: asNumber(pick(rawArgs, "selectedId", "selected_id")) || payload.ai.selected_id });
      case "translate_page":
        return clone(payload.translate);
      case "translate_snapshot":
        return { kind: "missing", reader: readerFor(asNumber(rawArgs.id), false), snapshot: null, reconcile_after_ms: 45000 };
      case "translate_attach":
      case "translate_detach":
        return null;
      case "probe_mcp":
        return { ok: true, elapsed_ms: 12, tools: [], error: "", hint: "", output: "" };
      case "login_mcp":
        return { ok: true, message: "已登录" };
      case "cancel_mcp_login":
        return;
      case "pi_page":
        return clone(payload.pi);
      case "diagnostics":
        return clone(payload.diag);
      case "log_client":
        return null;
      case "log_set_level":
        return clone(payload.diag.log);
      case "log_export":
        return null;
      case "window_chrome":
        return { controls: "windows", tray: true };
      case "tray_install":
        return true;
      case "plugin:window|is_maximized":
        return false;
      case "mark_read": {
        const id = asNumber(rawArgs.id);
        const row = articleById(id);
        if (row) row.read = asBool(rawArgs.read, true);
        return null;
      }
      case "star_article": {
        const id = asNumber(rawArgs.id);
        const row = articleById(id);
        if (row) row.starred = asBool(rawArgs.starred, true);
        return null;
      }
      case "unmark_read": {
        const ids = new Set(asNumList(rawArgs.ids));
        for (const row of state.articles) {
          if (ids.has(row.id)) row.read = false;
        }
        return null;
      }
      case "set_setting": {
        const id = asString(rawArgs.id);
        const value = asString(rawArgs.value);
        if (id === "focus_columns") { focusColumns.value = value; return { id, value }; }
        if (id === "focus_filter") state.filter = value;
        else if (id === "list_scope") state.listScope = value;
        else if (id === "theme_preference") {
          state.theme = value;
          state.ranking = { ...state.ranking, theme: value };
        } else {
          unexpected.push(command);
          throw new Error(`unexpected setting ${id}`);
        }
        return { id, value };
      }
      case "cite_article": {
        const id = asNumber(rawArgs.id);
        const row = articleById(id);
        return {
          kind: "article",
          article_id: id,
          title: row?.title ?? "",
          feed_title: row?.feed_title ?? "",
          url: row?.url ?? "",
          excerpt: "",
          block_id: 0,
        };
      }
      default:
        unexpected.push(command);
        throw new Error(`unexpected command ${command}`);
    }
  }

  async function invoke(command: string, args: Record<string, unknown> = {}): Promise<unknown> {
    const started = typeof performance !== "undefined" ? performance.now() : Date.now();
    const requestBytes = bytes(args);
    try {
      const result = handle(command, asRecord(args));
      const durationMs = (typeof performance !== "undefined" ? performance.now() : Date.now()) - started;
      ipc.push({
        method: command,
        requestBytes,
        responseBytes: bytes(result),
        durationMs,
      });
      return result;
    } catch (err) {
      const durationMs = (typeof performance !== "undefined" ? performance.now() : Date.now()) - started;
      ipc.push({
        method: command,
        requestBytes,
        responseBytes: 0,
        durationMs,
      });
      throw err;
    }
  }

  function audit(): HostAudit {
    return {
      unexpected: [...unexpected],
      ipc: ipc.map((row) => ({ ...row })),
      streamEmits,
      listedCount: listedCount(),
    };
  }

  function dispose(): void {
    clearStream();
  }

  return { invoke, listedCount, hasMore, streamLog: streamLogSnapshot, streamDone, audit, dispose, transformCallback, unregisterListener };
}

export function summarizeIpc(ipc: readonly IpcCallMeta[]): Array<{
  method: string;
  count: number;
  requestBytes: number;
  responseBytes: number;
  p50LatencyMs: number | null;
  p95LatencyMs: number | null;
}> {
  const groups = new Map<string, IpcCallMeta[]>();
  for (const row of ipc) {
    const list = groups.get(row.method) ?? [];
    list.push(row);
    groups.set(row.method, list);
  }
  const out: Array<{
    method: string;
    count: number;
    requestBytes: number;
    responseBytes: number;
    p50LatencyMs: number | null;
    p95LatencyMs: number | null;
  }> = [];
  for (const [method, rows] of groups) {
    const durations = rows.map((row) => row.durationMs).sort((a, b) => a - b);
    const p = (q: number) => {
      if (durations.length === 0) return null;
      const index = Math.min(durations.length - 1, Math.max(0, Math.ceil(q * durations.length) - 1));
      return durations[index] ?? null;
    };
    out.push({
      method,
      count: rows.length,
      requestBytes: rows.reduce((sum, row) => sum + row.requestBytes, 0),
      responseBytes: rows.reduce((sum, row) => sum + row.responseBytes, 0),
      p50LatencyMs: p(0.5),
      p95LatencyMs: p(0.95),
    });
  }
  return out.sort((a, b) => a.method.localeCompare(b.method));
}

export async function installStrictPerfHost(page: Page, payload: HostPayload): Promise<void> {
  await page.addInitScript(
    ({ source, payload: hostPayload }: { source: string; payload: HostPayload }) => {
      const factory = eval(`(${source})`) as (input: HostPayload) => PerfHost;
      const host = factory(hostPayload);
      Object.assign(window, {
        __rrssPerfCold: { firstArticleMs: null, source: "mutation-observer" },
        __rrssPerf: {
          listedCount: () => host.listedCount(),
          hasMore: () => host.hasMore(),
          streamLog: () => host.streamLog(),
          streamDone: () => host.streamDone(),
          audit: () => host.audit(),
          dispose: () => host.dispose(),
        },
        __TAURI_EVENT_PLUGIN_INTERNALS__: {
          unregisterListener: (event: string, id: number) => host.unregisterListener(event, id),
        },
        __TAURI_INTERNALS__: {
          metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
          transformCallback: (cb: (event: unknown) => void, once?: boolean) => host.transformCallback(cb, once),
          invoke: (command: string, args: Record<string, unknown> = {}) => host.invoke(command, args),
        },
      });
      const articleVisible = () => {
        const nodes = document.querySelectorAll("article.article, article[aria-label]");
        for (const node of nodes) {
          if (!(node instanceof HTMLElement)) continue;
          const rects = node.getClientRects();
          if (rects.length === 0) continue;
          const box = rects[0];
          if (!box || box.width <= 0 || box.height <= 0) continue;
          const style = getComputedStyle(node);
          if (style.visibility === "hidden" || style.display === "none" || style.opacity === "0") continue;
          return true;
        }
        return false;
      };
      const markCold = () => {
        const stamp = window.__rrssPerfCold;
        if (!stamp || stamp.firstArticleMs != null) return;
        if (articleVisible()) stamp.firstArticleMs = performance.now();
      };
      const coldObs = new MutationObserver(markCold);
      const root = document.documentElement ?? document;
      coldObs.observe(root, { childList: true, subtree: true });
      if (document.body && document.body !== root) {
        coldObs.observe(document.body, { childList: true, subtree: true });
      }
      const poll = () => {
        markCold();
        if (window.__rrssPerfCold?.firstArticleMs == null) requestAnimationFrame(poll);
        else coldObs.disconnect();
      };
      poll();
      document.addEventListener("DOMContentLoaded", markCold);
    },
    { source: createPerfHostFactory.toString(), payload },
  );
}
