import { useEffect, useId, useRef, useState, type ButtonHTMLAttributes, type ReactNode, type Ref } from "react";

/** 顶栏共用提示：悬停稍等，键盘聚焦立即显示；触发器仍保留自己的可访问名称。 */
export function ReaderTooltip({ label, shortcut, children, as: Tag = "span", triggerRef, className = "", placement = "bottom", delay = 400 }: { label: string; shortcut?: string; children: ReactNode; as?: "span" | "summary"; triggerRef?: Ref<HTMLElement>; className?: string; placement?: "bottom" | "left" | "top"; delay?: number }) {
  const [visible, setVisible] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const id = useId();
  const hide = () => { clearTimeout(timer.current); setVisible(false); };
  useEffect(() => () => clearTimeout(timer.current), []);
  return <Tag ref={triggerRef} className={`reader-tool-tip ${className}`} aria-label={Tag === "summary" ? label : undefined} onMouseEnter={() => {
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setVisible(true), delay);
  }} onMouseLeave={hide} onFocus={() => { clearTimeout(timer.current); setVisible(true); }} onBlur={hide} onPointerDown={hide}>
    {children}
    {visible ? <span id={id} role="tooltip" className="reader-tooltip" data-placement={placement}>{label}{shortcut ? ` · ${shortcut}` : ""}</span> : null}
  </Tag>;
}

export function ReaderTool({ label, shortcut, children, className = "", ...props }: ButtonHTMLAttributes<HTMLButtonElement> & { label: string; shortcut?: string }) {
  return <ReaderTooltip label={label} shortcut={shortcut}>
    <button type="button" className={`tool-btn reader-tool ${className}`} aria-label={label} {...props}>{children}</button>
  </ReaderTooltip>;
}
