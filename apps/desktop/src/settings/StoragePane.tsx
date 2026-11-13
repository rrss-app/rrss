import { useEffect, useRef, useState } from "react";
import { Icon } from "../components/Icon";
import { api, copyText, type StorageGroupView, type StorageItemView } from "../lib/api";
import { BackupSection } from "./BackupSection";
import { RetentionSection } from "./RetentionSection";
import { useMergedEditState, type SettingsEditState } from "./editState";
import type { Ask } from "./SettingsPanel";
import { useStorageSettings } from "./useStorageSettings";
import { useDelayed } from "../components/Pending";
import { locale } from "../i18n";

function scannedAt(ms: number) {
  if (!ms) return "";
  return new Date(ms).toLocaleTimeString(locale(), { hour: "2-digit", minute: "2-digit", hour12: false });
}

const PARTS = [
  { id: "articles", label: "文章与索引", keys: ["articles", "search_index"] },
  { id: "vectors", label: "向量", keys: ["vectors"] },
  { id: "media", label: "媒体", keys: ["images", "videos", "media_other", "untracked_media"] },
  { id: "ai", label: "AI 缓存", keys: ["ai_cache"] },
  { id: "translations", label: "翻译", keys: ["translations"] },
  { id: "recommendation_logs", label: "推荐日志", keys: ["recommendation_logs"] },
  { id: "engine", label: "对话引擎", keys: ["pi_program", "pi_sessions"] },
  { id: "backups", label: "备份", keys: ["auto_backups"] },
  { id: "logs", label: "运行日志", keys: ["logs"] },
  { id: "other", label: "其他", keys: [] },
  // 库文件里删数据后留下的空位：确实占着磁盘，压缩数据库可以还回去，所以画斜纹、单列一项。
  { id: "free", label: "可回收空位", keys: ["free"] },
];
const partId = (key: string) => PARTS.find((part) => part.keys.includes(key))?.id ?? "other";

/** 与宿主 `fmt_bytes` 同一档位，只用于悬停读数。 */
function formatBytes(bytes: number) {
  const kb = 1024;
  if (bytes >= kb ** 3) return `${(bytes / kb ** 3).toFixed(2)} GB`;
  if (bytes >= 100 * kb ** 2) return `${(bytes / kb ** 2).toFixed(0)} MB`;
  if (bytes >= kb ** 2) return `${(bytes / kb ** 2).toFixed(1)} MB`;
  if (bytes >= kb) return `${(bytes / kb).toFixed(0)} KB`;
  return bytes > 0 ? `${bytes} B` : "0";
}

/** 构成条：色段和图例互相联动，指到哪一项就只亮那一项，下面一行读出大小与占比。 */
/** 指针停稳这么久才换高亮项：快速扫过不让读数连跳。键盘聚焦立即切换。 */
const HOVER_SETTLE_MS = 120;

function Composition({ parts }: { parts: Array<{ id: string; label: string; bytes: number }> }) {
  const [hot, setHotNow] = useState<string | null>(null);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const setHot = (id: string | null) => { window.clearTimeout(timer.current); setHotNow(id); };
  const hover = (id: string) => {
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setHotNow(id), HOVER_SETTLE_MS);
  };
  const total = parts.reduce((sum, part) => sum + part.bytes, 0);
  const shown = parts.filter((part) => part.bytes > 0);
  const current = shown.find((part) => part.id === hot);
  const share = (bytes: number) => `${(bytes / total * 100).toFixed(bytes / total < 0.1 ? 1 : 0)}%`;
  return <div className="storage-composition-wrap" data-hot={hot ?? undefined} onMouseLeave={() => setHot(null)}>
    <div className="storage-composition" role="img" aria-label={total ? `已统计明细构成：${shown.map((p) => `${p.label} ${share(p.bytes)}`).join("，")}` : "暂无存储占用"}>
      {shown.map((part) => <i key={part.id} data-part={part.id} data-on={part.id === hot || undefined} style={{ width: `${part.bytes / total * 100}%` }}
        onMouseEnter={() => hover(part.id)} />)}
    </div>
    <p className="storage-readout" aria-live="polite">
      {current ? <><strong>{current.label}</strong>{formatBytes(current.bytes)} · {share(current.bytes)}</> : <span className="hint">指向色条或图例查看各部分大小</span>}
    </p>
    <div className="storage-legend">{shown.map((part) => (
      <button key={part.id} type="button" data-on={part.id === hot || undefined}
        onMouseEnter={() => hover(part.id)} onFocus={() => setHot(part.id)} onBlur={() => setHot(null)}>
        <i className="storage-swatch" data-part={part.id} aria-hidden="true" />{part.label}
      </button>
    ))}</div>
  </div>;
}

/** 数据目录：完整路径用等宽字，可在文件管理器里显示或复制。 */
function DataLocation({ path }: { path: string }) {
  const [copied, setCopied] = useState(false);
  return <div className="storage-location">
    <span className="storage-location-label">数据位置</span>
    <code title={path}>{path}</code>
    <button type="button" className="btn sm" onClick={() => void api.backupReveal(path)}><Icon name="folder" size={14} />在文件夹中显示</button>
    <button type="button" className="btn sm" aria-label="复制路径" onClick={() => void copyText(path).then(() => setCopied(true))}>
      <Icon name={copied ? "check" : "copy"} size={14} />复制
    </button>
  </div>;
}

