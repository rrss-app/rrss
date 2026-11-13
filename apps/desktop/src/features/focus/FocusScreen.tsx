import { memo, useCallback, useEffect, useRef, useState } from "react";
import { FocusPane } from "../../components/FocusPane";
import type { SignalSurface } from "../../bindings";
import { api, copyText, type ArticleView, type CitationView } from "../../lib/api";
import { trackEvent } from "../../lib/signals";
import { navigableRowIndices, rowKey } from "../../lib/focusUtils";
import { isTypingTarget } from "../../lib/shortcuts";
import { useWindowKeydown } from "../../lib/useWindowKeydown";
import { useFocusSnapshot, type FocusNotice, type FocusSession } from "./focusSession";
import { useExposureTracker } from "../../lib/useExposureTracker";
import { HiddenListDialog, HiddenMenuItem, type HiddenScope } from "./HiddenListDialog";
import { ClusterSummary } from "./ClusterSummary";
import { createClusterSummaries, useClusterSummaries } from "./clusterSummaries";

export const FocusScreen = memo(function FocusScreen({ session, active, shortcuts, onOpen, onCite, onCiteGroup, onRate, onOpenDigest, onOpenSearch, onOpenFeed, onManageModels, notify, surface = "focus", column }: {
  session: FocusSession;
  active: boolean;
  shortcuts: boolean;
  /** 曝光来源。栏目为 `focus_column`，主界面缺省 `focus`。 */
  surface?: SignalSurface;
  /** 栏目刊头与隐藏列表范围。缺省是主界面今日焦点。`unread` 是文件夹的可见未读数（与订阅树同一个数），还没读到时不写。 */
  column?: { scope: string; name: string; unread?: number };
  onOpen: (id: number) => void;
  onCite: (citation: CitationView) => void;
  /** 相近报道组尾「引用整组到对话」：各篇作为文章引用，交给外层放进新对话。 */
  onCiteGroup?: (citations: CitationView[]) => void;
  onRate: (article: ArticleView, up: boolean) => void;
  onOpenDigest: () => void;
  onOpenSearch: () => void;
  /** 卡片来源名：进入订阅模式并选中这个源。 */
  onOpenFeed: (feedId: number) => void;
  /** 综述没配摘要模型时的「去设置」：打开设置的模型页。 */
  onManageModels?: () => void;
  notify: (notice: FocusNotice) => void;
}) {
  const state = useFocusSnapshot(session);
  const scrollRoot = useRef<HTMLDivElement>(null);
  const observeExposure = useExposureTracker({ surface, root: scrollRoot, active: active && shortcuts });
  const hiddenScope: HiddenScope | undefined = column
    ? { scope: column.scope, listScope: state.page?.list_scope ?? "all", label: column.name } : undefined;
  const columnHeading = column ? {
    name: column.name,
    note: column.unread === undefined ? `只看「${column.name}」文件夹` : `${column.unread} 篇未读 · 只看「${column.name}」文件夹`,
  } : undefined;
  const [hiddenOpen, setHiddenOpen] = useState(false);
  // 相近报道综述：按簇读回 / 生成，换栏目范围时作废。
  const summaryScope = useRef(column?.scope);
  summaryScope.current = column?.scope;
  const [summaries] = useState(() => createClusterSummaries(() => {
    const scope = summaryScope.current;
    return scope && scope !== "all" ? scope : undefined;
  }, (id) => session.clearClusterFresh(id)));
  const summaryEntries = useClusterSummaries(summaries);
  useEffect(() => { summaries.reset(); }, [column?.scope, summaries]);
  // 展开一组就读回它的综述状态；重读焦点后再读一次，「有新报道」随之更新。
  useEffect(() => {
    if (state.expandedCluster != null) void summaries.load(state.expandedCluster);
  }, [state.expandedCluster, state.revision, state.dataRevision, summaries]);
  const [keyboard, setKeyboard] = useState({ revision: -1, key: null as string | null, returned: 0 });
  const focusedRowIndex = keyboard.revision === state.revision
    ? (state.page?.articles.findIndex(row => rowKey(row) === keyboard.key) ?? -1) : -1;
  const clearFocus = useCallback(() => setKeyboard({ revision: -1, key: null, returned: 0 }), []);
  useEffect(() => { session.start(); return session.stop; }, [session]);
  useEffect(() => {
    const el = scrollRoot.current;
    if (!el) return;
    const unbind = session.bindViewport(() => el.scrollTop, () => { el.scrollTop = 0; });
    // 回顶部的程序滚动不算浏览；监听保留到隐藏期间，状态由会话持有。
    const scrolled = () => { if (el.scrollTop > 0) session.startBrowsing(); };
    const wheeled = (event: WheelEvent) => { if (event.deltaY > 0) session.startBrowsing(); };
    el.addEventListener("scroll", scrolled, { passive: true });
    el.addEventListener("wheel", wheeled, { passive: true });
    el.addEventListener("touchmove", session.startBrowsing, { passive: true });
    return () => {
      unbind();
      el.removeEventListener("scroll", scrolled);
      el.removeEventListener("wheel", wheeled);
      el.removeEventListener("touchmove", session.startBrowsing);
    };
  }, [session]);
  useEffect(() => { if (focusedRowIndex < 0 && keyboard.key !== null) clearFocus(); }, [focusedRowIndex, keyboard.key, clearFocus]);
  useEffect(() => { if (!shortcuts) clearFocus(); }, [shortcuts, clearFocus]);
  useEffect(() => {
    if (!shortcuts || state.loading || !state.readerReturn) return;
    const row = state.page?.articles.find(row => row.row_kind === 0 && row.id === state.readerReturn!.id);
    setKeyboard({ revision: state.revision, key: row ? rowKey(row) : null, returned: state.readerReturn.sequence });
    session.clearReaderReturn();
  }, [shortcuts, state.loading, state.readerReturn, state.page, state.revision, session]);
  useEffect(() => {
    if (!active) return;
    window.addEventListener("pointerdown", clearFocus);
    return () => window.removeEventListener("pointerdown", clearFocus);
  }, [active, clearFocus]);

  useWindowKeydown(shortcuts && !hiddenOpen, (event) => {
    if (event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey || isTypingTarget(document.activeElement)) return;
    const key = event.key.toLowerCase();
    if (key === "r") { event.preventDefault(); void session.refresh(); return; }
    const rows = state.page?.articles ?? [];
    const indices = navigableRowIndices(rows);
    if (!indices.length || state.loading) return;
    const position = indices.indexOf(focusedRowIndex);
    if (["j", "k", "arrowdown", "arrowup"].includes(key)) {
      event.preventDefault();
      const forward = key === "j" || key === "arrowdown";
      const next = position < 0 ? 0 : Math.max(0, Math.min(position + (forward ? 1 : -1), indices.length - 1));
      session.startBrowsing();
      setKeyboard({ revision: state.revision, key: rowKey(rows[indices[next]]), returned: 0 });
      return;
    }
    const row = rows[focusedRowIndex];
    if (!row || row.row_kind !== 0) return;
    if (key === "enter" || key === "o") { event.preventDefault(); clearFocus(); onOpen(row.id); }
    if (key === "s") { event.preventDefault(); void session.toggleStar(row.id, row.starred); }
  });

  const cite = async (id: number) => {
    const row = session.findArticle(id);
    try {
      onCite(await api.citeArticle(id));
      trackEvent(id, "cite");
      notify({ message: `已引用《${row?.title ?? id}》` });
    } catch {
      if (row) {
        onCite({ kind: "article", article_id: id, title: row.title, feed_title: row.feed_title, url: row.url, excerpt: "", block_id: 0 });
        notify({ message: `已引用《${row.title}》` });
      }
    }
  };

  const citeGroup = async (clusterId: number) => {
    const rows = (state.page?.articles ?? []).filter((row) => row.row_kind === 0 && row.cluster_id === clusterId);
    if (!rows.length || !onCiteGroup) return;
    const citations = await Promise.all(rows.map(async (row): Promise<CitationView> => {
      try { return await api.citeArticle(row.id); }
      catch { return { kind: "article", article_id: row.id, title: row.title, feed_title: row.feed_title, url: row.url, excerpt: "", block_id: 0 }; }
    }));
    rows.forEach((row) => trackEvent(row.id, "cite"));
    onCiteGroup(citations);
  };

  const summarize = (clusterId: number) => {
    if (state.expandedCluster !== clusterId) void session.setExpanded(clusterId);
    void summaries.summarize(clusterId);
  };
  // 有综述（生成中、已生成、有新报道）时，展开的组把代表文章收成成员行（2026-11-08 方案 C）；
  // 「汇总这 N 篇」和没配模型时的各家一句话不算，代表卡片照旧。
  const clusterSummaryShown = (clusterId: number) => {
    const entry = summaryEntries.get(clusterId) ?? summaries.get(clusterId);
    return entry.generating || (!!entry.view && entry.view.state !== "none");
  };
  const clusterSummary = (clusterId: number, representativeId?: number) => (
    <ClusterSummary entry={summaryEntries.get(clusterId) ?? summaries.get(clusterId)} representativeId={representativeId}
      onGenerate={() => void summaries.generate(clusterId)} onRetryLoad={() => void summaries.load(clusterId)}
      onOpen={(id) => { clearFocus(); onOpen(id); }} onManageModels={onManageModels}
      onCopy={(text) => void copyText(text).then(() => notify({ message: "已复制综述" }), (err) => notify({ message: `复制失败：${String(err)}` }))} />
  );

  // 离开主视图只隐藏展示，滚动位置留在容器上；范围快照仍由本域唯一 session 保留。
  return <>{hiddenOpen && <HiddenListDialog scope={hiddenScope} onClose={() => setHiddenOpen(false)}
    onOpenArticle={(id) => { clearFocus(); onOpen(id); }} onChanged={() => void session.reload()} />}
  <FocusPane visible={active} columnHeading={columnHeading}
    hiddenMenuItem={(close) => <HiddenMenuItem scope={hiddenScope} onSelect={() => { close(); setHiddenOpen(true); }} />} scrollRootRef={scrollRoot} onObserveArticle={observeExposure}
    spotReviews={state.spotReviews} spotBusy={state.spotBusy} onSpotAction={(id, action) => void session.decideSpotCheck(id, action)}
    page={state.page} feeds={state.feeds} expandedCluster={state.expandedCluster}
    onFilter={session.setFilter} onListScope={session.setListScope} onExpand={session.setExpanded}
    onMarkGroupRead={(id) => void session.markGroupRead(id)} onCiteGroup={onCiteGroup ? (id) => void citeGroup(id) : undefined}
    onSummarize={summarize} clusterSummary={clusterSummary} clusterSummaryShown={clusterSummaryShown}
    onRanking={session.setRanking} onMarkAllRead={session.markAllRead} onRefresh={session.refresh} refreshing={state.refreshing}
    onLoadMore={session.loadMore} loadingMore={state.loadingMore} loading={state.loading} moreError={state.moreError} revision={state.revision}
    newCount={state.pendingNew} onShowNew={() => { clearFocus(); void session.reload(); }}
    focusedRowIndex={focusedRowIndex} returnFocus={focusedRowIndex >= 0 ? keyboard.returned : 0} onOpen={(id) => { clearFocus(); onOpen(id); }} onCite={(id) => void cite(id)}
    onRate={onRate} onStar={(row) => void session.toggleStar(row.id, row.starred)} onOpenDigest={column ? undefined : onOpenDigest} onOpenSearch={onOpenSearch} onSource={(feedId) => { clearFocus(); onOpenFeed(feedId); }}
  /></>;
});
