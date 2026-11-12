import { useRef, useState } from "react";
import { queryOptions, useQuery, useQueryClient } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import { api } from "../../lib/api";
import { formatNotice } from "../../lib/notice";
import { formatPublished, useMinuteClock } from "../../lib/time";
import { Dialog } from "../../settings/Dialog";
import { ChipBar } from "../../settings/chips";
import { settingsSchemaOptions } from "../../settings/settingsQuery";
import { settingsChanged, useSettingsWrite } from "../../settings/editState";
import { rulesKeys } from "../../settings/rulesQuery";
import { MenuButton } from "../../components/MenuSelect";
import { Icon } from "../../components/Icon";
import { Pending } from "../../components/Pending";

export type HiddenListFilter = "all" | "spot_liked" | number;

/** 焦点「显示」菜单的条数与弹层共用一份读取。 */
/** 订阅模式的隐藏列表范围；`label` 给弹层里的「当前范围」。 */
export type HiddenScope = { scope: string; listScope: string; label: string };
export const hiddenQueryOptions = (scope?: HiddenScope) => queryOptions({
  queryKey: scope ? ["hidden", "scope", scope.scope, scope.listScope] : ["hidden", "all"],
  queryFn: () => scope ? api.hiddenInScope(scope.scope, scope.listScope) : api.hidden(),
  retry: false, refetchOnWindowFocus: false,
});

/** 菜单打开时才读取条数；与隐藏列表弹层共用缓存。 */
export function HiddenMenuItem({ onSelect, scope }: { onSelect: () => void; scope?: HiddenScope }) {
  const hidden = useQuery(hiddenQueryOptions(scope));
  return <button type="button" role="menuitem" onClick={onSelect}>
    <Icon name="check" size={13} className="menu-check" /><span className="menu-label">被隐藏的文章</span>
    {hidden.data ? <span className="menu-count">{hidden.data.articles.length}</span> : null}
  </button>;
}

