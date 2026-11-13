import { useEffect, useLayoutEffect, useRef, type RefObject } from "react";

export type ModalLayerId = "settings" | "confirm" | "search" | "web-warning" | "close-ask";

const FOCUSABLE_SELECTOR = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled]):not([type=hidden])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  "summary",
  "[tabindex]:not([tabindex='-1'])",
].join(",");

function isDisabled(el: HTMLElement): boolean {
  if (el.getAttribute("aria-disabled") === "true") return true;
  return "disabled" in el && Boolean((el as HTMLButtonElement).disabled);
}

function isClosedDetailsContent(el: HTMLElement): boolean {
  let details = el.parentElement?.closest("details");
  while (details) {
    const summary = details.querySelector(":scope > summary");
    if (!details.open && el !== summary && !summary?.contains(el)) return true;
    details = details.parentElement?.closest("details");
  }
  return false;
}

/** 使用可见性样式而非布局尺寸，兼容无布局的组件测试环境。 */
export function isFocusable(el: HTMLElement): boolean {
  if (el.matches(".overlay-dismiss")) return false;
  if (isDisabled(el) || el.matches(":disabled") || el.closest("[inert]")) return false;
  if (el.hidden || el.hasAttribute("hidden") || el.closest("[hidden]")) return false;
  if (el.getAttribute("aria-hidden") === "true" || el.closest('[aria-hidden="true"]')) return false;
  if (isClosedDetailsContent(el)) return false;
  let node: HTMLElement | null = el;
  while (node) {
    const computed = getComputedStyle(node);
    if (computed.display === "none" || computed.visibility === "hidden") return false;
    node = node.parentElement;
  }
  return true;
}

export function getFocusableElements(root: HTMLElement): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE_SELECTOR)].filter((el) => el.tabIndex >= 0 && isFocusable(el));
}

function focusElement(el: HTMLElement | null | undefined): boolean {
  if (!el || !el.isConnected || !isFocusable(el)) return false;
  el.focus();
  return document.activeElement === el;
}

export function useModalFocus({
  layers,
  activeId,
  suspended,
  roots,
  initialFocus,
}: {
  layers: ModalLayerId[];
  activeId: ModalLayerId | null;
  suspended: boolean;
  roots: Partial<Record<ModalLayerId, RefObject<HTMLElement | null>>>;
  initialFocus: Partial<Record<ModalLayerId, string>>;
}) {
  const layersKey = layers.join(">");
  const openersRef = useRef<Partial<Record<ModalLayerId, HTMLElement | null>>>({});
  const lastFocusRef = useRef<Partial<Record<ModalLayerId, HTMLElement | null>>>({});
  const prevLayersRef = useRef<ModalLayerId[]>([]);
  const genRef = useRef(0);
  const rootsRef = useRef(roots);
  const initialRef = useRef(initialFocus);
  rootsRef.current = roots;
  initialRef.current = initialFocus;

  const focusInitial = (id: ModalLayerId) => {
    const root = rootsRef.current[id]?.current;
    if (!root) return;
    const initial = root.querySelector<HTMLElement>(initialRef.current[id] ?? FOCUSABLE_SELECTOR);
    if (!focusElement(initial)) focusElement(getFocusableElements(root)[0]);
  };

  const restoreOpener = (opener: HTMLElement | null, fallbackId: ModalLayerId | undefined) => {
    if (focusElement(opener)) return;
    if (!fallbackId) return;
    focusInitial(fallbackId);
  };

  useLayoutEffect(() => {
    const gen = ++genRef.current;
    const prev = prevLayersRef.current;
    const next = layersKey ? (layersKey.split(">") as ModalLayerId[]) : [];

    for (const id of next) {
      if (!prev.includes(id)) {
        const current = document.activeElement;
        openersRef.current[id] = current instanceof HTMLElement ? current : null;
      }
    }

    for (const id of [...prev].reverse()) {
      if (next.includes(id)) continue;
      const opener = openersRef.current[id] ?? null;
      delete openersRef.current[id];
      delete lastFocusRef.current[id];
      if (!suspended) restoreOpener(opener, next.at(-1));
    }

    const pushed = next.filter((id) => !prev.includes(id));
    const top = next.at(-1);
    if (pushed.length && top && !suspended && activeId === top) {
      focusInitial(top);
    }

    prevLayersRef.current = next;
    return () => {
      queueMicrotask(() => {
        if (genRef.current !== gen) return;
        for (const id of [...prevLayersRef.current].reverse()) {
          const opener = openersRef.current[id] ?? null;
          delete openersRef.current[id];
          delete lastFocusRef.current[id];
          restoreOpener(opener, undefined);
        }
        prevLayersRef.current = [];
      });
    };
  }, [layersKey, activeId, suspended]);

  useEffect(() => {
    const onFocusIn = (event: FocusEvent) => {
      const target = event.target;
      if (!(target instanceof HTMLElement)) return;
      for (const id of ["settings", "confirm", "search"] as const) {
        if (rootsRef.current[id]?.current?.contains(target)) lastFocusRef.current[id] = target;
      }
    };
    document.addEventListener("focusin", onFocusIn);
    return () => document.removeEventListener("focusin", onFocusIn);
  }, []);

  useLayoutEffect(() => {
    if (!activeId || suspended) return;
    const root = rootsRef.current[activeId]?.current;
    if (!root) return;

    const current = document.activeElement;
    if (!(current instanceof HTMLElement) || !root.contains(current) || !current.isConnected) {
      const last = lastFocusRef.current[activeId];
      if (!focusElement(last && root.contains(last) ? last : null)) focusInitial(activeId);
    }

    const onKeyDown = (event: KeyboardEvent) => {
      // WebKitGTK 将 Shift+Tab 报为 key=Unidentified，但 code 仍为 Tab。
      if (event.key !== "Tab" && event.code !== "Tab") return;
      const items = getFocusableElements(root);
      if (items.length === 0) {
        event.preventDefault();
        return;
      }
      const active = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      const index = active ? items.indexOf(active) : -1;
      const next = event.shiftKey
        ? items[index <= 0 ? items.length - 1 : index - 1]
        : items[index < 0 || index === items.length - 1 ? 0 : index + 1];
      event.preventDefault();
      next.focus();
    };

    const onFocusIn = (event: FocusEvent) => {
      if (event.target instanceof Node && root.contains(event.target)) return;
      const items = getFocusableElements(root);
      items[0]?.focus();
    };

    const onMutate = () => {
      const focused = document.activeElement;
      if (focused instanceof HTMLElement && focused.isConnected && root.contains(focused) && isFocusable(focused)) return;
      const last = lastFocusRef.current[activeId];
      if (focusElement(last && root.contains(last) ? last : null)) return;
      focusInitial(activeId);
    };

    root.addEventListener("keydown", onKeyDown);
    document.addEventListener("focusin", onFocusIn);
    const observer = new MutationObserver(onMutate);
    observer.observe(root, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ["disabled", "hidden", "inert"],
    });
    return () => {
      root.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("focusin", onFocusIn);
      observer.disconnect();
    };
  }, [activeId, suspended]);
}
