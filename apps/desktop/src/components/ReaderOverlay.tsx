import { Component, createRef, useCallback, useEffect, useRef, useState, type CSSProperties, type MouseEvent, type ReactNode } from "react";
import { useTranslation } from "../i18n";
import { openExternal, type ArticleView, type MediaRef, type ReaderView, type RelatedArticleView } from "../lib/api";
import { openOriginal } from "../lib/signals";
import { useExposureTracker, type ExposureObserver } from "../lib/useExposureTracker";
import { BlockView, type ReaderSelection } from "./BlockView";
import { ReaderSelectionTranslation } from "./ReaderSelectionTranslation";
import { ErrorBoundary } from "./ErrorBoundary";
import { Icon } from "./Icon";
import { ReaderMasthead } from "./ReaderMasthead";
import { ReaderEnding, type NextArticle } from "./ReaderEnding";
import { ReasonRow } from "./ReasonRow";
import { RelativeTime } from "./RelativeTime";
import { ReaderTool, ReaderTooltip } from "./ReaderTool";
import { ThumbIcon } from "./ThumbIcon";
import { keepsNativeContextMenu, type MenuAnchor } from "./FloatingMenu";
import { ReaderContextMenu, READER_MODES } from "./ReaderContextMenu";
import { Segmented } from "./Segmented";
import { useSearchTermHighlight } from "./searchTermHighlight";

