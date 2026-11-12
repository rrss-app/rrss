import { useRef, useState } from "react";
import { Icon } from "../components/Icon";
import { ChipBar, SettingRow, Toggle } from "./chips";
import type { SettingsEditState } from "./editState";
import type { Ask } from "./SettingsPanel";
import { useBackupSettings, type PrefsParts } from "./useBackupSettings";

const SCHEDULES = [
  { id: "off", label: "关" },
  { id: "daily", label: "每天" },
  { id: "weekly", label: "每周" },
];

/** 存储页里的「备份与恢复」：完整备份、从备份恢复、自动备份与只导出偏好。 */
export function BackupSection({ onAsk, onEditStateChange, onChanged, blocked }: {
  onAsk: Ask;
  onEditStateChange?: (state: SettingsEditState | null) => void;
  onChanged?: () => void;
  /** 存储清理或压缩进行中：两边不同时写库。 */
  blocked: boolean;
}) {
  const s = useBackupSettings(onAsk, onEditStateChange, onChanged);
  const data = s.query.data;
  const disabled = s.busy || blocked || !data;
  const [includeSecrets, setIncludeSecrets] = useState(false);
  const [parts, setParts] = useState<PrefsParts>({ rules: true, interests: true, memories: true });
  const fileRef = useRef<HTMLInputElement>(null);
  const loadError = s.query.isError ? "读取备份信息失败：" + String(s.query.error) : "";
  const notice = data?.restore_notice;
  return (
    <section className="settings-card backup-card settings-section" aria-labelledby="backup-title">
      <div className="settings-section-meta">
        <h4 id="backup-title">备份与恢复</h4>
      </div>
      <div className="settings-section-body">
        {notice && (
          <div className={notice.ok ? "notice ok" : "notice error"} role="status">
            <Icon name={notice.ok ? "check" : "info"} size={14} />
            <span>{notice.message}</span>
            <button type="button" className="btn sm ghost" disabled={s.busy} onClick={s.dismissNotice}>知道了</button>
          </div>
        )}
        {(s.status?.tone === "err" || loadError) && (
          <p className="notice error" role="alert"><Icon name="info" size={14} />{s.status?.tone === "err" ? s.status.text : loadError}</p>
        )}
        {s.restarting && (
          <p className="notice" role="status"><Icon name="spinner" size={14} className="spin" />正在备份当前数据，完成后应用会自动重启并恢复…</p>
        )}

        <div className="settings-option">
          <div>
            <strong>备份全部数据</strong>
            <p className="hint">文章、订阅、规则、记忆、对话和设置打包成一个文件。{includeSecrets && <b className="backup-em">这次包含密钥，请妥善保管。</b>}{data?.last_manual}</p>
          </div>
          <button type="button" className="btn primary" disabled={disabled} onClick={() => s.backup(includeSecrets)}>
            {s.op === "backup" ? <><Icon name="spinner" size={14} className="spin" />正在备份…</> : <><Icon name="download" size={14} />立即备份…</>}
          </button>
        </div>
        <SettingRow label="包含密钥" hint="API Key、翻译密钥，以及外部工具的环境变量和请求头。默认不包含：备份文件放到网盘或别的电脑也不会带出密钥，恢复后重新填写即可。">
          <Toggle warning label="备份包含密钥" on={includeSecrets} onLabel="包含" offLabel="不包含" disabled={disabled} onClick={() => setIncludeSecrets((v) => !v)} />
        </SettingRow>
        {s.done && (
          <div className="notice ok backup-result" role="status">
            <Icon name="check" size={14} />
            <span>
              已保存到 {s.done.path} · {s.done.size}<br />{s.done.summary}<br />
              <small className="muted">不含媒体缓存和对话引擎程序，恢复后会按需重新下载。</small>
            </span>
            <span className="backup-result-actions">
              <button type="button" className="btn sm" onClick={() => s.reveal(s.done!.path)}>打开所在文件夹</button>
              <button type="button" className="btn sm ghost" onClick={s.closeDone}>完成</button>
            </span>
          </div>
        )}

        <div className="settings-option">
          <div>
            <strong>从备份恢复</strong>
            <p className="hint">替换当前全部数据，替换前会自动备份当前数据。需要重启应用。</p>
          </div>
          <button type="button" className="btn" disabled={disabled} onClick={() => s.restore()}>
            {s.op === "inspect" ? <><Icon name="spinner" size={14} className="spin" />正在检查…</> : <><Icon name="upload" size={14} />选择备份文件…</>}
          </button>
        </div>

        <details className="advanced-fold backup-fold" data-backup-fold="auto">
          <summary>自动备份<span className="backup-fold-sum">{data?.auto_summary}</span></summary>
          <div className="settings-option">
            <div>
              <strong>定时备份</strong>
              <p className="hint">保存在数据目录的 backups 里，只保留最近 5 份。数据库升级前总会自动备份一次。自动备份始终不含密钥。</p>
            </div>
            <ChipBar className="segmented" value={data?.schedule ?? "off"} options={SCHEDULES} disabled={disabled} onChange={s.setSchedule} />
          </div>
          {data && data.entries.length > 0 ? (
            <ul className="backup-list" aria-label="自动备份">
              {data.entries.map((entry) => (
                <li key={entry.path} className="backup-entry">
                  <div><strong>{entry.when}</strong><span className="hint">{entry.note}</span></div>
                  {entry.restorable && (
                    <button type="button" className="btn sm ghost" disabled={disabled} onClick={() => s.restore(entry.path)}>恢复</button>
                  )}
                </li>
              ))}
            </ul>
          ) : (
            <p className="hint">还没有自动备份。</p>
          )}
        </details>

        <details className="advanced-fold backup-fold" data-backup-fold="prefs">
          <summary>只导出偏好<span className="backup-fold-sum">规则、兴趣、记忆</span></summary>
          <p className="hint">换机时只带偏好，不带文章。不含密钥。订阅请用订阅页的 OPML 导出。导入是合并，已有的不会重复。</p>
          <div className="backup-checks" role="group" aria-label="导出内容">
            {([["rules", "规则", data?.rules, "条"], ["interests", "兴趣", data?.interests, "个"], ["memories", "记忆", data?.memories, "条"]] as const).map(([key, label, n, unit]) => (
              <label key={key}>
                <input type="checkbox" checked={parts[key]} disabled={s.busy}
                  onChange={(e) => setParts((p) => ({ ...p, [key]: e.target.checked }))} />
                {label}{n !== undefined ? `（${n} ${unit}）` : ""}
              </label>
            ))}
          </div>
          <div className="row backup-prefs-actions">
            <button type="button" className="btn" disabled={disabled || !(parts.rules || parts.interests || parts.memories)} aria-busy={s.op === "export" || undefined}
              onClick={() => s.exportPrefs(parts)}><Icon name="download" size={14} />导出…</button>
            <input ref={fileRef} type="file" accept=".json,application/json" hidden onChange={(e) => {
              const file = e.target.files?.[0];
              e.target.value = "";
              if (file) s.importPrefs(file);
            }} />
            <button type="button" className="btn" disabled={disabled} aria-busy={s.op === "import" || undefined} onClick={() => fileRef.current?.click()}>
              <Icon name="upload" size={14} />导入…
            </button>
          </div>
          {s.status?.tone === "ok" && <p className="form-status" role="status">{s.status.text}</p>}
          {s.imported && (
            <div className={s.imported.skipped.length ? "notice warn backup-result" : "notice ok backup-result"} role="status">
              <Icon name={s.imported.skipped.length ? "info" : "check"} size={14} />
              <span>
                导入完成：{s.imported.summary}
                {s.imported.skipped.length > 0 && <>
                  <br />以下 {s.imported.skipped.length} 项没有导入：
                  <ul className="backup-skipped">{s.imported.skipped.map((line) => <li key={line}>{line}</li>)}</ul>
                </>}
              </span>
              <span className="backup-result-actions"><button type="button" className="btn sm ghost" onClick={s.closeImported}>完成</button></span>
            </div>
          )}
        </details>
      </div>
    </section>
  );
}
