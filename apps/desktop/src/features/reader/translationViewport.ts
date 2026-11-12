import type { TranslationIdentity, TranslationInitialViewport, TranslationViewport } from "../../bindings";
import { api } from "../../lib/api";

export const VIEWPORT_DEBOUNCE_MS = 180, VIEWPORT_MAX_WAIT_MS = 600;

/** 一份阅读票据的附着与视口发送；不拥有翻译结果、事件缓冲或恢复请求。 */
export function createTranslationViewport(ticket: number, bounded: (request: Promise<void>) => Promise<void>) {
  let identity: TranslationIdentity | null = null;
  let viewport: TranslationViewport | null = null, viewportKey = "", intent = 0;
  let lastSent: string | null = null, dirtySince: number | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let enabled = false, disposed = false, ready = false, sending = false, due = false, blocked = false;
  let attaching: number | null = null;
  // 启动超时后迟到的代际：本票据仍在读这篇，保持附着，关闭时再释放。
  const late = new Set<number>();

  const canSend = () => enabled && !disposed && !blocked && ready && viewport && identity?.generation != null;
  const send = () => {
    if (!canSend() || sending || !identity || !viewport || lastSent === viewportKey) return;
    const sentIdentity = identity;
    sending = true;
    due = false;
    dirtySince = undefined;
    lastSent = viewportKey;
    void bounded(api.translateHint({ identity: sentIdentity, ticket, intent, viewport })).catch(() => {
      // 通知失败不改变译文；只读恢复成功后再发，避免积累重试请求。
      if (identity?.generation === sentIdentity.generation) { blocked = true; lastSent = null; }
    }).finally(() => {
      sending = false;
      if (due) send();
    });
  };
  const schedule = () => {
    clearTimeout(timer);
    if (!canSend() || lastSent === viewportKey) { dirtySince = undefined; return; }
    dirtySince ??= Date.now();
    const delay = Math.min(VIEWPORT_DEBOUNCE_MS, Math.max(0, VIEWPORT_MAX_WAIT_MS - (Date.now() - dirtySince)));
    timer = setTimeout(() => { due = true; send(); }, delay);
  };
  const detach = (generation: number) => { void api.translateDetach(generation, ticket).catch(() => {}); };

  return {
    setViewport(value: TranslationViewport) {
      if (disposed) return;
      const visible = [...new Set(value.visible)].sort((a, b) => a - b);
      const nearby = [...new Set(value.nearby)].filter(id => !visible.includes(id)).sort((a, b) => a - b);
      const next = { visible, nearby }, key = JSON.stringify(next);
      if (key === viewportKey) return;
      viewport = next;
      viewportKey = key;
      intent++;
      due = false;
      schedule();
    },
    startInput(sourceHash: string): TranslationInitialViewport | null {
      return viewport ? { source_hash: sourceHash, ticket, intent, viewport } : null;
    },
    /** 接受权威快照；只有恢复到新代际或上次附着失败时才重新附着。 */
    accept(next: TranslationIdentity | null, initial: TranslationInitialViewport | null) {
      const previous = identity?.generation ?? null, generation = next?.generation ?? null;
      identity = next;
      blocked = false;
      if (generation !== null) late.delete(generation);
      if (previous !== null && previous !== generation) detach(previous);
      if (generation !== previous) { ready = false; lastSent = null; }
      if (initial && generation !== null) {
        // 启动已用本票据附着并提交初始视口；只有几何变化后才需要追加 hint。
        ready = true;
        lastSent = JSON.stringify(initial.viewport);
        return;
      }
      if (generation !== null && !ready && attaching !== generation) {
        lastSent = null;
        attaching = generation;
        void bounded(api.translateAttach(generation, ticket).then(() => {
          if (disposed || identity?.generation !== generation) detach(generation);
          else { ready = true; schedule(); }
        })).catch(() => {}).finally(() => { if (attaching === generation) attaching = null; });
      }
    },
    schedule(active: boolean) { enabled = active; schedule(); },
    /** 启动响应在本地超时后才到：已关闭就释放；仍在读这篇则保留附着，恢复后可继续发 hint。 */
    releaseLate(generation: number) {
      if (disposed) detach(generation);
      else if (identity?.generation !== generation) late.add(generation);
    },
    dispose() {
      disposed = true;
      clearTimeout(timer);
      if (identity?.generation != null) late.add(identity.generation);
      for (const generation of late) detach(generation);
      late.clear();
    },
  };
}
