import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode, type RefObject } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useRetainedScroll } from "../lib/useRetainedScroll";
import { reducedMotion, useViewEnter } from "../lib/motion";
import { useMinuteClock } from "../lib/time";
import type { ExposureObserver } from "../lib/useExposureTracker";
import type { ArticleView, FeedView, FocusPage, RankingSettings } from "../lib/api";
import { i18n, useTranslation } from "../i18n";
import { ArticleCard } from "./ArticleCard";
import { ArticleCardSkeleton } from "./ArticleCardSkeleton";
import { ClusterFoot, ClusterHead, ClusterMember, clusterLayout, groupSources, PANEL_GAP, type ClusterSlot } from "./FocusCluster";
import { Icon } from "./Icon";
import { MenuButton, MenuCheck } from "./MenuSelect";
import { Segmented } from "./Segmented";
import { useDelayed } from "./Pending";
import type { SpotAction, SpotReview } from "./SpotCheckReveal";

export type RankingToggle = "interest_boost" | "rerank" | "cluster_group" | "hide_noisy"
  | "rank_features" | "rank_source" | "rank_coverage" | "rank_freshness" | "rank_affinity" | "show_details";

/** B1 学来的排序分量（阶段 23）：只调先后，不影响隐藏。 */
const LEARNED: { key: RankingToggle & `rank_${string}` }[] = [
  { key: "rank_features" },
  { key: "rank_source" },
  { key: "rank_coverage" },
  { key: "rank_freshness" },
  { key: "rank_affinity" },
];

export const FILTERS = [
  { id: "focus" },
  { id: "hide_faded" },
  { id: "all" },
] as const;

export const FALLBACK_RANKING: RankingSettings = {
  show_details: false,
  interest_boost: true,
  rerank: false,
  cluster_group: true,
  hide_noisy: false,
  interest_ready: false,
  ranking_note: "",
  hide_threshold: 0,
  theme: "system",
  rank_features: true,
  rank_source: true,
  rank_coverage: true,
  rank_freshness: true,
  rank_affinity: true,
};

export const LIST_SCOPES = [
  { id: "all" },
  { id: "unread" },
  { id: "starred" },
] as const;

export function focusEmptyState(
  page: FocusPage | null,
  feeds: FeedView[],
): { message: string; action?: { label: string; filter?: string; scope?: string; ranking?: RankingToggle } } {
  const t = i18n.getFixedT(null, "focus");
  if (page === null) return { message: t("pane.empty.loading") };
  if ((page.articles ?? []).length > 0) return { message: "" };
  if (feeds.length === 0) return { message: t("pane.empty.noFeeds") };
  if (page.list_scope === "starred") {
    return { message: t("pane.empty.noStarred"), action: { label: t("pane.empty.viewAll"), scope: "all" } };
  }
  if (page.list_scope === "unread") {
    return { message: t("pane.empty.noUnread"), action: { label: t("pane.empty.viewAll"), scope: "all" } };
  }
  if (page.ranking?.hide_noisy && feeds.some((f) => f.noisy)) {
    return { message: t("pane.empty.noisyHidden"), action: { label: t("pane.empty.showNoisy"), ranking: "hide_noisy" } };
  }
  if (page.filter === "focus") {
    return { message: t("pane.empty.noFocus"), action: { label: t("pane.empty.viewAllArticles"), filter: "all" } };
  }
  return { message: t("pane.empty.none") };
}

export function noisyToggleLabel(feeds: FeedView[], hidden: boolean): string {
  const names = feeds.filter((f) => f.noisy).map((f) => f.title);
  if (names.length === 0) return "";
  const shown = names.slice(0, 2).join(i18n.t("list.enumSeparator", { ns: "focus" }));
  const key = names.length > 2 ? (hidden ? "pane.noisy.hiddenMore" : "pane.noisy.hideMore") : (hidden ? "pane.noisy.hidden" : "pane.noisy.hide");
  return i18n.t(key, { ns: "focus", names: shown });
}

export function emptyIcon(page: FocusPage | null, feeds: FeedView[]): "inbox" | "star" | "rss" | "checkAll" {
  if (feeds.length === 0) return "rss";
  if (page?.list_scope === "starred") return "star";
  if (page?.list_scope === "unread") return "checkAll";
  return "inbox";
}

