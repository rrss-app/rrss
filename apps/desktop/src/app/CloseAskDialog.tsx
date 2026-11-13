import { useState, type CSSProperties, type Ref } from "react";
import type { CloseChoice, TrayLabels } from "../lib/api";
import { Icon } from "../components/Icon";
import { i18n, useTranslation } from "../i18n";

/** 托盘菜单的文字：启动时交给宿主（展示文案不进后端）。 */
export const trayLabels = (): TrayLabels => ({
  open: i18n.t("tray.open", { ns: "shell" }),
  refresh: i18n.t("tray.refresh", { ns: "shell" }),
  quit: i18n.t("tray.quit", { ns: "shell" }),
});

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
  const { t } = useTranslation("shell");
  const [remember, setRemember] = useState(false);
  return (
    <div className="overlay center" style={style} inert={inert}>
      <button type="button" className="overlay-dismiss" aria-label={t("closeAsk.cancel")} onClick={onCancel} />
      <div ref={rootRef} className="panel close-ask" role="alertdialog" aria-modal="true" aria-labelledby="close-ask-title">
        <div className="close-ask-top">
          <span>{t("closeAsk.caption")}</span>
          <button type="button" className="tool-btn" aria-label={t("closeAsk.cancel")} onClick={onCancel}><Icon name="close" size={14} /></button>
        </div>
        <h3 id="close-ask-title">{t("closeAsk.question")}</h3>
        <label className="close-ask-remember">
          <input type="checkbox" checked={remember} onChange={(event) => setRemember(event.target.checked)} />{t("closeAsk.remember")}
        </label>
        <div className="row close-ask-actions">
          <button type="button" className="btn primary" data-close-tray onClick={() => onChoose("tray", remember)}>{t("closeAsk.tray")}</button>
          <button type="button" className="btn" onClick={() => onChoose("quit", remember)}>{t("closeAsk.quit")}</button>
        </div>
      </div>
    </div>
  );
}
