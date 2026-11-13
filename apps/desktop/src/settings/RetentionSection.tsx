import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { Stepper } from "./Stepper";
import { Icon } from "../components/Icon";
import { readGate, settingsChanged, useEditReport, useSettingsWrite, type SettingsEditState } from "./editState";
import { retentionKeys, retentionQueryOptions } from "./retentionQuery";
import type { Ask } from "./SettingsPanel";
import { useTranslation } from "../i18n";

type Status = { tone: "ok" | "err"; text: string };

/** 存储页「保留策略」：全局开关与天数、将清理的预览和立即清理。单个源在订阅页单独设置。 */
export function RetentionSection({ onAsk, onEditStateChange, onChanged, blocked }: {
  onAsk: Ask;
  onEditStateChange?: (state: SettingsEditState | null) => void;
  onChanged?: () => void;
  /** 存储清理、压缩或备份进行中：两边不同时写库。 */
  blocked: boolean;
}) {
  const { t } = useTranslation("settingsContent");
  const client = useQueryClient();
  const { writing, run: write } = useSettingsWrite({ mutationKey: retentionKeys.write, mutationFn: (fn: () => Promise<unknown>) => fn() });
  const [op, setOp] = useState<"" | "days" | "clean">("");
  const busy = writing || !!op;
  const query = useQuery({ ...retentionQueryOptions(), ...readGate(busy) });
  const data = query.data;
  const [daysText, setDaysText] = useState("");
  const [status, setStatus] = useState<Status | null>(null);
  const changed = useRef(onChanged);
  changed.current = onChanged;
  useEditReport(onEditStateChange, t("retention.title"), false, busy);
  // 输入框只在没有编辑时跟随已保存的值。
  const saved = data?.days ?? 0;
  const [editing, setEditing] = useState(false);
  useEffect(() => { if (!editing && saved > 0) setDaysText(String(saved)); }, [saved, editing]);

  const run = async <T,>(next: "days" | "clean", failure: string, fn: () => Promise<T>, onDone: (value: T) => void) => {
    if (client.isMutating({ mutationKey: retentionKeys.write })) return;
    setOp(next);
    setStatus(null);
    const result = await write(async () => {
      const value = await fn();
      await settingsChanged(client);
      await client.fetchQuery(retentionQueryOptions()).catch(() => undefined);
      return value;
    });
    setOp("");
    if (result.status === "busy" || !result.mounted) return;
    if (result.status === "done") onDone(result.data as T);
    else setStatus({ tone: "err", text: formatNotice(failure, result.error) });
  };

  const saveDays = (days: number) => void run("days", t("retention.saveFailed"), () => api.retentionSetDays(days), () => {
    setEditing(false);
    setStatus({ tone: "ok", text: days > 0 ? t("retention.savedDays", { count: days }) : t("retention.savedOff") });
  });
  const commitDays = (text = daysText) => {
    const days = Math.trunc(Number(text));
    if (!Number.isFinite(days) || days < 1) {
      setStatus({ tone: "err", text: t("retention.daysInvalid") });
      return;
    }
    if (days !== saved) saveDays(days);
    else setEditing(false);
  };
  const clean = () => {
    if (!data?.can_run) return;
    onAsk(data.confirm_title, data.confirm.join("\n"), () => {
      void run("clean", t("retention.cleanFailed"), () => api.retentionCleanNow(), (message) => {
        setStatus({ tone: "ok", text: message });
        changed.current?.();
      });
    }, { okLabel: data.confirm_ok });
  };

  const on = saved > 0;
  const disabled = busy || blocked || !data;
  return (
    <section className="settings-card retention-card settings-section" aria-labelledby="retention-title">
      <div className="settings-section-meta">
        <h4 id="retention-title">{t("retention.title")}</h4>
      </div>
      <div className="settings-section-body">
        {(status?.tone === "err" || query.isError) && (
          <p className="notice error" role="alert"><Icon name="info" size={14} />{status?.tone === "err" ? status.text : t("retention.readFailed", { error: String(query.error) })}</p>
        )}
        <details className="advanced-fold backup-fold" data-retention-fold open={on || undefined}>
          <summary>{t("retention.fold")}<span className="backup-fold-sum">{on ? t("retention.foldOn", { count: saved }) : t("retention.foldOff")}</span></summary>
          <div className="settings-option">
            <div>
              <strong>{t("retention.keep")}</strong>
              <p className="hint">{on
                ? t("retention.keepOnHint", { count: saved })
                : t("retention.keepOffHint")}</p>
            </div>
            <button type="button" className="chip-btn toggle" role="switch" aria-checked={on} aria-label={t("retention.keep")}
              data-on={on ? "1" : "0"} disabled={disabled} onClick={() => saveDays(on ? 0 : data?.default_days ?? 90)}>
              <span className="toggle-dot" aria-hidden="true" />{on ? t("retention.toggleOn") : t("retention.toggleOff")}
            </button>
          </div>
          {on && (
            <div className="settings-option">
              <div>
                <strong>{t("retention.days")}</strong>
                <p className="hint">{t("retention.daysHint")}{data && data.custom_feeds > 0 ? t("retention.customFeeds", { count: data.custom_feeds }) : ""}</p>
              </div>
              <span className="retention-days">
                <Stepper commitOnEnter label={t("retention.days")} value={daysText} disabled={disabled} min={1}
                  onChange={(next) => { setEditing(true); setDaysText(next); }} onCommit={commitDays} />{" "}{t("retention.daysUnit")}
              </span>
            </div>
          )}
          {on && (
            <p className="retention-protected">{t("retention.protectedA")}{"\n"}{t("retention.protectedB")}</p>
          )}
          {data?.preview && (
            <div className="settings-option">
              <div>
                <strong>{data.preview}</strong>
                {data.preview_hint && <p className="hint">{data.preview_hint}</p>}
              </div>
              {data.can_run && (
                <button type="button" className="btn danger" disabled={disabled} onClick={clean}>
                  {op === "clean" ? <><Icon name="spinner" size={14} className="spin" />{t("retention.cleaning")}</> : t("retention.cleanNow")}
                </button>
              )}
            </div>
          )}
        </details>
        {status?.tone === "ok" && <p className="form-status" role="status">{status.text}</p>}
      </div>
    </section>
  );
}
