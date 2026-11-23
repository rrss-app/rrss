/**
 * 推荐与阅读行为的原始信号日志：曝光、行为与阅读会话先进内存队列，再批量写入。
 *
 * - 只有 `startSignals()`（`main.tsx`）之后才会写入；组件测试不启动，入队的记录留在有上限的缓冲里。
 * - 日志尽力而为：写入失败丢弃本批并只告警一次，不打断界面。
 * - 曝光先写：拿到 ID 后，事件与会话按入队时选定的曝光补上 `exposure_id`。
 */
import { api, openExternal } from "./api";
import { reportClientError } from "./clientLog";
import type { ExposureInput, ReadSessionInput, SignalEventKind, SignalSurface } from "../bindings";

const FLUSH_MS = 5_000;
const FLUSH_AT = 50;
const BUFFER_LIMIT = 500;
const REGISTRY_LIMIT = 2_000;

type ExposureLink = { articleId: number; shownAt: number; id: number | null };
type Pending<T> = { row: T; exposure: ExposureLink | null };
type PendingExposure = { row: ExposureInput; exposure: ExposureLink };
type PendingEvent = Pending<{ article_id: number; kind: SignalEventKind; at: number }>;
export type ReadSessionRecord = Omit<ReadSessionInput, "exposure_id">;

function newSessionId(): string {
  return typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

let sessionId = newSessionId();
let sessionDay = Math.floor(Date.now() / 86_400_000);
let exposures: PendingExposure[] = [];
let events: PendingEvent[] = [];
let sessions: Pending<ReadSessionRecord>[] = [];
/** 当日去重与有限的最近曝光分开；跨日阅读仍可关联打开之前的曝光。 */
const exposed = new Set<number>();
const recent: ExposureLink[] = [];
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

/** 只由可交互且文档可见的曝光观察器调用。 */
export function currentExposureSession(now = Date.now()): string {
  const day = Math.floor(now / 86_400_000);
  if (day !== sessionDay) {
    sessionDay = day;
    sessionId = newSessionId();
    exposed.clear();
  }
  return sessionId;
}

function exposureBefore(articleId: number, at: number): ExposureLink | null {
  return recent.find(row => row.articleId === articleId && row.shownAt <= at) ?? null;
}

/** 卡片按规格判定可见（≥50% 且 ≥1 秒）后调用；同一 UTC 日同一篇只记第一次。 */
export function trackExposure(articleId: number, surface: SignalSurface, position: number, shownAt = Date.now(), spotCheck = false, explore = false): void {
  if (exposed.has(articleId)) return;
  exposed.add(articleId);
  if (exposed.size > REGISTRY_LIMIT) exposed.delete(exposed.values().next().value as number);
  const exposure: ExposureLink = { articleId, shownAt, id: null };
  recent.unshift(exposure);
  if (recent.length > REGISTRY_LIMIT) recent.pop();
  push(exposures, { exposure, row: { article_id: articleId, surface, position, shown_at: shownAt, session_id: sessionId,
    variant: explore ? "explore" : spotCheck ? "spot_check" : surface === "feeds" ? "time" : "baseline" } });
}

/** 用户动作成功后调用（打开、收藏、赞踩、引用、提问、打开原文）。 */
export function trackEvent(articleId: number, kind: SignalEventKind, at = Date.now()): void {
  push(events, { row: { article_id: articleId, kind, at }, exposure: exposureBefore(articleId, at) });
}

/** 打开原文：记一次 `open_original` 再交给系统浏览器。 */
export function openOriginal(articleId: number, url: string): Promise<void> {
  trackEvent(articleId, "open_original");
  return openExternal(url);
}

/** 阅读会话结束时调用一次；只关联打开之前的最近曝光，ID 可等待在途写入。 */
export function trackReadSession(session: ReadSessionRecord): void {
  push(sessions, { row: session, exposure: exposureBefore(session.article_id, session.opened_at) });
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

async function write(batch: { exposures: PendingExposure[]; events: PendingEvent[]; sessions: Pending<ReadSessionRecord>[] }): Promise<void> {
  try {
    if (batch.exposures.length) {
      const ids = await api.recordExposures(batch.exposures.map(({ row }) => row));
      batch.exposures.forEach(({ exposure }, i) => {
        exposure.id = ids[i] ?? null;
      });
    }
    if (batch.events.length) {
      await api.recordArticleEvents(batch.events.map(({ row, exposure }) => ({ ...row, exposure_id: exposure?.id ?? null })));
    }
    if (batch.sessions.length) {
      await api.recordReadSessions(batch.sessions.map(({ row, exposure }) => ({ ...row, exposure_id: exposure?.id ?? null })));
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
