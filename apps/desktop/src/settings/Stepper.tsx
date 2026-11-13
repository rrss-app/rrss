import { useEffect, useRef } from "react";
import { useTranslation } from "../i18n";

/** 输入与按住按钮只改草稿，离开输入框或结束一次按压才提交。 */
export function Stepper({ label, value, onChange, onCommit, min, max, step = 1, disabled = false, commitOnEnter = false }: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  onCommit?: (value: string) => void;
  min?: number;
  max?: number;
  step?: number;
  disabled?: boolean;
  commitOnEnter?: boolean;
}) {
  const { t } = useTranslation("settingsSystem");
  const current = useRef(value);
  current.current = value;
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const held = useRef(false);
  const change = useRef(onChange);
  change.current = onChange;
  useEffect(() => () => clearTimeout(timer.current), []);
  const move = (direction: number) => {
    const number = Number(current.current);
    const next = Math.min(max ?? Infinity, Math.max(min ?? -Infinity, (Number.isFinite(number) ? number : 0) + direction * step));
    current.current = String(Number(next.toFixed(8)));
    change.current(current.current);
  };
  const release = () => {
    clearTimeout(timer.current);
    if (!held.current) return;
    held.current = false;
    onCommit?.(current.current);
  };
  const button = (direction: -1 | 1) => <button type="button" disabled={disabled}
      className={direction === -1 ? "stepper-minus" : "stepper-plus"}
      aria-label={t(direction === -1 ? "stepper.decrease" : "stepper.increase", { label })}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        e.preventDefault();
        e.currentTarget.setPointerCapture(e.pointerId);
        held.current = true;
        move(direction);
        const repeat = () => { if (held.current) { move(direction); timer.current = setTimeout(repeat, 80); } };
        timer.current = setTimeout(repeat, 400);
      }}
      onPointerUp={release} onPointerCancel={release} onLostPointerCapture={release}
      onClick={(e) => { if (e.detail === 0) { move(direction); onCommit?.(current.current); } }}
    >{direction === -1 ? "−" : "+"}</button>;
  return <span className="settings-stepper">
    {button(-1)}
    <input aria-label={label} type="text" role="spinbutton" inputMode="decimal" value={value}
      aria-valuenow={value.trim() && Number.isFinite(Number(value)) ? Number(value) : undefined}
      aria-valuemin={min} aria-valuemax={max} disabled={disabled}
      onChange={(e) => { current.current = e.target.value; onChange(e.target.value); }}
      onKeyDown={(e) => {
        if (commitOnEnter && e.key === "Enter" && !e.nativeEvent.isComposing) { e.preventDefault(); e.currentTarget.blur(); }
        if (e.key === "ArrowUp" || e.key === "ArrowDown") { e.preventDefault(); move(e.key === "ArrowUp" ? 1 : -1); }
      }}
      onBlur={() => onCommit?.(current.current)} />
    {button(1)}
  </span>;
}
