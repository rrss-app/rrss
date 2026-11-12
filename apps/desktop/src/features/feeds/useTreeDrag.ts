import { useEffect, useRef, useState, type RefObject } from "react";
import type { MenuAnchor } from "../../components/FloatingMenu";

export type DropZone = "before" | "after" | "into";
/** 指针下的合法落点：目标行、上下半或中间，以及是否落在置顶区。 */
export type DropTarget = { targetKey: string; zone: DropZone; pinDrop: boolean };
/** 按下的行：`key` 是行的 data-key，`items` 是实际要移动的项（多选时不止一个）。 */
export type DragItem = { key: string; name: string; items: string[]; fromPin: boolean };
export type TreeDrag = DragItem & {
  extra: number;
  x: number;
  y: number;
  targetKey: string | null;
  zone: DropZone | null;
  line: { top: number; left: number } | null;
};

type Handlers = {
  /** 这一行能否拖、拖哪些项；不能拖返回 null。 */
  pick: (row: HTMLElement) => DragItem | null;
  /** 指针下的行是不是合法落点。 */
  target: (row: HTMLElement, y: number, item: DragItem) => DropTarget | null;
  drop: (item: DragItem, target: DropTarget) => void;
  /** 真正开始拖动（移动超过 5px）时调用一次，比如关掉菜单。 */
  start: () => void;
  contextMenu: (key: string, anchor: MenuAnchor) => void;
};

/**
 * 订阅树的整行拖动：用指针事件，不用 HTML5 拖放（Tauri 的文件拖入处理会干扰后者）。
 * 负责跟随标签、插入线、靠边自动滚动，以及拖完紧跟的 click 不算选中（`dragMoved`）。
 */
export function useTreeDrag(scrollRef: RefObject<HTMLDivElement | null>, handlers: Handlers) {
  const [drag, setDrag] = useState<TreeDrag | null>(null);
  const live = useRef<{ item: DragItem; target: DropTarget | null } | null>(null);
  const pointer = useRef<{ x: number; y: number } | null>(null);
  const dragMoved = useRef(false);
  const latest = useRef(handlers);
  latest.current = handlers;

  const lineAt = (el: HTMLElement, zone: "before" | "after") => {
    const scroller = scrollRef.current;
    if (!scroller) return null;
    const rect = el.getBoundingClientRect();
    const box = scroller.getBoundingClientRect();
    const depth = Number(el.style.getPropertyValue("--depth")) || 0;
    return { top: (zone === "before" ? rect.top : rect.bottom) - box.top + scroller.scrollTop - 1, left: 12 + depth * 16 };
  };
  const hover = (x: number, y: number, item: DragItem) => {
    pointer.current = { x, y };
    const el = document.elementFromPoint(x, y)?.closest<HTMLElement>(".feeds-row");
    const target = el ? latest.current.target(el, y, item) : null;
    live.current = { item, target };
    setDrag({
      ...item, extra: Math.max(0, item.items.length - 1), x, y,
      targetKey: target?.targetKey ?? null, zone: target?.zone ?? null,
      line: el && target && target.zone !== "into" ? lineAt(el, target.zone) : null,
    });
  };
  const hoverRef = useRef(hover);
  hoverRef.current = hover;

  useEffect(() => {
    const root = scrollRef.current;
    if (!root) return;
    let press: { item: DragItem; x: number; y: number; id: number } | null = null;
    const onDown = (event: PointerEvent) => {
      if (event.button !== 0 || !(event.target instanceof Element)) return;
      if (event.target.closest(".feeds-twist, .feeds-row-more, input, button")) return;
      const row = event.target.closest<HTMLElement>(".feeds-row");
      const item = row ? latest.current.pick(row) : null;
      if (!item) return;
      press = { item, x: event.clientX, y: event.clientY, id: event.pointerId };
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
      window.addEventListener("pointercancel", onUp);
    };
    const stop = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onUp);
    };
    const onMove = (event: PointerEvent) => {
      if (!press || event.pointerId !== press.id) return;
      if (!live.current && Math.hypot(event.clientX - press.x, event.clientY - press.y) < 5) return;
      if (!live.current) latest.current.start();
      hoverRef.current(event.clientX, event.clientY, press.item);
    };
    const onUp = (event: PointerEvent) => {
      if (!press || event.pointerId !== press.id) return;
      press = null;
      stop();
      const done = live.current;
      if (!done) return;
      // 拖完紧跟的 click 不算选中；松手不在行上时没有 click，下一轮清掉，免得吞掉之后的正常点击。
      dragMoved.current = true;
      setTimeout(() => { dragMoved.current = false; }, 0);
      live.current = null;
      setDrag(null);
      if (done.target) latest.current.drop(done.item, done.target);
    };
    const onMenu = (event: MouseEvent) => {
      if (!(event.target instanceof Element) || event.target.closest("input")) return;
      const row = event.target.closest<HTMLElement>(".feeds-row");
      if (!row?.dataset.key || row.dataset.key === "all") return;
      event.preventDefault();
      latest.current.contextMenu(row.dataset.key, { x: event.clientX, y: event.clientY });
    };
    root.addEventListener("pointerdown", onDown);
    root.addEventListener("contextmenu", onMenu);
    return () => {
      stop();
      root.removeEventListener("pointerdown", onDown);
      root.removeEventListener("contextmenu", onMenu);
    };
  }, [scrollRef]);

  const dragging = drag !== null;
  useEffect(() => {
    if (!dragging) return;
    let frame = 0;
    const tick = () => {
      const el = scrollRef.current;
      const point = pointer.current;
      const current = live.current;
      if (el && point && current) {
        const box = el.getBoundingClientRect();
        const before = el.scrollTop;
        if (point.y < box.top + 32) el.scrollTop -= 12;
        else if (point.y > box.bottom - 32) el.scrollTop += 12;
        if (el.scrollTop !== before) hoverRef.current(point.x, point.y, current.item);
      }
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [dragging, scrollRef]);

  return { drag, dragMoved };
}
