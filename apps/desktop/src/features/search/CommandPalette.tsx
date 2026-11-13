import { useCallback, useEffect, useLayoutEffect, useRef, useState, type MouseEvent, type ReactNode, type Ref, type RefObject } from "react";
import type { FeedView, SearchHitBlocksView, SearchHitView, SearchQuery, ThreadSearchHitView } from "../../lib/api";
import { useExposureTracker, type ExposureObserver } from "../../lib/useExposureTracker";
import type { ReaderSearchOpen } from "../reader";
import { SourceAvatar } from "../../components/ArticleCard";
import { Icon } from "../../components/Icon";
import { RelativeTime } from "../../components/RelativeTime";
import { Trans, useTranslation } from "../../i18n";
import { Highlight } from "./Highlight";
import { SearchFilterBar, type SearchKind } from "./SearchFilterBar";
import { Score, SearchPreview } from "./SearchPreview";
import { DEFAULT_FILTERS, filtersActive, SEARCH_LIMIT } from "./searchFilters";
import { useHitBlocks } from "./useHitBlocks";
import { useSearchSession } from "./useSearchSession";
import { THREAD_SEARCH_LIMIT, useThreadSearch, type ThreadSearch } from "./useThreadSearch";

/** 按浮层所在区域（主内容区，或阅读器 / 设置开着时的整窗）的宽度分三档。 */
export type SearchTier = "narrow" | "mid" | "wide";
export const searchTier = (width: number): SearchTier => width < 900 ? "narrow" : width >= 1300 ? "wide" : "mid";

const RECENT_MAX = 5;

export type CommandPaletteProps = {
  rootRef?: Ref<HTMLDivElement>;
  open: boolean;
  /** 宿主当前的最上层浮层是搜索；独立使用时默认在最上层。 */
  topmost?: boolean;
  /** `main`：盖在主内容区上（侧栏露出）；`window`：阅读器或设置开着时盖整窗。 */
  region?: "main" | "window";
  feeds?: FeedView[];
  onClose: () => void;
  onSearch: (query: SearchQuery) => Promise<SearchHitView[]>;
  /** 预览里的正文命中段；不给时预览只显示结果行的片段。 */
  onHitBlocks?: (articleId: number, query: string) => Promise<SearchHitBlocksView>;
  /** 预览元信息里的阅读时长（分钟，0 为不显示）。 */
  onReadingMinutes?: (articleId: number) => Promise<number>;
  /** 打开一篇：带上查询词、结果顺序和已取到的命中段，阅读器据此定位到命中段。 */
  onOpen: (id: number, search: ReaderSearchOpen) => void;
  /** 从这里打开的文章还开着：浮层收起但保留查询、筛选、结果与滚动，返回时恢复。 */
  keepAlive?: boolean;
  /** 从阅读器返回时正在读的那篇；恢复时把它设为选中行。 */
  returnTo?: number | null;
  /** 搜会话（只搜没归档的普通会话）；不给时浮层只搜文章。 */
  onSearchThreads?: (query: string, limit: number) => Promise<ThreadSearchHitView[]>;
  /** 打开一条会话；`messageId` 为命中的消息，标题命中时为空。 */
  onOpenThread?: (threadId: number, messageId: number | null) => void;
  /** 会话所在文件夹的名字，结果行与预览里标出。 */
  threadFolderName?: (folderId: number) => string | undefined;
};

const noBlocks = () => Promise.reject(new Error("no host"));

