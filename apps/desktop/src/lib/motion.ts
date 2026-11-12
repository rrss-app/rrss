import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";

/**
 * 动效的公共判断（规则见设计系统「动效」）：系统开了「减少动态效果」时一律直接到位，信息不靠动效传达。
 * 不支持 `matchMedia` 的环境（测试）按减少动效处理，结果与动画结束后一致。
 */
export function reducedMotion(): boolean {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return true;
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/** 与 `--ease` 相同的曲线，给 Web Animations 用。 */
export const EASE = "cubic-bezier(0.2, 0.75, 0.2, 1)";

/** 与 `--ease` 同形的缓出曲线，给脚本驱动的滚动用。 */
export function easeOut(t: number): number {
  return 1 - Math.pow(1 - Math.min(1, Math.max(0, t)), 3);
}

/**
 * 用 Web Animations 播一次进场（新内容淡入并上移），不改 DOM、不重新挂载；减少动效或不支持时什么都不做。
 * 只动透明度与位移，用独立的 `translate` 属性，不覆盖元素自己的 `transform`。
 */
export function playEnter(el: Element | null | undefined, { duration = 220, rise = 4 }: { duration?: number; rise?: number } = {}) {
  if (!el || typeof (el as HTMLElement).animate !== "function" || reducedMotion()) return;
  (el as HTMLElement).animate(
    [{ opacity: 0, translate: `0 ${rise}px` }, { opacity: 1, translate: "0 0" }],
    { duration, easing: EASE, id: ENTER_ID },
  );
}
const ENTER_ID = "rrss-enter";

/** 外层（例如整个主栏目）正在播进场：里面不再叠一层。 */
export function ancestorEntering(el: Element | null | undefined): boolean {
  for (let node = el?.parentElement; node; node = node.parentElement) {
    if (node.getAnimations?.().some((animation) => animation.id === ENTER_ID)) return true;
  }
  return false;
}

/**
 * 关闭后再留 `exitMs` 播退场，期间节点不卸下（同一个元素，不会出现两份）；重新打开时取消退场、沿用原节点。
 * 减少动效时立即卸下。
 */
export function usePresence(open: boolean, exitMs: number): { mounted: boolean; leaving: boolean } {
  const [shownOpen, setShownOpen] = useState(open);
  const [lingering, setLingering] = useState(false);
  if (shownOpen !== open) {
    setShownOpen(open);
    setLingering(!open && !reducedMotion());
  }
  useEffect(() => {
    if (!lingering) return;
    const timer = window.setTimeout(() => setLingering(false), exitMs);
    return () => window.clearTimeout(timer);
  }, [lingering, exitMs]);
  return { mounted: open || lingering, leaving: !open && lingering };
}

/** 同 `usePresence`，并在退场期间保留最后一个值（例如刚关掉的提示）。 */
export function useLingering<T>(value: T | null | undefined, exitMs: number): { value: T | null; leaving: boolean } {
  const [last, setLast] = useState<T | null>(value ?? null);
  if (value && value !== last) setLast(value);
  const { mounted, leaving } = usePresence(!!value, exitMs);
  return { value: mounted ? value ?? last : null, leaving };
}

export type RevealState = "open" | "opening" | "closing" | "closed";

/** 展开 / 收起的阶段：高度过渡走完才算「已展开」或「已收起」。减少动效时直接到位。 */
export function useReveal(open: boolean, { openMs = 220, closeMs = 160 } = {}): RevealState {
  const [shownOpen, setShownOpen] = useState(open);
  const [state, setState] = useState<RevealState>(open ? "open" : "closed");
  if (shownOpen !== open) {
    setShownOpen(open);
    setState(reducedMotion() ? (open ? "open" : "closed") : open ? "opening" : "closing");
  }
  useEffect(() => {
    if (state !== "opening" && state !== "closing") return;
    const timer = window.setTimeout(() => setState(state === "opening" ? "open" : "closed"), state === "opening" ? openMs : closeMs);
    return () => window.clearTimeout(timer);
  }, [state, openMs, closeMs]);
  return state;
}

/** 主栏目从隐藏切回时，新内容淡入并上移 4px；旧内容不做退场。首次挂载不播。 */
export function useViewEnter(root: RefObject<HTMLElement | null>, visible: boolean) {
  const wasVisible = useRef(visible);
  useLayoutEffect(() => {
    if (visible && !wasVisible.current) playEnter(root.current);
    wasVisible.current = visible;
  }, [root, visible]);
}

/**
 * 给即将被 React 卸下的浮层留一份只读的影子播退场（菜单收起 100ms）：真身照常卸下，焦点与监听不受影响；
 * 影子不可交互、对读屏隐藏，播完即删。开发模式下 StrictMode 模拟卸载时节点仍在，不留影子。
 */
export function exitGhost(node: HTMLElement | null, keyframes: Keyframe[], duration = 100) {
  if (!node || typeof node.animate !== "function" || reducedMotion()) return;
  const parent = node.parentNode;
  const next = node.nextSibling;
  const ghost = node.cloneNode(true) as HTMLElement;
  queueMicrotask(() => {
    if (node.isConnected || !parent?.isConnected) return;
    ghost.removeAttribute("id");
    ghost.setAttribute("aria-hidden", "true");
    ghost.inert = true;
    ghost.dataset.leaving = "";
    Object.assign(ghost.style, { pointerEvents: "none", animation: "none" });
    parent.insertBefore(ghost, next && next.parentNode === parent ? next : null);
    const remove = () => ghost.remove();
    const animation = ghost.animate(keyframes, { duration, easing: EASE, fill: "forwards" });
    animation.onfinish = remove;
    animation.oncancel = remove;
    window.setTimeout(remove, duration + 200);
  });
}