/** 统计没回来时的总量卡片骨架：和真实内容同高，数据到了原地替换。 */
function StorageSkeleton() {
  return <div className="storage-skeleton" role="status" aria-label="正在统计">
    <div className="storage-total"><i style={{ width: 140, height: 34 }} /><i style={{ width: 280 }} /></div>
    <i className="storage-skeleton-bar" />
    <div className="storage-skeleton-legend">{[0, 1, 2, 3, 4].map((n) => <i key={n} />)}</div>
  </div>;
}

function StorageRow({ item, disabled, pending, onClean }: {
  item: StorageItemView;
  disabled: boolean;
  pending: boolean;
  onClean: (item: StorageItemView) => void;
}) {
  return <div className="storage-row" data-storage-item={item.key}>
    <i className="storage-swatch" data-part={partId(item.key)} aria-hidden="true" />
    <div className="storage-name"><strong>{item.label}</strong>{item.hint && <span className="hint">{item.hint}</span>}</div>
    <div className="storage-act">
      {pending ? <span className="storage-busy" role="status"><Icon name="spinner" size={14} className="spin" />{item.action === "vacuum" ? "压缩中…" : "清理中…"}</span>
        : item.action ? <button type="button" className={`btn sm${item.action === "vacuum" ? "" : " danger"}`} disabled={disabled} onClick={() => onClean(item)}>{item.action_label}</button>
        : item.passive && <span className="storage-passive">{item.passive}</span>}
    </div>
    <div className="storage-size">{item.size}</div>
  </div>;
}

function StorageGroup({ group, disabled, pending, onClean }: {
  group: StorageGroupView;
  disabled: boolean;
  pending: string;
  onClean: (item: StorageItemView) => void;
}) {
  return <section className="storage-group settings-card settings-section" aria-label={group.label}>
    <div className="settings-section-meta">
      <h4>{group.label} <span className="settings-count">{group.size}</span></h4>
      <p className="hint">{group.key === "database" ? "随保留策略变小。" : group.key === "media" ? "清掉后需要时会重新生成。" : group.note}</p>
      {group.note && group.key === "database" && <p className="hint">{group.note}</p>}
    </div>
    <div className="settings-section-body">{group.items.map((item) => <StorageRow key={item.key} item={item} disabled={disabled}
      pending={!!pending && pending === item.action} onClean={onClean} />)}</div>
  </section>;
}

export function StoragePane({ onAsk, onEditStateChange, onChanged }: {
  onAsk: Ask;
  onEditStateChange?: (state: SettingsEditState | null) => void;
  onChanged?: () => void;
}) {
  const { report: part, busy: parts } = useMergedEditState(onEditStateChange);
  const report = { storage: part("storage"), retention: part("retention"), backup: part("backup") };
  const backupBusy = parts.backup || parts.retention;
  const { query, busy, pending, status, clean, refresh } = useStorageSettings(onAsk, report.storage, onChanged);
  const data = query.data;
  const refreshing = query.isFetching;
  const error = status?.tone === "err" ? status.text : query.isError ? "统计存储占用失败：" + String(query.error) + "。请点重新统计。" : "";
  const items = data?.groups.flatMap((g) => g.items) ?? [];
  const composition = PARTS.map((part) => ({ ...part, bytes: items.filter((i) => partId(i.key) === part.id).reduce((sum, i) => sum + Math.max(0, i.bytes), 0) }));
  // 首次统计：先整页留白等一小会儿，统计快就一次出全；慢了才画同形状的骨架，
  // 不让保留 / 备份先出来、再被晚到的占用分组整片推下去。
  const settled = !!data || query.isError;
  const late = useDelayed(250);
  if (!settled && !late) return <div className="storage-pane" aria-busy="true" />;
  return (
    <div className="storage-pane" aria-busy={!settled || undefined}>
      {error && <p className="notice error" role="alert"><Icon name="info" size={14} />{error}</p>}
      {status?.tone === "ok" && <p className="form-status" role="status">{status.text}</p>}
      <section className="settings-card settings-section">
        <div className="settings-section-meta"><h4>总量</h4><p className="hint">{data ? "数据库、媒体缓存、对话引擎与自动备份的合计。" : "统计需要读一遍数据库，大库可能要几秒，期间可以继续使用。"}</p></div>
        <div className="settings-section-body">
          {data ? <>
            <div className="storage-total"><span className="storage-total-num">{data.total}</span><span className="hint">{data.summary}</span></div>
            <Composition parts={composition} />
          </> : query.isError ? <p className="hint" role="status">未能统计</p> : <StorageSkeleton />}
          <div className="storage-scan">
            {data?.scanned_at ? <span className="hint">统计于 {scannedAt(data.scanned_at)}</span> : null}
            <button type="button" className="btn sm" disabled={refreshing || busy || backupBusy} onClick={() => void refresh()}>
              <Icon name="refresh" size={14} className={refreshing ? "spin" : undefined} />重新统计
            </button>
          </div>
          {data ? <DataLocation path={data.data_dir} /> : null}
        </div>
      </section>
      {!settled ? <section className="storage-group settings-card settings-section storage-skeleton" aria-hidden="true">
        <div className="settings-section-meta"><i style={{ width: 72 }} /></div>
        <div className="settings-section-body">{[0, 1, 2, 3].map((n) => <div key={n} className="storage-row"><i /><i /><span /><i /></div>)}</div>
      </section> : null}
      {data?.groups.map((group) => <StorageGroup key={group.key} group={group} disabled={busy || refreshing || backupBusy} pending={pending} onClean={clean} />)}
      <RetentionSection onAsk={onAsk} onEditStateChange={report.retention} onChanged={onChanged} blocked={busy || parts.backup} />
      <BackupSection onAsk={onAsk} onEditStateChange={report.backup} onChanged={onChanged} blocked={busy || parts.retention} />
    </div>
  );
}
