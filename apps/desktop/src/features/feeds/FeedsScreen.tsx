import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { useQuery } from "@tanstack/react-query";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { ArticleView, FeedTree } from "../../lib/api";
import { feedQueryOptions } from "../../lib/feedQuery";
import { formatDay, formatPublished, formatPublishedFull, useMinuteClock } from "../../lib/time";
import { useRetainedScroll } from "../../lib/useRetainedScroll";
import { useViewEnter } from "../../lib/motion";
import { useExposureTracker, type ExposureObserver } from "../../lib/useExposureTracker";
import { Icon } from "../../components/Icon";
import { Segmented } from "../../components/Segmented";
import { SourceAvatar } from "../../components/ArticleCard";
import { HiddenListDialog } from "../focus";
import { feedsInScope, scopeTitle, scopeUnread } from "./feedTree";
import { useFeedsSession, type FeedsSession } from "./feedsSession";

const LIST_SCOPES = [
  { id: "all", label: "全部" },
  { id: "unread", label: "未读" },
  { id: "starred", label: "收藏" },
] as const;

type Item = { kind: "day"; label: string } | { kind: "row"; row: ArticleView; position: number };

/** 按本地日期分组：组头与文章一起进虚拟列表。 */
function timelineItems(articles: ArticleView[], now: number): Item[] {
  const out: Item[] = [];
  let last = "";
  articles.forEach((row, position) => {
    const label = formatDay(row.published_at, now);
    if (label !== last) { out.push({ kind: "day", label }); last = label; }
    out.push({ kind: "row", row, position });
  });
  return out;
}

/** 范围内抓取失败或久未更新的源，空范围时逐条列出原因。 */
function troubledFeeds(tree: FeedTree | undefined, scope: string) {
  if (!tree) return [];
  return feedsInScope(tree, scope)
    .filter((item) => item.error || item.health === "failing" || item.health === "stale")
    .map((item) => ({ id: item.id, title: item.title, reason: item.error || item.health_detail || item.health_label }));
}

function MeasuredItem({ index, measure, observe, row, position, style, children }: {
  index: number; measure: (el: HTMLDivElement | null) => void; observe: ExposureObserver;
  row?: ArticleView; position: number; style: CSSProperties; children: React.ReactNode;
}) {
  const ref = useCallback((el: HTMLDivElement | null) => {
    measure(el);
    if (row) observe(el, row.id, position, false);
  }, [measure, observe, row, position]);
  return <div ref={ref} data-index={index} style={style}>{children}</div>;
}

