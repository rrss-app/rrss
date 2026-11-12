import { useEffect, useRef } from "react";

/**
 * 作用域化的 window keydown 监听：`enabled` 为 false 时不绑定监听器，处理函数总是读到最新闭包。
 * 各功能自持快捷键时用它，避免根壳用一个大 effect 分发。
 */
export function useWindowKeydown(enabled: boolean, handler: (event: KeyboardEvent) => void) {
  const handlerRef = useRef(handler);
  handlerRef.current = handler;
  useEffect(() => {
    if (!enabled) return;
    const onKey = (event: KeyboardEvent) => handlerRef.current(event);
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [enabled]);
}