export function ReaderOverlay({
  reader,
  webNav,
  webActions,
  webTip,
  onCollect,
  collecting,
  origin,
  onOpenUrl = openExternal,
  style,
  dismissible,
  summarizing,
  translating,
  translationStatus,
  translateLabel,
  onRegenerate,
  blockNotes,
  readingPosition,
  anchor,
  searchTerm,
  searchHits,
  returnThreadTitle,
  returnArticleTitle,
  onReturnArticle,
  related,
  onOpenRelated,
  onContinueAsking,
  onReadingPosition,
  onBody,
  translateMode,
  mediaLoading,
  onMediaRef,
  onClose,
  onCiteArticle,
  onCiteSelection,
  onTranslateSettings,
  onSummarize,
  onTranslate,
  onTranslateMode,
  onRate,
  onDismissNote,
  hasPrev = false,
  hasNext = false,
  onPrev,
  onNext,
  fetchingFulltext = false,
  onFetchFulltext,
  onStar,
  drawerOpen = false,
  drawerPinned = false,
  canPinDrawer = false,
  drawerRows = [],
  drawerTitle,
  drawerScope = "all",
  drawerSelectedId,
  drawerExpandedCluster,
  onDrawerExpand,
  positionLabel,
  article,
  showNextCard = false,
  nextArticle,
  searchPosition,
  onReturnToSearch,
  drawerHasMore = false,
  drawerLoadingMore = false,
  onDrawerOpen,
  onDrawerClose,
  onDrawerPin,
  onDrawerScope,
  onDrawerSelect,
  onDrawerOpenArticle,
  onDrawerRate,
  onDrawerLoadMore,
  onDrawerEdgeEnter,
  onDrawerEdgeLeave,
  stageRef,
  citationTray,
  onTrayEscape,
  inert = false,
}: {
  reader: ReaderView;
  /** 网页阅读替换左侧返回与篇目；普通文章不传。 */
  webNav?: ReactNode;
  /** 网页阅读放在翻译之前：视图切换、系统浏览器、收录。 */
  webActions?: ReactNode;
  webTip?: ReactNode;
  onCollect?: () => void;
  collecting?: boolean;
  origin?: ReactNode;
  onOpenUrl?: (href: string) => void;
  citationTray?: ReactNode;
  onTrayEscape?: () => boolean;
  style?: CSSProperties;
  dismissible: boolean;
  summarizing: boolean;
  translating: boolean;
  translationStatus?: ReactNode;
  translateLabel?: string;
  onRegenerate?: () => void;
  blockNotes?: ReadonlyMap<number, string>;
  readingPosition?: ReaderPosition;
  /** `persist`：搜索命中段，高亮保持到离开这篇；否则短暂闪一下。 */
  anchor?: ReaderAnchor;
  /** 从搜索打开：返回回到搜索，正文里标出查询词。 */
  searchTerm?: string;
  /** 正文命中段导航：第几处（零起）、共几处、N / Shift+N。 */
  searchHits?: { index: number; total: number; onStep: (dir: -1 | 1) => void };
  returnThreadTitle?: string;
  /** 从文末相关阅读岔出来时：上一篇的标题与返回；顶栏的返回改成「上一篇」。 */
  returnArticleTitle?: string;
  onReturnArticle?: () => void;
  /** 文末相关阅读（已按向量近邻取好，空时不传）。 */
  related?: RelatedArticleView[];
  onOpenRelated?: (item: RelatedArticleView) => void;
  onContinueAsking?: () => void;
  onReadingPosition?: (position: ReaderPosition) => void;
  onBody?: (body: HTMLDivElement | null) => void;
  translateMode: "original" | "bilingual" | "translated";
  mediaLoading: boolean;
  onMediaRef?: (media: MediaRef) => void;
  onClose: () => void;
  onCiteArticle: () => void;
  onCiteSelection: (excerpt: string, blockId: number) => void;
  onTranslateSettings?: () => void;
  onSummarize: () => void;
  onTranslate: () => void;
  onTranslateMode: (mode: "original" | "bilingual" | "translated") => void;
  onRate: (up: boolean) => void;
  onDismissNote?: () => void;
  hasPrev?: boolean;
  hasNext?: boolean;
  onPrev?: () => void;
  onNext?: () => void;
  fetchingFulltext?: boolean;
  onFetchFulltext?: () => void;
  onStar?: () => void;
  drawerOpen?: boolean;
  drawerPinned?: boolean;
  canPinDrawer?: boolean;
  drawerRows?: ArticleView[];
  /** 篇目抽屉标题：打开阅读器的列表名。 */
  drawerTitle?: string;
  drawerScope?: string;
  drawerSelectedId?: number;
  drawerExpandedCluster?: number | null;
  onDrawerExpand?: (id: number | null) => void;
  positionLabel?: string;
  article?: ArticleView;
  showNextCard?: boolean;
  nextArticle?: NextArticle;
  searchPosition?: { index: number; total: number };
  onReturnToSearch?: () => void;
  drawerHasMore?: boolean;
  drawerLoadingMore?: boolean;
  onDrawerOpen?: () => void;
  onDrawerClose?: () => void;
  onDrawerPin?: () => void;
  onDrawerScope?: (scope: string) => void;
  onDrawerSelect?: (id: number) => void;
  onDrawerOpenArticle?: (id: number) => void;
  onDrawerRate?: (id: number) => void;
  onDrawerLoadMore?: () => void;
  onDrawerEdgeEnter?: () => void;
  onDrawerEdgeLeave?: () => void;
  stageRef?: (node: HTMLDivElement | null) => void;
  inert?: boolean;
}) {
  const { t } = useTranslation("reader");
  const tools = useRef<HTMLDetailsElement>(null);
  const toolsTrigger = useRef<HTMLElement>(null);
  const [body, setBody] = useState<HTMLDivElement | null>(null);
  const [menu, setMenu] = useState<MenuAnchor | null>(null);
  const [selection, setSelection] = useState<{ pick: ReaderSelection; articleId: number } | null>(null);
  useSearchTermHighlight(body, searchHits ? searchTerm : undefined);
  const bindBody = useCallback((node: HTMLDivElement | null) => {
    setBody(node);
    onBody?.(node);
  }, [onBody]);
  const showContextMenu = (event: MouseEvent<HTMLDivElement>) => {
    if (event.defaultPrevented || keepsNativeContextMenu(event.target)) return;
    const selection = window.getSelection();
    if (selection && !selection.isCollapsed) return;
    event.preventDefault();
    closeTools();
    setMenu({ x: event.clientX, y: event.clientY });
  };
  const closeTools = () => {
    if (tools.current) tools.current.open = false;
  };
  const runTool = (action: () => void) => {
    closeTools();
    toolsTrigger.current?.focus();
    action();
  };
  useEffect(() => {
    closeTools();
    setMenu(null);
    setSelection(null);
  }, [reader.id, reader.source_hash, inert]);
  useEffect(() => {
    // 对话输入框可能仍持有焦点；把焦点带进阅读器，单键快捷键才能立即使用。
    body?.focus({ preventScroll: true });
  }, [body, returnThreadTitle]);
  // 收起时篇目抽屉与遮罩先播退场动画再卸载；钉住时收起没有遮罩，不让它闪一下。
  const drawerShown = useExitDelay(drawerOpen, DRAWER_EXIT_MS);
  const drawerClosing = drawerShown && !drawerOpen;
  const pinnedBeforeClose = useRef(drawerPinned);
  if (drawerOpen) pinnedBeforeClose.current = drawerPinned;
  const scrimShown = drawerShown && !(drawerOpen ? drawerPinned : pinnedBeforeClose.current);
  // Esc 在事件发生时读当前抽屉状态；不能用副作用注册时的快照，否则刚钉住后的 Esc 会被当成收起浮层。
  const drawerState = useRef({ open: drawerOpen, pinned: drawerPinned, close: onDrawerClose, trayEscape: onTrayEscape });
  drawerState.current = { open: drawerOpen, pinned: drawerPinned, close: onDrawerClose, trayEscape: onTrayEscape };
  useEffect(() => {
    const dismiss = (event: PointerEvent) => {
      if (event.target instanceof Node && !tools.current?.contains(event.target)) closeTools();
    };
    const escape = (event: KeyboardEvent) => {
      if (inert || event.isComposing || event.keyCode === 229 || event.key !== "Escape") return;
      if (tools.current?.open) {
        event.preventDefault();
        event.stopImmediatePropagation();
        closeTools();
        toolsTrigger.current?.focus();
        return;
      }
      const drawer = drawerState.current;
      if (drawer.open && !drawer.pinned && drawer.close) {
        event.preventDefault();
        event.stopImmediatePropagation();
        drawer.close();
      } else if (drawer.trayEscape?.()) {
        event.preventDefault();
        event.stopImmediatePropagation();
      }
    };
    document.addEventListener("pointerdown", dismiss);
    // 划词 / 右键浮层先在 window capture 处理；工具菜单即使失焦也先于根壳的返回。
    document.addEventListener("keydown", escape, true);
    return () => {
      document.removeEventListener("pointerdown", dismiss);
      document.removeEventListener("keydown", escape, true);
    };
  }, [inert]);

  const title =
    translateMode !== "original" && reader.translated_title
      ? reader.translated_title
      : reader.title;
  const webReader = webNav != null || webActions != null;

  return (
    <div ref={stageRef} className="overlay reader-stage" data-drawer-open={drawerOpen || undefined} data-drawer-pinned={drawerPinned || undefined} style={style} {...(inert ? { inert: true } : {})}>
      <div className={`drawer reader${reader.translation_ready ? " has-translation" : ""}`} data-citation-tray={!!citationTray || undefined} data-search-hits={(searchHits && !inert) || undefined}>
        <div className="drawer-head reader-head" data-tauri-drag-region="deep">
          <div className="reader-head-nav">
            {webNav ?? <>
              {onReturnArticle ? <ReaderTool label={returnArticleTitle ? t("head.returnTo", { title: returnArticleTitle }) : t("head.returnPrev")}
                aria-label={t("head.returnPrev")} shortcut="Esc" className="reader-back" onClick={onReturnArticle}>
                <Icon name="arrowLeft" size={16} />{t("head.prev")}
              </ReaderTool> : <ReaderTool label={returnThreadTitle ? t("head.returnToThread", { title: returnThreadTitle }) : searchTerm ? t("head.returnToSearch", { term: searchTerm }) : t("head.back")}
                aria-label={returnThreadTitle ? t("head.thread") : searchTerm ? t("head.backToSearch") : t("head.back")} shortcut="Esc" className="reader-back" onClick={onClose}>
                <Icon name="arrowLeft" size={16} />{returnThreadTitle ? t("head.thread") : searchTerm ? t("head.search") : t("head.back")}
              </ReaderTool>}
              {onDrawerOpen ? <ReaderTool label={t("head.drawer")} shortcut="L" onClick={onDrawerOpen}><Icon name="list" size={16} /></ReaderTool> : null}
              {onPrev || onNext ? <span className="reader-head-divider" /> : null}
              {onPrev || onNext ? <div className="reader-pager" role="group" aria-label={t("head.pager")}>
                {onPrev ? <ReaderTool label={t("head.prevArticle")} shortcut="K" disabled={!hasPrev} onClick={onPrev}><Icon name="chevronLeft" size={16} /></ReaderTool> : null}
                {onNext ? <ReaderTool label={t("head.nextArticle")} shortcut="J" disabled={!hasNext} onClick={onNext}><Icon name="chevronRight" size={16} /></ReaderTool> : null}
              </div> : null}
              {positionLabel ? <span className="reader-position" aria-label={t("head.position", { label: positionLabel })}>{positionLabel}</span> : null}
              {onDrawerRate ? <ReaderTool label={t("head.rateNext")} shortcut="X" onClick={() => onDrawerRate(reader.id)}><ThumbIcon down /></ReaderTool> : null}
            </>}
          </div>
          <ReaderScrollTitle body={body} title={title} />
          <div className="reader-head-actions">
            {webActions}
            {webReader ? <span className="reader-head-divider" /> : null}
            <ReaderTool label={translating ? t("tools.busy") : translateLabel ?? t("tools.translate")} shortcut="T" data-reader-action="translate" onClick={onTranslate} disabled={translating}><Icon name="translate" size={16} /></ReaderTool>
            <ReaderTool label={summarizing ? t("menu.summarizing") : t("menu.summarize")} onClick={onSummarize} disabled={summarizing}><Icon name="sparkles" size={16} /></ReaderTool>
            {webReader || !onStar ? null : <ReaderTool label={reader.starred ? t("menu.unstar") : t("menu.star")} shortcut="S" onClick={onStar} aria-pressed={reader.starred}><Icon name="star" size={16} filled={reader.starred} /></ReaderTool>}
            {webReader || !reader.url ? null : <ReaderTool label={t("menu.viewOriginal")} onClick={() => openOriginal(reader.id, reader.url)}><Icon name="external" size={16} /></ReaderTool>}
            <span className="reader-head-divider" />
            <details className="reader-tools" ref={tools}>
              <ReaderTooltip as="summary" triggerRef={toolsTrigger} label={t("tools.more")} className="tool-btn reader-tool"><span aria-hidden="true">⋯</span></ReaderTooltip>
              <div className="reader-tools-panel menu" role="group" aria-label={t("tools.more")}>
                <button type="button" className="menu-item" onClick={() => runTool(onCiteArticle)}><Icon name="quote" size={15} />{t("menu.citeFull")}</button>
                {onRegenerate ? <button type="button" className="menu-item" onClick={() => runTool(onRegenerate)} disabled={translating}><Icon name="translate" size={15} />{t("menu.retranslate")}</button> : null}
                {(reader.truncated || reader.pruned) && onFetchFulltext ? <button type="button" className="menu-item" aria-label={fetchingFulltext ? t("tools.fetching") : t("tools.fetchFulltext")} title={t("tools.fetchFulltextTitle")} onClick={() => runTool(onFetchFulltext)} disabled={fetchingFulltext}><Icon name="fileText" size={15} />{fetchingFulltext ? t("tools.fetching") : t("tools.fetchFulltext")}<kbd aria-hidden="true">U</kbd></button> : null}
                <div className="reader-tools-feedback">
                  <button type="button" className="menu-item" aria-pressed={reader.feedback === 1} onClick={() => runTool(() => onRate(true))}><ThumbIcon />{reader.feedback === 1 ? t("tools.liked") : t("ending.rating.up")}</button>
                  <button type="button" className="menu-item" aria-pressed={reader.feedback === -1} onClick={() => runTool(() => onRate(false))}><ThumbIcon down />{reader.feedback === -1 ? t("tools.disliked") : t("ending.rating.down")}</button>
                </div>
              </div>
            </details>
          </div>
        </div>
        {translationStatus}
        {reader.translation_ready ? (
          <div className="reader-view-rail" role="radiogroup" aria-label={t("rail.label")}>
            <span className="reader-rail-thumb" aria-hidden="true" style={{ transform: `translateY(${READER_MODES.findIndex(m => m.id === translateMode) * 30}px)` }} />
            {READER_MODES.map((m) => (
              <ReaderTooltip key={m.id} label={t(`mode.${m.id}`)} shortcut="V" placement="left" delay={300}>
                <button
                  type="button"
                  role="radio"
                  aria-label={t(`mode.${m.id}`)}
                  aria-checked={translateMode === m.id}
                  onClick={() => onTranslateMode(m.id)}
                >
                  {t(`mode.${m.id}`)[0]}
                </button>
              </ReaderTooltip>
            ))}
          </div>
        ) : null}
        <ReadingPosition key={`${reader.id}/${reader.source_hash}`} sourceHash={reader.source_hash} initial={readingPosition} anchor={anchor} onPosition={onReadingPosition} onBody={bindBody} onContextMenu={showContextMenu}>
          {webTip}
          <div className={`reader-column${origin ? " web-column" : ""}`}>
            {origin}
            <ReaderMasthead web={!!origin} reader={reader} article={article} title={title} originalTitle={translateMode === "bilingual" && reader.translated_title ? reader.title : undefined} />
            {fetchingFulltext ? <span className="reader-progress" role="status"><Icon name="spinner" size={14} className="spin" />{t("body.fetchingFulltext")}</span> : null}
            {reader.translation_note ? (
              <div className="reader-notice">
                <Icon name="info" size={15} />
                <span>{reader.translation_note}</span>
                {onDismissNote ? (
                  <button
                    type="button"
                    className="reader-notice-dismiss"
                    aria-label={t("body.dismissNotice")}
                    onClick={onDismissNote}
                  >
                    <Icon name="close" size={14} />
                  </button>
                ) : null}
              </div>
            ) : null}
            {reader.summary || summarizing ? <div className="reader-summary" aria-busy={summarizing}>
              <span className="reader-summary-label" role={summarizing ? "status" : undefined}><Icon name="sparkles" size={13} />{summarizing ? t("body.summarizing") : t("body.summary")}</span>
              {summarizing ? <div className="reader-summary-skeleton" aria-hidden="true"><span /><span /><span /></div> : <p>{reader.summary}</p>}
            </div> : null}
            {reader.summary_stale ? <div className="reader-summary-stale" role="status">
              <span>{t("body.summaryStale")}</span>
              <button type="button" className="btn sm ghost" onClick={onSummarize} disabled={summarizing}>{summarizing ? t("body.generating") : t("body.regenerate")}</button>
            </div> : null}
            <ReasonRow reasons={reader.reasons} />
            <div className="reader-opening-rule" aria-hidden="true" />
            {reader.pruned ? (
              <div className="reader-notice reader-pruned" role="status">
                <Icon name="info" size={15} />
                <span>{t("body.pruned")}</span>
                {reader.url ? <button type="button" className="tool-btn" onClick={() => openOriginal(reader.id, reader.url)}>{t("body.openOriginal")}</button> : null}
                {onFetchFulltext ? (
                  <button type="button" className="tool-btn" onClick={onFetchFulltext} disabled={fetchingFulltext}>
                    {fetchingFulltext ? t("tools.fetching") : t("body.refetch")}
                  </button>
                ) : null}
              </div>
            ) : null}
            <ErrorBoundary scope="reader"
              resetKey={reader.id}
              fallback={(_, reset) => (
                <div className="reader-body-error" role="alert">
                  <Icon name="info" size={15} />
                  <span>{t("body.renderError")}</span>
                  <button type="button" className="tool-btn" onClick={reset}>{t("body.retry")}</button>
                  {reader.url ? (
                    <button type="button" className="tool-btn" onClick={() => openOriginal(reader.id, reader.url)}>{t("menu.viewOriginal")}</button>
                  ) : null}
                </div>
              )}
            >
              <BlockView
                readerAppearance
                document={reader.document}
                translated={reader.translated_document}
                mode={reader.translation_ready ? translateMode : "original"}
                blockNotes={blockNotes}
                media={reader.media}
                mediaLoading={mediaLoading}
                onMediaRef={onMediaRef}
                embedCovers={reader.embed_covers}
                articleId={reader.id}
                selectable
                active={!inert}
                onOpenUrl={onOpenUrl}
                onCite={onCiteSelection}
                onTranslateSelection={pick => setSelection({ pick, articleId: reader.id })}
              />
            </ErrorBoundary>
            {!origin ? <ReaderEnding reader={reader} onRate={onRate} onStar={onStar} onCite={onCiteArticle}
              showNext={showNextCard} hasNext={hasNext} nextArticle={nextArticle} onNext={onNext}
              searchPosition={searchPosition} onReturnToSearch={onReturnToSearch}
              related={related} onOpenRelated={onOpenRelated} /> : null}
          </div>
        </ReadingPosition>
        <BackToTop body={body} />
        {searchHits && !inert ? (
          <div className="reader-search-pill" role="status" aria-label={t("hits.label")}>
            <span>{t("hits.position", { n: searchHits.index + 1, total: searchHits.total })}</span>
            {searchHits.total > 1 ? <>
              <button type="button" onClick={() => searchHits.onStep(1)}><kbd>N</kbd>{t("hits.next")}</button>
              <button type="button" onClick={() => searchHits.onStep(-1)}><kbd>⇧N</kbd>{t("hits.prev")}</button>
            </> : null}
          </div>
        ) : null}
        {citationTray}
        {selection && selection.articleId === reader.id && !inert ? <ReaderSelectionTranslation selection={selection.pick} onClose={() => setSelection(null)}
          onCite={onCiteSelection} onSettings={() => onTranslateSettings?.()} /> : null}
        {menu && body && !inert ? <ReaderContextMenu reader={reader} anchor={menu} boundary={body} mode={translateMode}
          translating={translating} summarizing={summarizing} onClose={() => setMenu(null)} onMode={onTranslateMode}
          onTranslate={onTranslate} onRegenerate={onRegenerate ?? onTranslate} onSummarize={onSummarize}
          onCollect={onCollect} collecting={collecting} onContinueAsking={onContinueAsking} onCite={onCiteArticle} onStar={onStar} onTop={() => scrollReaderToTop(body)} /> : null}
      </div>
      {onDrawerOpen ? <div className="reader-drawer-edge" onMouseEnter={onDrawerEdgeEnter} onMouseLeave={onDrawerEdgeLeave} aria-hidden="true" /> : null}
      {scrimShown && onDrawerClose ? <button type="button" className="reader-drawer-scrim" aria-label={t("drawer.collapse")} data-closing={drawerClosing || undefined}
        tabIndex={drawerClosing ? -1 : undefined} onClick={drawerClosing ? undefined : onDrawerClose} /> : null}
      {drawerShown && onDrawerClose ? <ReaderArticleDrawer
        closing={drawerClosing}
        active={drawerOpen && !drawerClosing && dismissible && !inert}
        rows={drawerRows}
        title={drawerTitle ?? t("drawer.defaultTitle")}
        currentId={reader.id}
        selectedId={drawerSelectedId}
        expandedCluster={drawerExpandedCluster}
        onExpand={onDrawerExpand}
        scope={drawerScope}
        pinned={drawerPinned}
        canPin={canPinDrawer}
        hasMore={drawerHasMore}
        loadingMore={drawerLoadingMore}
        onClose={onDrawerClose}
        onPin={onDrawerPin}
        onScope={onDrawerScope}
        onSelect={onDrawerSelect}
        onOpen={onDrawerOpenArticle}
        onRate={onDrawerRate}
        onLoadMore={onDrawerLoadMore}
      /> : null}
    </div>
  );
}

