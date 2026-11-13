import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api, copyText, type DiagnosticsPage, type LogStatusView, type LogUnavailableReason } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { Icon } from "../components/Icon";
import { ChipBar, SettingRow } from "./chips";
import { diagnosticsKeys } from "./diagnosticsQuery";
import type { Ask } from "./SettingsPanel";
import { locale } from "../i18n";

// 按原因给处理办法：另一个窗口在记录时检查权限没有用。
const UNAVAILABLE: Record<LogUnavailableReason, { title: string; text: string }> = {
  other_instance: { title: "另一个 RRSS 窗口正在记录日志", text: "这个窗口不写日志文件。关闭另一个窗口后重新启动 RRSS，就会恢复记录。" },
  not_writable: { title: "日志目录无法写入", text: "检查日志目录的权限后，重新启动 RRSS。" },
  disk_full: { title: "磁盘空间不足，日志已停止写入", text: "腾出磁盘空间后重新启动 RRSS，或在存储页清空运行日志。" },
};
const LEVELS = [{ id: "default", label: "默认" }, { id: "detailed", label: "详细" }];

/** 上次最后一条记录的时间：今天只写时刻，其他日子带上月日；读不出时间时为空。 */
function lastEvent(value: string) {
  const at = new Date(value);
  if (Number.isNaN(at.getTime())) return "";
  const time = at.toLocaleTimeString(locale(), { hour: "2-digit", minute: "2-digit", hour12: false });
  return at.toDateString() === new Date().toDateString() ? `今天 ${time}` : `${at.getMonth() + 1} 月 ${at.getDate()} 日 ${time}`;
}

/** 诊断页「运行日志」：位置、大小、本次运行的记录级别与导出诊断包。清空在存储页。 */
export function DiagLogSection({ log, onAsk, disabled }: { log: LogStatusView; onAsk: Ask; disabled: boolean }) {
  const client = useQueryClient();
  const [pending, setPending] = useState<"" | "level" | "export">("");
  const [copied, setCopied] = useState(false);
  const [saved, setSaved] = useState("");
  const [error, setError] = useState("");
  const blocked = disabled || !!pending;
  const unavailable = log.unavailable ? UNAVAILABLE[log.unavailable] : null;
  const unclean = log.previous_unclean === null ? null : lastEvent(log.previous_unclean);

  const setLevel = async (id: string) => {
    if (blocked || (id === "detailed") === log.detailed) return;
    setPending("level");
    setError("");
    try {
      const next = await api.logSetLevel(id === "detailed");
      client.setQueryData<DiagnosticsPage>(diagnosticsKeys.page, (page) => page && { ...page, log: next });
    } catch (cause) {
      setError(formatNotice("切换记录级别失败：", cause));
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
    "导出诊断包",
    "保存一个 ZIP 文件，包含最近的运行日志（导出时再次脱敏）和版本、系统与运行状态。\n不含数据库、设置和密钥。文件保存在你选择的位置，由你决定是否分享。",
    () => { void exportPackage(); },
    { okLabel: "选择保存位置" },
  );

  return (
    <section className="settings-card settings-section diag-log-section" aria-labelledby="diag-log-title">
      <div className="settings-section-meta">
        <div className="settings-section-head">
          <div><h4 id="diag-log-title">运行日志</h4><p className="hint">自动保存在本机，旧日志会自动清理。</p></div>
          <span className="diag-log-state" data-on={unavailable ? undefined : ""}>{unavailable ? "未记录" : "正在记录"}</span>
        </div>
      </div>
      <div className="settings-section-body">
        {unavailable && <p className="notice warn" role="status"><Icon name="info" size={14} /><span><strong>{unavailable.title}</strong>{unavailable.text}</span></p>}
        {unclean !== null && (
          <p className="notice warn" role="status"><Icon name="info" size={14} />
            <span><strong>上次没有正常退出</strong>{unclean && `最后记录于${unclean}。`}可以导出诊断包，帮助排查退出原因。</span>
          </p>
        )}
        <div className="diag-log">
          <code title={log.directory}>{log.directory}</code>
          <button type="button" className="btn sm" onClick={() => void api.backupReveal(log.file).catch(() => api.backupReveal(log.directory))}>
            <Icon name="folder" size={14} />在文件夹中显示
          </button>
          <button type="button" className="btn sm" aria-label="复制日志位置" onClick={() => void copyText(log.directory).then(() => setCopied(true))}>
            <Icon name={copied ? "check" : "copy"} size={14} />复制
          </button>
        </div>
        <p className="hint diag-log-size">当前文件 {log.current_size} · 全部日志 {log.total_size}</p>
        <SettingRow label="记录级别" hint={log.detailed ? "记录更多排查细节，日志量会增加。重启后恢复默认。" : "记录运行概况和错误。只在本次运行有效。"}>
          <ChipBar className="segmented" value={log.detailed ? "detailed" : "default"} options={LEVELS} disabled={blocked || !!unavailable} onChange={(id) => void setLevel(id)} />
        </SettingRow>
        <SettingRow label="反馈问题" hint="导出最近的日志与运行状态，附在反馈里更容易定位。">
          <button type="button" className="btn" disabled={blocked} aria-busy={pending === "export" || undefined} onClick={askExport}>
            <Icon name="download" size={14} />{pending === "export" ? "正在导出…" : "导出诊断包"}
          </button>
        </SettingRow>
        {saved && (
          <p className="form-status diag-log-saved" role="status">诊断包已保存。
            <button type="button" className="btn sm" onClick={() => void api.backupReveal(saved)}><Icon name="folder" size={14} />在文件夹中显示</button>
          </p>
        )}
        {error && <p className="notice error" role="alert"><Icon name="info" size={14} />{error}</p>}
      </div>
    </section>
  );
}
