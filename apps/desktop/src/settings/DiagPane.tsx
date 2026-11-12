import { api } from "../lib/api";
import { Icon } from "../components/Icon";
import { SettingRow } from "./chips";
import { DiagLogSection } from "./DiagLogSection";
import type { Ask } from "./SettingsPanel";
import type { SettingsEditState } from "./editState";
import { useDiagnosticsSettings } from "./useDiagnosticsSettings";

// 存储页只给用户看的数据位置；库文件、版本、日志模式等技术细节在这里（冒烟脚本也读 `db` 核对连的是哪个库）。
const ROWS: Array<{ key: "db" | "vec" | "stats" | "ai"; label: string }> = [
  { key: "db", label: "数据库" },
  { key: "vec", label: "向量检索" },
  { key: "stats", label: "数据统计" },
  { key: "ai", label: "AI 用量" },
];

export function DiagPane({ onAsk, onEditStateChange, onChanged }: {
  onAsk: Ask;
  onEditStateChange?: (state: SettingsEditState | null) => void;
  onChanged?: () => void;
}) {
  const { query, busy, pending, status, maintain, refresh } = useDiagnosticsSettings(onEditStateChange, onChanged);
  const data = query.data;
  const refreshing = query.isFetching;
  const error = status?.tone === "err" ? status.text : query.isError ? "读取诊断信息失败：" + String(query.error) + "。请刷新状态重试。" : "";
  return (
    <div className="diag-pane">
      {error && <p className="notice error" role="alert"><Icon name="info" size={14} />{error}</p>}
      {busy && <p className="form-status" role="status">正在执行维护操作…</p>}
      <section className="settings-card settings-section">
        <div className="settings-section-meta">
          <div className="settings-section-head">
            <div><h4>运行状态</h4><p className="hint">当前数据与用量概况。点击刷新可重新读取。</p></div>
          </div>
        </div>
        <div className="settings-section-body">
          <div className="settings-group-actions">
            <button type="button" className="btn sm" disabled={refreshing || busy} onClick={() => void refresh()}>
              <Icon name="refresh" size={14} className={refreshing ? "spin" : undefined} />刷新状态
            </button>
          </div>
          <dl className="diag-list">
            {ROWS.map((row) => (
              <div key={row.key} className="diag-row">
                <dt>{row.label}</dt><dd>{data ? data[row.key] || "—" : query.isError ? "未能读取" : "读取中…"}</dd>
              </div>
            ))}
          </dl>
        </div>
      </section>
      {data?.log && <DiagLogSection log={data.log} onAsk={onAsk} disabled={busy || refreshing} />}
      <section className="settings-card settings-section">
        <div className="settings-section-meta">
          <h4>索引维护</h4>
        </div>
        <div className="settings-section-body">
          <SettingRow label="生成文章向量" hint="给一批还没有向量的内容补上向量，相似文章和按意思搜索会更准。用远程模型会产生费用。">
            <button type="button" className="btn" disabled={busy || refreshing} aria-busy={pending === "生成向量" || undefined} onClick={() => void maintain("生成向量", () => api.embedNow(), "本轮向量处理已结束。", true)}>
              <Icon name="cpu" size={14} />处理一批
            </button>
          </SettingRow>
          <SettingRow label="允许失败内容重试" hint="之前生成失败的内容会重新排队处理。不会删除文章。">
            <button type="button" className="btn" disabled={busy || refreshing} aria-busy={pending === "重置失败计数" || undefined} onClick={() => void maintain("重置失败计数", () => api.resetFailed(), "已重新排队，稍后会再处理这些内容。")}>
              <Icon name="refresh" size={14} />重置失败计数
            </button>
          </SettingRow>
          {status?.tone === "ok" && <p className="form-status" role="status">{status.text}</p>}
        </div>
      </section>
    </div>
  );
}
