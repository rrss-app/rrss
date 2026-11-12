import type { CSSProperties, Ref } from "react";

export function ConfirmBar({
  title,
  message,
  onOk,
  onCancel,
  style,
  rootRef,
  inert = false,
  okLabel = "确认",
  cancelLabel = "取消",
  secondary,
}: {
  title: string;
  message: string;
  onOk: () => void;
  onCancel: () => void;
  style?: CSSProperties;
  rootRef?: Ref<HTMLDivElement>;
  inert?: boolean;
  okLabel?: string;
  cancelLabel?: string;
  /** 主按钮与取消之间的第三个选择，例如离开确认里的「不保存」。 */
  secondary?: { label: string; onClick: () => void };
}) {
  return (
    <div className="overlay center" style={style} inert={inert}>
      <button type="button" className="overlay-dismiss" aria-label="关闭" onClick={onCancel} />
      <div
        ref={rootRef}
        className="panel confirm-panel"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="confirm-title"
        aria-describedby="confirm-message"
      >
        <h3 id="confirm-title">{title}</h3>
        <p id="confirm-message">{message}</p>
        <div className="row confirm-actions">
          <button type="button" className="btn primary" data-confirm-ok onClick={onOk}>
            {okLabel}
          </button>
          {secondary && <button type="button" className="btn" data-confirm-secondary onClick={secondary.onClick}>{secondary.label}</button>}
          <button type="button" className={secondary ? "btn ghost" : "btn"} data-confirm-cancel onClick={onCancel}>
            {cancelLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
