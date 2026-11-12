import { useLayoutEffect, useRef, type RefObject } from "react";

/**
 * 主栏目隐藏后再显示时保住其中滚动容器的位置。
 *
 * `display: none` 期间滚动容器读到的 `scrollTop` 是 0，也可能收到迟到的滚动事件；这里只在可见时记录位置，
 * 重新显示时若宿主没有恢复就写回，并补发一次 `scroll`，让虚拟列表和续页检查按真实位置重新计算。
 */
export function useRetainedScroll(root: RefObject<HTMLElement | null>, visible: boolean) {
  const saved = useRef(new Map<HTMLElement, number>());
  const visibleRef = useRef(visible);
  const wasVisible = useRef(visible);

  useLayoutEffect(() => {
    const el = root.current;
    if (!el) return;
    const record = (event: Event) => {
      const target = event.target;
      if (!visibleRef.current || !(target instanceof HTMLElement) || !el.contains(target)) return;
      saved.current.set(target, target.scrollTop);
    };
    el.addEventListener("scroll", record, { capture: true, passive: true });
    return () => el.removeEventListener("scroll", record, { capture: true });
  }, [root]);

  useLayoutEffect(() => {
    visibleRef.current = visible;
    const shown = visible && !wasVisible.current;
    wasVisible.current = visible;
    if (!shown) return;
    for (const [target, top] of saved.current) {
      if (!target.isConnected) {
        saved.current.delete(target);
        continue;
      }
      if (Math.abs(target.scrollTop - top) >= 1) target.scrollTop = top;
      target.dispatchEvent(new Event("scroll"));
    }
  }, [visible]);
}