export function CommandPalette({ open, topmost = true, region = "window", feeds = [], onClose, onSearch, onHitBlocks, onReadingMinutes, onOpen, keepAlive = false, returnTo = null, rootRef,
  onSearchThreads, onOpenThread, threadFolderName }: CommandPaletteProps) {
  const { t } = useTranslation("search");
  const alive = open || keepAlive;
  const session = useSearchSession({ active: alive, search: onSearch });
  const { query, setQuery, filters, setFilters, status, hits, searched, error, retry } = session;
  const [active, setActive] = useState(0);
  // 「文章 / 对话」：默认文章；关掉浮层回到文章。
  const [kind, setKind] = useState<SearchKind>("articles");
  const threadSearch = useThreadSearch({ active: alive, query, search: onSearchThreads });
  const [threadActive, setThreadActive] = useState(0);
  useEffect(() => { if (!alive) setKind("articles"); }, [alive]);
  useEffect(() => { setThreadActive(0); }, [threadSearch.hits]);
  const onThreads = kind === "threads" && !!onSearchThreads;
  // 最近搜索只记本次运行，不落库。
  const [recent, setRecent] = useState<string[]>([]);
  const [tier, setTier] = useState<SearchTier>("mid");
  const previewHit = tier !== "narrow" && (session.status !== "short" && session.status !== "idle") ? session.hits[active] ?? null : null;
  // 只标题命中的篇没有正文命中段可取。
  const blocks = useHitBlocks({
    active: alive && !!onHitBlocks,
    articleId: previewHit?.body_match ? previewHit.id : null,
    query: session.searched,
    load: onHitBlocks ?? noBlocks,
  });
  const minutes = useHitBlocks<number>({
    active: alive && !!onReadingMinutes,
    articleId: previewHit?.id ?? null,
    query: session.searched,
    load: (id) => onReadingMinutes ? onReadingMinutes(id) : noBlocks(),
  });
  const inputRef = useRef<HTMLInputElement>(null);
  const frameRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const previewRef = useRef<HTMLDivElement>(null);
  const lastPointer = useRef("");
  const revealActive = useRef(false);
  // 打开文章前记下列表滚动；从阅读器返回时恢复，并把正在读的那篇设为选中行、短暂加深。
  const resume = useRef<{ scroll: number } | null>(null);
  const [flash, setFlash] = useState<number | null>(null);

  useEffect(() => { if (!alive) resume.current = null; }, [alive]);

  // 只在重新打开那一刻恢复，读打开时的最新值。
  const restore = useRef({ returnTo, hits: session.hits, active });
  restore.current = { returnTo, hits: session.hits, active };
  useLayoutEffect(() => {
    const saved = resume.current;
    if (!open || !saved) return;
    resume.current = null;
    const { returnTo: id, hits: rows, active: current } = restore.current;
    if (listRef.current) listRef.current.scrollTop = saved.scroll;
    const at = id == null ? -1 : rows.findIndex((hit) => hit.id === id);
    if (at >= 0) setActive(at);
    setFlash(at >= 0 ? at : current);
  }, [open]);

  useLayoutEffect(() => {
    if (flash == null) return;
    listRef.current?.querySelector<HTMLElement>(`[data-index="${flash}"]`)?.scrollIntoView?.({ block: "center" });
    const t = window.setTimeout(() => setFlash(null), 900);
    return () => window.clearTimeout(t);
  }, [flash]);

  useEffect(() => {
    if (!open) return;
    // 兜底聚焦输入框；焦点已在浮层里（键盘已经 Tab 走）时不抢回来。
    const t = window.setTimeout(() => {
      const input = inputRef.current;
      if (input && !input.closest(".command-palette")?.contains(document.activeElement)) input.focus();
    }, 0);
    return () => window.clearTimeout(t);
  }, [open]);

  useLayoutEffect(() => {
    const frame = frameRef.current;
    if (!open || !frame) return;
    const measure = () => setTier(searchTier(frame.clientWidth));
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(frame);
    return () => observer.disconnect();
  }, [open]);

  // 结果集变化时高亮回到首项；hits 只在会话给出新结果时换引用。
  useEffect(() => { setActive(0); }, [hits]);

  // ↑↓ 换行时把选中行滚进可视区；悬停换行不动列表。换选中行时预览回到顶部。
  useLayoutEffect(() => {
    if (revealActive.current) {
      revealActive.current = false;
      listRef.current?.querySelector<HTMLElement>('[role="option"][aria-selected="true"]')?.scrollIntoView?.({ block: "nearest" });
    }
    if (previewRef.current) previewRef.current.scrollTop = 0;
  }, [active, hits, threadActive, kind]);

  const remember = useCallback((term: string) => {
    const q = term.trim();
    if (q.length < 2) return;
    setRecent((cur) => [q, ...cur.filter((x) => x !== q)].slice(0, RECENT_MAX));
  }, []);

  const close = () => {
    if (status === "ready" && hits.length > 0) remember(searched);
    onClose();
  };

  if (!open) return null;

  const threadHits = threadSearch.hits;
  const selectedThread = threadHits[threadActive] ?? null;
  const pickThread = (hit: ThreadSearchHitView) => {
    remember(threadSearch.searched || query);
    onClose();
    onOpenThread?.(hit.thread_id, hit.message_id);
  };
  const pick = (id: number, at?: { blockId: number }) => {
    const hit = hits.find((x) => x.id === id);
    remember(searched || query);
    resume.current = { scroll: listRef.current?.scrollTop ?? 0 };
    onClose();
    onOpen(id, {
      query: searched || query.trim(), results: hits.map((x) => x.id),
      resultRows: hits.map(({ id: resultId, title, feed_title }) => ({ id: resultId, title, feed_title })),
      bodyMatch: hit?.body_match ?? true,
      blocks: blocks.peek(id, searched), blockId: at?.blockId,
    });
  };
  const move = (delta: number) => {
    if (onThreads) {
      if (!threadHits.length) return;
      revealActive.current = true;
      setThreadActive((i) => Math.max(0, Math.min(threadHits.length - 1, i + delta)));
      return;
    }
    if (!hits.length) return;
    revealActive.current = true;
    setActive((i) => Math.max(0, Math.min(hits.length - 1, i + delta)));
  };
  const clear = () => { setQuery(""); inputRef.current?.focus(); };
  const selected = hits[active] ?? null;
  const hasQuery = query.trim().length > 0;
  const loading = status === "loading";
  const showResults = hits.length > 0 && (status === "ready" || status === "loading" || status === "error");
  const none = status === "ready" && hits.length === 0;
  const articleCount = showResults ? t(hits.length >= SEARCH_LIMIT ? "count.articlesCapped" : "count.articles", { count: hits.length }) : none ? t("count.articles", { count: 0 }) : "";
  const threadReady = threadSearch.status === "ready" || (threadSearch.status !== "idle" && threadHits.length > 0);
  const threadCount = threadReady ? t(threadHits.length >= THREAD_SEARCH_LIMIT ? "count.threadsCapped" : "count.threads", { count: threadHits.length }) : "";
  const count = onThreads ? threadCount : articleCount;
  const kindCounts = {
    articles: showResults ? (hits.length >= SEARCH_LIMIT ? `${hits.length}+` : String(hits.length)) : none ? "0" : "",
    threads: threadReady ? (threadHits.length >= THREAD_SEARCH_LIMIT ? `${threadHits.length}+` : String(threadHits.length)) : "",
  };
  const tips = <SearchTips />;

  return (
    <div className="overlay command-palette-stage" data-region={region} style={{ zIndex: 50 }}>
      <button type="button" className="overlay-dismiss" aria-label={t("close")} onClick={close} />
      <div ref={frameRef} className="search-frame" data-tier={tier}>
        <div ref={rootRef} className="command-palette" role="dialog" aria-modal="true" aria-label={t("dialog")}>
          <div className="command-palette-field">
            <Icon name="search" size={18} className="command-palette-icon" />
            <input
              ref={inputRef}
              type="search"
              className="command-palette-input"
              placeholder={t("placeholder")}
              aria-label={t("dialog")}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.nativeEvent.isComposing) return;
                if (e.key === "Escape") {
                  e.preventDefault();
                  close();
                  return;
                }
                if (e.key === "ArrowDown" || e.key === "ArrowUp") {
                  e.preventDefault();
                  move(e.key === "ArrowDown" ? 1 : -1);
                  return;
                }
                if (e.key === "Enter" && onThreads && selectedThread) {
                  e.preventDefault();
                  pickThread(selectedThread);
                } else if (e.key === "Enter" && !onThreads && selected) {
                  e.preventDefault();
                  pick(selected.id);
                }
              }}
            />
            {count ? <span className="search-count">{count}</span> : null}
            {hasQuery ? <button type="button" className="search-clear" aria-label={t("clear")} onClick={clear}><Icon name="close" size={12} /></button> : null}
            <kbd className="kbd">Esc</kbd>
            {loading ? <i className="search-busy" aria-hidden="true" /> : null}
          </div>
          <SearchFilterBar filters={filters} feeds={feeds} onChange={setFilters}
            kind={onSearchThreads ? kind : undefined} counts={kindCounts} onKind={(next) => { setKind(next); inputRef.current?.focus(); }} />
          <div className="search-body">
            <SearchList listRef={listRef} active={open && topmost} loading={loading} dim={loading && hits.length > 0}
              onMouseMove={(e) => {
                // 只响应真实的鼠标移动：↑↓ 或滚动让行滑到静止的指针下不算。窄档没有预览，悬停不改选中。
                const at = `${e.clientX},${e.clientY}`;
                if (at === lastPointer.current) return;
                lastPointer.current = at;
                if (tier === "narrow") return;
                const row = (e.target as Element).closest<HTMLElement>("[data-index]");
                if (row) (onThreads ? setThreadActive : setActive)(Number(row.dataset.index));
              }}>{observe => onThreads ? (
                <ThreadResults search={threadSearch} hasQuery={hasQuery} short={status === "short"} active={threadActive}
                  folderName={threadFolderName} onPick={pickThread} />
              ) : <>
              {status === "error" ? (
                <div className="search-error" role="alert">
                  <Icon name="info" size={15} />
                  <span>{t("error.articles", { message: error })}</span>
                  <button type="button" className="btn sm" onClick={retry}>{t("retry", { ns: "common" })}</button>
                </div>
              ) : null}
              {!hasQuery ? (
                <>
                  {recent.length ? (
                    <>
                      <div className="search-caption">{t("recent.caption")}</div>
                      <div className="search-recent">
                        {recent.map((term) => (
                          <button key={term} type="button" onClick={() => { setQuery(term); inputRef.current?.focus(); }}>
                            <Icon name="clock" size={14} />{term}<span>{t("recent.session")}</span>
                          </button>
                        ))}
                      </div>
                    </>
                  ) : null}
                  {tier === "narrow" ? tips : null}
                </>
              ) : status === "short" ? (
                <p className="command-palette-hint">{t("hint.short")}</p>
              ) : loading && hits.length === 0 ? (
                <p className="command-palette-hint" role="status">{t("hint.loading")}</p>
              ) : none ? (
                <div className="search-none">
                  <h3>{t("none.title", { query: searched })}</h3>
                  {threadHits.length && onSearchThreads ? <p>{t("none.inThreads", { count: threadHits.length })}
                    <button type="button" className="link-btn" onClick={() => setKind("threads")}>{t("none.seeThreads")}</button></p> : null}
                  <p>{t("none.advice")}</p>
                  <div className="search-none-actions">
                    {filtersActive(filters) ? <button type="button" className="btn" onClick={() => setFilters(DEFAULT_FILTERS)}>{t("none.clearFilters")}</button> : null}
                    <button type="button" className="btn" onClick={clear}>{t("none.reset")}</button>
                  </div>
                </div>
              ) : showResults ? (
                <div role="listbox" aria-label={t("results.articles")}>
                  {hits.map((hit, i) => (
                    <SearchRow key={hit.id} hit={hit} index={i} query={searched} selected={i === active} flash={i === flash} onPick={pick} observe={observe} />
                  ))}
                </div>
              ) : null}
            </>}</SearchList>
            {tier !== "narrow" && onThreads ? (
              <div ref={previewRef} className="search-preview">
                {selectedThread ? <ThreadPreview key={selectedThread.thread_id} hit={selectedThread} query={threadSearch.searched}
                  folder={selectedThread.folder_id != null ? threadFolderName?.(selectedThread.folder_id) : undefined} /> : <ThreadTips />}
              </div>
            ) : tier !== "narrow" ? (
              <div ref={previewRef} className="search-preview">
                {selected && showResults
                  ? <SearchPreview key={selected.id} hit={selected} query={searched} blocks={blocks} minutes={minutes.data ?? 0}
                      results={tier === "wide" ? hits : null} onOpen={pick} />
                  : tips}
              </div>
            ) : null}
          </div>
          <div className="command-palette-foot">
            <span><kbd className="kbd">↑</kbd><kbd className="kbd">↓</kbd> {t("foot.select")}</span>
            <span><kbd className="kbd">Enter</kbd> {onThreads ? t("foot.openThread") : t("foot.openArticle")}</span>
            <span className="search-foot-gap" />
            <span>{t("foot.local")}</span>
          </div>
        </div>
      </div>
    </div>
  );
}

