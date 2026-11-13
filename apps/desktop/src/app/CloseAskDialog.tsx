import { useState, type CSSProperties, type Ref } from "react";
import type { CloseChoice, TrayLabels } from "../lib/api";
import { Icon } from "../components/Icon";

const TEXT = {
  caption: "关闭 RRSS",
  question: "你希望关闭主窗口时怎么做？",
  remember: "记住我的选择",
  tray: "隐藏到托盘",
  quit: "退出程序",
  cancel: "取消",
};

/** 托盘菜单的文字：启动时交给宿主（展示文案不进后端）。 */
export const TRAY_LABELS: TrayLabels = { open: "打开 RRSS", refresh: "立即刷新", quit: "退出程序" };

/**
 * 关闭询问（阶段 14 · 后台驻留）：设置为「每次询问」时，宿主拦下关闭后弹出。
 * ×、Esc、点遮罩都是取消，窗口不关；勾「记住」时由宿主写进设置。
 */
export function CloseAskDialog({ rootRef, inert = false, style, onChoose, onCancel }: {
  rootRef?: Ref<HTMLDivElement>;
  inert?: boolean;
  style?: CSSProperties;
  onChoose: (choice: CloseChoice, remember: boolean) => void;
  onCancel: () => void;
}) {
  const [remember, setRemember] = useState(false);
  return (
    <div className="overlay center" style={style} inert={inert}>
      <button type="button" className="overlay-dismiss" aria-label={TEXT.cancel} onClick={onCancel} />
      <div ref={rootRef} className="panel close-ask" role="alertdialog" aria-modal="true" aria-labelledby="close-ask-title">
        <div className="close-ask-top">
          <span>{TEXT.caption}</span>
          <button type="button" className="tool-btn" aria-label={TEXT.cancel} onClick={onCancel}><Icon name="close" size={14} /></button>
        </div>
        <h3 id="close-ask-title">{TEXT.question}</h3>
        <label className="close-ask-remember">
          <input type="checkbox" checked={remember} onChange={(event) => setRemember(event.target.checked)} />{TEXT.remember}
        </label>
        <div className="row close-ask-actions">
          <button type="button" className="btn primary" data-close-tray onClick={() => onChoose("tray", remember)}>{TEXT.tray}</button>
          <button type="button" className="btn" onClick={() => onChoose("quit", remember)}>{TEXT.quit}</button>
        </div>
      </div>
    </div>
  );
}
