import { useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { getFocusableElements } from "../lib/useModalFocus";
import { useEditReport, type SettingsEditState } from "./editState";
import type { Ask } from "./SettingsPanel";

/**
 * 设置页里的编辑弹层外壳（连接、模型、技能、MCP、工具说明共用）。
 * 设置本身是模态层：这里自管 Tab 循环，关闭后把焦点还给打开它的按钮，并把草稿 / 写入状态上报给设置壳的离开守卫。
 * Esc 与「取消」有未保存修改时先确认：给了 `onSave` 是「保存并离开 / 不保存 / 继续编辑」三选，否则二选。
 * 草稿与保存逻辑留在各自的弹层里；`onSave` 成功时由弹层自己关闭并返回 true，失败留在弹层里并返回 false。
 */
export function Dialog({ title, subtitle, dirty, busy, onAsk, onClose, onSave, onEditStateChange, leading, actions, closeInHeader = false, cancelLabel = "取消", className, children }: {
  title: string;
  /** 标题下的一行小字，例如另一种填写方式的入口。 */
  subtitle?: ReactNode;
  closeInHeader?: boolean;
  dirty: boolean;
  busy: boolean;
  onAsk: Ask;
  onClose: () => void;
  onSave?: () => Promise<boolean>;
  onEditStateChange?: (state: SettingsEditState | null) => void;
  /** 左下角的操作，例如删除。 */
  leading?: ReactNode;
  /** 「取消」右侧的操作，最后一个通常是主按钮。 */
  actions: ReactNode;
  /** 只读弹层用「关闭」。 */
  cancelLabel?: string;
  /** `wide`：列表类弹层，更宽更高。 */
  className?: string;
  children: ReactNode;
}) {
  const root = useRef<HTMLDivElement>(null);
  const [opener] = useState(() => (document.activeElement instanceof HTMLElement ? document.activeElement : null));
  const titleId = useId();

  useEditReport(onEditStateChange, title, dirty, busy, onSave);

  useLayoutEffect(() => {
    const node = root.current!;
    getFocusableElements(node)[0]?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Tab" && event.code !== "Tab") return;
      event.stopPropagation();
      event.preventDefault();
      const items = getFocusableElements(node);
      const index = items.indexOf(document.activeElement as HTMLElement);
      const next = event.shiftKey ? items[index <= 0 ? items.length - 1 : index - 1] : items[(index + 1) % items.length];
      next?.focus();
    };
    node.addEventListener("keydown", onKeyDown);
    return () => {
      node.removeEventListener("keydown", onKeyDown);
      // 经确认层关闭时设置仍是 inert，要等确认层撤掉的下一帧再还焦点；StrictMode 的模拟卸载不还。
      const restore = () => {
        if (!node.isConnected && opener?.isConnected && document.activeElement !== opener) opener.focus();
      };
      restore();
      requestAnimationFrame(restore);
    };
  }, [opener]);

  const cancel = () => {
    if (busy) return;
    if (!dirty) onClose();
    else if (onSave) {
      onAsk(`${title}有未保存的修改`, "离开前保存这些修改吗？", () => void onSave(),
        { okLabel: "保存并离开", secondary: { label: "不保存", onClick: onClose }, cancelLabel: "继续编辑" });
    } else onAsk(`${title}有未保存的修改`, "关闭后本次填写的内容不会保存。", onClose, { okLabel: "不保存并离开", cancelLabel: "继续编辑" });
  };

  return (
    <div className="settings-dialog-scrim">
      <div ref={root} className={className ? `panel settings-dialog ${className}` : "panel settings-dialog"} role="dialog" aria-modal="true" aria-labelledby={titleId}
        aria-busy={busy ? "true" : "false"}
        onKeyDown={(event) => {
          if (event.key === "Escape" && !event.defaultPrevented) { event.preventDefault(); cancel(); }
        }}>
        {closeInHeader ? <div className="row settings-dialog-head"><h3 id={titleId}>{title}</h3><span className="spacer" /><button type="button" className="btn sm ghost" disabled={busy} onClick={cancel}>关闭</button></div> : <h3 id={titleId}>{title}</h3>}
        {subtitle ? <p className="settings-dialog-sub">{subtitle}</p> : null}
        <div className="settings-dialog-body">{children}</div>
        <div className="row settings-dialog-actions">
          {leading}
          <span className="spacer" />
          <button type="button" className="btn ghost" disabled={busy} onClick={cancel}>{cancelLabel}</button>
          {actions}
        </div>
      </div>
    </div>
  );
}
