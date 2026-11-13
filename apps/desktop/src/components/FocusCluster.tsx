import type { ArticleView } from "../lib/api";
import type { ClusterSourceView } from "../bindings";
import { SourceAvatar } from "./ArticleCard";
import { useTranslation } from "../i18n";
import { Icon } from "./Icon";
import { RelativeTime } from "./RelativeTime";

/**
 * 相近报道的成组面板（阶段 14 · 相近报道成组与汇总）。焦点列表是虚拟化的行，面板按行拼：
 * 组头、代表、成员各占一行，组尾放在最后一行里；底板由 `FocusPane` 按这几行的位置画一块。
 */
export type ClusterGroup = { clusterId: number; first: number; last: number; open: boolean };
export type ClusterSlot = ClusterGroup & { role: "head" | "rep" | "member" };

/** 面板上下与相邻文章之间的留白；底板按它缩进，行在首尾补同样的内边距。 */
export const PANEL_GAP = 12;

/** 每行在哪个组、是组头 / 代表 / 成员；不在组里为 null。组从标题行开始，接着的同簇文章都算这一组。 */
export function clusterLayout(rows: ArticleView[], expanded: number | null): { slots: (ClusterSlot | null)[]; groups: ClusterGroup[] } {
  const slots: (ClusterSlot | null)[] = rows.map(() => null);
  const groups: ClusterGroup[] = [];
  for (let i = 0; i < rows.length; i += 1) {
    const row = rows[i];
    if (row.row_kind !== 1 || !row.cluster_id) continue;
    let end = i + 1;
    while (end < rows.length && rows[end].row_kind === 0 && rows[end].cluster_id === row.cluster_id) end += 1;
    const group = { clusterId: row.cluster_id, first: i, last: end - 1, open: expanded === row.cluster_id };
    groups.push(group);
    for (let k = i; k < end; k += 1) slots[k] = { ...group, role: k === i ? "head" : k === i + 1 ? "rep" : "member" };
    i = end - 1;
  }
  return { slots, groups };
}

/** 这组的来源：宿主在标题行上给出（收起时也有）；没有时用已展开的成员按先后去重。 */
export function groupSources(rows: ArticleView[], group: ClusterGroup): ClusterSourceView[] {
  const given = rows[group.first]?.cluster_sources;
  if (given?.length) return given;
  const seen = new Set<number>();
  return rows.slice(group.first + 1, group.last + 1).flatMap((row) => {
    if (row.row_kind !== 0 || seen.has(row.feed_id)) return [];
    seen.add(row.feed_id);
    return [{ feed_id: row.feed_id, title: row.feed_title, icon_href: row.feed_icon_href }];
  });
}

/**
 * 组头：事件名、「相近报道 · N 篇 · M 个来源 · 起止日期」与叠放的来源头像（只有一个来源时不写）。
 * 整条都能点；键盘与读屏走右侧的按钮。
 */
