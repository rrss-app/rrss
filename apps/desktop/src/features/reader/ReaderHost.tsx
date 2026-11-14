import {
  forwardRef,
  useCallback,
  useContext,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { LIBRARY_FIXTURE, LIBRARY_FIXTURE_EN } from "../../components/BlockView";
/** 无宿主预览合成的阅读视图；没有真实翻译服务，走整包示例译文，不建渐进会话。 */
const PREVIEW_SOURCE = "preview-";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { settingsSchemaOptions } from "../../settings/settingsQuery";
import { readerHasSelection, useReaderTranslation } from "./useReaderTranslation";
import { TranslationStatus, translationAction } from "./TranslationStatus";
import { translationNotes, unitFailed } from "./translationState";
import { ReaderOverlay, readerDrawerKey, scrollReaderToTop, type ReaderPosition } from "../../components/ReaderOverlay";
import { api, onEmbedCover, onMediaReady, type ArticleView, type CitationView, type MediaRef, type ReaderView, type RelatedArticleView, type SearchHitBlocksView, type SearchHitView } from "../../lib/api";
import { classifyEmbed } from "../../components/embedUrl";
import { parseDocument } from "../../lib/blockIr";
import { formatNotice } from "../../lib/notice";
import { isTauri } from "../../lib/preview";
import { formatPublishedFull } from "../../lib/time";
import { isTypingTarget } from "../../lib/shortcuts";
import {
  createReaderRequestTracker,
  type ReaderOpKind,
  type ReaderOpTicket,
  type ReaderTicket,
} from "./readerRequests";

import type { ReaderCitationSource } from "./ReaderCitationTray";
import { createReaderCache } from "./readerCache";
import { createArticleWriteOrder } from "../../lib/articleWriteOrder";
import { warmImages } from "../../lib/imageWarm";
import { openOriginal, trackEvent } from "../../lib/signals";
import { WebFailure, WebFirstTip, WebOrigin, WebReadingActions, WebReadingBar, WebReadingNav, type WebPage } from "./WebReading";
import { WebLinksContext } from "../../lib/webLinks";
import type { FulltextError, WebVerifyStateEvent } from "../../bindings";
import type { ReadSessionVia } from "../../bindings";
import { i18n, useTranslation } from "../../i18n";
import { inferReadingVia, useReadingSession } from "./useReadingSession";

function mergeReaderMedia(view: ReaderView, incoming: MediaRef): ReaderView {
  const exists = view.media.some((item) => item.src === incoming.src);
  const media = exists ? view.media.map((item) => (item.src === incoming.src ? incoming : item)) : [...view.media, incoming];
  return { ...view, media };
}

export type ReaderOpenOptions = {
  /** 阅读器篇目抽屉切换，保留返回列表语义但单独记录入口。 */
  /** related：从文末「相关阅读」打开，作为岔路记入返回栈，顶栏显示「回到上一篇」。 */
  via?: "drawer" | "related";
  /** 引用发送的来源；thread 同时建立本次打开的对话返回目标。 */
  citationSource?: ReaderCitationSource;
  /** 消息摘录的原文块；失效时沿用按篇阅读位置。 */
  anchor?: { blockId: number };
  /** 从简报 / 对话等非列表入口打开：不提供上下篇。 */
  skipNav?: boolean;
  /** 焦点列表入口：关闭时把当前篇交还列表定位。 */
  returnToList?: boolean;
  /** 从搜索打开：定位到命中段、N / Shift+N 跳命中，上下篇按搜索结果顺序，返回回到搜索。 */
  search?: ReaderSearchOpen;
};

export type ReaderSearchOpen = {
  query: string;
  /** 本次搜索结果的文章顺序，作上下篇。 */
  results: number[];
  resultRows?: Array<Pick<SearchHitView, "id" | "title" | "feed_title">>;
  /** 正文有没有命中；只标题命中时从头打开，不取命中段。 */
  bodyMatch: boolean;
  /** 搜索浮层已取到的命中段；没有时阅读器自己取。 */
  blocks?: SearchHitBlocksView;
  /** 打开到这一段（预览里点了某处命中）；默认第一处。 */
  blockId?: number;
};

/** 阅读会话里的搜索上下文。命中段只在正文版本与取命中段时一致时生效，否则按阅读位置规则打开。 */
type SearchSession = { query: string; results: number[]; resultRows: Array<Pick<SearchHitView, "id" | "title" | "feed_title">>; hits?: { sourceHash: string; blockIds: number[] }; index: number };

function searchSession(search: ReaderSearchOpen): SearchSession {
  const blocks = search.blocks;
  const blockIds = blocks?.hits.map((hit) => hit.block_id) ?? [];
  const at = search.blockId === undefined ? 0 : blockIds.indexOf(search.blockId);
  return { query: search.query, results: search.results, resultRows: search.resultRows ?? [], hits: blocks ? { sourceHash: blocks.source_hash, blockIds } : undefined, index: Math.max(0, at) };
}

export type ReaderHostHandle = {
  openArticle: (id: number, opts?: ReaderOpenOptions) => Promise<void>;
  openWebLink: (url: string) => Promise<void>;
  openExtracted: (event: WebVerifyStateEvent) => Promise<void>;
  back: () => boolean;
  markCollected: (id: number) => void;
  close: () => void;
  getSourceArticle: () => Readonly<{ id: number; title: string }> | null;
  getReturnArticleId: () => number | null;
  getDismissedArticleIds: () => number[];
};

export type ReaderNotice = { message: string; actionLabel?: string; onAction?: () => void };

export type TranslateMode = "original" | "bilingual" | "translated";

export type ReaderHostProps = {
  /** 阅读器叠层是否在导航栈中；关闭后在途结果全部作废。 */
  open: boolean;
  /** 阅读器是最上层叠层（沿用宿主叠层契约）。 */
  dismissible: boolean;
  /** 阅读器快捷键生效（最上层且没有搜索 / 离开确认压在上面）。 */
  shortcuts: boolean;
  inert: boolean;
  style?: CSSProperties;
  /** 上下篇上下文：当前列表、续载以及与焦点共用的筛选 / 合辑展开。 */
  nav: { articles: ArticleView[]; hasMore: boolean; loadingMore?: boolean; scope?: string; expandedCluster?: number | null; revision?: number; reset?: number; loadMore: () => Promise<void>; setScope?: (scope: string) => Promise<unknown>; setExpanded?: (id: number | null) => Promise<unknown>; setCurrent?: (id: number | null) => void; getCurrent?: () => { articles: ArticleView[]; hasMore: boolean; loadingMore?: boolean; scope?: string; expandedCluster?: number | null; revision?: number; reset?: number } };
  /** 列表打开时阅读会话记的来源；订阅时间线为 `feeds`，焦点栏目为 `focus_column`。 */
  listVia?: ReadSessionVia;
  /** 篇目抽屉的标题：打开阅读器的列表名，缺省「今日焦点」。 */
  listTitle?: string;
  onRequestOpen: () => void;
  onOpenUrl?: (href: string) => void;
  onClose: () => void;
  /** 列表同步：宿主已在打开时标记已读，或用户在阅读器里标回未读。 */
  onReadChange: (id: number, read: boolean) => void;
  /** 收藏动作由应用层执行（列表与刷新随之更新）；返回是否成功。 */
  onToggleStar: (id: number, starred: boolean) => Promise<boolean>;
  onRate: (article: ArticleView, up: boolean) => void | Promise<boolean>;
  /** `x` 与篇目里的踩：只记反馈、不弹规则建议；未提供时退回 `onRate`。 */
  onDislike?: (article: ArticleView) => void | Promise<boolean>;
  onCite: (chip: CitationView) => void;
  citationTray?: ReactNode;
  onTrayEscape?: () => boolean;
  onContinueAsking?: () => void;
  onReturnToThread?: (id: number) => void;
  /** 从搜索打开的文章关闭时回到搜索浮层；参数是关闭时正在读的那篇。 */
  onReturnToSearch?: (id: number) => void;
  onTranslateSettings?: () => void;
  notify: (notice: ReaderNotice) => void;
  onAsk: (title: string, message: string, onOk: () => void) => void;
};

function needsEmbedCovers(view: ReaderView): boolean {
  return parseDocument(view.document).blocks.some((block) => {
    if (block.t !== "embed") return false;
    const kind = classifyEmbed(block.url, block.kind).kind;
    if (kind !== "youtube" && kind !== "bilibili") return false;
    const openUrl = classifyEmbed(block.url, block.kind);
    return !(view.embed_covers ?? []).some((cover) => !cover.failed && cover.cover_href && (cover.url === block.url || (openUrl.kind !== "external" && cover.open_url === openUrl.openUrl)));
  });
}

export const READING_POSITION_LIMIT = 200;
/** 阅读区宽度达到该值才真正钉住篇目抽屉；窄于此时钉住偏好仍保留，但按浮层显示。 */
export const READER_DRAWER_PIN_MIN_WIDTH = 1100;

type Session = { web?: WebPage; ticket: ReaderTicket; view: ReaderView; skipNav: boolean; returnToList: boolean; readingVia: ReadSessionVia; openedAt: number | null; fromThread?: Extract<ReaderCitationSource, { kind: "thread" }>; anchor?: ReaderOpenOptions["anchor"]; search?: SearchSession; detour?: boolean };

/** 文末「相关阅读」最多几条（样板 A）。 */
const RELATED_LIMIT = 3;
const relatedKey = (articleId: number) => ["related_articles", articleId, RELATED_LIMIT] as const;

/** 当前篇显示后多久开始预取下一篇：让正文先排好、首屏图片先解码。 */
const PREFETCH_DELAY_MS = 400;
/** 指针进入左缘感应条后多久拉出篇目：掠过或甩向「返回」时不打开；条内上下移动不重新计时。 */
const DRAWER_EDGE_MS = 440;
/** 预热只取前几张已缓存到本地的图片：首屏和紧随其后的最要紧，整篇都解码太占内存。 */
const mediaHrefs = (view: ReaderView) => view.media.map((media) => media.href).filter(Boolean).slice(0, 6);

function ReaderOpening({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation("reader");
  return <div className="overlay reader-stage" data-reader-opening="true">
    <div className="drawer reader" role="dialog" aria-label={t("host.opening")} aria-busy="true">
      <div className="drawer-head reader-head" data-tauri-drag-region="deep"><button type="button" className="tool-btn reader-back" onClick={onClose}>{t("host.back")}</button></div>
      <div className="reader-opening-body" data-reader-opening-skeleton="true" role="status" aria-label={t("host.opening")}>
        <div className="reader-opening-source"><i /><i /></div>
        <div className="reader-opening-title"><i /></div>
        <div className="reader-opening-meta"><i /></div>
        <div className="reader-opening-rule" aria-hidden="true" />
        {[0, 1, 2].map((paragraph) => <div className="reader-opening-paragraph" key={paragraph}><i /><i /><i /></div>)}
      </div>
    </div>
  </div>;
}

/**
 * 阅读器宿主：拥有阅读会话、文章加载、图片更新、阅读工具与阅读器快捷键。
 * 根壳只负责把它挂进叠层栈并提供导航上下文；打开文章通过 ref 句柄表达意图。
 */
export const ReaderHost = forwardRef<ReaderHostHandle, ReaderHostProps>(function ReaderHost(props, ref) {
  const { open, dismissible, shortcuts, inert, style, nav, onClose, onRate } = props;
  const { t } = useTranslation("reader");
  const webActions = useContext(WebLinksContext);
  const [extractingSession, setExtractingSession] = useState<number | null>(null);
  const [requests] = useState(createReaderRequestTracker);
  const [cache] = useState(createReaderCache);
  const [readOrder] = useState(createArticleWriteOrder);
  const [session, setSession] = useState<Session | null>(null);
  const returns = useRef<Session[]>([]);
  const [failure, setFailure] = useState<(WebPage & { error: Pick<FulltextError, "kind"> }) | null>(null);
  const [opening, setOpening] = useState<{ ticket: ReaderTicket; id: number; late: boolean } | null>(null);
  /** 已为哪次打开推入叠层显示骨架；这次打开失败时要把空壳关掉。 */
  const lateTicket = useRef<ReaderTicket | null>(null);
  /** 预取的下一篇：只读快照（不标已读），打开时当作缓存先显示，再由打开结果核对。 */
  const prefetched = useRef(new Map<number, ReaderView>());
  const [ops, setOps] = useState<Partial<Record<ReaderOpKind, ReaderOpTicket>>>({});
  const [translateMode, setTranslateMode] = useState<TranslateMode>("original");
  const [drawerOpen, setDrawerOpen] = useState(false);
  const [drawerPinned, setDrawerPinned] = useState(false);
  const [drawerSelectedId, setDrawerSelectedId] = useState<number | undefined>();
  const drawerSelectedRef = useRef<number | undefined>(undefined);
  const [canPinDrawer, setCanPinDrawer] = useState(false);
  const [dismissedRevision, setDismissedRevision] = useState(0);
  const dismissedIds = useRef(new Set<number>());
  // 阅读器自己取的命中段可能先于文章到达，先存着，显示这次打开时再用。
  const earlyHitBlocks = useRef<{ ticket: ReaderTicket; blocks: SearchHitBlocksView } | null>(null);
  const stageObserver = useRef<ResizeObserver | null>(null);
  const drawerPinnedRef = useRef(false);
  const canPinRef = useRef(false);
  const edgeTimer = useRef<number | undefined>(undefined);
  const drawerActionsRef = useRef<{ close: () => void; open: () => void; openArticle: (id: number) => void; rate: (id: number) => Promise<void>; rows: () => ArticleView[] }>({
    close: () => {}, open: () => {}, openArticle: () => {}, rate: async () => {}, rows: () => [],
  });

  // 快捷键与异步续作读取最新状态，不依赖闭包快照。
  const sessionRef = useRef(session);
  sessionRef.current = session;
  const opsRef = useRef(ops);
  opsRef.current = ops;
  const navRef = useRef(nav);
  navRef.current = nav;
  const propsRef = useRef(props);
  propsRef.current = props;
  const translateModeRef = useRef(translateMode);
  translateModeRef.current = translateMode;

  const baseReader = session?.view ?? null;
  const readingSession = useReadingSession({
    open, dismissible, inert, view: baseReader, ticket: session?.ticket ?? null,
    openedAt: session?.openedAt ?? null, via: session?.readingVia ?? "focus", translated: translateMode !== "original",
  });
  const userMode = useRef(false);
  const pendingMode = useRef<TranslateMode | null>(null);
  useEffect(() => {
    userMode.current = false;
    pendingMode.current = null;
    setTranslateMode("original");
  }, [baseReader?.source_hash]);
  const positions = useRef(new Map<string, ReaderPosition>());
  const settings = useQuery({ ...settingsSchemaOptions(), enabled: open });
  const queryClient = useQueryClient();
  // 相关阅读：岔路里的文章与网页不再列，免得越走越远；没有向量或找不到够近的时返回空，整块不出现。
  const relatedId = session && !session.detour && !session.web ? session.view.id : null;
  const related = useQuery({
    queryKey: relatedKey(relatedId ?? 0),
    queryFn: () => api.relatedArticles(relatedId!, RELATED_LIMIT),
    enabled: open && relatedId != null,
    staleTime: 5 * 60_000,
    retry: false,
  });
  const debug = settings.data?.values.some(value => value.id === "ui_debug_mode" && value.value === "1") ?? false;
  const currentNav = () => navRef.current.getCurrent?.() ?? navRef.current;
  drawerPinnedRef.current = drawerPinned;
  canPinRef.current = canPinDrawer;
  drawerSelectedRef.current = drawerSelectedId;
  const selectDrawerRow = useCallback((id: number | undefined) => {
    drawerSelectedRef.current = id;
    setDrawerSelectedId(id);
  }, []);
  const effectiveDrawerPinned = drawerPinned && canPinDrawer;
  const drawerOpenRef = useRef(drawerOpen);
  drawerOpenRef.current = drawerOpen;
  const pinnedNow = () => drawerPinnedRef.current && canPinRef.current;
  const navigationRows = () => currentNav().articles.filter((article) => article.row_kind === 0 && !dismissedIds.current.has(article.id));
  const navigationIds = () => navigationRows().map((article) => article.id);
  const listedRows = () => {
    void dismissedRevision;
    return currentNav().articles.filter((article) => article.row_kind === 1 || (article.row_kind === 0 && !dismissedIds.current.has(article.id)));
  };
  const articleForRate = (view: ReaderView): ArticleView => currentNav().articles.find((article) => article.row_kind === 0 && article.id === view.id) ?? {
    id: view.id, title: view.title, feed_id: 0, feed_title: "", feed_icon_href: "", published_at: null, url: view.url,
    read: true, starred: view.starred, score: view.score, summary: view.summary, row_kind: 0,
    cluster_id: 0, cluster_size: 1, cluster_label: "", cluster_span: "", interest_reason: null, noisy: false, spot_check: false,
  };
  const observeStage = useCallback((node: HTMLDivElement | null) => {
    stageObserver.current?.disconnect();
    stageObserver.current = null;
    if (!node || typeof ResizeObserver === "undefined") return;
    const apply = () => setCanPinDrawer(node.clientWidth >= READER_DRAWER_PIN_MIN_WIDTH);
    const observer = new ResizeObserver(apply);
    observer.observe(node);
    stageObserver.current = observer;
    apply();
  }, []);

  const setView = useCallback((articleId: number, update: (cur: ReaderView) => ReaderView) => {
    const cur = sessionRef.current;
    if (!cur || cur.view.id !== articleId) return;
    const view = update(cur.view);
    if (requests.owns(cur.ticket) && !view.source_hash.startsWith(PREVIEW_SOURCE)) cache.write(view, cur.ticket);
    // 引用锚点只用于这次打开；正文换版后按现有位置规则从顶部开始。
    const next = { ...cur, view, anchor: view.source_hash === cur.view.source_hash ? cur.anchor : undefined };
    sessionRef.current = next;
    setSession(next);
  }, [cache, requests]);

  const acceptMedia = useCallback((articleId: number, incoming: MediaRef) => {
    const update = (view: ReaderView) => mergeReaderMedia(view, incoming);
    cache.update(articleId, update);
    setView(articleId, update);
  }, [cache, setView]);

  const preview = baseReader?.source_hash?.startsWith(PREVIEW_SOURCE) ?? false;
  const translation = useReaderTranslation(preview ? null : baseReader, session?.ticket ?? null, requests.owns, next => {
    setView(next.id, cur => cur.source_hash !== next.source_hash ? next : {
      ...cur, translated_title: next.translated_title, translated_document: next.translated_document,
      translation_ready: next.translation_ready, translation_note: next.translation_note,
    });
  }, () => { if (!userMode.current) setTranslateMode("bilingual"); }, settings.dataUpdatedAt);
  const tr = translation.state;
  const readingOnBody = readingSession.onBody;
  const translationOnBody = translation.onBody;
  const onReaderBody = useCallback((body: HTMLDivElement | null) => {
    translationOnBody(body);
    readingOnBody(body);
  }, [translationOnBody, readingOnBody]);
  const invalidated = tr.data?.snapshot.phase === "invalidated";
  const reader = useMemo(() => baseReader && (tr.data ? {
    ...baseReader,
    translated_document: invalidated ? null : tr.data.document,
    translated_title: invalidated ? "" : tr.data.title,
    translation_ready: !invalidated && tr.data.ready,
    translation_note: "",
  } : baseReader), [baseReader, tr.data, invalidated]);
  useEffect(() => {
    // 渐进译文是会话覆盖层；把实际可阅读的数据也记入缓存，重开仍须重新握手。
    if (reader && session && requests.owns(session.ticket) && !preview) cache.write(reader, session.ticket);
  }, [cache, requests, reader, session, preview]);
  const blockNotes = useMemo(() => translationNotes(tr.data, parseDocument(baseReader?.document), translateMode, debug),
    [tr.data, translateMode, debug, baseReader?.document]);
  const changeMode = (mode: TranslateMode) => {
    userMode.current = true;
    if (readerHasSelection()) pendingMode.current = mode;
    else setTranslateMode(mode);
  };
  useEffect(() => {
    const flush = () => {
      if (!readerHasSelection() && pendingMode.current) { setTranslateMode(pendingMode.current); pendingMode.current = null; }
    };
    document.addEventListener("selectionchange", flush);
    return () => document.removeEventListener("selectionchange", flush);
  }, []);

  useEffect(() => () => stageObserver.current?.disconnect(), []);
  useEffect(() => {
    if (nav.reset === undefined) return;
    dismissedIds.current.clear();
  }, [nav.reset, nav.scope]);
  useEffect(() => {
    if (!drawerOpen) return;
    const rows = (navRef.current.getCurrent?.() ?? navRef.current).articles
      .filter((article) => article.row_kind === 1 || (article.row_kind === 0 && !dismissedIds.current.has(article.id)));
    const currentId = sessionRef.current?.view.id;
    const selected = drawerSelectedRef.current;
    const next = rows.some((row) => readerDrawerKey(row) === selected)
      ? selected
      : rows.find((row) => row.row_kind === 0 && row.id === currentId)?.id ?? (rows[0] ? readerDrawerKey(rows[0]) : undefined);
    if (next !== selected) selectDrawerRow(next);
  }, [drawerOpen, nav.articles, nav.scope, nav.reset, session?.view.id, selectDrawerRow]);

  /** 显示一份阅读视图并把它记为当前会话；之前会话的工具 busy 状态随之作废。 */
  const showReader = (ticket: ReaderTicket, view: ReaderView, skipNav: boolean, returnToList: boolean, options?: ReaderOpenOptions, openedAt: number | null = null, web?: WebPage) => {
    setOpening(null);
    setTranslateMode("original");
    userMode.current = false; pendingMode.current = null;
    setOps({});
    const next = { web, ticket, view, skipNav, returnToList, openedAt, readingVia: inferReadingVia(options, propsRef.current.listVia), anchor: options?.anchor,
      detour: options?.via === "related" || undefined,
      search: options?.search ? searchSession(earlyHitBlocks.current?.ticket === ticket && !options.search.blocks
        ? { ...options.search, blocks: earlyHitBlocks.current.blocks } : options.search) : undefined,
      fromThread: options?.citationSource?.kind === "thread" ? options.citationSource
        : sessionRef.current?.view.id === view.id ? sessionRef.current.fromThread : undefined };
    sessionRef.current = next;
    setSession(next);
    navRef.current.setCurrent?.(skipNav ? null : view.id);
    propsRef.current.onRequestOpen();
  };

  const finishOp = (ticket: ReaderOpTicket) => {
    setOps((cur) => (cur[ticket.kind] === ticket ? { ...cur, [ticket.kind]: undefined } : cur));
  };

  /**
   * 对当前显示的文章执行一次阅读工具操作。结果与失败只在操作仍归属当前会话且未被同类操作取代时生效；
   * busy 的清理只针对本次操作。正在等待另一篇打开时忽略操作，不替尚未显示的文章发请求。
   */
  const runOp = async <T,>(
    kind: ReaderOpKind,
    request: (articleId: number) => Promise<T>,
    onResult: (result: T, articleId: number) => void,
    onError: (err: unknown, articleId: number) => void,
  ) => {
    const cur = sessionRef.current;
    if (!cur || opsRef.current[kind]) return;
    const op = requests.beginOp(kind, cur.ticket);
    if (!op) return;
    setOps((prev) => ({ ...prev, [kind]: op }));
    try {
      const result = await request(op.articleId);
      if (requests.ownsOp(op)) onResult(result, op.articleId);
    } catch (err) {
      if (requests.ownsOp(op)) onError(err, op.articleId);
    } finally {
      finishOp(op);
    }
  };

  const openArticle = async (id: number, opts?: ReaderOpenOptions, web?: WebPage) => {
    if (!web && opts?.via !== "related") returns.current = [];
    setFailure(null);
    const skipNav = opts?.skipNav ?? false;
    const returnToList = !skipNav && (opts?.returnToList ?? false);
    if (drawerOpen && effectiveDrawerPinned) selectDrawerRow(id);
    translation.detach();
    const ticket = requests.open(id);
    const early = prefetched.current.get(id);
    prefetched.current.delete(id);
    const cached = cache.read(ticket) ?? early;
    if (cached) showReader(ticket, cached, skipNav, returnToList, opts, null, web);
    else setOpening({ ticket, id, late: false });
    const search = opts?.search;
    if (search && search.bodyMatch && !search.blocks) {
      // 和打开并行取命中段；到达时这次打开仍是当前会话才生效。
      earlyHitBlocks.current = null;
      void api.searchHitBlocks(id, search.query).then((blocks) => {
        if (!requests.owns(ticket)) return;
        const cur = sessionRef.current;
        if (cur?.ticket !== ticket) { earlyHitBlocks.current = { ticket, blocks }; return; }
        if (!cur.search) return;
        const next = { ...cur, search: searchSession({ ...search, blocks }) };
        sessionRef.current = next;
        setSession(next);
      }, () => {});
    }
    try {
      const r = cache.reconcile(await readOrder.open(id, () => api.openArticle(id)), ticket, cached);
      trackEvent(id, "open");
      // 打开动作成功后宿主已标已读；列表按原文 ID 同步，即使响应已过期也不撤回。失败则不同步。
      propsRef.current.onReadChange(id, true);
      if (!requests.owns(ticket)) return;
      const openedAt = Date.now();
      if (cached) {
        if (sessionRef.current?.ticket !== ticket) return;
        setView(id, () => r);
        const confirmed = { ...sessionRef.current!, openedAt };
        sessionRef.current = confirmed;
        setSession(confirmed);
      } else showReader(ticket, r, skipNav, returnToList, opts, openedAt, web);
      if (needsEmbedCovers(r)) {
        void api.prefetchEmbeds(id).catch(() => {});
      }
      if (r.media.some((m) => !m.href)) {
        const op = requests.beginOp("media", ticket);
        if (!op) return;
        setOps((cur) => ({ ...cur, media: op }));
        try {
          const media = await api.prefetch(id);
          if (requests.ownsOp(op)) setView(id, (cur) => ({ ...cur, media }));
        } finally {
          finishOp(op);
        }
      }
    } catch (err) {
      if (!requests.owns(ticket)) return;
      if (web) { failWeb(web.url, err); return; }
      if (!cached && !isTauri()) {
        const row = navRef.current.articles.find((a) => a.id === id);
        showReader(ticket, {
          id,
          source_hash: `${PREVIEW_SOURCE}${id}`,
          title: row?.title ?? i18n.t("host.previewTitle", { ns: "reader" }),
          meta: [row?.feed_title, formatPublishedFull(row?.published_at)].filter(Boolean).join(" · "),
          feed_icon_href: row?.feed_icon_href ?? "",
          url: row?.url ?? "",
          summary: row?.summary ?? "",
          score: row?.score ?? 0,
          feedback: 0,
          document: LIBRARY_FIXTURE,
          reasons: [],
          media: [],
          translated_title: "",
          translated_document: null,
          translation_ready: false,
          translation_note: "",
          truncated: false, pruned: false, summary_stale: false,
          starred: row?.starred ?? false,
          embed_covers: [],
        }, skipNav, returnToList, opts);
        return;
      }
      setOpening(null);
      if (!sessionRef.current && lateTicket.current === ticket) propsRef.current.onClose();
      propsRef.current.notify({ message: formatNotice(i18n.t("host.openFailedPrefix", { ns: "reader" }), err) });
    }
  };

  const failWeb = (url: string, err: unknown) => {
    const kind = err && typeof err === "object" && "kind" in err ? err.kind : null;
    const error: Pick<FulltextError, "kind"> = { kind: kind === "Waf" || kind === "Empty" || kind === "NotArticle" ? kind : "Network" };
    setOpening(null);
    sessionRef.current = null;
    setSession(null);
    setFailure({ url, error });
    propsRef.current.onRequestOpen();
  };

  const openWebLink = async (url: string, retry = false) => {
    if (!retry && sessionRef.current && requests.owns(sessionRef.current.ticket)) returns.current.push(sessionRef.current);
    readingSession.finish("back");
    translation.detach();
    const ticket = requests.open(0);
    setFailure(null);
    setOps({});
    setDrawerOpen(false);
    setOpening({ ticket, id: 0, late: false });
    try {
      const info = await api.openWebLink(url);
      if (requests.owns(ticket)) await openArticle(info.article_id, { skipNav: true }, { url, info });
    } catch (err) {
      if (requests.owns(ticket)) failWeb(url, err);
    }
  };

  const openExtracted = async (event: WebVerifyStateEvent) => {
    if (event.article_id == null) return;
    const previous = sessionRef.current;
    if (previous && requests.owns(previous.ticket)) returns.current.push(previous);
    const web = previous?.view.id === event.article_id ? previous.web : undefined;
    await openArticle(event.article_id, { skipNav: true }, web ?? { url: event.url });
  };
  const reextract = async () => {
    const cur = sessionRef.current;
    if (!cur || !requests.owns(cur.ticket) || extractingSession === cur.ticket.session) return;
    const url = cur.web?.info?.actual_url ?? cur.web?.url ?? cur.view.url;
    setExtractingSession(cur.ticket.session);
    try {
      const result = await webActions?.extract?.(url);
      if (!requests.owns(cur.ticket) || !result || result.url !== url) return;
      if (result.state !== "Extracted") { propsRef.current.notify({ message: result.reason ?? i18n.t("host.noArticle", { ns: "reader" }) }); return; }
      if (result.article_id !== cur.view.id) return;
      const view = await api.reader(cur.view.id);
      if (requests.owns(cur.ticket)) setView(cur.view.id, () => view);
    } catch (error) {
      if (requests.owns(cur.ticket)) propsRef.current.notify({ message: formatNotice(i18n.t("host.extractFailedPrefix", { ns: "reader" }), error) });
    } finally { setExtractingSession(old => old === cur.ticket.session ? null : old); }
  };

  /** 打开文末的相关阅读：当前这篇进返回栈，Esc / 「回到上一篇」回到它的文末；上下篇仍按原来的列表走。 */
  const openRelated = (item: RelatedArticleView) => {
    const cur = sessionRef.current;
    if (!cur || !requests.owns(cur.ticket)) return;
    returns.current.push(cur);
    readingSession.finish("next");
    queryClient.setQueryData<RelatedArticleView[]>(relatedKey(cur.view.id), rows => rows?.map(row => row.article_id === item.article_id ? { ...row, read: true } : row));
    void openArticle(item.article_id, { skipNav: true, via: "related" });
  };

  const back = () => {
    const previous = returns.current.pop();
    if (!previous) return false;
    readingSession.finish("back");
    translation.detach();
    const next = { ...previous, ticket: requests.open(previous.view.id), openedAt: Date.now() };
    sessionRef.current = next;
    setSession(next);
    setFailure(null);
    setOpening(null);
    setOps({});
    navRef.current.setCurrent?.(next.skipNav ? null : next.view.id);
    return true;
  };

  const markCollected = useCallback((id: number) => {
    const update = (cur: Session): Session => cur.view.id === id && cur.web?.info
      ? { ...cur, web: { ...cur.web, info: { ...cur.web.info, temporary: false } } } : cur;
    returns.current = returns.current.map(update);
    if (sessionRef.current) { sessionRef.current = update(sessionRef.current); setSession(sessionRef.current); }
  }, []);
  const collect = () => runOp("collect", async id => {
    await api.collectArticle(id);
    // 收录已提交，即使用户打开了下一张网页，返回记录也必须反映它。
    markCollected(id);
  }, () => {},
    err => propsRef.current.notify({ message: formatNotice(i18n.t("host.collectFailedPrefix", { ns: "reader" }), err) }));

  const openingTicket = opening?.ticket;
  useEffect(() => {
    if (!openingTicket) return;
    const ticket = openingTicket;
    const timer = window.setTimeout(() => {
      lateTicket.current = ticket;
      propsRef.current.onRequestOpen();
      setOpening(current => {
        if (current?.ticket !== ticket) return current;
        return { ...current, late: true };
      });
    }, 250);
    return () => window.clearTimeout(timer);
  }, [openingTicket]);

  // 正文显示后再备下一篇：取只读快照并预热两篇的前几张本地图片，按 J / K 切换时图片直接出现。
  const shownId = open ? session?.view.id : undefined;
  const prefetchIds = !open || !session ? [] : session.search ? session.search.results : session.skipNav ? [] : navigationIds();
  const prefetchId = shownId === undefined ? undefined : prefetchIds[prefetchIds.indexOf(shownId) + 1];
  useEffect(() => {
    const cur = sessionRef.current;
    if (shownId === undefined || !cur) return;
    warmImages(mediaHrefs(cur.view));
    if (prefetchId === undefined || cache.has(prefetchId)) return;
    const ready = prefetched.current.get(prefetchId);
    if (ready) { warmImages(mediaHrefs(ready)); return; }
    const timer = window.setTimeout(() => {
      void api.reader(prefetchId).then((view) => {
        prefetched.current.set(prefetchId, view);
        // 只留最近两篇，快速翻页时不堆积。
        for (const id of prefetched.current.keys()) if (prefetched.current.size > 2) prefetched.current.delete(id);
        warmImages(mediaHrefs(view));
      }, () => {});
    }, PREFETCH_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [shownId, prefetchId, cache]);

  const rateArticle = (article: ArticleView, up: boolean, dismiss = false) => {
    const result = dismiss && propsRef.current.onDislike ? propsRef.current.onDislike(article) : onRate(article, up);
    void result?.then(saved => {
      if (!saved) return;
      const update = (view: ReaderView) => ({ ...view, feedback: up ? 1 : -1 });
      cache.update(article.id, update);
      setView(article.id, update);
    });
  };

  const closeReader = () => {
    requests.close();
    setOpening(null);
    readingSession.finish("back");
    const cur = sessionRef.current;
    const thread = cur?.fromThread;
    if (thread) propsRef.current.onReturnToThread?.(thread.threadId);
    else if (cur?.search) propsRef.current.onReturnToSearch?.(cur.view.id);
    propsRef.current.onClose();
  };

  useImperativeHandle(ref, () => ({
    openArticle,
    openWebLink,
    openExtracted,
    back,
    markCollected,
    close: closeReader,
    getSourceArticle: () => {
      const view = sessionRef.current?.view;
      return view ? { id: view.id, title: view.title } : null;
    },
    getReturnArticleId: () => {
      // 岔路里关掉阅读器时，列表仍定位到岔开之前的那篇。
      const origin = sessionRef.current?.detour ? returns.current.find(item => !item.detour) : sessionRef.current;
      return origin?.returnToList ? origin.view.id : null;
    },
    getDismissedArticleIds: () => [...dismissedIds.current],
  }));

  useEffect(() => {
    if (open) return;
    // 阅读器一旦关闭，在途的打开 / 工具结果都不再属于任何会话。
    requests.close();
    returns.current = [];
    setFailure(null);
    sessionRef.current = null;
    setSession(null);
    setOpening(null);
    setOps({});
    setDrawerOpen(false);
    selectDrawerRow(undefined);
    if (edgeTimer.current !== undefined) window.clearTimeout(edgeTimer.current);
  }, [open, requests, selectDrawerRow]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    onMediaReady((event) => {
      acceptMedia(event.article_id, event.media);
    }).then((u) => { if (disposed) u(); else unlisten = u; }).catch(() => {});
    return () => { disposed = true; unlisten?.(); };
  }, [acceptMedia]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    onEmbedCover((event) => {
      const update = (view: ReaderView) => {
        const covers = view.embed_covers ?? [];
        const exists = covers.some((item) => item.url === event.cover.url);
        const next = exists ? covers.map((item) => item.url === event.cover.url ? event.cover : item) : [...covers, event.cover];
        return { ...view, embed_covers: next };
      };
      cache.update(event.article_id, update);
      setView(event.article_id, update);
    }).then((u) => { if (disposed) u(); else unlisten = u; }).catch(() => {});
    return () => { disposed = true; unlisten?.(); };
  }, [cache, setView]);

  const openNeighbor = async (dir: -1 | 1) => {
    // 相关阅读的岔路里按 J / K：先回到原来那篇，再按原来的列表走。
    if (sessionRef.current?.detour) {
      if (back()) await openNeighbor(dir);
      return;
    }
    const cur = sessionRef.current;
    if (cur?.search) {
      // 从搜索打开：上下篇按搜索结果顺序，邻篇同样打开到它的命中段。
      const { results, query } = cur.search;
      const nextId = results[results.indexOf(cur.view.id) + dir];
      if (nextId !== undefined) await openArticle(nextId, { skipNav: true, search: { query, results, resultRows: cur.search.resultRows, bodyMatch: true } });
      return;
    }
    if (!cur || cur.skipNav) return;
    const ids = navigationIds();
    const pos = ids.indexOf(cur.view.id);
    if (pos < 0) return;
    let nextId = ids[pos + dir];
    if (!nextId && dir === 1 && currentNav().hasMore) {
      await navRef.current.loadMore();
      if (!requests.owns(cur.ticket) || sessionRef.current !== cur) return;
      const nextIds = navigationIds();
      const nextPos = nextIds.indexOf(cur.view.id);
      if (nextPos < 0) return;
      nextId = nextIds[nextPos + 1];
    }
    if (nextId) await openArticle(nextId, { returnToList: cur.returnToList });
  };

  const closeDrawer = () => setDrawerOpen(false);
  const openDrawer = () => {
    if (sessionRef.current?.skipNav) return;
    const id = sessionRef.current?.view.id;
    selectDrawerRow(id);
    setDrawerOpen(true);
  };
  const scheduleDrawerOpen = () => {
    if (drawerOpen || edgeTimer.current !== undefined) return;
    edgeTimer.current = window.setTimeout(() => { edgeTimer.current = undefined; openDrawer(); }, DRAWER_EDGE_MS);
  };
  const cancelDrawerOpen = () => {
    if (edgeTimer.current !== undefined) { window.clearTimeout(edgeTimer.current); edgeTimer.current = undefined; }
  };
  const openDrawerArticle = (id: number) => {
    if (!effectiveDrawerPinned) setDrawerOpen(false);
    selectDrawerRow(id);
    void openArticle(id, { returnToList: sessionRef.current?.returnToList, via: "drawer" });
  };
  const rateAndOpenNext = async (id: number) => {
    const cur = sessionRef.current;
    if (!cur || cur.skipNav || dismissedIds.current.has(id)) return;
    const rows = navigationRows();
    const row = rows.find((article) => article.id === id);
    if (!row) return;
    const current = id === cur.view.id;
    rateArticle(row, false, true);
    dismissedIds.current.add(id);
    setDismissedRevision((value) => value + 1);
    const at = rows.findIndex((article) => article.id === id);
    if (current && !rows[at + 1] && currentNav().hasMore) {
      await navRef.current.loadMore();
      if (!requests.owns(cur.ticket) || sessionRef.current !== cur) return;
    }
    const remaining = navigationRows();
    const nextId = remaining[at]?.id ?? remaining[at - 1]?.id;
    if (nextId) selectDrawerRow(nextId);
    if (!current) return;
    if (!pinnedNow()) setDrawerOpen(false);
    if (nextId) await openArticle(nextId, { returnToList: cur.returnToList, via: "drawer" });
    else onClose();
  };
  const expandDrawerCluster = (id: number | null) => { void navRef.current.setExpanded?.(id); };
  const openDrawerRow = (key: number) => {
    const row = listedRows().find(row => readerDrawerKey(row) === key);
    if (!row) return;
    if (row.row_kind === 1) expandDrawerCluster(currentNav().expandedCluster === row.cluster_id ? null : row.cluster_id);
    else openDrawerArticle(row.id);
  };
  drawerActionsRef.current = { close: closeDrawer, open: openDrawer, openArticle: openDrawerRow, rate: rateAndOpenNext, rows: listedRows };

  const cycleTranslateMode = () => {
    if (!reader?.translation_ready) return;
    const modes: TranslateMode[] = ["original", "bilingual", "translated"];
    const i = modes.indexOf(translateModeRef.current);
    changeMode(modes[(i + 1) % modes.length]);
  };

  const fetchFulltext = () =>
    runOp(
      "fulltext",
      (id) => api.fetchFulltext(id),
      (next, id) => {
        setView(id, () => next);
        propsRef.current.notify({ message: i18n.t("host.fulltextUpdated", { ns: "reader" }) });
      },
      (err, id) => {
        const msg = formatNotice("", err);
        const url = sessionRef.current?.view.id === id ? sessionRef.current.view.url : "";
        propsRef.current.notify({
          message: msg || i18n.t("host.fulltextFailed", { ns: "reader" }),
          actionLabel: url ? i18n.t("host.viewOriginal", { ns: "reader" }) : undefined,
          onAction: url ? () => void openOriginal(id, url) : undefined,
        });
      },
    );

  const summarize = () =>
    runOp(
      "summary",
      (id) => api.summarize(id),
      (text, id) => setView(id, (cur) => ({ ...cur, summary: text, summary_stale: false })),
      (err) => propsRef.current.notify({ message: formatNotice(i18n.t("host.summaryFailedPrefix", { ns: "reader" }), err) }),
    );

  const regenerate = () => {
    const current = sessionRef.current;
    if (!current || tr.busy) return;
    propsRef.current.onAsk(i18n.t("host.regenerate.title", { ns: "reader" }), i18n.t("host.regenerate.detail", { ns: "reader" }), () => {
      if (requests.owns(current.ticket)) void translation.start("regenerate");
    });
  };
  const translate = () => {
    const current = sessionRef.current;
    if (preview && current) {
      setView(current.view.id, cur => ({ ...cur, translated_title: "Opening Day", translated_document: LIBRARY_FIXTURE_EN, translation_ready: true }));
      if (!userMode.current) setTranslateMode("bilingual");
      return;
    }
    if (tr.busy) return;
    const action = translationAction(tr);
    if (action.kind === "regenerate") return regenerate();
    if (action.kind === "recover") return translation.recover();
    if (action.kind === "save") return translation.retrySave();
    if (action.kind === "retry") return translation.start("retry_failed");
    return translation.start();
  };
  const locateFailed = () => {
    const block = tr.data?.snapshot.unit_map.find((_, i) => unitFailed(tr.data!.snapshot.units[i]));
    if (block === undefined) return;
    changeMode("bilingual");
    requestAnimationFrame(() => document.querySelector(block === null ? ".drawer.reader .reader-title" : `.drawer.reader [data-reader-document] [data-block-id="${block}"]`)?.scrollIntoView({ block: "center" }));
  };

  const toggleStar = async () => {
    const cur = sessionRef.current;
    if (!cur) return;
    const { id, starred } = cur.view;
    if (await propsRef.current.onToggleStar(id, starred)) {
      cache.update(id, v => ({ ...v, starred: !starred }));
      setView(id, v => ({ ...v, starred: !starred }));
    }
  };

  const markUnread = async () => {
    const cur = sessionRef.current;
    if (!cur) return;
    const id = cur.view.id;
    try {
      await readOrder.write(id, () => api.markRead(id, false));
      propsRef.current.onReadChange(id, false);
    } catch (err) {
      propsRef.current.notify({ message: formatNotice(i18n.t("host.unreadFailedPrefix", { ns: "reader" }), err) });
    }
  };

  const citeArticle = async () => {
    const cur = sessionRef.current;
    if (!cur) return;
    const { id, title, url } = cur.view;
    const onCite = propsRef.current.onCite;
    try {
      const chip = await api.citeArticle(id);
      markCollected(id);
      onCite(chip);
      trackEvent(id, "cite");
    } catch {
      onCite({ kind: "article", article_id: id, title, feed_title: "", url, excerpt: "", block_id: 0 });
    }
  };

  const citeSelection = useCallback(async (excerpt: string, blockId: number) => {
    const cur = sessionRef.current;
    if (!cur) return;
    const { id, title, url } = cur.view;
    const onCite = propsRef.current.onCite;
    try {
      const chip = await api.citeSelection(id, excerpt, blockId);
      markCollected(id);
      onCite(chip);
      trackEvent(id, "cite");
    } catch {
      onCite({ kind: "span", article_id: id, title, feed_title: "", url, excerpt, block_id: blockId });
    }
  }, [markCollected]);

  /** 在命中段之间循环跳转。 */
  const stepSearchHit = (dir: -1 | 1) => {
    const cur = sessionRef.current;
    const hits = cur?.search?.hits;
    if (!cur?.search || !hits || hits.sourceHash !== cur.view.source_hash || hits.blockIds.length === 0) return;
    const total = hits.blockIds.length;
    const next = { ...cur, search: { ...cur.search, index: (cur.search.index + dir + total) % total } };
    sessionRef.current = next;
    setSession(next);
  };

  const actions = { openNeighbor, stepSearchHit, cycleTranslateMode, fetchFulltext, toggleStar, markUnread, translate };
  const actionsRef = useRef(actions);
  actionsRef.current = actions;
  const hasReader = reader !== null;

  useEffect(() => {
    if (!shortcuts || !hasReader) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing || e.keyCode === 229 || e.ctrlKey || e.metaKey || e.altKey) return;
      if (isTypingTarget(document.activeElement)) return;
      // 右键 / 划词浮层自己处理方向键和 Enter；它们挂在 body 上，不在阅读区内。
      if (document.querySelector(".reader-context-menu, .reader-selection-menu, .reader-selection-translation, .reader-citation-target-menu, .reader-tools[open]")) return;
      const cur = sessionRef.current;
      if (!cur) return;
      const act = actionsRef.current;
      const key = e.key.toLowerCase();
      const stage = document.querySelector(".reader-stage");
      const behind = !(e.target instanceof Node && stage?.contains(e.target));
      const onReaderControl = !behind && e.target instanceof Element && !!e.target.closest("button, a, summary, input, textarea, select");
      const run = (action: () => void) => {
        e.preventDefault();
        // 焦点还在被阅读器盖住的列表上时，不能让那一行的 Enter / Space 再打开旧篇。
        if (behind) e.stopPropagation();
        action();
      };
      const endingCard = !behind && e.target instanceof Element ? e.target.closest<HTMLElement>(".reader-next") : null;
      if (endingCard && (key === "enter" || key === " " || key === "j")) {
        return run(() => {
          if (endingCard.dataset.hasNext === "true") void act.openNeighbor(1);
          else endingCard.focus({ preventScroll: true });
        });
      }
      if (drawerOpenRef.current) {
        if (key === "l") return run(drawerActionsRef.current.close);
        if (key === "escape" && !pinnedNow()) return;
        if (key === "arrowdown" || key === "arrowup") {
          const rows = drawerActionsRef.current.rows();
          if (!rows.length) return;
          const index = Math.max(0, rows.findIndex((row) => readerDrawerKey(row) === drawerSelectedRef.current));
          const next = Math.max(0, Math.min(rows.length - 1, index + (key === "arrowdown" ? 1 : -1)));
          return run(() => selectDrawerRow(readerDrawerKey(rows[next])));
        }
        const selectedId = drawerSelectedRef.current;
        if (key === "enter" && (!onReaderControl || (e.target instanceof Element && !!e.target.closest(".reader-article-drawer-main"))) && selectedId !== undefined) return run(() => drawerActionsRef.current.openArticle(selectedId));
        if (key === "x") return run(() => { if (selectedId !== undefined && selectedId > 0) void drawerActionsRef.current.rate(selectedId); });
      }
      if (key === "q" && cur.fromThread) return run(() => propsRef.current.onContinueAsking?.());
      if (key === "l") return run(drawerActionsRef.current.open);
      if (key === "x") return run(() => void drawerActionsRef.current.rate(cur.view.id));
      if (key === "n" && cur.search?.hits) return run(() => act.stepSearchHit(e.shiftKey ? -1 : 1));
      if (key === "j") return run(() => void act.openNeighbor(1));
      if (key === "k") return run(() => void act.openNeighbor(-1));
      if (key === "home") return run(() => scrollReaderToTop(document.querySelector(".drawer.reader .drawer-body")));
      if (key === " " && (e.target === document.body || (e.target instanceof Element && e.target.matches(".drawer.reader .drawer-body")))) {
        return run(() => {
          const body = document.querySelector(".drawer.reader .drawer-body");
          if (!body) return;
          const atBottom = body.scrollHeight - body.clientHeight <= 4 || body.scrollTop + body.clientHeight >= body.scrollHeight - 4;
          if (!atBottom) {
            body.scrollBy({ top: body.clientHeight * 0.85, behavior: "smooth" });
            return;
          }
          const card = document.querySelector<HTMLElement>(".drawer.reader .reader-next");
          if (!card) return;
          card.focus({ preventScroll: true });
          card.scrollIntoView({ block: "nearest", behavior: "instant" });
          if (!window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
            delete card.dataset.pulse;
            void card.offsetWidth;
            card.dataset.pulse = "true";
            window.setTimeout(() => { if (card.isConnected) delete card.dataset.pulse; }, 600);
          }
        });
      }
      if (key === "t") return run(() => void act.translate());
      if (key === "v") return run(act.cycleTranslateMode);
      if (key === "u" && (cur.view.truncated || cur.view.pruned)) return run(() => void act.fetchFulltext());
      if (key === "s") return run(() => void act.toggleStar());
      if (key === "m") return run(() => void act.markUnread());
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [shortcuts, hasReader, selectDrawerRow]);

  // 命中段只在正文版本未变时生效；版本变了按阅读位置规则打开，不跳。
  const searchHits = session?.search?.hits && reader && session.search.hits.sourceHash === reader.source_hash && session.search.hits.blockIds.length > 0
    ? session.search.hits.blockIds : null;
  const searchIndex = session?.search?.index ?? 0;
  const searchBlockId = searchHits ? searchHits[Math.min(searchIndex, searchHits.length - 1)] : undefined;
  // 同一块换了会话（重新打开）也要重新定位，所以带上票据。
  const searchTicket = session?.ticket;
  const searchAnchor = useMemo(() => searchBlockId === undefined || !searchTicket ? undefined : { blockId: searchBlockId, persist: true as const },
    [searchBlockId, searchTicket]);

  if (!open) return null;
  if (opening?.late && !session) return <ReaderOpening onClose={closeReader} />;
  if (failure) return <div className="overlay reader-stage" style={style} {...(inert ? { inert: true } : {})}>
    <div className="drawer reader">
      <WebReadingBar page={failure} returnTitle={returns.current.at(-1)?.view.title} onBack={() => { if (!back()) closeReader(); }} />
      <div className="drawer-body"><div className="reader-column web-column"><WebFailure page={failure} onRetry={() => void openWebLink(failure.url, true)} /></div></div>
    </div>
  </div>;
  if (!session || !reader) return null;

  const navRows = session.skipNav ? [] : navigationRows();
  const rows = session.skipNav ? [] : listedRows();
  const ids = session.search ? session.search.results : navRows.map((row) => row.id);
  const pos = ids.indexOf(reader.id);
  const pager = !session.skipNav || !!session.search;

  return (
    <ReaderOverlay
      reader={reader}
      onOpenUrl={props.onOpenUrl}
      webNav={session.web ? <WebReadingNav page={session.web} returnTitle={returns.current.at(-1)?.view.title} onBack={() => { if (!back()) closeReader(); }} /> : undefined}
      webActions={session.web ? <WebReadingActions page={session.web} onCollect={() => void collect()} collecting={!!ops.collect} /> : undefined}
      webTip={session.web ? <WebFirstTip key={session.ticket.session} page={session.web} /> : undefined}
      onCollect={session.web?.info?.temporary ? () => void collect() : undefined}
      collecting={!!ops.collect}
      origin={session.web?.info || reader.web_extracted_at != null ? <WebOrigin info={session.web?.info} extractedAt={reader.web_extracted_at} url={session.web?.info?.actual_url ?? session.web?.url ?? reader.url} onExtract={() => void reextract()} extracting={extractingSession === session.ticket.session} /> : undefined}
      citationTray={props.citationTray}
      onTrayEscape={props.onTrayEscape}
      returnThreadTitle={session.fromThread?.title}
      returnArticleTitle={session.detour ? returns.current.at(-1)?.view.title : undefined}
      onReturnArticle={session.detour ? () => { if (!back()) closeReader(); } : undefined}
      related={related.data && related.data.length ? related.data : undefined}
      onOpenRelated={openRelated}
      onContinueAsking={session.fromThread ? props.onContinueAsking : undefined}
      anchor={searchAnchor ?? session.anchor}
      searchTerm={session.search?.query}
      searchHits={searchHits ? { index: Math.min(searchIndex, searchHits.length - 1), total: searchHits.length, onStep: stepSearchHit } : undefined}
      article={navRows.find(row => row.id === reader.id)}
      showNextCard={session.returnToList || !!session.search}
      nextArticle={session.search ? session.search.resultRows.find(row => row.id === ids[pos + 1]) : pos >= 0 ? navRows[pos + 1] : undefined}
      searchPosition={session.search ? { index: Math.max(0, pos), total: ids.length } : undefined}
      onReturnToSearch={session.search ? closeReader : undefined}
      positionLabel={session.returnToList && pos >= 0 ? `${pos + 1} / ${ids.length}${currentNav().hasMore ? "+" : ""}` : undefined}
      onBody={onReaderBody}
      readingPosition={positions.current.get(`${reader.id}/${reader.source_hash}`)}
      onReadingPosition={position => {
        positions.current.set(`${reader.id}/${reader.source_hash}`, position);
        if (positions.current.size > READING_POSITION_LIMIT) positions.current.delete(positions.current.keys().next().value!);
      }}
      style={style}
      dismissible={dismissible}
      inert={inert}
      summarizing={!!ops.summary}
      translating={tr.busy}
      translationStatus={<TranslationStatus view={tr} debug={debug} onAction={() => void translate()} onLocate={locateFailed} />}
      translateLabel={translationAction(tr).kind === "regenerate" ? t("status.action.start") : translationAction(tr).label}
      onRegenerate={reader.translation_ready ? regenerate : undefined}
      blockNotes={blockNotes}
      translateMode={translateMode}
      mediaLoading={!!ops.media}
      onMediaRef={(media) => acceptMedia(reader.id, media)}
      hasPrev={pager && pos > 0}
      hasNext={pager && pos >= 0 && (pos < ids.length - 1 || (!session.search && nav.hasMore))}
      onPrev={session.detour ? undefined : () => void openNeighbor(-1)}
      onNext={session.detour ? undefined : () => void openNeighbor(1)}
      fetchingFulltext={!!ops.fulltext}
      onFetchFulltext={() => void fetchFulltext()}
      onStar={() => void toggleStar()}
      onClose={closeReader}
      onCiteArticle={() => void citeArticle()}
      onCiteSelection={citeSelection}
      onTranslateSettings={props.onTranslateSettings}
      onSummarize={() => void summarize()}
      onTranslate={() => void translate()}
      onDismissNote={() => setView(reader.id, (cur) => ({ ...cur, translation_note: "" }))}
      onTranslateMode={changeMode}
      stageRef={observeStage}
      onRate={(up) => rateArticle(articleForRate(reader), up)}
      drawerOpen={drawerOpen && !session.skipNav}
      drawerPinned={drawerOpen && effectiveDrawerPinned}
      canPinDrawer={canPinDrawer}
      drawerRows={rows}
      drawerTitle={props.listTitle}
      drawerScope={currentNav().scope ?? "all"}
      drawerSelectedId={drawerSelectedId}
      drawerExpandedCluster={currentNav().expandedCluster}
      onDrawerExpand={expandDrawerCluster}
      drawerHasMore={!!currentNav().hasMore}
      drawerLoadingMore={!!currentNav().loadingMore}
      onDrawerOpen={session.skipNav ? undefined : openDrawer}
      onDrawerClose={closeDrawer}
      onDrawerPin={() => setDrawerPinned((value) => !value)}
      onDrawerScope={(scope) => { dismissedIds.current.clear(); void navRef.current.setScope?.(scope); }}
      onDrawerSelect={selectDrawerRow}
      onDrawerOpenArticle={openDrawerArticle}
      onDrawerRate={session.skipNav ? undefined : (id) => void rateAndOpenNext(id)}
      onDrawerLoadMore={() => void navRef.current.loadMore()}
      onDrawerEdgeEnter={scheduleDrawerOpen}
      onDrawerEdgeLeave={cancelDrawerOpen}
    />
  );
});
