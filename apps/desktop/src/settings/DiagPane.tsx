import { api } from "../lib/api";
import { Icon } from "../components/Icon";
import { useTranslation } from "../i18n";
import { formatError } from "../lib/errors";
import { SettingRow } from "./chips";
import { DiagLogSection } from "./DiagLogSection";
import type { Ask } from "./SettingsPanel";
import type { SettingsEditState } from "./editState";
import { useDiagnosticsSettings } from "./useDiagnosticsSettings";

// 存储页只给用户看的数据位置；库文件、版本、日志模式等技术细节在这里（冒烟脚本也读 `db` 核对连的是哪个库）。
const ROWS: Array<"db" | "vec" | "stats" | "ai"> = ["db", "vec", "stats", "ai"];

export function DiagPane({ onAsk, onEditStateChange, onChanged }: {
  onAsk: Ask;
  onEditStateChange?: (state: SettingsEditState | null) => void;
  onChanged?: () => void;
}) {
  const { t } = useTranslation("settingsSystem");
  const { query, busy, pending, status, maintain, refresh } = useDiagnosticsSettings(onEditStateChange, onChanged);
  const data = query.data;
  const refreshing = query.isFetching;
  const error = status?.tone === "err" ? status.text : query.isError ? t("diag.loadFailed", { message: formatError(query.error) }) : "";
  return (
    <div className="diag-pane">
      {error && <p className="notice error" role="alert"><Icon name="info" size={14} />{error}</p>}
      {busy && <p className="form-status" role="status">{t("diag.busy")}</p>}
      <section className="settings-card settings-section">
        <div className="settings-section-meta">
          <div className="settings-section-head">
            <div><h4>{t("diag.status.title")}</h4><p className="hint">{t("diag.status.hint")}</p></div>
          </div>
        </div>
        <div className="settings-section-body">
          <div className="settings-group-actions">
            <button type="button" className="btn sm" disabled={refreshing || busy} onClick={() => void refresh()}>
              <Icon name="refresh" size={14} className={refreshing ? "spin" : undefined} />{t("diag.status.refresh")}
            </button>
          </div>
          <dl className="diag-list">
            {ROWS.map((key) => (
              <div key={key} className="diag-row">
                <dt>{t(`diag.rows.${key}`)}</dt><dd>{data ? data[key] || "—" : query.isError ? t("diag.status.failed") : t("diag.status.loading")}</dd>
              </div>
            ))}
          </dl>
        </div>
      </section>
      {data?.log && <DiagLogSection log={data.log} onAsk={onAsk} disabled={busy || refreshing} />}
      <section className="settings-card settings-section">
        <div className="settings-section-meta">
          <h4>{t("diag.index.title")}</h4>
        </div>
        <div className="settings-section-body">
          <SettingRow label={t("diag.index.embed.label")} hint={t("diag.index.embed.hint")}>
            <button type="button" className="btn" disabled={busy || refreshing} aria-busy={pending === "embed" || undefined} onClick={() => void maintain("embed", () => api.embedNow(), t("diag.index.embed.done"), true)}>
              <Icon name="cpu" size={14} />{t("diag.index.embed.action")}
            </button>
          </SettingRow>
          <SettingRow label={t("diag.index.retry.label")} hint={t("diag.index.retry.hint")}>
            <button type="button" className="btn" disabled={busy || refreshing} aria-busy={pending === "resetFailed" || undefined} onClick={() => void maintain("resetFailed", () => api.resetFailed(), t("diag.index.retry.done"))}>
              <Icon name="refresh" size={14} />{t("diag.index.retry.action")}
            </button>
          </SettingRow>
          {status?.tone === "ok" && <p className="form-status" role="status">{status.text}</p>}
        </div>
      </section>
    </div>
  );
}
