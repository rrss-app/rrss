import { i18n, useTranslation } from "../../i18n";
import { Icon } from "../../components/Icon";
import type { TranslationSessionView } from "./translationSession";
import { unitFailed } from "./translationState";

export function translationAction(view: TranslationSessionView): { kind: "recover" | "save" | "retry" | "start" | "regenerate"; label: string } {
  if (view.disconnected) return { kind: "recover", label: i18n.t("status.action.recover", { ns: "reader" }) };
  if (view.failure === "retry_unavailable") return { kind: "regenerate", label: i18n.t("status.action.regenerate", { ns: "reader" }) };
  const snapshot = view.data?.snapshot;
  if (snapshot?.save === "save_failed") return { kind: "save", label: i18n.t("status.action.save", { ns: "reader" }) };
  if (snapshot?.phase === "partial") return { kind: "retry", label: i18n.t("status.action.retry", { ns: "reader" }) };
  // 零成功没有可复用的块；普通启动同样只补失败处，内存记录过期后也不会被迫走「重新翻译全文」。
  if (snapshot?.phase === "failed") return { kind: "start", label: i18n.t("status.action.retryTranslation", { ns: "reader" }) };
  if (snapshot?.phase === "invalidated") return { kind: "start", label: i18n.t("status.action.restart", { ns: "reader" }) };
  if (snapshot?.phase === "completed") return { kind: "regenerate", label: i18n.t("status.action.regenerate", { ns: "reader" }) };
  return { kind: "start", label: i18n.t("status.action.start", { ns: "reader" }) };
}

export function TranslationStatus({ view, debug, onAction, onLocate }: {
  view: TranslationSessionView; debug: boolean; onAction: () => void; onLocate: () => void;
}) {
  const { t } = useTranslation("reader");
  const snapshot = view.data?.snapshot;
  const failed = snapshot?.units.filter(unitFailed).length ?? 0;
  const done = snapshot?.units.filter(unit => unit === "done").length ?? 0;
  const total = snapshot?.units.length ?? 0;
  const exception = view.issue || view.disconnected || snapshot?.save === "save_failed" || ["partial", "failed", "invalidated"].includes(snapshot?.phase ?? "");
  let message = view.note;
  if (!message && exception) message = snapshot?.save === "kept"
    ? snapshot.error ? t("status.keptWithError", { error: t(`status.error.${snapshot.error}`) }) : t("status.kept")
    : snapshot?.error ? t(`status.error.${snapshot.error}`) : t("status.partial");
  if (!message && debug && snapshot) message = view.busy ? snapshot.phase === "queued" ? t("status.queued") : t("status.running") : t("status.done");
  if (!message) return null;
  return <div className={`reader-translation-status${exception || view.note ? " warning" : ""}`} role="status">
    <div className="reader-translation-copy"><Icon name={view.busy && !exception ? "spinner" : "info"} className={view.busy && !exception ? "spin" : ""} size={15} />
      <span>{message}</span>{debug && snapshot ? <small>{failed ? t("status.progressFailed", { done, total, failed }) : t("status.progress", { done, total })}</small> : null}</div>
    {exception ? <div className="reader-translation-actions">
      {failed && view.data?.ready ? <button type="button" className="tool-btn" onClick={onLocate}>{t("status.locate")}</button> : null}
      <button type="button" className="tool-btn primary" onClick={onAction} disabled={view.busy}>{view.busy ? t("status.busy") : translationAction(view).label}</button>
    </div> : null}
    {debug && total > 0 && !exception ? <progress aria-label={t("status.progressLabel")} max={total} value={done} /> : null}
  </div>;
}
