import { useLayoutEffect, useRef, type KeyboardEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { exitGhost } from "../lib/motion";

/** 链接、图片、代码块和输入控件保留系统菜单。段落里的行内 `code` 不在此列；`pre` 内的 `code` 仍经 `pre` 命中。 */
export function keepsNativeContextMenu(target: EventTarget | null): boolean {
  return target instanceof Element && !!target.closest('a, img, pre, input, textarea, select, button, [contenteditable]:not([contenteditable="false"]), audio, video');
}

export type MenuAnchor = { x: number; y: number; selection?: DOMRect };

/** 阅读器浮层菜单：定位、键盘、打开时焦点与关闭后归还。简报和对话的划词条不走这里。 */
export function FloatingMenu({ anchor, boundary, className, label, onClose, onKeyDown, children, placement = "above", role = "menu", restoreFocusTo }: {
  anchor: MenuAnchor;
  boundary?: HTMLElement | null;
  className: string;
  label: string;
  onClose: () => void;
  onKeyDown?: (event: KeyboardEvent<HTMLDivElement>) => void;
  children: ReactNode;
  placement?: "above" | "below";
  role?: "menu" | "dialog";
  restoreFocusTo?: Element | null;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const close = useRef(onClose);
  close.current = onClose;
  const placed = useRef(placement);
  placed.current = placement;

  // 收起 100ms：真身照常卸下（焦点归还、监听移除不变），留一份影子朝打开时的方向退回去。
  useLayoutEffect(() => {
    const menu = ref.current;
    return () => {
      const shift = placed.current === "below" ? "0 -4px" : "0 4px";
      exitGhost(menu, [{ opacity: 1 }, { opacity: 0, translate: shift, scale: "0.97" }]);
    };
  }, []);

  useLayoutEffect(() => {
    const menu = ref.current!;
    const opener = restoreFocusTo ?? document.activeElement;
    const position = () => {
      const bounds = boundary?.getBoundingClientRect();
      const left = Math.max(0, bounds?.left ?? 0) + 8;
      const top = Math.max(0, bounds?.top ?? 0) + 8;
      const right = Math.min(window.innerWidth, bounds?.right ?? window.innerWidth) - 8;
      const bottom = Math.min(window.innerHeight, bounds?.bottom ?? window.innerHeight) - 8;
      menu.style.maxWidth = `${Math.max(0, right - left)}px`;
      menu.style.maxHeight = `${Math.max(0, bottom - top)}px`;
      const { offsetWidth: width, offsetHeight: height } = menu;
      const selection = anchor.selection;
      const x = selection ? selection.left + selection.width / 2 - width / 2 : anchor.x;
      let y = selection ? (placement === "below" ? selection.bottom + 8 : selection.top - height - 8) : anchor.y;
      if (selection && placement === "below" && y + height > bottom) y = selection.top - height - 8;
      else if (selection && placement === "above" && y < top) y = selection.bottom + 8;
      menu.style.left = `${Math.max(left, Math.min(x, right - width))}px`;
      menu.style.top = `${Math.max(top, Math.min(y, bottom - height))}px`;
    };
    position();
    (menu.querySelector<HTMLElement>('[role="menuitemradio"][aria-checked="true"]')
      ?? menu.querySelector<HTMLElement>("button:not(:disabled)"))?.focus({ preventScroll: true });
    const down = (event: Event) => {
      if (event.target instanceof Node && !menu.contains(event.target)) close.current();
    };
    // 滚到右键目标所产生的 scroll 可能排在 contextmenu 之后派发；只响应打开后的位移。
    const scrollTop = boundary?.scrollTop, scrollLeft = boundary?.scrollLeft;
    // 打开前把触发器滚进视野产生的 scroll 在下一帧才派发，同样不算打开后的位移。
    let settled = false;
    const frame = requestAnimationFrame(() => { settled = true; });
    const scroll = (event: Event) => {
      if (!settled || (event.target === boundary && boundary?.scrollTop === scrollTop && boundary?.scrollLeft === scrollLeft)) return;
      down(event);
    };
    const key = (event: globalThis.KeyboardEvent) => {
      if (event.isComposing || event.key !== "Escape") return;
      event.preventDefault();
      event.stopImmediatePropagation();
      close.current();
    };
    const resize = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(position);
    resize?.observe(menu);
    if (boundary) resize?.observe(boundary);
    window.addEventListener("resize", position);
    window.addEventListener("pointerdown", down, true);
    window.addEventListener("scroll", scroll, true);
    window.addEventListener("keydown", key, true);
    return () => {
      cancelAnimationFrame(frame);
      resize?.disconnect();
      window.removeEventListener("resize", position);
      window.removeEventListener("pointerdown", down, true);
      window.removeEventListener("scroll", scroll, true);
      window.removeEventListener("keydown", key, true);
      if (opener instanceof HTMLElement && opener.isConnected && !opener.closest("[inert]")) opener.focus({ preventScroll: true });
    };
  }, [anchor, boundary, placement, restoreFocusTo]);

  return createPortal(
    <div ref={ref} role={role} aria-label={label} className={className} data-scroll-boundary data-floating-menu={placement}
      onMouseDown={event => event.preventDefault()}
      onKeyDown={event => {
        if (event.nativeEvent.isComposing || event.ctrlKey || event.metaKey || event.altKey) return;
        event.stopPropagation();
        const items = [...event.currentTarget.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")];
        const index = items.indexOf(document.activeElement as HTMLButtonElement);
        if (role === "dialog") {
          if (event.key === "Tab") {
            event.preventDefault();
            items[(index + (event.shiftKey ? -1 : 1) + items.length) % items.length]?.focus({ preventScroll: true });
          }
          return;
        }
        if (["ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight"].includes(event.key)) {
          event.preventDefault();
          const direction = event.key === "ArrowDown" || event.key === "ArrowRight" ? 1 : -1;
          items[(index + direction + items.length) % items.length]?.focus({ preventScroll: true });
        } else if (event.key === "Home" || event.key === "End") {
          event.preventDefault();
          items[event.key === "Home" ? 0 : items.length - 1]?.focus({ preventScroll: true });
        } else if (event.key === "Tab") {
          event.preventDefault();
          onClose();
        } else onKeyDown?.(event);
      }}>
      {children}
    </div>, document.body,
  );
}