/** 随 open 挂载滚动容器，确保 hook 初始化时 root 已存在；查询保活不保留曝光计时。 */
function SearchList({ listRef, active, loading, dim, onMouseMove, children }: {
  listRef: RefObject<HTMLDivElement | null>;
  active: boolean;
  loading: boolean;
  dim: boolean;
  onMouseMove: (event: MouseEvent<HTMLDivElement>) => void;
  children: (observe: ExposureObserver) => ReactNode;
}) {
  const observe = useExposureTracker({ surface: "search", root: listRef, active });
  return <div ref={listRef} className="search-list" data-dim={dim || undefined} aria-busy={loading} onMouseMove={onMouseMove}>{children(observe)}</div>;
}

function SearchRow({ hit, index, query, selected, flash, onPick, observe }: { hit: SearchHitView; index: number; query: string; selected: boolean; flash: boolean; onPick: (id: number) => void; observe: ExposureObserver }) {
  const { id } = hit;
  const { t } = useTranslation("search");
  const ref = useCallback((element: HTMLButtonElement | null) => observe(element, id, index), [observe, id, index]);
  return (
    <button ref={ref} type="button" role="option" aria-selected={selected} data-index={index} className="search-row" data-read={hit.read || undefined} data-flash={flash || undefined}
      onClick={() => onPick(hit.id)}>
      <span className="search-row-head">
        <span className="command-palette-title"><Highlight text={hit.title} query={query} /></span>
        <Score value={hit.score} />
      </span>
      <span className="search-row-meta">
        <SourceAvatar title={hit.feed_title} size={14} iconHref={hit.feed_icon_href} />
        <span className="search-row-feed">{hit.feed_title}</span>
        {hit.published_at != null ? <><span aria-hidden="true">·</span><RelativeTime ms={hit.published_at} /></> : null}
        {hit.read ? <><span aria-hidden="true">·</span><span>{t("state.read")}</span></> : null}
        {hit.starred ? <><span aria-hidden="true">·</span><span className="search-row-star"><Icon name="star" size={11} filled />{t("state.starred")}</span></> : null}
      </span>
      {hit.snippet ? <span className="search-row-snippet"><Highlight text={hit.snippet} query={query} /></span> : null}
    </button>
  );
}

