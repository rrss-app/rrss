import type { ReaderView, TranslationCommandError, TranslationEvent, TranslationReply, TranslationStartMode, TranslationInitialViewport } from "../../bindings";
import { i18n } from "../../i18n";
import { createTranslationViewport } from "./translationViewport";
import { api, onTranslationEvent } from "../../lib/api";
import { formatError } from "../../lib/errors";
import { formatNotice } from "../../lib/notice";
import { applyTranslationEvent, restoreTranslation, sameTranslation, translationActive, type TranslationData } from "./translationState";

export type TranslationSessionView = { data: TranslationData | null; autoBilingual: boolean; busy: boolean; disconnected: boolean; issue: boolean; failure: TranslationCommandError["kind"] | null; note: string };
const MAX_EVENTS = 128, MAX_BYTES = 2 * 1024 * 1024, REQUEST_TIMEOUT_MS = 15_000;
let nextTicket = Date.now();

class ConnectionTimeout extends Error {}

async function bounded<T>(request: Promise<T>): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try { return await Promise.race([request, new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new ConnectionTimeout(i18n.t("translation.timeout", { ns: "reader" }))), REQUEST_TIMEOUT_MS); })]); }
  finally { clearTimeout(timer); }
}

/** 一份阅读票据的监听 / 握手与有界恢复。所有恢复都是只读；只有 start 会发起翻译。 */
export function createTranslationSession(reader: ReaderView, changed: (view: TranslationSessionView) => void, sourceChanged: (reader: ReaderView) => void) {
  const ticket = nextTicket = Math.max(Date.now(), nextTicket + 1);
  const viewport = createTranslationViewport(ticket, bounded);
  let autoBilingual = false, issue = false;
  let failure: TranslationCommandError["kind"] | null = null;
  let disposed = false, busy = false, data: TranslationData | null = null, disconnected = false, note = "";
  let buffer: TranslationEvent[] = [], bytes = 0, overflow = false;
  let listenEpoch = 0;
  let unlisten: (() => void) | null = null, listening: Promise<void> | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined, reconcileAfter = 45_000;
  const publish = () => { if (!disposed) changed({ data, autoBilingual, busy: busy || (!disconnected && translationActive(data)), disconnected, issue, failure, note }); };
  const clearBuffer = () => { buffer = []; bytes = 0; overflow = false; };
  const fail = (error: unknown, connection: boolean) => {
    disconnected = connection; issue = true;
    const typed = error && typeof error === "object" && "kind" in error ? error as TranslationCommandError : null;
    failure = typed?.kind ?? null;
    note = connection ? i18n.t("translation.disconnected", { ns: "reader" }) : formatNotice(i18n.t("translation.failedPrefix", { ns: "reader" }), typed?.error ?? error);
    clearTimeout(timer); clearBuffer(); publish();
  };
  const schedule = () => {
    clearTimeout(timer);
    if (!disposed && !disconnected && translationActive(data)) timer = setTimeout(() => void recover(), reconcileAfter);
    viewport.schedule(!disposed && !disconnected && !busy && translationActive(data));
  };
  const remember = (event: TranslationEvent) => {
    if (overflow) return;
    bytes += JSON.stringify(event).length * 2;
    if (buffer.length >= MAX_EVENTS || bytes > MAX_BYTES) { buffer = []; overflow = true; return; }
    buffer.push(event);
  };
  const receive = (event: TranslationEvent) => {
    if (disposed || (disconnected && !busy) || event.identity.article_id !== reader.id || event.identity.source_hash !== reader.source_hash) return;
    if (busy) { remember(event); return; }
    if (!data || !sameTranslation(data.snapshot.identity, event.identity)) return;
    try {
      data = applyTranslationEvent(data, event);
      publish(); schedule();
      if (data.snapshot.phase === "invalidated") void recover();
    } catch { remember(event); void recover(); }
  };
  const listen = () => {
    if (!listening) {
      const epoch = ++listenEpoch;
      listening = bounded(onTranslationEvent(receive).then(dispose => {
        if (disposed || epoch !== listenEpoch) dispose(); else unlisten = dispose;
      })).catch(error => { if (epoch === listenEpoch) { listenEpoch++; listening = null; } throw error; });
    }
    return listening;
  };
  function accept(reply: TranslationReply, initial: TranslationInitialViewport | null) {
    if (reply.reader.id !== reader.id) throw new Error(i18n.t("translation.articleMismatch", { ns: "reader" }));
    if (reply.reader.source_hash !== reader.source_hash) { sourceChanged(reply.reader); return false; }
    reconcileAfter = Math.max(1000, reply.reconcile_after_ms);
    note = reply.notice ? formatError(reply.notice) : ""; issue = reply.kind === "missing" && !!note; failure = null;
    if (reply.snapshot) {
      if (data && sameTranslation(data.snapshot.identity, reply.snapshot.identity) && reply.snapshot.seq < data.snapshot.seq) throw new Error(i18n.t("translation.snapshotStale", { ns: "reader" }));
      data = restoreTranslation(reply.reader, reply.snapshot, data);
    } else { data = null; sourceChanged(reply.reader); }
    disconnected = false;
    viewport.accept(data?.snapshot.identity ?? null, initial);
    let gap = overflow;
    if (data) for (const event of buffer.sort((a, b) => a.seq - b.seq)) {
      if (!sameTranslation(data.snapshot.identity, event.identity)) continue;
      try { data = applyTranslationEvent(data, event); } catch { gap = true; break; }
    }
    clearBuffer();
    return gap;
  }
  async function request(kind: "recover" | "start" | "save", mode: TranslationStartMode = "default") {
    if (disposed || busy || (kind === "start" && !disconnected && translationActive(data))) return;
    busy = true;
    viewport.schedule(false);
    note = ""; clearTimeout(timer); publish();
    let again = false, settled = false;
    try {
      await listen();
      if (disposed) return;
      // 注册成功后才发命令；重新连接后仍然只读，不把恢复变成启动。
      const generation = data?.snapshot.identity.generation ?? null;
      const initial = viewport.startInput(reader.source_hash);
      const operation = kind === "start" ? api.translate(reader.id, mode, initial)
        : kind === "save" && generation !== null ? api.translateRetrySave(reader.id, generation)
        : api.translateSnapshot(reader.id, kind === "recover" && data?.snapshot.phase !== "invalidated" ? generation : null);
      const reply = await bounded(operation.then(value => {
        const generation = value.snapshot?.identity.generation;
        // 初始视口可能已经附着；即使本地等待超时，也要收尾迟到的启动响应。
        if (kind === "start" && initial && generation != null && (disposed || settled)) viewport.releaseLate(generation);
        return value;
      }));
      if (disposed) return;
      if (kind === "start") autoBilingual = true;
      again = accept(reply, kind === "start" ? initial : null);
      if (again && kind === "recover") { fail(new Error(i18n.t("translation.eventGap", { ns: "reader" })), true); again = false; }
    } catch (error) {
      if (!disposed) fail(error, kind !== "start" || !unlisten || error instanceof ConnectionTimeout);
    } finally {
      settled = true;
      if (!disposed) { busy = false; publish(); schedule(); }
    }
    if (again && !disposed) await request("recover");
  }
  const recover = () => request("recover");
  const onFocus = () => { if (!disconnected) void recover(); };
  window.addEventListener("focus", onFocus);
  void recover();
  return {
    start: (mode: TranslationStartMode = "default") => request("start", mode),
    setViewport: viewport.setViewport,
    recover,
    retrySave: () => request("save"),
    dispose() { disposed = true; clearTimeout(timer); viewport.dispose(); clearBuffer(); unlisten?.(); window.removeEventListener("focus", onFocus); },
  };
}