/** 订阅时间线：按发布时间倒序，没有分数、原因与合辑，规则先过滤。 */
export const FeedsScreen = memo(function FeedsScreen({ session, active, onOpen, onFetch }: {
  session: FeedsSession;
  active: boolean;
  onOpen: (id: number) => void;
  /** 抓取全部订阅源；完成后重读时间线。 */
  onFetch: () => Promise<unknown>;
}) {
  const state = useFeedsSession(session);
  const tree = useQuery(feedQueryOptions()).data;
  const now = useMinuteClock();
  const root = useRef<HTMLDivElement>(null);
  const scroll = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const [scrollMargin, setScrollMargin] = useState(0);
  const [hiddenOpen, setHiddenOpen] = useState(false);
  const [fetching, setFetching] = useState(false);
  const observe = useExposureTracker({ surface: "feeds", root: scroll, active });
  const items = useMemo(() => timelineItems(state.articles, now), [state.articles, now]);
  const title = scopeTitle(tree, state.scope);
  const unread = scopeUnread(state.counts, state.scope);
  const singleFeed = state.scope.startsWith("feed:");

  useEffect(() => { session.start(); }, [session]);
  useRetainedScroll(root, active);
  // 切回这个主栏目：新内容淡入并上移 4px，旧内容不做退场。
  useViewEnter(root, active);
  const virtual = useVirtualizer({
    count: items.length,
    getScrollElement: () => scroll.current,
    estimateSize: (i) => items[i]?.kind === "day" ? 36 : 76,
    overscan: 10,
    getItemKey: (i) => { const item = items[i]; return item?.kind === "row" ? `r${item.row.id}` : `d${i}-${item?.label}`; },
    useCachedMeasurements: !active,
    scrollMargin,
  });

  // 页眉随列表滚走，虚拟列表扣掉它的高度。
  useLayoutEffect(() => {
    const el = scroll.current;
    const top = list.current;
    if (!active || !el || !top) return;
    const measure = () => {
      if (!el.clientHeight) return;
      const next = Math.round(top.getBoundingClientRect().top - el.getBoundingClientRect().top + el.scrollTop);
      setScrollMargin((old) => old === next ? old : next);
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [active]);

  // 换范围或列表范围后回到会话给的位置：看过的范围回到上次离开处，否则回到顶部。
  const restoreTop = useRef(state.restoreTop);
  restoreTop.current = state.restoreTop;
  useLayoutEffect(() => { if (scroll.current) scroll.current.scrollTop = restoreTop.current; }, [state.revision]);
  useEffect(() => {
    const el = scroll.current;
    if (!active || !el) return;
    const note = () => session.noteScroll(el.scrollTop);
    el.addEventListener("scroll", note, { passive: true });
    return () => el.removeEventListener("scroll", note);
  }, [active, session]);

  // 接近底部时续页；没有填满一屏时也续。
  const requestedFor = useRef(-1);
  useEffect(() => {
    const el = scroll.current;
    if (!active || !el || state.loading || state.loadingMore || state.moreError || !state.hasMore) return;
    const maybeLoad = () => {
      if (requestedFor.current === state.articles.length) return;
      if (el.scrollHeight - el.scrollTop - el.clientHeight < 480 || el.scrollHeight <= el.clientHeight + 80) {
        requestedFor.current = state.articles.length;
        void session.loadMore();
      }
    };
    maybeLoad();
    el.addEventListener("scroll", maybeLoad, { passive: true });
    return () => el.removeEventListener("scroll", maybeLoad);
  }, [active, session, state.loading, state.loadingMore, state.moreError, state.hasMore, state.articles.length]);

  // 吸顶组头：取视口顶部那一项所在的日期。
  const firstVisible = virtual.getVirtualItems().find((v) => v.end > (virtual.scrollOffset ?? 0));
  let sticky = "";
  if (firstVisible && (virtual.scrollOffset ?? 0) > scrollMargin) {
    for (let i = firstVisible.index; i >= 0; i--) { const item = items[i]; if (item?.kind === "day") { sticky = item.label; break; } }
  }

  const fetchAll = async () => {
    if (fetching) return;
    setFetching(true);
    try { await onFetch(); await session.refresh(); } finally { setFetching(false); }
  };
  const troubled = state.articles.length === 0 && !state.loading ? troubledFeeds(tree, state.scope) : [];

  return <div className="main-view feeds-view" ref={root} hidden={!active} inert={!active}>
    {hiddenOpen ? <HiddenListDialog scope={{ scope: state.scope, listScope: state.listScope, label: title.name }} onClose={() => setHiddenOpen(false)}
      onOpenArticle={(id) => { setHiddenOpen(false); onOpen(id); }} onChanged={() => void session.refresh()} /> : null}
    <div className="toolbar" data-tauri-drag-region="deep">
      <Segmented className="toolbar-group segmented" role="group" aria-label="阅读范围">
        {LIST_SCOPES.map((item) => <button key={item.id} type="button" className="tab" data-on={state.listScope === item.id ? "1" : "0"}
          onClick={() => { if (item.id !== state.listScope) void session.setListScope(item.id); }}>{item.label}</button>)}
      </Segmented>
      <span className="toolbar-spacer" />
      <div className="toolbar-actions">
        <button type="button" className="tab tool" title="把当前范围内可见的未读标为已读" aria-label="标为已读"
          disabled={!unread} onClick={() => void session.markScopeRead()}>
          <Icon name="check" size={15} /><span className="tool-label">标为已读</span>
        </button>
        <button type="button" className="tab tool" title="抓取全部订阅源" aria-label="刷新" aria-busy={fetching} onClick={() => void fetchAll()}>
          <Icon name="refresh" size={15} /><span className="tool-label">刷新</span>
        </button>
      </div>
    </div>
    <div className="scroll feeds-scroll" ref={scroll} tabIndex={-1}>
      <div className="feeds-column">
        <header className="feeds-head">
          <div className="feeds-path">订阅{title.path.length ? ` · ${title.path.join(" › ")}` : ""}</div>
          <h1>{title.name}<small>{unread} 篇未读 · 按时间</small></h1>
        </header>
        <div className="feeds-sticky" aria-hidden="true">{sticky ? <div className="feeds-day" data-sticky="1">{sticky}</div> : null}</div>
        {state.articles.length === 0 && !state.loading ? <div className="feeds-empty">
          <p>这个范围没有文章</p>
          {troubled.map((feed) => <small key={feed.id}>{feed.title}：{feed.reason}</small>)}
        </div> : null}
        <div className="feeds-list" ref={list} aria-busy={state.loading} style={{ height: virtual.getTotalSize(), position: "relative" }}>
          {virtual.getVirtualItems().map((v) => {
            const item = items[v.index];
            if (!item) return null;
            const style: CSSProperties = { position: "absolute", top: 0, left: 0, width: "100%", transform: `translateY(${v.start - scrollMargin}px)` };
            if (item.kind === "day") return <MeasuredItem key={v.key} index={v.index} measure={virtual.measureElement} observe={observe} position={-1} style={style}>
              <div className="feeds-day">{item.label}</div>
            </MeasuredItem>;
            const row = item.row;
            return <MeasuredItem key={v.key} index={v.index} measure={virtual.measureElement} observe={observe} row={row} position={item.position} style={style}>
              <button type="button" className="feeds-item" data-read={row.read ? "1" : "0"} data-article-id={row.id} onClick={() => onOpen(row.id)}>
                <span className="feeds-item-dot" aria-label={row.read ? "已读" : "未读"} />
                <span className="feeds-item-body">
                  <span className="feeds-item-title">{row.title}</span>
                  {row.summary ? <span className="feeds-item-summary">{row.summary}</span> : null}
                  {singleFeed ? null : <span className="feeds-item-source"><SourceAvatar title={row.feed_title} iconHref={row.feed_icon_href} /><span>{row.feed_title}</span></span>}
                </span>
                <time dateTime={row.published_at ? new Date(row.published_at).toISOString() : undefined} title={formatPublishedFull(row.published_at)}>
                  {formatPublished(row.published_at, now)}
                </time>
              </button>
            </MeasuredItem>;
          })}
        </div>
        {state.moreError ? <div className="notice error" role="alert">
          <span>{state.moreError}</span>
          <button type="button" className="btn sm" onClick={() => void session.loadMore()} disabled={state.loadingMore}>重试加载更多</button>
        </div> : null}
        {!state.hasMore && !state.loading && state.hiddenCount > 0 ? <button type="button" className="feeds-hidden" onClick={() => setHiddenOpen(true)}>
          另有 {state.hiddenCount} 篇被规则隐藏
        </button> : null}
      </div>
    </div>
  </div>;
});