function DatedMasthead() {
  const { t } = useTranslation("focus");
  const date = new Date(useMinuteClock());
  const digits = t("masthead.digits");
  const ten = t("masthead.ten");
  const chinese = (n: number) => n < 10 ? digits[n] : `${n >= 20 ? digits[Math.floor(n / 10)] : ""}${ten}${n % 10 ? digits[n % 10] : ""}`;
  return <header className="focus-masthead">
    <p className="focus-kicker">{t("masthead.date", { month: chinese(date.getMonth() + 1), day: chinese(date.getDate()), weekday: t("masthead.weekdays")[date.getDay()] })}</p>
    <h1>{t("masthead.title")}</h1>
  </header>;
}

function FocusMasthead({ column }: { column?: { name: string; note: string } }) {
  const { t } = useTranslation("focus");
  if (!column) return <DatedMasthead />;
  return <header className="focus-masthead">
    <p className="focus-kicker">{t("masthead.columnKicker")}</p>
    <h1>{column.name}<small>{column.note}</small></h1>
  </header>;
}

function FocusSkeleton({ onHeight }: { onHeight: (height: number) => void }) {
  const { t } = useTranslation("focus");
  const late = useDelayed(250);
  const root = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    if (late && root.current) onHeight(root.current.offsetHeight);
  }, [late, onHeight]);
  return late ? <div ref={root} className="focus-skeleton" role="status" aria-label={t("pane.loading")} data-loading-skeleton="focus">
    {[0, 1, 2, 3, 4, 5].map((n) => <ArticleCardSkeleton key={n} headline={n === 0} />)}
  </div> : null;
}

const NO_ROWS: ArticleView[] = [];

/** 展开 / 收起后的这么久里，行的位置变化与面板底板平滑过渡；其余时候（测高校正、续页）直接到位。 */
const SETTLE_MS = 320;

/** `key` 变化后的一小段时间为 true。渲染时就判定，位置变化与过渡属性在同一次提交里生效。 */
function useSettling(key: unknown): boolean {
  const [state, setState] = useState({ key, settling: false });
  let current = state;
  if (state.key !== key) {
    current = { key, settling: !reducedMotion() };
    setState(current);
  }
  useEffect(() => {
    if (!current.settling) return;
    const timer = window.setTimeout(() => setState((old) => (old.key === key ? { key, settling: false } : old)), SETTLE_MS);
    return () => window.clearTimeout(timer);
  }, [current.settling, key]);
  return current.settling;
}

/** 稳定合并两个 ref；重渲染不卸载观察目标，也不重置连续可见时间。 */
function MeasuredFocusRow({ row, index, position, headline, readerReturn, slot, measure, observe, remeasure, style, children }: {
  row: ArticleView;
  index: number;
  position: number;
  headline: boolean;
  readerReturn: number;
  /** 在相近报道组里的位置：组的首尾行补上与相邻文章之间的留白。 */
  slot: ClusterSlot | null;
  measure: (element: HTMLDivElement | null) => void;
  observe?: ExposureObserver;
  /** 行内内容变化（如抽查说明）时才重新测高，不在每次渲染时测。 */
  remeasure?: unknown;
  style: CSSProperties;
  children: ReactNode;
}) {
  const { id, row_kind: kind, spot_check: spotCheck } = row;
  const elementRef = useRef<HTMLDivElement | null>(null);
  const ref = useCallback((element: HTMLDivElement | null) => {
    elementRef.current = element;
    measure(element);
    if (kind === 0) observe?.(element, id, position, spotCheck);
  }, [measure, observe, kind, id, position, spotCheck]);
  useLayoutEffect(() => { measure(elementRef.current); }, [remeasure, measure]);
  const gap = slot ? { paddingTop: index === slot.first ? PANEL_GAP : 0, paddingBottom: index === slot.last ? PANEL_GAP : 0 } : null;
  return <div ref={ref} data-index={index} data-headline={headline ? "true" : undefined}
    data-cluster={slot?.role} data-cluster-open={slot?.open ? "" : undefined} data-cluster-last={slot && index === slot.last ? "" : undefined}
    data-reader-return={readerReturn || undefined} style={gap ? { ...style, ...gap } : style}>{children}</div>;
}

