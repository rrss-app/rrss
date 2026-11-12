import { useCallback, useLayoutEffect, useRef, type RefObject } from "react";
import type { SignalSurface } from "../bindings";
import { trackExposure } from "./signals";
import { createExposureGate } from "./exposureGate";

export type ExposureObserver = (element: HTMLElement | null, articleId: number, position: number, spotCheck?: boolean) => void;

export function useExposureTracker({ surface, root, active }: {
  surface: SignalSurface;
  root: RefObject<HTMLElement | null>;
  active: boolean;
}): ExposureObserver {
  const elements = useRef(new Map<number, { element: HTMLElement; position: number; spotCheck: boolean }>());
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
    const gate = createExposureGate({ onExpose: (id, position, shownAt) => trackExposure(id, surface, position, shownAt, elements.current.get(id)?.spotCheck) });
    const targets = new WeakMap<Element, number>();
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
      const deadline = gate.nextDeadline();
      if (deadline === null) return;
      timer.current = setTimeout(() => {
        timer.current = null;
        const now = Date.now();
        // 先处理尚未回调的滑出记录，防止在阈值边界上误记曝光。
        update(observer.takeRecords(), now);
        gate.tick(now);
        schedule();
      }, Math.max(0, deadline - Date.now()));
    };
    const observer = new IntersectionObserver(entries => {
      update(entries, Date.now());
      schedule();
    }, { root: root.current, threshold: [0, 0.5, 1] });
    const syncActive = () => {
      gate.setActive(activeRef.current && document.visibilityState === "visible", Date.now());
      schedule();
    };
    // 登记目标也用于过滤虚拟行卸载后迟到的观察记录。
    const observeTarget = (element: HTMLElement, articleId: number) => {
      targets.set(element, articleId);
      observer.observe(element);
    };
    runtime.current = { observer, gate, observeTarget, schedule, syncActive };
    syncActive();
    for (const [id, row] of elements.current) observeTarget(row.element, id);
    document.addEventListener("visibilitychange", syncActive);
    return () => {
      document.removeEventListener("visibilitychange", syncActive);
      observer.disconnect();
      cancelTimer();
      runtime.current = null;
    };
  }, [root, surface]);

  useLayoutEffect(() => {
    activeRef.current = active;
    runtime.current?.syncActive();
  }, [active]);

  return useCallback((element: HTMLElement | null, articleId: number, position: number, spotCheck = false) => {
    if (typeof IntersectionObserver === "undefined") return;
    const previous = elements.current.get(articleId);
    const current = runtime.current;
    if (previous?.element === element && previous.position === position && previous.spotCheck === spotCheck) return;
    if (previous) {
      current?.observer.unobserve(previous.element);
      current?.gate.forget(articleId);
      elements.current.delete(articleId);
    }
    if (element) {
      elements.current.set(articleId, { element, position, spotCheck });
      current?.observeTarget(element, articleId);
    }
    current?.schedule();
  }, []);
}
