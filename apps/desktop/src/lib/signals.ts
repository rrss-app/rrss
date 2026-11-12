/**
 * 推荐评测的原始日志（阶段 23「数据底座 E0」）：曝光、行为与阅读会话先进内存队列，再批量写入。
 *
 * - 只有 `startSignals()`（`main.tsx`）之后才会写入；组件测试不启动，入队的记录留在有上限的缓冲里。
 * - 日志尽力而为：写入失败丢弃本批并只告警一次，不打断界面。
 * - 曝光先写：拿到 ID 后，同一批里的事件与会话按文章补上 `exposure_id`。
 */
import { api, openExternal } from "./api";
import { reportClientError } from "./clientLog";
import type { ReadSessionInput, SignalEventKind, SignalSurface } from "../bindings";

const FLUSH_MS = 5_000;
const FLUSH_AT = 50;
const BUFFER_LIMIT = 500;
const REGISTRY_LIMIT = 2_000;

type PendingExposure = { article_id: number; surface: SignalSurface; position: number; shown_at: number; variant: "baseline" | "spot_check" };
type PendingEvent = { article_id: number; kind: SignalEventKind; at: number };
export type ReadSessionRecord = Omit<ReadSessionInput, "exposure_id">;

function newSessionId(): string {
  return typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

const sessionId = newSessionId();
let exposures: PendingExposure[] = [];
let events: PendingEvent[] = [];
let sessions: ReadSessionRecord[] = [];
/** 本次启动内已曝光的文章 → 曝光 ID（写入前为 null）。同一篇只记第一次。 */
const exposed = new Map<number, number | null>();
let timer: ReturnType<typeof setTimeout> | null = null;
let started = false;
let flushing: Promise<void> | null = null;
let warned = false;

function push<T>(queue: T[], item: T): void {
  queue.push(item);
  if (queue.length > BUFFER_LIMIT) queue.splice(0, queue.length - BUFFER_LIMIT);
  schedule();
}

function schedule(): void {
  if (!started) return;
  if (exposures.length + events.length + sessions.length >= FLUSH_AT) {
    void flushSignals();
    return;
  }
  timer ??= setTimeout(() => {
    timer = null;
    void flushSignals();
  }, FLUSH_MS);
}

function remember(articleId: number, id: number | null): void {
  exposed.delete(articleId);
  exposed.set(articleId, id);
  if (exposed.size > REGISTRY_LIMIT) exposed.delete(exposed.keys().next().value as number);
}

/** 卡片按规格判定可见（≥50% 且 ≥1 秒）后调用；本次启动内同一篇只记第一次。 */
export function trackExposure(articleId: number, surface: SignalSurface, position: number, shownAt = Date.now(), spotCheck = false): void {
  if (exposed.has(articleId)) return;
  remember(articleId, null);
  push(exposures, { article_id: articleId, surface, position, shown_at: shownAt, variant: spotCheck ? "spot_check" : surface === "feeds" ? "time" : "baseline" });
}

/** 用户动作成功后调用（打开、收藏、赞踩、引用、提问、打开原文）。 */
export function trackEvent(articleId: number, kind: SignalEventKind, at = Date.now()): void {
  push(events, { article_id: articleId, kind, at });
}

/** 打开原文：记一次 `open_original` 再交给系统浏览器。 */
export function openOriginal(articleId: number, url: string): Promise<void> {
  trackEvent(articleId, "open_original");
  return openExternal(url);
}

/** 阅读会话结束时调用一次；`exposure_id` 在写入时按文章补上。 */
export function trackReadSession(session: ReadSessionRecord): void {
  push(sessions, session);
}

/** 写出当前队列；并发调用共用同一次写入。 */
export function flushSignals(): Promise<void> {
  if (flushing) return flushing;
  if (timer) {
    clearTimeout(timer);
    timer = null;
  }
  const batch = { exposures, events, sessions };
  exposures = [];
  events = [];
  sessions = [];
  if (!batch.exposures.length && !batch.events.length && !batch.sessions.length) return Promise.resolve();
  flushing = write(batch).finally(() => {
    flushing = null;
    if (exposures.length + events.length + sessions.length) schedule();
  });
  return flushing;
}

async function write(batch: { exposures: PendingExposure[]; events: PendingEvent[]; sessions: ReadSessionRecord[] }): Promise<void> {
  try {
    if (batch.exposures.length) {
      const ids = await api.recordExposures(batch.exposures.map((row) => ({ ...row, session_id: sessionId })));
      batch.exposures.forEach((row, i) => {
        if (exposed.has(row.article_id)) remember(row.article_id, ids[i] ?? null);
      });
    }
    const exposureOf = (articleId: number) => exposed.get(articleId) ?? null;
    if (batch.events.length) {
      await api.recordArticleEvents(batch.events.map((row) => ({ ...row, exposure_id: exposureOf(row.article_id) })));
    }
    if (batch.sessions.length) {
      await api.recordReadSessions(batch.sessions.map((row) => ({ ...row, exposure_id: exposureOf(row.article_id) })));
    }
  } catch (err) {
    if (!warned) {
      console.warn("signals: dropped a batch", err);
      reportClientError("signals", new Error("signals batch dropped"), "warn");
    }
    warned = true;
  }
}

/** 开始定时写入，并在页面隐藏时尽量写出；返回停止函数。只在应用入口调用。 */
export function startSignals(): () => void {
  started = true;
  const onHidden = () => {
    if (document.visibilityState === "hidden") void flushSignals();
  };
  const onPageHide = () => void flushSignals();
  document.addEventListener("visibilitychange", onHidden);
  window.addEventListener("pagehide", onPageHide);
  schedule();
  return () => {
    started = false;
    document.removeEventListener("visibilitychange", onHidden);
    window.removeEventListener("pagehide", onPageHide);
    if (timer) clearTimeout(timer);
    timer = null;
  };
}