function SearchTips() {
  const { t } = useTranslation("search");
  return (
    <div className="search-tips">
      <h4>{t("tips.articlesTitle")}</h4>
      <p><Trans t={t} i18nKey="tips.articlesBody" components={{ code: <code /> }} /></p>
    </div>
  );
}

/** 对话结果：每条会话一行，图标 · 标题（命中高亮）· 文件夹与时间 · 命中消息的片段（标出是你还是助手说的）。 */
function ThreadResults({ search, hasQuery, short, active, folderName, onPick }: {
  search: ThreadSearch;
  hasQuery: boolean;
  short: boolean;
  active: number;
  folderName?: (folderId: number) => string | undefined;
  onPick: (hit: ThreadSearchHitView) => void;
}) {
  const { t } = useTranslation("search");
  if (!hasQuery) return <p className="command-palette-hint">{t("hint.threadsEmpty")}</p>;
  if (short) return <p className="command-palette-hint">{t("hint.short")}</p>;
  if (search.status === "error") return <div className="search-error" role="alert"><Icon name="info" size={15} /><span>{t("error.threads", { message: search.error })}</span></div>;
  if (search.status === "loading" && !search.hits.length) return <p className="command-palette-hint" role="status">{t("hint.loading")}</p>;
  if (search.status === "ready" && !search.hits.length) {
    return <div className="search-none"><h3>{t("none.threadsTitle", { query: search.searched })}</h3><p>{t("none.threadsAdvice")}</p></div>;
  }
  return <div role="listbox" aria-label={t("results.threads")}>
    {search.hits.map((hit, i) => {
      const folder = hit.folder_id != null ? folderName?.(hit.folder_id) : undefined;
      return <button key={hit.thread_id} type="button" role="option" aria-selected={i === active} data-index={i} className="search-row search-thread-row"
        onClick={() => onPick(hit)}>
        <span className="search-row-head">
          <Icon name="chat" size={14} className="search-thread-icon" />
          <span className="command-palette-title"><Highlight text={hit.title} query={search.searched} /></span>
        </span>
        <span className="search-row-meta">
          {folder ? <><span className="search-thread-folder"><Icon name="folder" size={11} />{folder}</span><span aria-hidden="true">·</span></> : null}
          <RelativeTime ms={hit.message_id ? hit.created_at : hit.updated_at} />
        </span>
        {hit.snippet ? <span className="search-row-snippet">
          <span className="search-thread-who">{t("thread.snippetWho", { who: t(hit.role === "user" ? "thread.who.user" : "thread.who.assistant") })}</span><Highlight text={hit.snippet} query={search.searched} />
        </span> : null}
      </button>;
    })}
  </div>;
}

