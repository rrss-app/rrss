import type { SecretView } from "../bindings";
import { cloneElement, isValidElement, useId, useState, type ReactNode } from "react";
import { Icon } from "../components/Icon";
import { Segmented } from "../components/Segmented";
import { useReveal } from "../lib/motion";

export function ChipBar({
  value,
  options,
  onChange,
  disabled = false,
  className,
}: {
  value: string;
  options: Array<{ id: string; label: string }>;
  onChange: (id: string) => void;
  disabled?: boolean;
  className?: string;
}) {
  const Group = className?.split(" ").includes("segmented") ? Segmented : "div";
  return (
    <Group className={className ? `chips ${className}` : "chips"}>
      {options.map((o) => (
        <button
          key={o.id}
          type="button"
          className="chip-btn"
          data-on={value === o.id ? "1" : "0"}
          aria-pressed={value === o.id}
          disabled={disabled}
          onClick={() => onChange(o.id)}
        >
          {o.label}
        </button>
      ))}
    </Group>
  );
}

export function Toggle({
  on,
  onLabel,
  offLabel,
  onClick,
  disabled = false,
  label,
  warning = false,
}: {
  on: boolean;
  onLabel: string;
  offLabel: string;
  onClick: () => void;
  disabled?: boolean;
  label?: string;
  warning?: boolean;
}) {
  return (
    <button type="button" className="chip-btn toggle" data-warning={warning || undefined} data-on={on ? "1" : "0"} aria-pressed={on} aria-label={label} disabled={disabled} onClick={onClick}>
      <span className="toggle-dot" aria-hidden="true" />
      {/* 两种状态文字叠在同一格里、只显示当前那个：宽度取较长者，开关不随文字左右挪。 */}
      {onLabel || offLabel ? <span className="toggle-label"><span aria-hidden={!on}>{onLabel}</span><span aria-hidden={on}>{offLabel}</span></span> : null}
    </button>
  );
}

export function Field({ label, children, hint, htmlFor }: { label: string; children: ReactNode; hint?: string; htmlFor?: string }) {
  const fieldId = useId();
  const isControl = isValidElement<{ id?: string; "aria-describedby"?: string }>(children)
    && typeof children.type === "string"
    && ["input", "textarea", "select"].includes(children.type);
  const controlId = htmlFor ?? (isControl ? children.props.id || fieldId : undefined);
  const hintId = fieldId + "-hint";
  return (
    <div className="field" role={controlId ? undefined : "group"} aria-labelledby={controlId ? undefined : fieldId + "-label"}>
      {controlId ? <label htmlFor={controlId}>{label}</label> : <span id={fieldId + "-label"}>{label}</span>}
      {isControl ? cloneElement(children, {
        id: controlId,
        "aria-describedby": [children.props["aria-describedby"], hint ? hintId : undefined].filter(Boolean).join(" ") || undefined,
      }) : children}
      {hint ? <small id={hintId} className="field-hint">{hint}</small> : null}
    </div>
  );
}

/** 设置行：左侧标签与说明，`note` 接在说明后（估算、状态等随数据变化的几行），右侧控件。 */
export function SettingRow({ label, hint, note, id: rowId, children }: { label: string; hint?: string; note?: ReactNode; id?: string; children: ReactNode }) {
  const id = useId();
  return (
    <div className="settings-option" id={rowId} role="group" aria-labelledby={id} aria-describedby={hint ? id + "-hint" : undefined}>
      <div>
        <strong id={id}>{label}</strong>
        {hint ? <p id={id + "-hint"} className="hint">{hint}</p> : null}
        {note}
      </div>
      {children}
    </div>
  );
}

export function SecretField({
  label,
  value,
  onChange,
  placeholder,
  disabled = false,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
}) {
  const [show, setShow] = useState(false);
  const inputId = useId();
  return (
    <Field label={label} htmlFor={inputId}>
      <div className="secret-input">
        <input
          id={inputId}
          type={show ? "text" : "password"}
          aria-label={label}
          disabled={disabled}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder={placeholder}
          autoComplete="off"
        />
        <button type="button" className="btn sm" disabled={disabled} onClick={() => setShow((v) => !v)} aria-label={(show ? "隐藏" : "显示") + label} aria-pressed={show} title={show ? "隐藏密钥" : "显示密钥"}>
          <Icon name="eye" size={14} />{show ? "隐藏" : "显示"}
        </button>
      </div>
    </Field>
  );
}

/** 已保存的密钥只接收状态和首尾几位；输入仅用于替换，undefined 保留原值，空串明确清除。 */
export function SavedSecretField({ label, saved, value, onChange, placeholder }: {
  label: string; saved: SecretView; value?: string; onChange: (value: string | undefined) => void; placeholder?: string;
}) {
  if (saved.state === "set" && value === undefined) return <Field label={label}>
    <div className="row"><span className="mono">{saved.hint ? `已保存 · ${saved.hint}` : "已保存"}</span>
      <button type="button" className="btn sm" onClick={() => onChange("")}>更换{label}</button>
    </div>
  </Field>;
  return <>
    {saved.state === "missing" && <p className="notice warn">备份未包含此密钥，需要重新填写 {label}。</p>}
    <SecretField label={label} value={value ?? ""} onChange={onChange} placeholder={placeholder} />
    {saved.state === "set" && <div className="row"><span className="hint">填写新值替换；留空保存将清除密钥。</span>
      <button type="button" className="btn sm ghost" onClick={() => onChange(undefined)}>取消更换</button></div>}
  </>;
}

/**
 * L1 折叠组：标题行带摘要；收起时内容仍挂载，未保存的草稿不丢。展开状态由页面持有。
 * 内容高度用 `grid-template-rows: 0fr → 1fr` 过渡并淡入（展开 220ms、收起约七成），收起走完才隐藏。
 */
export function Fold({ title, summary, open, onToggle, className, children }: {
  title: string;
  summary: string;
  open: boolean;
  onToggle: () => void;
  className?: string;
  children: ReactNode;
}) {
  const id = useId();
  const reveal = useReveal(open);
  return (
    <section className={className ? `settings-fold ${className}` : "settings-fold"} data-open={open ? "1" : "0"}>
      <button type="button" className="settings-fold-head" aria-expanded={open} aria-controls={id} onClick={onToggle}>
        <strong>{title}</strong>
        <span className="settings-fold-summary">{summary}</span>
        <Icon name="chevronDown" size={16} />
      </button>
      <div className="settings-fold-reveal" data-state={reveal}>
        <div className="settings-fold-clip">
          <div id={id} className="settings-fold-body">{children}</div>
        </div>
      </div>
    </section>
  );
}

/** 草稿保存栏：只在有改动时出现，吸底。保存中按钮转圈、不换文字，宽度不跳。 */
export function SaveBar({ label, busy, saving = busy, onSave, onUndo }: { label: string; busy: boolean; saving?: boolean; onSave: () => void; onUndo: () => void }) {
  return (
    <div className="settings-savebar" role="group" aria-label={`${label}的未保存修改`}>
      <span className="hint">有未保存的修改</span>
      <button type="button" className="btn primary" disabled={busy} aria-busy={saving || undefined} onClick={onSave}>保存</button>
      <button type="button" className="btn ghost" disabled={busy} onClick={onUndo}>撤销</button>
    </div>
  );
}