/** 数据库文章 ID 为正；合辑选中键独立编码，避免与文章 ID 碰撞。 */
export function readerDrawerKey(row: ArticleView): number { return row.row_kind === 1 ? -row.cluster_id : row.id; }

function ReaderArticleDrawer({ closing, active, rows, title, currentId, selectedId, expandedCluster, onExpand, scope, pinned, canPin, hasMore, loadingMore, onClose, onPin, onScope, onSelect, onOpen, onRate, onLoadMore }: {
  closing: boolean;
  active: boolean;
  rows: ArticleView[];
  title: string;
  currentId: number;
  selectedId?: number;
  expandedCluster?: number | null;
  onExpand?: (id: number | null) => void;
  scope: string;
  pinned: boolean;
  canPin: boolean;
  hasMore: boolean;
  loadingMore: boolean;
  onClose: () => void;
  onPin?: () => void;
  onScope?: (scope: string) => void;
  onSelect?: (id: number) => void;
  onOpen?: (id: number) => void;
  onRate?: (id: number) => void;
  onLoadMore?: () => void;
}) {
  const { t } = useTranslation("reader");
  const listRef = useRef<HTMLDivElement>(null);
  const observe = useExposureTracker({ surface: "drawer", root: listRef, active });
  useEffect(() => {
    if (!selectedId) return;
    listRef.current?.querySelector<HTMLElement>(`[data-reader-drawer-row="${selectedId}"]`)?.scrollIntoView?.({ block: "nearest" });
  }, [selectedId]);
  const onScroll = () => {
    const list = listRef.current;
    if (list && hasMore && !loadingMore && list.scrollTop + list.clientHeight >= list.scrollHeight - 80) onLoadMore?.();
  };
  const scopes = ["all", "unread", "starred"] as const;
  let articlePosition = 0;
  return <aside className="reader-article-drawer" aria-label={t("head.drawer")} data-closing={closing || undefined} {...(closing ? { inert: true } : {})}>
    <div className="reader-article-drawer-head">
      <strong>{title}</strong><span className="reader-article-drawer-count">{rows.filter((row) => row.row_kind === 0).length}</span><span className="drawer-head-spacer" />
      {canPin && onPin ? <button type="button" className="tool-btn icon-only" aria-label={pinned ? t("drawer.unpin") : t("drawer.pin")} aria-pressed={pinned} title={pinned ? t("drawer.unpin") : t("drawer.pin")} onClick={onPin}><Icon name="pin" size={15} /></button> : null}
      <button type="button" className="tool-btn icon-only" aria-label={t("drawer.collapse")} title={t("drawer.collapseTitle")} onClick={onClose}><Icon name="close" size={15} /></button>
    </div>
    <Segmented className="reader-article-drawer-scopes" role="group" aria-label={t("drawer.filter")}>
      {scopes.map((id) => <button key={id} type="button" aria-pressed={scope === id} onClick={() => onScope?.(id)}>{t(`drawer.scope.${id}`)}</button>)}
    </Segmented>
    <div className="reader-article-drawer-list" ref={listRef} onScroll={onScroll}>
      {rows.map((row) => {
        const cluster = row.row_kind === 1;
        const key = readerDrawerKey(row);
        const position = articlePosition;
        if (row.row_kind === 0) articlePosition++;
        // 时间与焦点列表同一种相对时间，不跟随浏览器区域格式。
        const meta = cluster
          ? [t("drawer.clusterSize", { count: row.cluster_size }), row.cluster_span].filter(Boolean).join(" · ")
          : <>{row.feed_title}{row.published_at ? <>{row.feed_title ? " · " : ""}<RelativeTime ms={row.published_at} /></> : null}</>;
        const copy = <span className="reader-article-drawer-copy"><strong>{cluster ? <><span className="reader-article-drawer-cluster">{t("drawer.cluster")}</span>{row.cluster_label}</> : row.title}</strong><small>{meta}</small></span>;
        const gutter = <span className="reader-article-drawer-gutter" aria-hidden="true"><span className="reader-article-drawer-dot" /></span>;
        return <ReaderDrawerRow key={`${row.row_kind}-${row.id}`} row={row} currentId={currentId} selectedId={selectedId} position={position} observe={observe}>
          {cluster ? <button type="button" className="reader-article-drawer-main" aria-expanded={expandedCluster === row.cluster_id} onMouseEnter={() => onSelect?.(key)} onFocus={() => onSelect?.(key)} onClick={() => onExpand?.(expandedCluster === row.cluster_id ? null : row.cluster_id)}>{gutter}{copy}</button> : <button type="button" className="reader-article-drawer-main" aria-current={row.id === currentId ? "true" : undefined} onMouseEnter={() => onSelect?.(row.id)} onFocus={() => onSelect?.(row.id)} onClick={() => onOpen?.(row.id)}>{gutter}{copy}</button>}
          {!cluster && onRate ? <button type="button" className="reader-article-drawer-rate" aria-label={t("drawer.rate", { title: row.title })} title={t("drawer.rateTitle")} onMouseDown={(event) => event.stopPropagation()} onClick={(event) => { event.stopPropagation(); onRate(row.id); }}><ThumbIcon down /></button> : null}
        </ReaderDrawerRow>;
      })}
      {loadingMore ? <div className="reader-article-drawer-loading">{t("drawer.loading")}</div> : null}
    </div>
    <div className="reader-article-drawer-foot"><span><kbd>↑↓</kbd>{t("drawer.footMove")}</span><span><kbd>Enter</kbd>{t("drawer.footOpen")}</span><span><kbd>X</kbd>{t("ending.rating.down")}</span><span><kbd>L</kbd>{t("drawer.footCollapse")}</span></div>
  </aside>;
}

