import { useRef, useState } from "react";
import { Icon } from "../components/Icon";
import { useTranslation } from "../i18n";
import { formatError } from "../lib/errors";
import { ChipBar, SettingRow, Toggle } from "./chips";
import type { SettingsEditState } from "./editState";
import type { Ask } from "./SettingsPanel";
import { useBackupSettings, type PrefsParts } from "./useBackupSettings";

const SCHEDULES = ["off", "daily", "weekly"] as const;

/** 存储页里的「备份与恢复」：完整备份、从备份恢复、自动备份与只导出偏好。 */
export function BackupSection({ onAsk, onEditStateChange, onChanged, blocked }: {
  onAsk: Ask;
  onEditStateChange?: (state: SettingsEditState | null) => void;
  onChanged?: () => void;
  /** 存储清理或压缩进行中：两边不同时写库。 */
  blocked: boolean;
}) {
  const { t } = useTranslation("settingsSystem");
  const s = useBackupSettings(onAsk, onEditStateChange, onChanged);
  const scheduleOptions = SCHEDULES.map((id) => ({ id, label: t(`backup.schedule.${id}`) }));
  const data = s.query.data;
  const disabled = s.busy || blocked || !data;
  const [includeSecrets, setIncludeSecrets] = useState(false);
  const [parts, setParts] = useState<PrefsParts>({ rules: true, interests: true, memories: true });
  const fileRef = useRef<HTMLInputElement>(null);
  const loadError = s.query.isError ? t("backup.loadFailed", { message: formatError(s.query.error) }) : "";
  const notice = data?.restore_notice;
  return (
    <section className="settings-card backup-card settings-section" aria-labelledby="backup-title">
      <div className="settings-section-meta">
        <h4 id="backup-title">{t("backup.title")}</h4>
      </div>
      <div className="settings-section-body">
        {notice && (
          <div className={notice.ok ? "notice ok" : "notice error"} role="status">
            <Icon name={notice.ok ? "check" : "info"} size={14} />
            <span>{notice.message}</span>
            <button type="button" className="btn sm ghost" disabled={s.busy} onClick={s.dismissNotice}>{t("backup.dismiss")}</button>
          </div>
        )}
        {(s.status?.tone === "err" || loadError) && (
          <p className="notice error" role="alert"><Icon name="info" size={14} />{s.status?.tone === "err" ? s.status.text : loadError}</p>
        )}
        {s.restarting && (
          <p className="notice" role="status"><Icon name="spinner" size={14} className="spin" />{t("backup.restarting")}</p>
        )}

        <div className="settings-option">
          <div>
            <strong>{t("backup.all.title")}</strong>
            <p className="hint">{t("backup.all.hint")}{includeSecrets && <b className="backup-em">{t("backup.all.withSecrets")}</b>}{data?.last_manual}</p>
          </div>
          <button type="button" className="btn primary" disabled={disabled} onClick={() => s.backup(includeSecrets)}>
            {s.op === "backup" ? <><Icon name="spinner" size={14} className="spin" />{t("backup.all.running")}</> : <><Icon name="download" size={14} />{t("backup.all.action")}</>}
          </button>
        </div>
        <SettingRow label={t("backup.secrets.label")} hint={t("backup.secrets.hint")}>
          <Toggle warning label={t("backup.secrets.toggle")} on={includeSecrets} onLabel={t("backup.secrets.on")} offLabel={t("backup.secrets.off")} disabled={disabled} onClick={() => setIncludeSecrets((v) => !v)} />
        </SettingRow>
        {s.done && (
          <div className="notice ok backup-result" role="status">
            <Icon name="check" size={14} />
            <span>
              {t("backup.done.saved", { path: s.done.path, size: s.done.size })}<br />{s.done.summary}<br />
              <small className="muted">{t("backup.done.note")}</small>
            </span>
            <span className="backup-result-actions">
              <button type="button" className="btn sm" onClick={() => s.reveal(s.done!.path)}>{t("backup.done.reveal")}</button>
              <button type="button" className="btn sm ghost" onClick={s.closeDone}>{t("backup.done.close")}</button>
            </span>
          </div>
        )}

        <div className="settings-option">
          <div>
            <strong>{t("backup.restore.title")}</strong>
            <p className="hint">{t("backup.restore.hint")}</p>
          </div>
          <button type="button" className="btn" disabled={disabled} onClick={() => s.restore()}>
            {s.op === "inspect" ? <><Icon name="spinner" size={14} className="spin" />{t("backup.restore.checking")}</> : <><Icon name="upload" size={14} />{t("backup.restore.action")}</>}
          </button>
        </div>

        <details className="advanced-fold backup-fold" data-backup-fold="auto">
          <summary>{t("backup.auto.title")}<span className="backup-fold-sum">{data?.auto_summary}</span></summary>
          <div className="settings-option">
            <div>
              <strong>{t("backup.auto.scheduled")}</strong>
              <p className="hint">{t("backup.auto.hint")}</p>
            </div>
            <ChipBar className="segmented" value={data?.schedule ?? "off"} options={scheduleOptions} disabled={disabled} onChange={s.setSchedule} />
          </div>
          {data && data.entries.length > 0 ? (
            <ul className="backup-list" aria-label={t("backup.auto.title")}>
              {data.entries.map((entry) => (
                <li key={entry.path} className="backup-entry">
                  <div><strong>{entry.when}</strong><span className="hint">{entry.note}</span></div>
                  {entry.restorable && (
                    <button type="button" className="btn sm ghost" disabled={disabled} onClick={() => s.restore(entry.path)}>{t("backup.auto.restore")}</button>
                  )}
                </li>
              ))}
            </ul>
          ) : (
            <p className="hint">{t("backup.auto.empty")}</p>
          )}
        </details>

        <details className="advanced-fold backup-fold" data-backup-fold="prefs">
          <summary>{t("backup.prefs.title")}<span className="backup-fold-sum">{t("backup.prefs.sum")}</span></summary>
          <p className="hint">{t("backup.prefs.hint")}</p>
          <div className="backup-checks" role="group" aria-label={t("backup.prefs.contentAria")}>
            {([["rules", data?.rules], ["interests", data?.interests], ["memories", data?.memories]] as const).map(([key, n]) => (
              <label key={key}>
                <input type="checkbox" checked={parts[key]} disabled={s.busy}
                  onChange={(e) => setParts((p) => ({ ...p, [key]: e.target.checked }))} />
                {t(`backup.prefs.part.${key}`)}{n !== undefined ? t(`backup.prefs.count.${key}`, { count: n }) : ""}
              </label>
            ))}
          </div>
          <div className="row backup-prefs-actions">
            <button type="button" className="btn" disabled={disabled || !(parts.rules || parts.interests || parts.memories)} aria-busy={s.op === "export" || undefined}
              onClick={() => s.exportPrefs(parts)}><Icon name="download" size={14} />{t("backup.prefs.export")}</button>
            <input ref={fileRef} type="file" accept=".json,application/json" hidden onChange={(e) => {
              const file = e.target.files?.[0];
              e.target.value = "";
              if (file) s.importPrefs(file);
            }} />
            <button type="button" className="btn" disabled={disabled} aria-busy={s.op === "import" || undefined} onClick={() => fileRef.current?.click()}>
              <Icon name="upload" size={14} />{t("backup.prefs.import")}
            </button>
          </div>
          {s.status?.tone === "ok" && <p className="form-status" role="status">{s.status.text}</p>}
          {s.imported && (
            <div className={s.imported.skipped.length ? "notice warn backup-result" : "notice ok backup-result"} role="status">
              <Icon name={s.imported.skipped.length ? "info" : "check"} size={14} />
              <span>
                {t("backup.prefs.imported", { summary: s.imported.summary })}
                {s.imported.skipped.length > 0 && <>
                  <br />{t("backup.prefs.skipped", { count: s.imported.skipped.length })}
                  <ul className="backup-skipped">{s.imported.skipped.map((line) => <li key={line}>{line}</li>)}</ul>
                </>}
              </span>
              <span className="backup-result-actions"><button type="button" className="btn sm ghost" onClick={s.closeImported}>{t("backup.done.close")}</button></span>
            </div>
          )}
        </details>
      </div>
    </section>
  );
}
