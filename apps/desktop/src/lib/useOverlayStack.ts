import { useCallback, useEffect, useRef, useState } from "react";

export type LeaveVerdict = "ok" | "busy" | "dirty";

export type PendingLeave<T extends string> = { id: T; proceed: () => void };

export type OverlayStackOptions<T extends string> = {
  /** 关闭 / 离开某层前询问；"dirty" 进入待确认，"busy" 只提示不离开。 */
  guard?: (id: T) => LeaveVerdict;
  onBusy?: (id: T) => void;
  /** 用户确认丢弃后、执行离开前调用（例如清掉脏标记）。 */
  onDiscard?: (id: T) => void;
  /** 层内还有返回目标时先消费后退；返回 true 保留该叠层。 */
  onBack?: (id: T) => boolean;
  /** 某层已从栈中移除。 */
  onClosed?: (id: T) => void;
};

type HistoryState = { rrss?: boolean; overlay?: string } | null;

/**
 * 叠层栈 controller：拥有栈、history 同步、浏览器 / 鼠标后退与通用离开守卫。
 * Escape 由根壳按“搜索 > 待确认 > 栈顶”的优先级调用 `closeTop`，不在这里绑定键盘。
 */
export function useOverlayStack<T extends string>(options: OverlayStackOptions<T> = {}) {
  const [stack, setStack] = useState<T[]>([]);
  const [pendingLeave, setPendingLeave] = useState<PendingLeave<T> | null>(null);
  const stackRef = useRef(stack);
  stackRef.current = stack;
  const pendingRef = useRef(pendingLeave);
  pendingRef.current = pendingLeave;
  const optionsRef = useRef(options);
  optionsRef.current = options;
  const skipPop = useRef(false);

  const open = useCallback((id: T) => {
    if (stackRef.current.includes(id)) return;
    history.pushState({ rrss: true, overlay: id }, "");
    setStack((s) => [...s, id]);
  }, []);

  const closeTopWithoutGuard = useCallback(() => {
    const top = stackRef.current.at(-1);
    if (top === undefined) return;
    const viaHistory = Boolean((history.state as HistoryState)?.rrss);
    setStack((s) => s.slice(0, -1));
    optionsRef.current.onClosed?.(top);
    if (viaHistory) {
      skipPop.current = true;
      history.back();
    }
  }, []);

  /** 通用离开请求：守卫放行则直接执行，脏则等待确认，忙则只提示。 */
  const requestLeave = useCallback((id: T, proceed: () => void) => {
    const verdict = optionsRef.current.guard?.(id) ?? "ok";
    if (verdict === "busy") {
      optionsRef.current.onBusy?.(id);
      return;
    }
    if (verdict === "dirty") {
      setPendingLeave({ id, proceed });
      return;
    }
    proceed();
  }, []);

  const closeTop = useCallback(() => {
    if (pendingRef.current) {
      setPendingLeave(null);
      return;
    }
    const top = stackRef.current.at(-1);
    if (top === undefined) return;
    requestLeave(top, () => {
      if (!optionsRef.current.onBack?.(top)) closeTopWithoutGuard();
    });
  }, [requestLeave, closeTopWithoutGuard]);

  const confirmLeave = useCallback(() => {
    const pending = pendingRef.current;
    if (!pending) return;
    optionsRef.current.onDiscard?.(pending.id);
    setPendingLeave(null);
    pending.proceed();
  }, []);

  const cancelLeave = useCallback(() => setPendingLeave(null), []);

  useEffect(() => {
    const onPop = () => {
      if (skipPop.current) {
        skipPop.current = false;
        return;
      }
      const top = stackRef.current.at(-1);
      if (top === undefined) return;
      const guarded = pendingRef.current !== null || (optionsRef.current.guard?.(top) ?? "ok") !== "ok";
      if (guarded) {
        // 浏览器已经后退，先恢复该层的历史位置，再走与返回按钮相同的保护。
        history.pushState({ rrss: true, overlay: top }, "");
        closeTop();
        return;
      }
      if (optionsRef.current.onBack?.(top)) {
        history.pushState({ rrss: true, overlay: top }, "");
        return;
      }
      setStack((s) => s.slice(0, -1));
      optionsRef.current.onClosed?.(top);
    };
    const onMouseBack = (e: MouseEvent) => {
      if (e.button !== 3 || stackRef.current.length === 0) return;
      e.preventDefault();
      closeTop();
    };
    window.addEventListener("popstate", onPop);
    window.addEventListener("mouseup", onMouseBack);
    return () => {
      window.removeEventListener("popstate", onPop);
      window.removeEventListener("mouseup", onMouseBack);
    };
  }, [closeTop]);

  return {
    stack,
    top: stack[stack.length - 1] as T | undefined,
    pendingLeave,
    open,
    closeTop,
    closeTopWithoutGuard,
    requestLeave,
    confirmLeave,
    cancelLeave,
  };
}
