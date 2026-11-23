import { useCallback, useLayoutEffect, useRef, type RefObject } from "react";
import type { SignalSurface } from "../bindings";
import { currentExposureSession, trackExposure } from "./signals";
import { createExposureGate } from "./exposureGate";

export type ExposureObserver = (element: HTMLElement | null, articleId: number, position: number, spotCheck?: boolean, explore?: boolean) => void;

export function useExposureTracker({ surface, root, active }: {
  surface: SignalSurface;
  root: RefObject<HTMLElement | null>;
  active: boolean;
}): ExposureObserver {
  const elements = useRef(new Map<number, { element: HTMLElement; position: number; spotCheck: boolean; explore: boolean }>());
  const activeRef = useRef(active);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const runtime = useRef<{
    observer: IntersectionObserver;
    gate: ReturnType<typeof createExposureGate>;
    observeTarget: (element: HTMLElement, articleId: number) => void;
    schedule: () => void;
    syncActive: () => void;
  } | null>(null);

  useLayoutEffect(() => {
    if (typeof IntersectionObserver === "undefined" || !root.current) return;
    const gate = createExposureGate({ onExpose: (id, position, shownAt) => trackExposure(id, surface, position, shownAt, elements.current.get(id)?.spotCheck, elements.current.get(id)?.explore) });
    const targets = new WeakMap<Element, number>();
    let sessionId: string | null = null;
    let sessionDay = Math.floor(Date.now() / 86_400_000);
    const interactive = () => activeRef.current && document.visibilityState === "visible";
    const update = (entries: IntersectionObserverEntry[], now: number) => {
      for (const entry of entries) {
        const id = targets.get(entry.target);
        const row = id === undefined ? undefined : elements.current.get(id);
        if (id !== undefined && row?.element === entry.target) gate.update(id, row.position, entry.intersectionRatio, now);
      }
    };
    const cancelTimer = () => {
      if (timer.current !== null) clearTimeout(timer.current);
      timer.current = null;
    };
    const schedule = () => {
      cancelTimer();
      const now = Date.now();
      const midnight = interactive() && elements.current.size ? (Math.floor(now / 86_400_000) + 1) * 86_400_000 : null;
      const deadline = Math.min(gate.nextDeadline() ?? Infinity, midnight ?? Infinity);
      if (!Number.isFinite(deadline)) return;
      timer.current = setTimeout(() => {
        timer.current = null;
        const now = Date.now();
        gate.setActive(interactive(), now);
        // 先处理尚未回调的滑出记录，防止在阈值边界上误记曝光。
        if (!syncDay(now)) update(observer.takeRecords(), now);
        gate.tick(now);
        schedule();
      }, Math.max(0, deadline - Date.now()));
    };
    const observer = new IntersectionObserver(entries => {
      const now = Date.now();
      gate.setActive(interactive(), now);
      if (!syncDay(now)) update(entries, now);
      schedule();
    }, { root: root.current, threshold: [0, 0.5, 1] });
    const syncActive = () => {
      const now = Date.now();
      gate.setActive(interactive(), now);
      syncDay(now);
      schedule();
    };
    // 登记目标也用于过滤虚拟行卸载后迟到的观察记录。
    const observeTarget = (element: HTMLElement, articleId: number) => {
      targets.set(element, articleId);
      observer.observe(element);
    };
    const syncDay = (now: number) => {
      if (!interactive()) return false;
      const next = currentExposureSession(now);
      if (next === sessionId) return false;
      const day = Math.floor(now / 86_400_000);
      const reset = sessionId !== null || day !== sessionDay;
      sessionId = next;
      sessionDay = day;
      if (reset) {
        gate.reset();
        observer.disconnect();
        for (const [id, row] of elements.current) observeTarget(row.element, id);
      }
      return reset;
    };
    runtime.current = { observer, gate, observeTarget, schedule, syncActive };
    syncActive();
    for (const [id, row] of elements.current) observeTarget(row.element, id);
    document.addEventListener("visibilitychange", syncActive);
    window.addEventListener("focus", syncActive);
    return () => {
      document.removeEventListener("visibilitychange", syncActive);
      window.removeEventListener("focus", syncActive);
      observer.disconnect();
      cancelTimer();
      runtime.current = null;
    };
  }, [root, surface]);

  useLayoutEffect(() => {
    activeRef.current = active;
    runtime.current?.syncActive();
  }, [active]);

  return useCallback((element: HTMLElement | null, articleId: number, position: number, spotCheck = false, explore = false) => {
    if (typeof IntersectionObserver === "undefined") return;
    const previous = elements.current.get(articleId);
    const current = runtime.current;
    if (previous?.element === element && previous.position === position && previous.spotCheck === spotCheck && previous.explore === explore) return;
    if (previous) {
      current?.observer.unobserve(previous.element);
      current?.gate.forget(articleId);
      elements.current.delete(articleId);
    }
    if (element) {
      elements.current.set(articleId, { element, position, spotCheck, explore });
      current?.observeTarget(element, articleId);
    }
    current?.schedule();
  }, []);
}
