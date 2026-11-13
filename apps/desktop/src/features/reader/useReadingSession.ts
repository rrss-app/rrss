import { useCallback, useEffect, useRef, useState } from "react";
import type { ReadSessionEndedBy, ReadSessionVia, ReadingSpeedsView } from "../../bindings";
import { api, type ReaderView } from "../../lib/api";
import { parseDocument } from "../../lib/blockIr";
import { trackReadSession } from "../../lib/signals";
import type { ReaderOpenOptions } from "./ReaderHost";
import type { ReaderTicket } from "./readerRequests";
import { createReadingSession, readingBlocks } from "./readingSession";
import { observeReaderViewport } from "./readerViewport";

/** 搜索优先；抽屉切换显式覆盖原会话的 returnToList。列表打开记为列表所属的来源（焦点或订阅）。 */
export function inferReadingVia(options?: ReaderOpenOptions, list: ReadSessionVia = "focus"): ReadSessionVia {
  if (options?.search) return "search";
  if (options?.citationSource || options?.anchor) return "citation";
  if (options?.via === "related") return "related";
  if (options?.via === "drawer") return "drawer";
  if (!options?.skipNav && options?.returnToList) return list;
  return options?.skipNav ? "digest" : "drawer";
}

const FOCUS_TIMEOUT = 10 * 60_000;
// 阅读器首次挂载预取；包含失败结果，整个前端生命周期只请求一次。
let speedsRequest: Promise<ReadingSpeedsView | null> | undefined;
let cachedSpeeds: ReadingSpeedsView | null | undefined;
type Options = {
  open: boolean; dismissible: boolean; inert?: boolean;
  view: ReaderView | null; ticket: ReaderTicket | null; openedAt: number | null;
  via: ReadSessionVia; translated: boolean;
};
type Runtime = {
  options: Options;
  current: { articleId: number; ticket: ReaderTicket; model: ReturnType<typeof createReadingSession>; lastBlock?: number } | null;
  completedTicket: ReaderTicket | null;
  viewport: ReturnType<typeof observeReaderViewport> | null;
  visible: number[]; disposeBody: (() => void) | null;
  active: boolean; blurredAt: number | null; focusTimer: ReturnType<typeof setTimeout> | null;
  playing: Set<HTMLMediaElement>; mediaTimer: ReturnType<typeof setInterval> | null;
  mounted: boolean;
};

function finish(runtime: Runtime, now: number, endedBy: ReadSessionEndedBy) {
  const current = runtime.current;
  if (!current) return;
  runtime.current = null;
  runtime.completedTicket = current.ticket;
  trackReadSession(current.model.finish(now, endedBy));
  if (runtime.focusTimer) clearTimeout(runtime.focusTimer);
  runtime.focusTimer = null;
}

function syncActive(runtime: Runtime, now: number) {
  const foreground = document.hasFocus() && document.visibilityState === "visible";
  // 后台计时器可能被暂停；恢复事件也先检查已经过去的期限。
  if (runtime.blurredAt !== null && now - runtime.blurredAt >= FOCUS_TIMEOUT) {
    finish(runtime, runtime.blurredAt + FOCUS_TIMEOUT, "focus_timeout");
  }
  if (foreground) {
    runtime.blurredAt = null;
    if (runtime.focusTimer) clearTimeout(runtime.focusTimer);
    runtime.focusTimer = null;
  } else {
    runtime.blurredAt ??= now;
    if (runtime.current && !runtime.focusTimer) {
      runtime.focusTimer = setTimeout(() => {
        runtime.focusTimer = null;
        syncActive(runtime, Date.now());
      }, Math.max(0, runtime.blurredAt + FOCUS_TIMEOUT - now));
    }
  }
  const { open, dismissible, inert, view } = runtime.options;
  const active = foreground && open && dismissible && !inert && runtime.current?.articleId === view?.id;
  runtime.current?.model.setActive(active, now);
  if (active && !runtime.active) runtime.current?.model.activity(now);
  runtime.active = active;
  if (active && runtime.current?.lastBlock !== undefined && runtime.visible.includes(runtime.current.lastBlock)) {
    runtime.current.model.markEnd();
  }
  return active;
}

function activity(runtime: Runtime, now = Date.now()) {
  if (syncActive(runtime, now)) runtime.current?.model.activity(now);
}

