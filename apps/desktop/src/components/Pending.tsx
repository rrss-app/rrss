import { useEffect, useState, type ReactNode } from "react";

export const PENDING_DELAY_MS = 200;

/** 挂载后过 `ms` 毫秒才变真：读得快就什么也不画，读得慢才出占位。 */
export function useDelayed(ms = PENDING_DELAY_MS) {
  const [late, setLate] = useState(false);
  useEffect(() => {
    const timer = window.setTimeout(() => setLate(true), ms);
    return () => window.clearTimeout(timer);
  }, [ms]);
  return late;
}

/**
 * 读取中的占位提示：200ms 内读完就不出现。切换分类时宁可空白片刻再整页出现，
 * 也不要先闪一下「正在读取…」再逐块填进来。
 */
export function Pending({ children, className = "hint" }: { children: ReactNode; className?: string }) {
  return useDelayed() ? <p className={className} role="status">{children}</p> : null;
}
