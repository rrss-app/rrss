import { useEffect, useRef } from "react";
import { Icon } from "./Icon";

export function Toast({
  message,
  onDismiss,
  holdMs = 5000,
  actionLabel,
  onAction,
  leaving = false,
}: {
  message: string;
  onDismiss: () => void;
  holdMs?: number;
  actionLabel?: string;
  onAction?: () => void;
  /** 已关掉、正在播退场（200ms 下沉淡出）：不再计时、不可交互。 */
  leaving?: boolean;
}) {
  const onDismissRef = useRef(onDismiss);
  onDismissRef.current = onDismiss;
  useEffect(() => {
    if (leaving) return;
    const id = window.setTimeout(() => onDismissRef.current(), holdMs);
    return () => window.clearTimeout(id);
  }, [message, holdMs, actionLabel, leaving]);
  return (
    <div className="toast" role="status" data-leaving={leaving ? "" : undefined} inert={leaving} aria-hidden={leaving || undefined}>
      <span>{message}</span>
      {actionLabel && onAction ? (
        <button type="button" className="toast-action" onClick={onAction}>
          {actionLabel}
        </button>
      ) : null}
      <button type="button" className="toast-dismiss" aria-label="关闭通知" onClick={onDismiss}>
        <Icon name="close" size={14} />
      </button>
    </div>
  );
}