/** 对话预览：会话标题、命中的是哪条消息，以及那条消息的片段；Enter 打开并定位过去。 */
function ThreadPreview({ hit, query, folder }: { hit: ThreadSearchHitView; query: string; folder?: string }) {
  const { t } = useTranslation("search");
  const who = t(hit.role === "user" ? "thread.who.user" : "thread.who.assistant");
  return <div className="search-preview-main">
    <div className="search-preview-source"><Icon name="chat" size={13} />{folder ? t("thread.sourceIn", { folder }) : t("thread.source")}</div>
    <h2><Highlight text={hit.title} query={query} /></h2>
    <div className="search-preview-meta">
      {t(!hit.message_id ? "thread.hitTitle" : hit.role === "user" ? "thread.hitUser" : "thread.hitAssistant")} · <RelativeTime ms={hit.message_id ? hit.created_at : hit.updated_at} />
    </div>
    {hit.snippet ? <div className="search-preview-context"><h4>{who}</h4><p><Highlight text={hit.snippet} query={query} /></p></div> : null}
    <p className="search-preview-go">{t(hit.message_id ? "thread.goMessage" : "thread.goLatest")}</p>
  </div>;
}

function ThreadTips() {
  const { t } = useTranslation("search");
  return (
    <div className="search-tips">
      <h4>{t("tips.threadsTitle")}</h4>
      <p>{t("tips.threadsBody")}</p>
    </div>
  );
}
