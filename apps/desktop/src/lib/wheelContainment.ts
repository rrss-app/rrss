import { reducedMotion } from "./motion";

/**
 * 全局滚轮规则（取代各处的 `overscroll-behavior: contain`）：
 * - 只有真有内容可滚的盒子才接住滚轮；滚到它的边界就停在这里，不带动外层。
 * - 下一格会越过边界时平滑停到边界（最后一格不跳）；减少动态效果时直接到边界。
 * - `overflow: auto` 但内容没超出的盒子（短代码块、短列表）不拦，交给外层继续滚。
 * - 标了 `data-scroll-boundary` 的浮层（下拉菜单、设置面板）：里面没有可滚的盒子时，滚动也不穿到后面。
 * 缩放手势（Ctrl + 滚轮）和不可取消的事件不管。
 */
export function containWheel(event: WheelEvent) {
  if (event.defaultPrevented || event.ctrlKey || !event.cancelable) return;
  const horizontal = event.shiftKey || Math.abs(event.deltaX) > Math.abs(event.deltaY);
  const delta = horizontal ? event.deltaX || event.deltaY : event.deltaY;
  if (!delta) return;
  let target = event.target instanceof Element ? event.target : null;
  while (target) {
    if (target instanceof HTMLElement) {
      const size = horizontal ? target.clientWidth : target.clientHeight;
      const end = Math.max(0, (horizontal ? target.scrollWidth : target.scrollHeight) - size);
      // 先用尺寸筛掉绝大多数节点，只对真能滚的再读计算样式。
      if (end > 1) {
        const style = getComputedStyle(target);
        const overflow = horizontal ? style.overflowX : style.overflowY;
        if (overflow === "auto" || overflow === "scroll") {
          const position = horizontal ? target.scrollLeft : target.scrollTop;
          const unit = event.deltaMode === 1 ? parseFloat(style.lineHeight) || 16 : event.deltaMode === 2 ? size : 1;
          const next = position + delta * unit;
          if (next <= 0 || next >= end) {
            event.preventDefault();
            const edge = next <= 0 ? 0 : end;
            if (Math.abs(position - edge) >= 1) scrollToEdge(target, horizontal, edge);
          }
          return;
        }
      }
      if (target.hasAttribute("data-scroll-boundary")) {
        event.preventDefault();
        return;
      }
    }
    target = target.parentElement;
  }
}

function scrollToEdge(target: HTMLElement, horizontal: boolean, edge: number) {
  if (typeof target.scrollTo === "function" && !reducedMotion()) {
    target.scrollTo(horizontal ? { left: edge, behavior: "smooth" } : { top: edge, behavior: "smooth" });
  } else if (horizontal) target.scrollLeft = edge;
  else target.scrollTop = edge;
}

/** 应用启动时装一次。 */
export function installWheelContainment(scope: Window = window) {
  scope.addEventListener("wheel", containWheel, { passive: false });
  return () => scope.removeEventListener("wheel", containWheel);
}
