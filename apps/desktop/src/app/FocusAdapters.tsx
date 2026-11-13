import { forwardRef, useMemo, type ComponentProps } from "react";
import { useQuery } from "@tanstack/react-query";
import { ReaderHost, type ReaderHostHandle } from "../features/reader";
import { FocusScreen, useFocusRanking, useFocusSnapshot, type FocusSession } from "../features/focus";
import { scopeTitle, scopeUnread, useFeedsSession, type FeedsSession } from "../features/feeds";
import { useTranslation } from "../i18n";
import { feedQueryOptions } from "../lib/feedQuery";
import { SettingsPanel } from "../settings/SettingsPanel";

/** 阅读器的上下篇跟随打开它的列表：主焦点、栏目或订阅时间线。 */
export type ReaderList = "focus" | "column" | "feeds";

/** 跨域适配只在使用者处订阅快照，根壳不复制文章列表。 */
export const FocusReader = forwardRef<ReaderHostHandle, Omit<ComponentProps<typeof ReaderHost>, "nav" | "listVia" | "listTitle" | "onReadChange" | "onToggleStar" | "onRate" | "onDislike"> & { focus: FocusSession; column: FocusSession; feeds: FeedsSession; list: ReaderList; columnName: string; onRateArticle: ComponentProps<typeof ReaderHost>["onRate"]; onDislikeArticle: ComponentProps<typeof ReaderHost>["onDislike"] }>(function FocusReader({ focus, column, feeds, list, columnName, onRateArticle, onDislikeArticle, ...props }, ref) {
  const { t } = useTranslation("focus");
  useFocusSnapshot(focus);
  useFocusSnapshot(column);
  const feedsState = useFeedsSession(feeds);
  const tree = useQuery(feedQueryOptions()).data;
  const source = list === "column" ? column : focus;
  const feedsNav = () => { const s = feeds.getSnapshot(); return { articles: s.articles, hasMore: s.hasMore, loadingMore: s.loadingMore, scope: s.listScope, revision: s.revision }; };
  const nav = list === "feeds"
    ? { ...feedsNav(), loadMore: feeds.loadMore, setScope: feeds.setListScope, getCurrent: feedsNav }
    : { ...source.getNavigation(), loadMore: source.loadMore, setScope: source.setListScope, setExpanded: source.setExpanded, setCurrent: source.readerAt, getCurrent: source.getNavigation };
  // 已读与收藏在打开它的列表里回写，另一个焦点列表只改本地行，下次读取时自然一致。
  const setRead = (id: number, read: boolean) => { focus.setRead(id, read); column.setRead(id, read); void feeds.setRead(id, read); };
  const toggleStar = async (id: number, starred: boolean) => {
    const ok = await source.toggleStar(id, starred);
    if (ok) { feeds.setStarred(id, !starred); (source === focus ? column : focus).setStarred(id, !starred); }
    return ok;
  };
  const listVia = list === "feeds" ? "feeds" : list === "column" ? "focus_column" : "focus";
  const listTitle = list === "feeds" ? scopeTitle(tree, feedsState.scope).name : list === "column" ? columnName || t("reader.columnFallback") : t("reader.focusTitle");
  return <ReaderHost {...props} ref={ref} nav={nav} listVia={listVia} listTitle={listTitle}
    onReadChange={setRead} onToggleStar={toggleStar}
    onRate={onRateArticle} onDislike={onDislikeArticle} />;
});

/** 栏目页：刊头的篇数用订阅树的文件夹未读数，订阅栏的计数只在这里订阅，根壳不跟着重绘。 */
export function FocusColumnScreen({ feeds, scope, name, ...props }: Omit<ComponentProps<typeof FocusScreen>, "column" | "surface"> & { feeds: FeedsSession; scope: string; name: string }) {
  const { counts } = useFeedsSession(feeds);
  const unread = counts ? scopeUnread(counts, scope) : undefined;
  const heading = useMemo(() => ({ scope, name, unread }), [scope, name, unread]);
  return <FocusScreen {...props} surface="focus_column" column={heading} />;
}

export function FocusSettings({ focus, ...props }: Omit<ComponentProps<typeof SettingsPanel>, "ranking" | "onRanking" | "onChanged"> & { focus: FocusSession }) {
  const ranking = useFocusRanking(focus);
  return <SettingsPanel {...props} ranking={ranking} onRanking={focus.setRanking} 
    onChanged={(tree) => void (tree ? focus.feedsChanged(tree) : focus.reload())} />;
}