/** 与焦点行一样保持 ref 稳定，选中、合辑展开等重渲染不重置可见计时。 */
function ReaderDrawerRow({ row, currentId, selectedId, position, observe, children }: {
  row: ArticleView;
  currentId: number;
  selectedId?: number;
  position: number;
  observe: ExposureObserver;
  children: ReactNode;
}) {
  const { id, row_kind: kind } = row;
  const ref = useCallback((element: HTMLDivElement | null) => {
    if (kind === 0) observe(element, id, position);
  }, [observe, kind, id, position]);
  const cluster = kind === 1;
  return <div ref={ref} className={`reader-article-drawer-row${cluster ? " is-cluster" : ""}${!cluster && id === currentId ? " current" : ""}${readerDrawerKey(row) === selectedId ? " selected" : ""}${row.read ? " read" : " unread"}`} data-reader-drawer-row={readerDrawerKey(row)}>{children}</div>;
}

const DRAWER_EXIT_MS = 180;

/** `open` 变假后再保留 `ms` 毫秒，给退场动画留时间；减少动效时立即卸载。 */
function useExitDelay(open: boolean, ms: number) {
  const [shown, setShown] = useState(open);
  useEffect(() => {
    if (open) { setShown(true); return; }
    const reduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    const timer = window.setTimeout(() => setShown(false), reduced ? 0 : ms);
    return () => window.clearTimeout(timer);
  }, [open, ms]);
  return open || shown;
}