export function HiddenListDialog({ scope, initialFilter = "all", onOpenArticle, onClose, onChanged }: {
  /** 订阅模式：按订阅范围与列表范围读取；不给时为全部订阅、焦点的列表范围。 */
  scope?: HiddenScope;
  initialFilter?: HiddenListFilter;
  onOpenArticle: (id: number) => void;
  onClose: () => void;
  /** The focus session owns its reads; notify it after a successful release. */
  onChanged?: () => void;
}) {
  const client = useQueryClient();
  const query = useQuery(hiddenQueryOptions(scope));
  const schema = useQuery(settingsSchemaOptions());
  const now = useMinuteClock();
  const [filter, setFilter] = useState(String(initialFilter));
  const [released, setReleased] = useState<Set<number>>(() => new Set());
  const [error, setError] = useState("");
  const { writing, run } = useSettingsWrite({ mutationKey: rulesKeys.write, mutationFn: async (action: () => Promise<unknown>) => {
    await action();
    await settingsChanged(client);
    void client.invalidateQueries({ queryKey: ["hidden"] });
    onChanged?.();
  } });
  const release = async (id: number, action: () => Promise<unknown>) => {
    setError("");
    const result = await run(action);
    if (result.status === "busy" || !result.mounted) return;
    if (result.status === "failed") setError(formatNotice("放行失败：", result.error));
    else setReleased(old => new Set(old).add(id));
  };
  const data = query.data;
  const rows = (data?.articles ?? []).filter(article => filter === "all" || (filter === "spot_liked"
    ? article.spot_liked : article.reasons.some(reason => String(reason.rule_id) === filter)));
  const scroll = useRef<HTMLDivElement>(null);
  const virtual = useVirtualizer({ count: rows.length, getScrollElement: () => scroll.current,
    estimateSize: () => 128, getItemKey: index => rows[index].id, overscan: 4 });
  const options = [{ id: "all", label: `全部 ${data?.articles.length ?? 0}` },
    ...(data?.spot_liked_count ? [{ id: "spot_liked", label: `抽查时你赞过 ${data.spot_liked_count}` }] : []),
    ...(data?.rule_counts ?? []).map(rule => ({ id: String(rule.rule_id), label: `${rule.label} ${rule.count}` }))];
  const threshold = schema.data?.values.find(value => value.id === "hide_threshold")?.value ?? "…";
  const listScope = scope?.listScope ?? schema.data?.values.find(value => value.id === "list_scope")?.value;
  return <Dialog title="被隐藏的文章" dirty={false} busy={writing} onAsk={() => {}} onClose={onClose}
    actions={null} cancelLabel="关闭" className="wide hidden-list-dialog"
    leading={<small className="hidden-list-hint">这里只是列出来，不会改动任何东西。想放行某篇，选一项后才生效。</small>}>
    <p className="hint">当前范围：{scope ? scope.label : "全部订阅"}{listScope === "unread" ? " · 未读" : listScope === "starred" ? " · 收藏" : ""} · 近 7 天 · 隐藏阈值 {threshold} · 只读</p>
    {query.isPending && <Pending>正在读取被隐藏的文章…</Pending>}
    {(query.isError || schema.isError || error) && <p className="notice error" role="alert">
      {error || formatNotice("读取失败：", query.error ?? schema.error)}
      {(query.isError || schema.isError) && <button className="btn sm" onClick={() => { void query.refetch(); void schema.refetch(); }}>重试读取</button>}
    </p>}
    {data && <>
      <div role="group" aria-label="按规则筛选"><ChipBar value={filter} options={options} onChange={setFilter} /></div>
      <div ref={scroll} className="hidden-list-scroll" data-scroll-boundary aria-busy={writing}>
        {!rows.length && <p className="settings-empty">此筛选下没有被隐藏的文章。</p>}
        <div role="list" aria-label="被隐藏的文章列表" style={{ height: virtual.getTotalSize(), position: "relative" }}>
          {virtual.getVirtualItems().map(item => {
            const article = rows[item.index];
            const done = released.has(article.id);
            const reason = article.reasons.find(reason => String(reason.rule_id) === filter) ?? article.reasons[0];
            return <div key={article.id} ref={virtual.measureElement} data-index={item.index} data-article-id={article.id}
              role="listitem" className="hidden-list-row" data-released={done || undefined}
              style={{ position: "absolute", top: 0, left: 0, width: "100%", transform: `translateY(${item.start}px)` }}>
              <div className="hidden-list-main">
                <strong>{article.title}</strong>
                <span className="muted">{article.feed_title} · {formatPublished(article.published_at, now) || "时间未知"}</span>
                <span className="hidden-list-reasons">{article.reasons.map((reason, index) => <span key={`${reason.rule_id}-${index}`}>
                  {reason.label} <b className="score down">{reason.weight}</b>{" · "}
                </span>)}总分 <b>{article.score}</b></span>
                {article.spot_liked && <span className="hint">抽查时放回焦点，你打开或赞了它。</span>}
                {done && <span className="hidden-list-released" role="status">已放行，会出现在焦点里。</span>}
              </div>
              <div className="hidden-list-actions">
                <button className="btn sm ghost" disabled={writing} onClick={() => { onClose(); onOpenArticle(article.id); }}>打开</button>
                {!done && <MenuButton label={`放行 ${article.title}`} face="放行…" className="btn sm" disabled={writing}>
                  {close => <>
                    <button role="menuitem" onClick={() => { close(); void release(article.id, () => api.releaseArticle(article.id)); }}>只放行这一篇</button>
                    <button role="menuitem" disabled={!reason} onClick={() => { close(); void release(article.id, () => api.setRuleAction(reason.rule_id, 2)); }}>停用「{reason?.label ?? "规则不可用"}」</button>
                    <button role="menuitem" disabled={!reason} onClick={() => { close(); void release(article.id, () => api.allowRuleForFeed(reason.rule_id, article.feed_id)); }}>对「{article.feed_title}」放行</button>
                  </>}
                </MenuButton>}
              </div>
            </div>;
          })}
        </div>
      </div>
    </>}
  </Dialog>;
}
