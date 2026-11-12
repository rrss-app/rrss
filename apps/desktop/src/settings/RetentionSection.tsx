import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { Stepper } from "./Stepper";
import { Icon } from "../components/Icon";
import { readGate, settingsChanged, useEditReport, useSettingsWrite, type SettingsEditState } from "./editState";
import { retentionKeys, retentionQueryOptions } from "./retentionQuery";
import type { Ask } from "./SettingsPanel";

type Status = { tone: "ok" | "err"; text: string };

/** 存储页「保留策略」：全局开关与天数、将清理的预览和立即清理。单个源在订阅页单独设置。 */
export function RetentionSection({ onAsk, onEditStateChange, onChanged, blocked }: {
  onAsk: Ask;
  onEditStateChange?: (state: SettingsEditState | null) => void;
  onChanged?: () => void;
  /** 存储清理、压缩或备份进行中：两边不同时写库。 */
  blocked: boolean;
}) {
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
  useEditReport(onEditStateChange, "保留策略", false, busy);
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

  const saveDays = (days: number) => void run("days", "保存保留设置失败：", () => api.retentionSetDays(days), () => {
    setEditing(false);
    setStatus({ tone: "ok", text: days > 0 ? `已设为保留 ${days} 天。` : "已关闭保留策略，不再清理文章。" });
  });
  const commitDays = (text = daysText) => {
    const days = Math.trunc(Number(text));
    if (!Number.isFinite(days) || days < 1) {
      setStatus({ tone: "err", text: "保留天数需要是大于 0 的整数。" });
      return;
    }
    if (days !== saved) saveDays(days);
    else setEditing(false);
  };
  const clean = () => {
    if (!data?.can_run) return;
    onAsk(data.confirm_title, data.confirm.join("\n"), () => {
      void run("clean", "清理失败：", () => api.retentionCleanNow(), (message) => {
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
        <h4 id="retention-title">保留策略</h4>
      </div>
      <div className="settings-section-body">
        {(status?.tone === "err" || query.isError) && (
          <p className="notice error" role="alert"><Icon name="info" size={14} />{status?.tone === "err" ? status.text : "读取保留设置失败：" + String(query.error)}</p>
        )}
        <details className="advanced-fold backup-fold" data-retention-fold open={on || undefined}>
          <summary>旧文章正文<span className="backup-fold-sum">{on ? `保留 ${saved} 天` : "关闭"}</span></summary>
          <div className="settings-option">
            <div>
              <strong>保留文章正文</strong>
              <p className="hint">{on
                ? `入库满 ${saved} 天的文章会清掉正文、向量、译文和配图，只留标题、链接和阅读记录。`
                : "关闭时不清理任何文章。开启后按文章收录的时间计算，单个源可以在订阅页单独设置或永久保留。"}</p>
            </div>
            <button type="button" className="chip-btn toggle" role="switch" aria-checked={on} aria-label="保留文章正文"
              data-on={on ? "1" : "0"} disabled={disabled} onClick={() => saveDays(on ? 0 : data?.default_days ?? 90)}>
              <span className="toggle-dot" aria-hidden="true" />{on ? "已开启" : "已关闭"}
            </button>
          </div>
          {on && (
            <div className="settings-option">
              <div>
                <strong>保留天数</strong>
                <p className="hint">按文章收录的时间计算。{data && data.custom_feeds > 0 ? `有 ${data.custom_feeds} 个源单独设置了保留。` : ""}</p>
              </div>
              <span className="retention-days">
                <Stepper commitOnEnter label="保留天数" value={daysText} disabled={disabled} min={1}
                  onChange={(next) => { setEditing(true); setDaysText(next); }} onCommit={commitDays} /> 天
              </span>
            </div>
          )}
          {on && (
            <p className="retention-protected">不会清理：已收藏的、被对话引用的、出现在简报来源里的文章。{"\n"}未读文章同样按天数清理；清理后仍能按标题搜到，打开时可以重新抓取正文。</p>
          )}
          {data?.preview && (
            <div className="settings-option">
              <div>
                <strong>{data.preview}</strong>
                {data.preview_hint && <p className="hint">{data.preview_hint}</p>}
              </div>
              {data.can_run && (
                <button type="button" className="btn danger" disabled={disabled} onClick={clean}>
                  {op === "clean" ? <><Icon name="spinner" size={14} className="spin" />清理中…</> : "立即清理"}
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