export function FocusPane({
  page,
  feeds,
  expandedCluster,
  onFilter,
  onListScope,
  onMarkAllRead,
  onExpand,
  onMarkGroupRead,
  onCiteGroup,
  onSummarize,
  clusterSummary,
  clusterSummaryShown,
  onRanking,
  onRefresh,
  onLoadMore,
  onOpen,
  onCite,
  onSource,
  onRate,
  onStar,
  onOpenDigest,
  onOpenSearch,
  refreshing = false,
  focusedRowIndex = -1,
  returnFocus = 0,
  loading = false,
  loadingMore = false,
  moreError = "",
  revision = 0,
  newCount = 0,
  onShowNew,
  visible = true,
  scrollRootRef,
  onObserveArticle,
  spotReviews = {},
  spotBusy = false,
  onSpotAction,
  hiddenMenuItem,
  columnHeading,
}: {
  page: FocusPage | null;
  feeds: FeedView[];
  expandedCluster: number | null;
  onFilter: (f: string) => void;
  onListScope?: (scope: string) => void;
  onMarkAllRead?: () => void;
  onExpand: (id: number | null) => void;
  /** 相近报道组尾：整组标已读、引用整组到新对话。 */
  onMarkGroupRead?: (clusterId: number) => void;
  onCiteGroup?: (clusterId: number) => void;
  /** 组头「汇总」与面板内的综述（代表文章下、成员列表上方），由焦点功能域提供。 */
  onSummarize?: (clusterId: number) => void;
  /** 面板内的综述块，放在组头下；代表文章还是大卡片时传它的 id，免得一句话列表里重复它。 */
  clusterSummary?: (clusterId: number, representativeId?: number) => ReactNode;
  /** 这组有综述（生成中、已生成、有新报道）：展开时代表文章收成成员行，综述独立在组头下。 */
  clusterSummaryShown?: (clusterId: number) => boolean;
  onRanking: (key: RankingToggle, value: boolean) => void;
  onRefresh: () => void;
  onLoadMore?: () => void;
  onOpen: (id: number) => void;
  onCite: (id: number) => void;
  onSource?: (feedId: number) => void;
  onRate: (row: ArticleView, up: boolean) => void;
  onStar?: (row: ArticleView) => void;
  onOpenDigest?: () => void;
  onOpenSearch?: () => void;
  refreshing?: boolean;
  focusedRowIndex?: number;
  /** 仅焦点阅读返回传入：居中并把 DOM 焦点交给列表，后续 Enter / o 由键盘选中行处理。 */
  returnFocus?: number;
  loading?: boolean;
  loadingMore?: boolean;
  moreError?: string;
  revision?: number;
  /** 后台发现、还没并进列表的新文章数（阶段 17 · 后台更新不动视口）；点提示重读并回顶部。 */
  newCount?: number;
  onShowNew?: () => void;
  /** 主栏目保活：隐藏时不续页、不把虚拟行尺寸写成 0，范围变化的回顶部延到重新显示。 */
  visible?: boolean;
  /** 交给功能域监听曝光；组件只登记文章行，并继续负责虚拟列表测量。 */
  scrollRootRef?: RefObject<HTMLDivElement | null>;
  onObserveArticle?: ExposureObserver;
  spotReviews?: Record<number, SpotReview>;
  spotBusy?: boolean;
  onSpotAction?: (id: number, action: SpotAction) => void;
  /** 「显示」菜单的「被隐藏的文章」入口（阶段 23 R1），由焦点功能域提供。 */
  hiddenMenuItem?: (close: () => void) => ReactNode;
  /** 栏目刊头。有值时不显示简报横幅与导语。 */
  columnHeading?: { name: string; note: string };
}) {
  const { t } = useTranslation("focus");
  const root = useRef<HTMLDivElement>(null);
  const localParent = useRef<HTMLDivElement>(null);
  const parent = scrollRootRef ?? localParent;
  const head = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLDivElement>(null);
  // 头版页眉和导语栏随列表一起滚走；虚拟列表要扣掉它们在滚动区里占的高度。
  const [scrollMargin, setScrollMargin] = useState(0);
  const [initialListHeight, setInitialListHeight] = useState(0);
  const visibleRef = useRef(visible);
  const pendingTop = useRef(false);
  const returned = useRef(0);
  const rows = page?.articles ?? NO_ROWS;
  let articleCount = 0;
  const articlePositions = rows.map(row => row.row_kind === 0 ? articleCount++ : -1);
  // 续页只追加到已有行后面；按整个列表的下标判定，不能放大每个虚拟窗口的首行。
  const headline = rows[0]?.row_kind === 0 && !rows[0].read;
  const ranking = page?.ranking ?? FALLBACK_RANKING;
  const layout = useMemo(() => clusterLayout(rows, expandedCluster), [rows, expandedCluster]);
  const settling = useSettling(expandedCluster);
  const estimate = (i: number) => {
    const slot = layout.slots[i];
    const edge = slot ? (i === slot.first ? PANEL_GAP : 0) + (i === slot.last ? PANEL_GAP : 0) : 0;
    if (slot?.role === "head") return 40 + edge + (slot.open ? 64 : 0);
    if (slot?.role === "rep" && slot.open && clusterSummaryShown?.(slot.clusterId)) return 40 + edge + (i === slot.last ? 40 : 0);
    // 成员压成一行；展开的组最后一行还带组尾。
    if (slot?.role === "member") return 40 + edge + (slot.open && i === slot.last ? 40 : 0);
    return (i === 0 && headline ? 200 : rows[i]?.summary ? 132 : 88) + edge;
  };
  const virtual = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parent.current,
    estimateSize: estimate,
    overscan: 8,
    getItemKey: (i) => `${rows[i]?.row_kind}-${rows[i]?.id}-${rows[i]?.cluster_id}`,
    // 隐藏后行高都是 0；只沿用已测尺寸，重新显示后再按实际高度测量。
    useCachedMeasurements: !visible,
    scrollMargin,
  });
  const pageKey = `${revision}-${page?.filter}-${page?.list_scope}-${ranking.cluster_group}-${ranking.interest_boost}-${ranking.rerank}-${ranking.hide_noisy}-${LEARNED.map((l) => +ranking[l.key]).join("")}`;
  const requestedFor = useRef(0);
  const empty = focusEmptyState(page, feeds);
  const hasEmpty = page !== null && !!empty.message;
  const noisyLabel = noisyToggleLabel(feeds, ranking.hide_noisy);
  useRetainedScroll(root, visible);
  // 切回这个主栏目：新内容淡入并上移 4px，旧内容不做退场。
  useViewEnter(root, visible);
  useLayoutEffect(() => {
    const el = parent.current;
    const top = list.current;
    if (!visible || !el || !top) return;
    const measure = () => {
      // 隐藏时读不到布局，沿用上次的值。
      if (!el.clientHeight) return;
      const next = Math.round(top.getBoundingClientRect().top - el.getBoundingClientRect().top + el.scrollTop);
      setScrollMargin((old) => (old === next ? old : next));
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    if (head.current) observer.observe(head.current);
    observer.observe(el);
    return () => observer.disconnect();
    // 空状态提示夹在页眉和列表之间，出现 / 消失时要重量。
  }, [visible, hasEmpty, parent]);
  useLayoutEffect(() => {
    visibleRef.current = visible;
    if (!visible || !pendingTop.current) return;
    pendingTop.current = false;
    const el = parent.current;
    if (!el) return;
    el.scrollTop = 0;
    el.dispatchEvent(new Event("scroll"));
  }, [visible, parent]);
  useEffect(() => {
    requestedFor.current = 0;
    // 隐藏容器写 scrollTop 无效，回顶部留到重新显示。
    if (!visibleRef.current) pendingTop.current = true;
    else if (parent.current && parent.current.scrollTop !== 0) {
      const el = parent.current;
      el.scrollTop = 0;
      // 尽快同步虚拟列表的滚动位置：新页很快到达时，它若还按旧位置校正行高，会把列表拉回原处。
      // 放进微任务：滚动回调里虚拟列表会 flushSync，不能在 React 提交阶段里同步触发。
      // 本来就在顶部时不派发：多余的滚动事件会关掉开着的下拉菜单。
      queueMicrotask(() => el.dispatchEvent(new Event("scroll")));
    }
  }, [pageKey, parent]);
  const expandedWas = useRef(expandedCluster);
  const articles = page?.articles;
  useLayoutEffect(() => {
    const previous = expandedWas.current;
    expandedWas.current = expandedCluster;
    if (previous == null || previous === expandedCluster) return;
    const el = parent.current;
    const index = (articles ?? []).findIndex((row) => row.row_kind === 1 && row.cluster_id === previous);
    if (!el || index < 0) return;
    const node = el.querySelector<HTMLElement>(`[data-index="${index}"]`);
    if (node) {
      if (node.getBoundingClientRect().bottom > el.getBoundingClientRect().top + 1) return;
    } else {
      const first = virtual.getVirtualItems()[0]?.index;
      if (first === undefined || index >= first) return;
    }
    virtual.scrollToIndex(index, { align: "start" });
  }, [expandedCluster, articles, parent, virtual]);
  useEffect(() => {
    if (!visible || focusedRowIndex < 0 || rows.length === 0) return;
    if (!returnFocus) {
      returned.current = 0;
      virtual.scrollToIndex(focusedRowIndex, { align: "auto" });
      return;
    }
    // 返回只定位一次；随后用户滚动续页不能又被拉回刚读的行。
    if (returned.current === returnFocus) return;
    parent.current?.focus({ preventScroll: true });
    // 先让窗口改宽后的布局与 ResizeObserver 提交，避免用旧容器高度居中。
    let frame = requestAnimationFrame(() => {
      frame = requestAnimationFrame(() => {
        returned.current = returnFocus;
        virtual.scrollToIndex(focusedRowIndex, { align: "center" });
        // 已挂载行同步实测；远处尚未挂载的行仍由虚拟列表在落位时测量和校正。
        parent.current?.querySelectorAll<HTMLElement>("[data-index]").forEach(element => virtual.measureElement(element));
        virtual.scrollToIndex(focusedRowIndex, { align: "center" });
      });
    });
    return () => cancelAnimationFrame(frame);
  }, [visible, focusedRowIndex, returnFocus, pageKey, rows.length, virtual, parent]);
  useEffect(() => {
    const el = parent.current;
    if (!visible || !el || !onLoadMore || loading || loadingMore || moreError || !page?.has_more || rows.length === 0) return;
    const maybeLoad = () => {
      // 隐藏后容器高度为 0，会被误判为没填满。
      if (!visibleRef.current || requestedFor.current === rows.length) return;
      const remain = el.scrollHeight - el.scrollTop - el.clientHeight;
      const unfilled = el.scrollHeight <= el.clientHeight + 80;
      if (remain < 480 || unfilled) {
        requestedFor.current = rows.length;
        onLoadMore();
      }
    };
    maybeLoad();
    el.addEventListener("scroll", maybeLoad, { passive: true });
    return () => el.removeEventListener("scroll", maybeLoad);
  }, [visible, onLoadMore, page?.has_more, rows.length, pageKey, loading, loadingMore, moreError, parent]);
  return (
    <div className="focus" ref={root} hidden={!visible} inert={!visible}>
      <div className="toolbar" data-tauri-drag-region="deep">
        <Segmented className="toolbar-group segmented" role="group" aria-label={t("pane.filterLabel")}>
          {FILTERS.map((f) => (
            <button key={f.id} type="button" className="tab" data-on={page?.filter === f.id ? "1" : "0"} onClick={() => onFilter(f.id)}>
              {t(`pane.filter.${f.id}`)}
            </button>
          ))}
        </Segmented>
        <Segmented className="toolbar-group segmented" role="group" aria-label={t("pane.scopeLabel")}>
          {LIST_SCOPES.map((s) => (
            <button
              key={s.id}
              type="button"
              className="tab"
              data-on={page?.list_scope === s.id ? "1" : "0"}
              onClick={() => onListScope?.(s.id)}
            >
              {t(`pane.scope.${s.id}`)}
            </button>
          ))}
        </Segmented>
        <MenuButton label={t("pane.display")} className="tab menu-trigger" menuClassName="toolbar-menu"
          face={<><Icon name="eye" size={14} />{t("pane.display")}<Icon name="chevronDown" size={12} className="menu-caret" /></>}>
          {(close) => <>
            <MenuCheck checked={ranking.cluster_group} onSelect={() => onRanking("cluster_group", !ranking.cluster_group)}>{t("pane.menu.clusterGroup")}</MenuCheck>
            <MenuCheck checked={ranking.interest_boost} onSelect={() => onRanking("interest_boost", !ranking.interest_boost)}>{t(ranking.interest_ready ? "pane.menu.interestBoost" : "pane.menu.interestBoostNotReady")}</MenuCheck>
            <MenuCheck checked={ranking.rerank} onSelect={() => onRanking("rerank", !ranking.rerank)}>{t("pane.menu.rerank")}</MenuCheck>
            <MenuCheck checked={ranking.hide_noisy} onSelect={() => onRanking("hide_noisy", !ranking.hide_noisy)}>{t("pane.menu.hideNoisy")}</MenuCheck>
            <hr role="separator" />
            {LEARNED.map((l) => (
              <MenuCheck key={l.key} checked={ranking[l.key]} title={t("pane.menu.learnedTitle")}
                onSelect={() => onRanking(l.key, !ranking[l.key])}>{t(`pane.learned.${l.key}`)}</MenuCheck>
            ))}
            <hr role="separator" />
            <MenuCheck checked={ranking.show_details} onSelect={() => onRanking("show_details", !ranking.show_details)}>{t("pane.menu.showDetails")}</MenuCheck>
            {hiddenMenuItem?.(close)}
          </>}
        </MenuButton>
        <span className="toolbar-spacer" />
        <div className="toolbar-actions">
          {onOpenSearch ? (
            <button type="button" className="tab tool" title={t("pane.searchTitle")} aria-label={t("pane.search")} onClick={onOpenSearch}>
              <Icon name="search" size={15} /><span className="tool-label">{t("pane.search")}</span>
            </button>
          ) : null}
          <button
            type="button"
            className="tab tool"
            title={t("pane.refreshTitle")}
            aria-label={t("pane.refresh")}
            aria-busy={refreshing}
            onClick={onRefresh}
            disabled={refreshing}
            data-busy={refreshing ? "1" : "0"}
          >
            <Icon name="refresh" size={15} className={refreshing ? "spin" : undefined} /><span className="tool-label">{t("pane.refresh")}</span>
          </button>
          {onMarkAllRead ? (
            <button type="button" className="tab tool" title={t("pane.markAllReadTitle")} aria-label={t("pane.markAllRead")} onClick={onMarkAllRead}>
              <Icon name="checkAll" size={15} /><span className="tool-label">{t("pane.markAllRead")}</span>
            </button>
          ) : null}
        </div>
      </div>
      <div className="scroll" data-testid="focus-scroll" ref={parent} aria-busy={page === null || loading} tabIndex={returnFocus ? -1 : undefined}>
        {/* 零高度的吸顶条：提示浮在列表上方，出现和消失都不挤动下面的内容。 */}
        <div className="focus-new" aria-live="polite">
          {newCount > 0 && onShowNew && !loading ? (
            <button type="button" className="focus-new-pill" data-testid="focus-new-content" onClick={onShowNew}>
              <Icon name="arrowUp" size={13} />{t("pane.newContent", { count: newCount })}
            </button>
          ) : null}
        </div>
        <div className="focus-head" ref={head}>
          <FocusMasthead column={columnHeading} />
          {!columnHeading && page?.digest_banner ? (
            <button type="button" className="focus-lede" onClick={onOpenDigest}>
              <span className="focus-lede-icon"><Icon name="digest" size={18} /></span>
              <span className="focus-lede-text"><strong>{t("pane.digestTitle")}</strong>{page.digest_lede ? <span>{page.digest_lede}</span> : null}</span>
              <span className="focus-lede-cta">{t("pane.digestCta")}<Icon name="chevronRight" size={14} /></span>
            </button>
          ) : null}
          {ranking.ranking_note || noisyLabel ? (
            <div className="focus-banners">
              {ranking.ranking_note ? (
                <p className="focus-banner note"><Icon name="info" size={14} />{ranking.ranking_note}</p>
              ) : null}
              {noisyLabel ? (
                <button
                  type="button"
                  className="focus-banner"
                  onClick={() => onRanking("hide_noisy", !ranking.hide_noisy)}
                >
                  <Icon name={ranking.hide_noisy ? "eye" : "filter"} size={14} />{noisyLabel}
                </button>
              ) : null}
            </div>
          ) : null}
        </div>
        {hasEmpty ? (
          <div className="empty">
            <span className="empty-icon"><Icon name={emptyIcon(page, feeds)} size={22} /></span>
            <p>{empty.message}</p>
            {empty.action ? (
              <button
                type="button"
                className="btn primary"
                onClick={() => {
                  if (empty.action?.filter) onFilter(empty.action.filter);
                  if (empty.action?.scope) onListScope?.(empty.action.scope);
                  if (empty.action?.ranking) onRanking(empty.action.ranking, false);
                }}
              >
                {empty.action.label}
              </button>
            ) : null}
          </div>
        ) : null}
        <div className="article-list" ref={list} data-settling={settling ? "" : undefined}
          style={{ height: page === null ? undefined : virtual.getTotalSize(), minHeight: page === null ? initialListHeight : undefined, position: "relative" }}>
          {page === null ? <FocusSkeleton onHeight={setInitialListHeight} /> : null}
          {(() => {
            const items = virtual.getVirtualItems();
            const from = items[0]?.index ?? 0;
            const to = items.at(-1)?.index ?? -1;
            const sizes = virtual.measurementsCache;
            // 每组一块底板，铺在这组几行的后面：浅底、描边与圆角连成一块，展开时加阴影。
            return layout.groups.filter((group) => group.last >= from && group.first <= to && sizes[group.first] && sizes[group.last]).map((group) => {
              const top = sizes[group.first].start + PANEL_GAP;
              const bottom = sizes[group.last].end - PANEL_GAP;
              return <div key={`panel-${group.clusterId}`} className="cluster-panel" aria-hidden="true" data-open={group.open ? "" : undefined}
                style={{ height: Math.max(0, bottom - top), transform: `translateY(${top - scrollMargin}px)` }} />;
            });
          })()}
          {virtual.getVirtualItems().map((v) => {
            const row = rows[v.index];
            const slot = layout.slots[v.index];
            const focused = v.index === focusedRowIndex && row.row_kind === 0;
            return (
              <MeasuredFocusRow
                key={v.key}
                row={row}
                index={v.index}
                position={articlePositions[v.index]}
                headline={v.index === 0 && headline}
                readerReturn={v.index === focusedRowIndex ? returnFocus : 0}
                slot={slot}
                measure={virtual.measureElement}
                observe={onObserveArticle}
                remeasure={spotReviews[row.id]}
                style={{
                  position: "absolute",
                  top: 0,
                  left: 0,
                  width: "100%",
                  transform: `translateY(${v.start - scrollMargin}px)`,
                }}
              >
                {slot?.role === "head" ? <>
                  <ClusterHead row={row} open={slot.open} sources={groupSources(rows, slot)} onExpand={onExpand} onSummarize={onSummarize} />
                  {slot.open ? clusterSummary?.(slot.clusterId, clusterSummaryShown?.(slot.clusterId) ? undefined : rows[slot.first + 1]?.id) : null}
                </> : slot?.role === "member" || (slot?.role === "rep" && slot.open && clusterSummaryShown?.(slot.clusterId)) ? (
                  <ClusterMember row={row} focused={focused} representative={slot.role === "rep"} onOpen={onOpen} />
                ) : (
                  <ArticleCard
                    row={row}
                    representative={slot?.role === "rep"}
                    details={ranking.show_details}
                    review={spotReviews[row.id]}
                    reviewBusy={spotBusy}
                    hideThreshold={ranking.hide_threshold}
                    onReview={onSpotAction ? action => onSpotAction(row.id, action) : undefined}
                    focused={focused}
                    onOpen={onOpen}
                    onCite={onCite}
                    onSource={onSource}
                    onRate={onRate}
                    onStar={onStar}
                  />
                )}
                {slot?.open && v.index === slot.last && slot.role !== "head" ? (
                  <ClusterFoot count={slot.last - slot.first} onCollapse={() => onExpand(null)}
                    onMarkRead={onMarkGroupRead ? () => onMarkGroupRead(slot.clusterId) : undefined}
                    onCite={onCiteGroup ? () => onCiteGroup(slot.clusterId) : undefined} />
                ) : null}
              </MeasuredFocusRow>
            );
          })}
        </div>
        {moreError ? (
          <div className="notice error" role="alert">
            <span>{moreError}</span>
            <button type="button" className="btn sm" onClick={onLoadMore} disabled={loadingMore}>{t("pane.retryLoadMore")}</button>
          </div>
        ) : null}
      </div>
    </div>
  );
}