export function ClusterHead({ row, open, sources, onExpand, onSummarize }: {
  row: ArticleView;
  open: boolean;
  sources: ClusterSourceView[];
  onExpand: (id: number | null) => void;
  /** 「汇总」：展开面板并带出综述，还没生成就直接生成。 */
  onSummarize?: (id: number) => void;
}) {
  const { t } = useTranslation("focus");
  const toggle = () => onExpand(open ? null : row.cluster_id);
  const others = Math.max(0, row.cluster_size - 1);
  const shown = sources.length > 1 ? sources : [];
  return (
    <div className="cluster-head" onClick={toggle}>
      <Icon name="layers" size={14} />
      <span className="cluster-label">
        {row.cluster_label ? <b title={row.cluster_label}>{row.cluster_label}</b> : null}
        <span className="cluster-note">
          {t("clusterHead.note", { size: row.cluster_size, sources: shown.length ? t("clusterHead.noteSources", { count: shown.length }) : "", span: row.cluster_span ? ` · ${row.cluster_span}` : "" })}
        </span>
      </span>
      {row.cluster_fresh ? <span className="cluster-fresh" title={t("clusterHead.freshTitle")}>{t("clusterHead.fresh", { count: row.cluster_fresh })}</span> : null}
      {shown.length ? <span className="cluster-stack" aria-hidden="true">
        {shown.slice(0, 5).map((source) => <SourceAvatar key={source.feed_id} title={source.title} iconHref={source.icon_href} />)}
      </span> : null}
      {onSummarize ? (
        <button type="button" className="cluster-summarize" onClick={(event) => { event.stopPropagation(); onSummarize(row.cluster_id); }}>
          <Icon name="sparkles" size={13} />{t("clusterHead.summarize")}
        </button>
      ) : null}
      <button type="button" className="cluster" aria-expanded={open}
        aria-label={t(open ? "clusterHead.ariaOpen" : "clusterHead.ariaClosed", { label: row.cluster_label, size: row.cluster_size })}
        onClick={(event) => { event.stopPropagation(); toggle(); }}>
        {open ? t("clusterHead.collapse") : t("clusterHead.expand", { count: others })}<Icon name="chevronDown" size={12} />
      </button>
    </div>
  );
}

/**
 * 展开后的其余成员：一行「来源头像 · 来源 · 标题 · 相对时间」，已读变灰，点开就读。
 * 有综述时代表文章也收成这样一行，排第一并标「代表」（阶段 14 · 2026-11-08 综述位置方案 C）。
 */
export function ClusterMember({ row, focused = false, representative = false, onOpen }: {
  row: ArticleView; focused?: boolean; representative?: boolean; onOpen: (id: number) => void;
}) {
  const { t } = useTranslation("focus");
  return (
    <article
      className={`article cluster-member${row.read ? "" : " unread"}${focused ? " focused" : ""}`}
      tabIndex={0}
      aria-label={row.title}
      onClick={() => onOpen(row.id)}
      onKeyDown={(event) => {
        if (event.target === event.currentTarget && (event.key === "Enter" || event.key === " ")) {
          event.preventDefault();
          event.stopPropagation();
          onOpen(row.id);
        }
      }}
    >
      <SourceAvatar title={row.feed_title} iconHref={row.feed_icon_href} />
      <span className="cluster-member-source" title={row.feed_title}>{row.feed_title}</span>
      {representative
        ? <span className="cluster-member-main"><h3 className="cluster-member-title">{row.title}</h3><span className="cluster-member-rep">{t("clusterMember.representative")}</span></span>
        : <h3 className="cluster-member-title">{row.title}</h3>}
      {row.published_at != null ? <RelativeTime className="cluster-member-time" ms={row.published_at} /> : <span />}
    </article>
  );
}

/** 组尾：「收起 · 共 N 篇」，右侧整组标已读（直接生效、不带撤销）与引用整组到对话。 */
export function ClusterFoot({ count, onCollapse, onMarkRead, onCite }: {
  count: number;
  onCollapse: () => void;
  onMarkRead?: () => void;
  onCite?: () => void;
}) {
  const { t } = useTranslation("focus");
  return (
    <div className="cluster-foot">
      <button type="button" className="cluster-foot-btn" onClick={onCollapse}><Icon name="chevronDown" size={12} className="cluster-up" />{t("clusterFoot.collapse")}</button>
      <span>{t("clusterFoot.total", { count })}</span>
      <span className="cluster-foot-spacer" />
      {onMarkRead ? <button type="button" className="cluster-foot-btn" onClick={onMarkRead}><Icon name="check" size={13} />{t("clusterFoot.markRead")}</button> : null}
      {onCite ? <button type="button" className="cluster-foot-btn" onClick={onCite}><Icon name="quote" size={13} />{t("clusterFoot.cite")}</button> : null}
    </div>
  );
}