function ReaderScrollTitle({ body, title }: { body: HTMLDivElement | null; title: string }) {
  const { t } = useTranslation("reader");
  const [scroll, setScroll] = useState({ scrolled: false, progress: 0 });
  useEffect(() => {
    if (!body) return;
    const update = () => {
      const extent = body.scrollHeight - body.clientHeight;
      setScroll({ scrolled: body.scrollTop > 120, progress: extent > 0 ? Math.min(1, Math.max(0, body.scrollTop / extent)) : 0 });
    };
    update();
    const resize = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(update);
    resize?.observe(body);
    if (body.firstElementChild) resize?.observe(body.firstElementChild);
    body.addEventListener("scroll", update, { passive: true });
    return () => { resize?.disconnect(); body.removeEventListener("scroll", update); };
  }, [body]);
  return <div className="reader-scroll-heading" data-scrolled={scroll.scrolled}>
    <button type="button" className="reader-scroll-title" aria-label={t("top.titleLabel", { title })} aria-hidden={!scroll.scrolled} tabIndex={scroll.scrolled ? 0 : -1} onClick={() => scrollReaderToTop(body)}>{title}</button>
    <span className="reader-scroll-line" aria-hidden="true"><span style={{ transform: `scaleX(${scroll.progress})` }} /></span>
  </div>;
}

