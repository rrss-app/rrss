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
import { useTranslation } from "../../i18n";
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
  const { t } = useTranslation("focus");
  const hidden = useQuery(hiddenQueryOptions(scope));
  return <button type="button" role="menuitem" onClick={onSelect}>
    <Icon name="check" size={13} className="menu-check" /><span className="menu-label">{t("hidden.title")}</span>
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
  const { t } = useTranslation("focus");
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
    if (result.status === "failed") setError(formatNotice(t("hidden.releaseFailed"), result.error));
    else setReleased(old => new Set(old).add(id));
  };
  const data = query.data;
  const rows = (data?.articles ?? []).filter(article => filter === "all" || (filter === "spot_liked"
    ? article.spot_liked : article.reasons.some(reason => String(reason.rule_id) === filter)));
  const scroll = useRef<HTMLDivElement>(null);
  const virtual = useVirtualizer({ count: rows.length, getScrollElement: () => scroll.current,
    estimateSize: () => 128, getItemKey: index => rows[index].id, overscan: 4 });
  const options = [{ id: "all", label: t("hidden.optionAll", { count: data?.articles.length ?? 0 }) },
    ...(data?.spot_liked_count ? [{ id: "spot_liked", label: t("hidden.optionSpotLiked", { count: data.spot_liked_count }) }] : []),
    ...(data?.rule_counts ?? []).map(rule => ({ id: String(rule.rule_id), label: `${rule.label} ${rule.count}` }))];
  const threshold = schema.data?.values.find(value => value.id === "hide_threshold")?.value ?? "…";
  const listScope = scope?.listScope ?? schema.data?.values.find(value => value.id === "list_scope")?.value;
  return <Dialog title={t("hidden.title")} dirty={false} busy={writing} onAsk={() => {}} onClose={onClose}
    actions={null} cancelLabel={t("hidden.close")} className="wide hidden-list-dialog"
    leading={<small className="hidden-list-hint">{t("hidden.hint")}</small>}>
    <p className="hint">{t("hidden.range", { scope: scope ? scope.label : t("hidden.allFeeds"), listScope: listScope === "unread" ? t("hidden.rangeUnread") : listScope === "starred" ? t("hidden.rangeStarred") : "", threshold })}</p>
    {query.isPending && <Pending>{t("hidden.loading")}</Pending>}
    {(query.isError || schema.isError || error) && <p className="notice error" role="alert">
      {error || formatNotice(t("hidden.loadFailed"), query.error ?? schema.error)}
      {(query.isError || schema.isError) && <button className="btn sm" onClick={() => { void query.refetch(); void schema.refetch(); }}>{t("hidden.retryLoad")}</button>}
    </p>}
    {data && <>
      <div role="group" aria-label={t("hidden.filterLabel")}><ChipBar value={filter} options={options} onChange={setFilter} /></div>
      <div ref={scroll} className="hidden-list-scroll" data-scroll-boundary aria-busy={writing}>
        {!rows.length && <p className="settings-empty">{t("hidden.empty")}</p>}
        <div role="list" aria-label={t("hidden.listLabel")} style={{ height: virtual.getTotalSize(), position: "relative" }}>
          {virtual.getVirtualItems().map(item => {
            const article = rows[item.index];
            const done = released.has(article.id);
            const reason = article.reasons.find(reason => String(reason.rule_id) === filter) ?? article.reasons[0];
            return <div key={article.id} ref={virtual.measureElement} data-index={item.index} data-article-id={article.id}
              role="listitem" className="hidden-list-row" data-released={done || undefined}
              style={{ position: "absolute", top: 0, left: 0, width: "100%", transform: `translateY(${item.start}px)` }}>
              <div className="hidden-list-main">
                <strong>{article.title}</strong>
                <span className="muted">{article.feed_title} · {formatPublished(article.published_at, now) || t("hidden.timeUnknown")}</span>
                <span className="hidden-list-reasons">{article.reasons.map((reason, index) => <span key={`${reason.rule_id}-${index}`}>
                  {reason.label} <b className="score down">{reason.weight}</b>{" · "}
                </span>)}{t("hidden.totalScore")}{" "}<b>{article.score}</b></span>
                {article.spot_liked && <span className="hint">{t("hidden.spotLikedNote")}</span>}
                {done && <span className="hidden-list-released" role="status">{t("hidden.releasedNote")}</span>}
              </div>
              <div className="hidden-list-actions">
                <button className="btn sm ghost" disabled={writing} onClick={() => { onClose(); onOpenArticle(article.id); }}>{t("hidden.open")}</button>
                {!done && <MenuButton label={t("hidden.releaseLabel", { title: article.title })} face={t("hidden.releaseFace")} className="btn sm" disabled={writing}>
                  {close => <>
                    <button role="menuitem" onClick={() => { close(); void release(article.id, () => api.releaseArticle(article.id)); }}>{t("hidden.releaseOne")}</button>
                    <button role="menuitem" disabled={!reason} onClick={() => { close(); void release(article.id, () => api.setRuleAction(reason.rule_id, 2)); }}>{t("hidden.disableRule", { label: reason?.label ?? t("hidden.ruleUnavailable") })}</button>
                    <button role="menuitem" disabled={!reason} onClick={() => { close(); void release(article.id, () => api.allowRuleForFeed(reason.rule_id, article.feed_id)); }}>{t("hidden.allowFeed", { feed: article.feed_title })}</button>
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
