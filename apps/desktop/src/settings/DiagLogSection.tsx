import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api, copyText, type DiagnosticsPage, type LogStatusView, type LogUnavailableReason } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { Icon } from "../components/Icon";
import { ChipBar, SettingRow } from "./chips";
import { diagnosticsKeys } from "./diagnosticsQuery";
import type { Ask } from "./SettingsPanel";
import { i18n, locale, useTranslation } from "../i18n";

// 按原因给处理办法：另一个窗口在记录时检查权限没有用。
const LEVELS = ["default", "detailed"] as const;

/** 上次最后一条记录的时间：今天只写时刻，其他日子带上月日；读不出时间时为空。 */
function lastEvent(value: string) {
  const at = new Date(value);
  if (Number.isNaN(at.getTime())) return "";
  const time = at.toLocaleTimeString(locale(), { hour: "2-digit", minute: "2-digit", hour12: false });
  return at.toDateString() === new Date().toDateString()
    ? i18n.t("diagLog.lastEvent.today", { ns: "settingsSystem", time })
    : i18n.t("diagLog.lastEvent.date", { ns: "settingsSystem", month: at.getMonth() + 1, day: at.getDate(), time });
}

/** 诊断页「运行日志」：位置、大小、本次运行的记录级别与导出诊断包。清空在存储页。 */
export function DiagLogSection({ log, onAsk, disabled }: { log: LogStatusView; onAsk: Ask; disabled: boolean }) {
  const { t } = useTranslation("settingsSystem");
  const client = useQueryClient();
  const [pending, setPending] = useState<"" | "level" | "export">("");
  const [copied, setCopied] = useState(false);
  const [saved, setSaved] = useState("");
  const [error, setError] = useState("");
  const blocked = disabled || !!pending;
  const unavailable = log.unavailable ?? null;
  const levelOptions = LEVELS.map((id) => ({ id, label: t(`diagLog.level.${id}`) }));
  const unclean = log.previous_unclean === null ? null : lastEvent(log.previous_unclean);

  const setLevel = async (id: string) => {
    if (blocked || (id === "detailed") === log.detailed) return;
    setPending("level");
    setError("");
    try {
      const next = await api.logSetLevel(id === "detailed");
      client.setQueryData<DiagnosticsPage>(diagnosticsKeys.page, (page) => page && { ...page, log: next });
    } catch (cause) {
      setError(formatNotice(t("diagLog.levelFailed"), cause));
    } finally {
      setPending("");
    }
  };
  const exportPackage = async () => {
    setPending("export");
    setError("");
    setSaved("");
    try {
      const path = await api.logExport();
      if (path) setSaved(path);
    } catch (cause) {
      setError(formatNotice("", cause));
    } finally {
      setPending("");
    }
  };
  const askExport = () => onAsk(
    t("diagLog.export.title"),
    t("diagLog.export.confirm"),
    () => { void exportPackage(); },
    { okLabel: t("diagLog.export.okLabel") },
  );

  return (
    <section className="settings-card settings-section diag-log-section" aria-labelledby="diag-log-title">
      <div className="settings-section-meta">
        <div className="settings-section-head">
          <div><h4 id="diag-log-title">{t("diagLog.title")}</h4><p className="hint">{t("diagLog.hint")}</p></div>
          <span className="diag-log-state" data-on={unavailable ? undefined : ""}>{unavailable ? t("diagLog.state.off") : t("diagLog.state.on")}</span>
        </div>
      </div>
      <div className="settings-section-body">
        {unavailable && <p className="notice warn" role="status"><Icon name="info" size={14} /><span><strong>{t(`diagLog.unavailable.${unavailable}.title`)}</strong>{t(`diagLog.unavailable.${unavailable}.text`)}</span></p>}
        {unclean !== null && (
          <p className="notice warn" role="status"><Icon name="info" size={14} />
            <span><strong>{t("diagLog.unclean.title")}</strong>{unclean ? t("diagLog.unclean.bodyAt", { when: unclean }) : t("diagLog.unclean.body")}</span>
          </p>
        )}
        <div className="diag-log">
          <code title={log.directory}>{log.directory}</code>
          <button type="button" className="btn sm" onClick={() => void api.backupReveal(log.file).catch(() => api.backupReveal(log.directory))}>
            <Icon name="folder" size={14} />{t("diagLog.reveal")}
          </button>
          <button type="button" className="btn sm" aria-label={t("diagLog.copyAria")} onClick={() => void copyText(log.directory).then(() => setCopied(true))}>
            <Icon name={copied ? "check" : "copy"} size={14} />{t("diagLog.copy")}
          </button>
        </div>
        <p className="hint diag-log-size">{t("diagLog.size", { current: log.current_size, total: log.total_size })}</p>
        <SettingRow label={t("diagLog.level.label")} hint={log.detailed ? t("diagLog.level.detailedHint") : t("diagLog.level.defaultHint")}>
          <ChipBar className="segmented" value={log.detailed ? "detailed" : "default"} options={levelOptions} disabled={blocked || !!unavailable} onChange={(id) => void setLevel(id)} />
        </SettingRow>
        <SettingRow label={t("diagLog.feedback.label")} hint={t("diagLog.feedback.hint")}>
          <button type="button" className="btn" disabled={blocked} aria-busy={pending === "export" || undefined} onClick={askExport}>
            <Icon name="download" size={14} />{pending === "export" ? t("diagLog.feedback.exporting") : t("diagLog.export.title")}
          </button>
        </SettingRow>
        {saved && (
          <p className="form-status diag-log-saved" role="status">{t("diagLog.saved")}
            <button type="button" className="btn sm" onClick={() => void api.backupReveal(saved)}><Icon name="folder" size={14} />{t("diagLog.reveal")}</button>
          </p>
        )}
        {error && <p className="notice error" role="alert"><Icon name="info" size={14} />{error}</p>}
      </div>
    </section>
  );
}