export function scrollReaderToTop(body: Element | null) {
  body?.scrollTo({ top: 0, behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "instant" : "smooth" });
}

function BackToTop({ body }: { body: HTMLDivElement | null }) {
  const { t } = useTranslation("reader");
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    if (!body) return;
    const update = () => setVisible(body.scrollTop > body.clientHeight);
    update();
    const resize = typeof ResizeObserver === "undefined" ? undefined : new ResizeObserver(update);
    resize?.observe(body);
    body.addEventListener("scroll", update, { passive: true });
    return () => { resize?.disconnect(); body.removeEventListener("scroll", update); };
  }, [body]);
  return <button type="button" className="reader-to-top" data-visible={visible} aria-hidden={!visible} tabIndex={visible ? 0 : -1}
    title={t("top.title")} onClick={() => scrollReaderToTop(body)}><Icon name="arrowUp" />{t("menu.top")}</button>;
}

export type ReaderPosition = { block: string | null; offset: number };
type Anchor = ReaderPosition | null;
export type ReaderAnchor = { blockId: number; persist?: boolean };
type PositionProps = { sourceHash: string; children: ReactNode; initial?: ReaderPosition; anchor?: ReaderAnchor; onPosition?: (position: ReaderPosition) => void; onBody?: (body: HTMLDivElement | null) => void; onContextMenu?: (event: MouseEvent<HTMLDivElement>) => void };
const BLOCKS = "[data-reader-document] > [data-block-id]";
const SEARCH_HIT = "reader-search-hit";
/** 块按文序排列，底边单调；二分只读 O(log n) 次布局，长文滚动和每次补丁都不逐块测量。 */
function firstVisible(body: HTMLElement): HTMLElement | undefined {
  const nodes = body.querySelectorAll<HTMLElement>(BLOCKS);
  const top = body.getBoundingClientRect().top + 4;
  let low = 0, high = nodes.length;
  while (low < high) {
    const mid = (low + high) >> 1;
    if (nodes[mid].getBoundingClientRect().bottom > top) high = mid; else low = mid + 1;
  }
  return nodes[low];
}
/** 在 DOM 提交前记录可见块，提交后恢复；直接复用旧 scrollTop 会在插入译文时跳段。 */
class ReadingPosition extends Component<PositionProps, Record<string, never>, Anchor> {
  private body = createRef<HTMLDivElement>();
  private frame: number | undefined;
  private highlightTimer: number | undefined;
  private highlighted: HTMLElement | null = null;
  /** 打开时要停的位置。用户动手滚动前，正文变高（配图、封面晚到）都重新对准它。 */
  private target: ReaderPosition | null = null;
  /** 最近一次对准后的 scrollTop；滚动停在别处说明是用户（或「回到顶部」）在滚，不再追。 */
  private expected = 0;
  private resize: ResizeObserver | null = null;
  private release = () => { this.target = null; };
  private hold = (position: ReaderPosition | null) => {
    this.target = position;
    const content = this.body.current?.firstElementChild;
    if (!position || !content || typeof ResizeObserver === "undefined") return;
    this.resize ??= new ResizeObserver(() => { if (this.target) this.locate(this.target); });
    this.resize.observe(content);
  };
  private remember = () => {
    this.frame = undefined;
    const body = this.body.current;
    if (!body) return;
    const block = firstVisible(body);
    this.props.onPosition?.(body.scrollTop < 8 || !block ? { block: null, offset: 0 } : { block: block.dataset.blockId!, offset: block.getBoundingClientRect().top - body.getBoundingClientRect().top });
  };
  private scrolled = () => {
    const body = this.body.current;
    if (this.target && body && Math.abs(body.scrollTop - this.expected) > 2) this.release();
    if (this.frame === undefined) this.frame = requestAnimationFrame(this.remember);
  };
  private locate = (position?: ReaderPosition) => {
    const body = this.body.current;
    const block = position?.block && body?.querySelector<HTMLElement>(`${BLOCKS}[data-block-id="${position.block}"]`);
    if (!body || !block) return null;
    const rect = block.getBoundingClientRect();
    // 重开默认原文可能比原来的双语块短，至少留一行在当前段，不能滑进下一段。
    const offset = Math.max(position!.offset, -Math.max(0, rect.height - 32));
    body.scrollTop += rect.top - body.getBoundingClientRect().top - offset;
    this.expected = body.scrollTop;
    return block;
  };
  private jump = () => {
    const anchor = this.props.anchor;
    const id = anchor?.blockId;
    // 块序号从 0 起：第 0 块（正文第一段）同样可以作锚点。
    if (id === undefined || !Number.isSafeInteger(id) || id < 0) return false;
    const target = { block: String(id), offset: Math.min(120, (this.body.current?.clientHeight ?? 0) * 0.2) };
    const block = this.locate(target);
    if (!block) return false;
    this.hold(target);
    this.clearHighlight();
    this.highlighted = block;
    if (anchor!.persist) block.classList.add(SEARCH_HIT);
    else {
      block.classList.add("reader-block-highlight");
      this.highlightTimer = window.setTimeout(() => this.clearHighlight(), 1200);
    }
    return true;
  };
  /** 搜索命中段的高亮保持到离开这篇；正文重绘（换译文视图、补图）后补回。 */
  private keepHit = () => {
    const anchor = this.props.anchor;
    if (!anchor?.persist || (this.highlighted?.isConnected && this.highlighted.classList.contains(SEARCH_HIT))) return;
    const block = this.body.current?.querySelector<HTMLElement>(`${BLOCKS}[data-block-id="${anchor.blockId}"]`);
    if (!block) return;
    this.clearHighlight();
    this.highlighted = block;
    block.classList.add(SEARCH_HIT);
  };
  private clearHighlight = () => {
    window.clearTimeout(this.highlightTimer);
    this.highlighted?.classList.remove("reader-block-highlight", SEARCH_HIT);
    this.highlighted = null;
  };
  componentDidMount() {
    if (!this.jump() && this.locate(this.props.initial)) this.hold(this.props.initial!);
    this.props.onBody?.(this.body.current);
  }
  componentWillUnmount() {
    this.resize?.disconnect();
    this.clearHighlight();
    if (this.frame !== undefined) cancelAnimationFrame(this.frame);
    this.remember();
    this.props.onBody?.(null);
  }
  getSnapshotBeforeUpdate(previous: Readonly<PositionProps>): Anchor {
    const body = this.body.current;
    if (!body || previous.sourceHash !== this.props.sourceHash || body.scrollTop < 8) return null;
    const block = firstVisible(body);
    return block ? { block: block.dataset.blockId!, offset: block.getBoundingClientRect().top } : null;
  }
  componentDidUpdate(previous: Readonly<PositionProps>, _state: Record<string, never>, anchor: Anchor) {
    const body = this.body.current;
    if (!body) return;
    if (previous.sourceHash !== this.props.sourceHash) { this.release(); body.scrollTop = 0; return; }
    if (previous.anchor !== this.props.anchor) {
      if (!this.props.anchor) this.clearHighlight();
      if (this.jump()) return;
    }
    this.keepHit();
    // 没有可锚定的正文块（空正文、摘要区、渲染出错）时不动滚动位置。
    if (!anchor) return;
    const block = body.querySelector<HTMLElement>(`${BLOCKS}[data-block-id="${anchor.block}"]`);
    if (block) {
      const rect = block.getBoundingClientRect();
      const top = Math.max(anchor.offset, body.getBoundingClientRect().top - Math.max(0, rect.height - 32));
      body.scrollTop += rect.top - top;
    }
    // 重绘（插入译文、配图到达）已按可见块对准；用户还没滚动时，之后的变高改为守住这个块。
    if (this.target) {
      const held = firstVisible(body);
      this.target = held ? { block: held.dataset.blockId!, offset: held.getBoundingClientRect().top - body.getBoundingClientRect().top } : null;
      this.expected = body.scrollTop;
    }
  }
  render() { return <div className="drawer-body" tabIndex={-1} ref={this.body} onScroll={this.scrolled} onContextMenu={this.props.onContextMenu}
    onWheel={this.release} onTouchStart={this.release} onKeyDown={this.release}
    onMouseDown={event => {
      this.release();
      // 自定义右键菜单保留原焦点与选区；原生菜单目标继续交给浏览器。
      if (event.button === 2 && !keepsNativeContextMenu(event.target)) event.preventDefault();
    }}>{this.props.children}</div>; }
}