/** 聚合状态只存在 ref 中；正文绑定独立于翻译，预览和未确认的缓存不启动会话。 */
export function useReadingSession(options: Options) {
  const [speeds, setSpeeds] = useState<ReadingSpeedsView | null | undefined>(() => cachedSpeeds);
  useEffect(() => {
    let mounted = true;
    speedsRequest ??= api.readingSpeeds().catch(() => null).then(value => { cachedSpeeds = value; return value; });
    void speedsRequest.then(value => { if (mounted) setSpeeds(value); });
    return () => { mounted = false; };
  }, []);
  const runtime = useRef<Runtime>({
    options, current: null, completedTicket: null, viewport: null, visible: [], disposeBody: null,
    active: false, blurredAt: null, focusTimer: null, playing: new Set(), mediaTimer: null, mounted: false,
  });
  runtime.current.options = options;
  const { open, view, ticket, openedAt, via, translated, dismissible, inert } = options;

  const onBody = useCallback((body: HTMLElement | null) => {
    const state = runtime.current;
    state.disposeBody?.();
    state.visible = [];
    state.current?.model.setVisibleBlocks([], Date.now());
    state.disposeBody = null;
    if (!body) return;
    const onActivity = () => activity(state);
    const setPlaying = (media: HTMLMediaElement, playing: boolean) => {
      activity(state);
      if (playing) state.playing.add(media);
      else state.playing.delete(media);
      if (state.playing.size && !state.mediaTimer) {
        state.mediaTimer = setInterval(() => {
          for (const media of state.playing) if (!body.contains(media) || media.paused || media.ended) state.playing.delete(media);
          if (state.playing.size) activity(state);
          else if (state.mediaTimer) { clearInterval(state.mediaTimer); state.mediaTimer = null; }
        }, 5_000);
      } else if (!state.playing.size && state.mediaTimer) {
        clearInterval(state.mediaTimer); state.mediaTimer = null;
      }
    };
    const mediaChanged = (event: Event) => {
      if (event.target instanceof HTMLMediaElement) setPlaying(event.target, event.type === "playing");
    };
    for (const media of body.querySelectorAll<HTMLMediaElement>("audio, video")) {
      if (!media.paused && !media.ended) setPlaying(media, true);
    }
    body.addEventListener("scroll", onActivity, { passive: true });
    body.addEventListener("wheel", onActivity, { passive: true });
    body.addEventListener("timeupdate", onActivity, true);
    for (const event of ["playing", "pause", "ended", "emptied"]) body.addEventListener(event, mediaChanged, true);
    const observer = observeReaderViewport(body, ({ visible }) => {
      state.visible = visible;
      const now = Date.now();
      syncActive(state, now);
      if (state.current?.articleId === state.options.view?.id) state.current?.model.setVisibleBlocks(visible, now);
    });
    state.viewport = observer;
    state.disposeBody = () => {
      observer.dispose(); state.viewport = null;
      body.removeEventListener("scroll", onActivity); body.removeEventListener("wheel", onActivity);
      body.removeEventListener("timeupdate", onActivity, true);
      for (const event of ["playing", "pause", "ended", "emptied"]) body.removeEventListener(event, mediaChanged, true);
      state.playing.clear();
      if (state.mediaTimer) clearInterval(state.mediaTimer);
      state.mediaTimer = null;
    };
  }, []);

  useEffect(() => {
    const state = runtime.current, now = Date.now();
    if (!open) finish(state, now, "back");
    else if (state.current && state.current.articleId !== view?.id) finish(state, now, "next");
    if (open && view && ticket && openedAt !== null && !view.source_hash?.startsWith("preview-") && state.completedTicket !== ticket) {
      if (!state.current) {
        const blocks = readingBlocks(parseDocument(view.document).blocks);
        const mode = translated ? "translated" : "original";
        const charsPerMs = speeds ? {
          chinese: speeds[`chinese_${mode}`] ?? undefined,
          english: speeds[`english_${mode}`] ?? undefined,
        } : undefined;
        state.current = { articleId: view.id, ticket, lastBlock: blocks.at(-1)?.id,
          model: createReadingSession({ articleId: view.id, openedAt, via, blocks, translated, charsPerMs }) };
        state.active = false;
        state.blurredAt = null;
      } else state.current.ticket = ticket;
    }
    syncActive(state, now);
    state.current?.model.setTranslated(translated, now);
    state.viewport?.read();
  }, [open, view, ticket, openedAt, via, translated, dismissible, inert, speeds]);

  useEffect(() => {
    const state = runtime.current;
    state.mounted = true;
    let movedAt = -Infinity;
    const onActivity = () => activity(state);
    const onPointer = () => {
      const now = Date.now();
      if (now - movedAt < 1_000) return;
      movedAt = now; activity(state, now);
    };
    const onFocus = () => syncActive(state, Date.now());
    const onHide = () => finish(state, Date.now(), "close");
    window.addEventListener("keydown", onActivity, true);
    window.addEventListener("pointermove", onPointer, { passive: true });
    document.addEventListener("selectionchange", onActivity);
    window.addEventListener("focus", onFocus); window.addEventListener("blur", onFocus);
    document.addEventListener("visibilitychange", onFocus);
    // 在 startSignals 的冒泡监听器写出队列之前入队。
    window.addEventListener("pagehide", onHide, true);
    return () => {
      state.mounted = false;
      window.removeEventListener("keydown", onActivity, true); window.removeEventListener("pointermove", onPointer);
      document.removeEventListener("selectionchange", onActivity);
      window.removeEventListener("focus", onFocus); window.removeEventListener("blur", onFocus);
      document.removeEventListener("visibilitychange", onFocus); window.removeEventListener("pagehide", onHide, true);
      if (state.focusTimer) clearTimeout(state.focusTimer);
      state.focusTimer = null;
      // StrictMode 的同次提交重挂不应写出一个虚假的关闭会话。
      queueMicrotask(() => {
        if (state.mounted) return;
        finish(state, Date.now(), "close");
        state.disposeBody?.();
        state.disposeBody = null;
      });
    };
  }, []);

  return { onBody, finish: useCallback((endedBy: ReadSessionEndedBy) => finish(runtime.current, Date.now(), endedBy), []) };
}
