import { useCallback, useEffect, useRef, useState, useSyncExternalStore, type Ref } from "react";
import { useFeedUpdates } from "./app/useFeedUpdates";
import { readingMinutes } from "./lib/readingTime";
import { api, onWindowCloseRequested, onWindowRestored, type ArticleView, type CitationView, type ThreadView } from "./lib/api";
import type { WebVerifyStateEvent } from "./bindings";
import { WebLinksContext } from "./lib/webLinks";
import { useWebLinks, WebSafetyWarning, WebWindowScene } from "./features/reader";
import { trackEvent } from "./lib/signals";
import { isTauri } from "./lib/preview";
import { isMetaK, isTypingTarget } from "./lib/shortcuts";
import { useModalFocus, type ModalLayerId } from "./lib/useModalFocus";
import { useOverlayStack } from "./lib/useOverlayStack";
import { useWindowKeydown } from "./lib/useWindowKeydown";
import { ConfirmBar } from "./components/ConfirmBar";
import { CloseAskDialog, TRAY_LABELS } from "./app/CloseAskDialog";
import { AboutOverlay } from "./components/AboutOverlay";
import { Toast } from "./components/Toast";
import { ProbePage, RenderFixture } from "./components/DevPages";
import { Icon } from "./components/Icon";
import { MainViewFrame } from "./components/MainViewFrame";
import { ChipBar } from "./settings/chips";
import { formatNotice } from "./lib/notice";
import { createArticleWriteOrder } from "./lib/articleWriteOrder";
import { useLingering } from "./lib/motion";
import { siteRulesKey, siteRulesOptions, siteRulesWriteKey } from "./settings/siteRulesQuery";
import { rulesKeys, rulesQueryOptions } from "./settings/rulesQuery";
import { memoryKeys, prefetchMemory } from "./settings/memoryQuery";
import { diagnosticsKeys, prefetchDiagnostics } from "./settings/diagnosticsQuery";
import { feedKeys, feedQueryOptions } from "./lib/feedQuery";
import { QueryClient, QueryClientProvider, useQuery, useQueryClient } from "@tanstack/react-query";
import { prefetchTranslateSettings, translateKeys } from "./settings/translateQuery";
import { piKeys, prefetchPiSettings } from "./settings/piQuery";
import { usePiInstallNotice } from "./settings/usePiInstall";
import { prefetchAiSettings } from "./settings/aiQuery";
import { aiKeys } from "./settings/aiKeys";
import { prefetchSettingsSchema, settingsKeys } from "./settings/settingsQuery";
import { retentionKeys, retentionQueryOptions } from "./settings/retentionQuery";
import { backupKeys, backupQueryOptions } from "./settings/backupQuery";
import { settingsChanged, type SettingsEditState } from "./settings/editState";
import type { AskOptions, SettingsTabId } from "./settings/SettingsPanel";
import type { ReaderHostHandle, ReaderOpenOptions } from "./features/reader";
import { CommandPalette } from "./features/search";
import { ChatActivity, ChatContextMeter, ChatModelChip, ChatPane, Composer, JumpToLatest, LinkPreview, createDraftStore, createJumpController, useConversationRuntime } from "./features/chat";
import { DigestScreen } from "./features/digest";
import { FocusScreen, FocusTraining, columnName, createFocusSession, type FocusNotice, type FocusTrainingHandle } from "./features/focus";
import { FocusColumnScreen, FocusReader, FocusSettings, type ReaderList } from "./app/FocusAdapters";
import { FeedsScreen, createFeedsSession } from "./features/feeds";
import { useReaderCitations } from "./app/useReaderCitations";
import { Sidebar, type MainView, type SidebarHandle } from "./app/Sidebar";
import { WindowControls } from "./app/WindowControls";

const THEMES = [
  { id: "system", label: "跟随系统" },
  { id: "light", label: "浅色" },
  { id: "dark", label: "深色" },
];

/**
 * 设置各分类共用查询层预载。只预热还没读过的查询：已有快照的页面打开时按新鲜度自己重读，
 * 读取失败的由用户显式重试，预载不能替用户重试，也不能在写入中途插一次读取。
 */
function primeSettingsPages(client: QueryClient) {
  if (!isTauri()) return;
  const warm = (key: readonly unknown[], prefetch: () => unknown) => { if (!client.getQueryState(key)) void prefetch(); };
  if (!client.isMutating({ mutationKey: feedKeys.write })) warm(feedKeys.tree, () => client.prefetchQuery(feedQueryOptions()));
  if (!client.isMutating({ mutationKey: rulesKeys.write })) warm(rulesKeys.page("all"), () => client.prefetchQuery(rulesQueryOptions("all")));
  warm(memoryKeys.page(""), () => prefetchMemory(client));
  warm(aiKeys.page, () => prefetchAiSettings(client));
  warm(translateKeys.page, () => prefetchTranslateSettings(client));
  warm(piKeys.page, () => prefetchPiSettings(client));
  warm(diagnosticsKeys.page, () => prefetchDiagnostics(client));
  // 注册表行、保留与备份也会在切分类时出现「正在读取」；占用统计要扫库，不预载。
  if (!client.isMutating({ mutationKey: siteRulesWriteKey })) warm(siteRulesKey, () => client.prefetchQuery(siteRulesOptions()));
  warm(settingsKeys.schema, () => prefetchSettingsSchema(client));
  if (!client.isMutating({ mutationKey: retentionKeys.write })) warm(retentionKeys.page, () => client.prefetchQuery(retentionQueryOptions()));
  if (!client.isMutating({ mutationKey: backupKeys.write })) warm(backupKeys.page, () => client.prefetchQuery(backupQueryOptions()));
}

type Overlay = "web-warning" | "confirm" | "about" | "settings" | "reader" | "training" | "close-ask";

export function App() {
  const [queries] = useState(() => new QueryClient());
  const hash = typeof window !== "undefined" ? window.location.hash.replace("#", "") : "";
  if (hash === "probe") return <ProbePage />;
  if (hash === "render") return <RenderFixture />;
  return <QueryClientProvider client={queries}><DesktopShell /></QueryClientProvider>;
}

function DesktopShell() {
  const queries = useQueryClient();
  const [view, setViewState] = useState<MainView>("focus");
  // 进入订阅之前的视图。只在从别的视图切进来时记下，已在订阅里不覆盖；按钮和 Esc 都回到这里。
  const viewRef = useRef<MainView>("focus");
  const [feedsReturn, setFeedsReturn] = useState<MainView>("focus");
  // 主栏目到过才挂，之后只隐藏；没打开过的简报 / 对话不预载。
  const [visited, setVisited] = useState<ReadonlySet<MainView>>(() => new Set(["focus"]));
  const setView = useCallback((next: MainView) => {
    if (next === "feeds" && viewRef.current !== "feeds") setFeedsReturn(viewRef.current);
    viewRef.current = next;
    setViewState(next);
    setVisited((seen) => (seen.has(next) ? seen : new Set([...seen, next])));
  }, []);
  const [threadId, setThreadId] = useState(0);
  const [threadHeading, setThreadHeading] = useState({ id: 0, title: "新对话", archived: false });
  const onThreadTitle = useCallback((id: number, title: string, archived: boolean) => {
    setThreadHeading(previous => previous.id === id && previous.title === title && previous.archived === archived ? previous : { id, title, archived });
  }, []);
  const [toast, setToast] = useState<FocusNotice | null>(null);
  // 提示关掉后留 200ms 下沉淡出；换成新提示时直接在原处更新。
  const shownToast = useLingering(toast, 200);
  usePiInstallNotice(setToast);
  const [feedsSession] = useState(() => createFeedsSession(setToast));
  const [focus] = useState(() => createFocusSession(setToast, () => queries.fetchQuery(feedQueryOptions()), () => settingsChanged(queries)));
  const [column] = useState(() => createFocusSession(setToast, () => queries.fetchQuery(feedQueryOptions()), () => settingsChanged(queries), {
    onMarked: () => { void feedsSession.refreshCounts(); },
  }));
  const [columnId, setColumnId] = useState<string | null>(null);
  useFeedUpdates(queries, focus, column, feedsSession, columnId !== null, setToast);
  const columnTitle = columnName(useQuery(feedQueryOptions()).data, columnId);
  const [readerList, setReaderList] = useState<ReaderList>("focus");
  const readerListRef = useRef<ReaderList>("focus");
  const readerOwner = useRef(focus);
  const [drafts] = useState(createDraftStore);
  const [jump] = useState(createJumpController);
  const readerRef = useRef<ReaderHostHandle>(null);
  const trainingRef = useRef<FocusTrainingHandle>(null);
  // 评分的所有写入方（训练条、阅读器 x / 篇目踩）共用一条按文章的写入顺序。
  const [ratings] = useState(createArticleWriteOrder);
  const sidebarRef = useRef<SidebarHandle>(null);
  const digestContext = useRef("digest:0");
  const [digestThreadRevision, setDigestThreadRevision] = useState(0);
  const setDigestContext = useCallback((context: string) => { digestContext.current = context; }, []);
  const current = useRef({ view, threadId });
  current.current = { view, threadId };
  const [siteRuleSeed, setSiteRuleSeed] = useState<{ domain: string; url: string }>();
  const [settingsTab, setSettingsTab] = useState<SettingsTabId>("subscribe");
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);
  // 从搜索打开的文章还开着：浮层收起但保留会话，阅读器返回时回到搜索。
  const [searchAway, setSearchAway] = useState(false);
  const searchAwayRef = useRef(false);
  searchAwayRef.current = searchAway;
  const [searchReturnId, setSearchReturnId] = useState<number | null>(null);
  const [confirm, setConfirm] = useState<{ title: string; message: string; onOk: () => void } & AskOptions | null>(null);
  const [leaveSaving, setLeaveSaving] = useState(false);
  // 设置分类只上报通用离开状态，根壳不区分分类。
  const settingsEditState = useRef<SettingsEditState | null>(null);
  const onSettingsEditStateChange = useCallback((state: SettingsEditState | null) => { settingsEditState.current = state; }, []);
  const reloadThreads = useCallback(() => sidebarRef.current?.reloadThreads(), []);
  // 运行层每次渲染都取最新回调，这里不必稳定引用。
  const chat = useConversationRuntime({ onError: (message, fix) => showChatError(message, fix), onDone: reloadThreads });
  const overlays = useOverlayStack<Overlay>({
    onBack: id => id === "reader" && (readerRef.current?.back() ?? false),
    guard: (id) => {
      if (id !== "settings") return "ok";
      const edit = settingsEditState.current;
      return edit?.busy ? "busy" : edit?.dirty ? "dirty" : "ok";
    },
    onBusy: () => setToast({ message: "正在保存或测试" + (settingsEditState.current?.label ?? "设置") + "，请等本次操作完成后再离开。" }),
    onDiscard: () => {
      if (settingsEditState.current) settingsEditState.current.dirty = false;
    },
    onClosed: (id) => {
      if (id === "web-warning") webLinks.cancel();
      if (id === "settings") primeSettingsPages(queries);
      // 阅读期间保留当前条目供上下篇定位，返回未读列表时按宿主范围重读。
      if (id === "reader") {
        webLinks.cancel();
        // 阅读器关了却没回到搜索（例如发出引用后去了对话）：保留的搜索会话作废。
        setSearchAway(false);
        readerCitations.end();
        void readerOwner.current.readerClosed(readerRef.current?.getReturnArticleId(), readerRef.current?.getDismissedArticleIds());
      }
    },
  });
  const { stack, top, open, closeTop, pendingLeave } = overlays;
  const readerOpen = useRef(false);
  readerOpen.current = stack.includes("reader");
  // 能在设置里修好的对话错误带一个「去设置」，直接打开对应分类。
  const openChatFix = useCallback((tab: SettingsTabId) => { setSettingsTab(tab); open("settings"); }, [open]);
  function showChatError(message: string, fix?: SettingsTabId) {
    setToast(fix ? { message, actionLabel: "去设置", onAction: () => openChatFix(fix) } : { message });
  }
  const layer = (id: Overlay) => ({ zIndex: 20 + Math.max(0, stack.indexOf(id)) });
  const openSearch = useCallback(() => {
    // 阅读从搜索打开的文章时再唤出搜索：恢复原来的结果，选中正在读的这篇。
    if (searchAwayRef.current) setSearchReturnId(readerRef.current?.getSourceArticle()?.id ?? null);
    setCommandPaletteOpen(true);
  }, []);
  const closeSearch = useCallback(() => { setCommandPaletteOpen(false); setSearchAway(false); }, []);
  const returnToSearch = useCallback((id: number) => {
    if (!searchAwayRef.current) return;
    setSearchReturnId(id);
    setCommandPaletteOpen(true);
    setSearchAway(false);
  }, []);
  const openDigest = useCallback(() => setView("digest"), [setView]);
  const openSettings = useCallback(() => open("settings"), [open]);
  const openAbout = useCallback(() => open("about"), [open]);
  const openTraining = useCallback(() => open("training"), [open]);
  const readerCitations = useReaderCitations({ drafts, chat, getThreads: () => sidebarRef.current?.getThreads() ?? [],
    getSourceArticle: () => readerRef.current?.getSourceArticle() ?? null,
    notify: setToast, inert: (top !== undefined && top !== "reader") || commandPaletteOpen || !!pendingLeave,
    onSent: (id, digestId, sameReader) => {
      if (digestId !== null) setDigestThreadRevision(n => n + 1);
      // 等待期间用户已离开或打开另一层时，不用迟到结果关闭那一层。
      if (!sameReader || (top !== undefined && top !== "reader") || commandPaletteOpen || pendingLeave) return;
      setThreadId(id); setView("chat"); if (top === "reader") overlays.closeTopWithoutGuard();
    },
  });
  const readerCitationsRef = useRef(readerCitations);
  readerCitationsRef.current = readerCitations;
  const defaultCitationSource = useCallback((): NonNullable<ReaderOpenOptions["citationSource"]> => current.current.view === "digest"
    ? { kind: "digest", digestId: Number(digestContext.current.slice("digest:".length)) } : { kind: "new" }, []);
  const openArticle = useCallback((id: number, options?: ReaderOpenOptions) => {
    // 阅读中经搜索 / 设置换篇等同站内换篇：托盘及已改选的发到目标保留。
    if (options?.citationSource || !readerOpen.current) readerCitationsRef.current.begin(options?.citationSource ?? defaultCitationSource());
    const owner = readerListRef.current === "column" ? column : focus;
    readerOwner.current = owner;
    owner.readerOpened();
    return readerRef.current?.openArticle(id, options) ?? Promise.resolve();
  }, [column, defaultCitationSource, focus]);
  const chooseReaderList = useCallback((next: ReaderList) => { readerListRef.current = next; setReaderList(next); }, []);
  const openFocusArticle = useCallback((id: number) => { chooseReaderList("focus"); return openArticle(id, { returnToList: true }); }, [chooseReaderList, openArticle]);
  const openColumnArticle = useCallback((id: number) => { chooseReaderList("column"); return openArticle(id, { returnToList: true }); }, [chooseReaderList, openArticle]);
  const openFeedsArticle = useCallback((id: number) => { chooseReaderList("feeds"); void openArticle(id, { returnToList: true }); }, [chooseReaderList, openArticle]);
  const openColumn = useCallback((id: string) => { void column.setFeedScope(id); setColumnId(id); setView("column"); }, [column, setView]);
  const onColumnGone = useCallback(() => { setColumnId(null); if (viewRef.current === "column") setView("focus"); }, [setView]);
  const openSubscribe = useCallback(() => { setSettingsTab("subscribe"); open("settings"); }, [open]);
  const openFeedFromCard = useCallback((feedId: number) => { void feedsSession.setScope(`feed:${feedId}`); setView("feeds"); }, [feedsSession, setView]);
  const conversationSource = useCallback(() => {
    const { view, threadId } = current.current;
    if (view !== "chat") return undefined;
    return { kind: "thread" as const, threadId, title: sidebarRef.current?.getThreads().find(t => t.id === threadId)?.title ?? "新对话" };
  }, []);
  const openArticleFromMessage = useCallback((id: number, anchor?: ReaderOpenOptions["anchor"]) => {
    void openArticle(id, { skipNav: true, anchor, citationSource: conversationSource() });
  }, [openArticle, conversationSource]);
  const openWebReader = useCallback((href: string, extracted?: WebVerifyStateEvent) => {
    if (!readerOpen.current) readerCitationsRef.current.begin(conversationSource() ?? defaultCitationSource());
    readerOwner.current = focus;
    focus.readerOpened();
    if (extracted) void readerRef.current?.openExtracted(extracted);
    else void readerRef.current?.openWebLink(href);
  }, [conversationSource, defaultCitationSource, focus]);
  const webLinks = useWebLinks({ openReader: openWebReader, openExtracted: event => openWebReader(event.url, event), notify: setToast,
    showWarning: () => open("web-warning"), closeWarning: () => { if (top === "web-warning") closeTop(); },
    prepareCitation: () => {
      if (!readerOpen.current && !readerCitationsRef.current.tray) readerCitationsRef.current.begin(conversationSource() ?? defaultCitationSource());
      const add = readerCitationsRef.current.captureAdd();
      return chip => { readerRef.current?.markCollected(chip.article_id); add(chip); };
    },
  });
  const openContentLink = webLinks.openContentLink;
  const releaseWebView = useRef(webLinks.windows.release);
  releaseWebView.current = webLinks.windows.release;
  // 切回阅读视图时原网页场景先出栈，阅读器要等安全判定后才入栈；短暂为空不算离开。
  const hasWebHost = stack.includes("reader") || stack.includes("web-warning");
  useEffect(() => {
    if (hasWebHost) return;
    const timer = window.setTimeout(() => releaseWebView.current(), 1500);
    return () => window.clearTimeout(timer);
  }, [hasWebHost]);
  const origin = readerCitations.origins.get(threadId);
  const backToArticle = () => {
    if (origin) void openArticle(origin.id, { citationSource: conversationSource(), returnToList: true });
  };
  const returnToThread = (id: number) => { setThreadId(id); setView("chat"); };
  const rateArticle = useCallback((row: ArticleView, up: boolean) => trainingRef.current?.rate(row, up), []);
  // 阅读器 `x` / 篇目里的踩只记反馈，不弹规则建议，便于连续丢弃；规则建议留给阅读工具和列表里的踩。
  const dislikeArticle = useCallback(async (row: ArticleView) => {
    try { await ratings.write(row.id, () => api.rate(row.id, false)); trackEvent(row.id, "down"); return true; }
    catch (err) { setToast({ message: formatNotice("评分失败：", err) }); return false; }
  }, [ratings]);
  const addCitation = useCallback((citation: CitationView) => {
    drafts.addCitations(current.current.view === "digest" ? digestContext.current : drafts.threadContext(current.current.threadId), [citation]);
  }, [drafts]);

  // 根壳只订阅外观这个低频标量，不订阅焦点文章数组。
  const savedTheme = useSyncExternalStore(focus.subscribe, () => focus.getRanking().theme || "system");
  const feeds = useSyncExternalStore(focus.subscribe, () => focus.getSnapshot().feeds);
  const [theme, setTheme] = useState(savedTheme);
  useEffect(() => { setTheme(savedTheme); document.documentElement.dataset.theme = savedTheme; }, [savedTheme]);
  useEffect(() => {
    const prefetch = window.setTimeout(() => primeSettingsPages(queries), 800);
    return () => window.clearTimeout(prefetch);
  }, [queries]);
  // 后台驻留：托盘菜单文字由前端给；宿主拦下关闭后弹关闭询问；从托盘唤回时焦点直接采用最新内容。
  const openOverlay = useRef(open);
  openOverlay.current = open;
  useEffect(() => {
    void api.trayInstall(TRAY_LABELS).catch((error) => console.error("tray unavailable", error));
    let disposed = false;
    const stops: Array<() => void> = [];
    const keep = (subscription: Promise<() => void>) => void subscription
      .then((stop) => { if (disposed) stop(); else stops.push(stop); })
      // 没有宿主（预览、组件测试）时订阅不到窗口事件，忽略即可。
      .catch(() => {});
    keep(onWindowCloseRequested(() => openOverlay.current("close-ask")));
    keep(onWindowRestored(() => { void focus.adoptLatest(); void column.adoptLatest(); }));
    return () => { disposed = true; stops.forEach((stop) => stop()); };
  }, [focus, column]);

  const settingsRootRef = useRef<HTMLDivElement>(null);
  const confirmRootRef = useRef<HTMLDivElement>(null);
  const searchRootRef = useRef<HTMLDivElement>(null);
  const webWarningRootRef = useRef<HTMLDivElement>(null);
  const closeAskRootRef = useRef<HTMLDivElement>(null);
  const modalLayers: ModalLayerId[] = stack.filter((item): item is ModalLayerId & Overlay => item === "settings" || item === "confirm" || item === "web-warning" || item === "close-ask");
  if (pendingLeave) modalLayers.push("confirm");
  if (commandPaletteOpen) modalLayers.push("search");
  const modalTop = commandPaletteOpen ? "search" : pendingLeave ? "confirm" : top === "settings" || top === "confirm" || top === "web-warning" || top === "close-ask" ? top : null;
  useModalFocus({ layers: modalLayers, activeId: modalTop, suspended: false,
    roots: { settings: settingsRootRef, confirm: confirmRootRef, search: searchRootRef, "web-warning": webWarningRootRef, "close-ask": closeAskRootRef },
    initialFocus: { settings: "[data-settings-back]", confirm: "[data-confirm-cancel]", search: "input", "close-ask": "[data-close-tray]" },
  });
  const trapBackground = modalTop !== null;

  useWindowKeydown(true, (event) => {
    if (event.isComposing || event.keyCode === 229) return;
    if (isMetaK(event)) { event.preventDefault(); openSearch(); return; }
    if (event.key === "Escape" && commandPaletteOpen) { event.preventDefault(); closeSearch(); return; }
    if (!event.defaultPrevented && event.key === "Backspace" && top === "reader" && !commandPaletteOpen && !pendingLeave
      && !event.isComposing && !event.ctrlKey && !event.metaKey && !event.altKey && !isTypingTarget(document.activeElement)) {
      event.preventDefault(); readerRef.current?.close(); return;
    }
    if (!event.defaultPrevented && event.key === "Escape") {
      if (stack.length > 0 || pendingLeave) {
        event.preventDefault();
        if (top === "reader" && !pendingLeave) readerRef.current?.close(); else closeTop();
      } else if (view === "chat" && origin) { event.preventDefault(); backToArticle(); }
      else if (view === "feeds" && !isTypingTarget(document.activeElement)) { event.preventDefault(); setView(feedsReturn); }
    }
  });

  const { loadMessages, release, seedThread, resetNewModel } = chat;
  const selectThread = useCallback((id: number, saved?: ThreadView) => {
    setThreadId(id);
    setView("chat");
    if (id > 0) {
      seedThread(id, saved);
      void loadMessages(id).catch((err) => setToast({ message: formatNotice("会话加载失败：", err) }));
    } else resetNewModel("chat");
  }, [loadMessages, resetNewModel, seedThread, setView]);
  // 搜索里打开一条会话：载入最新一页，向前翻到命中的消息，滚到它并短暂高亮（N4 · 搜索对话）。
  const { loadOlder } = chat;
  const openThreadHit = useCallback(async (id: number, messageId: number | null) => {
    setThreadId(id);
    setView("chat");
    seedThread(id, sidebarRef.current?.getThreads().find((thread) => thread.id === id));
    try { await loadMessages(id); } catch (err) { setToast({ message: formatNotice("会话加载失败：", err) }); return; }
    if (messageId == null) return;
    for (let page = 0; page < 50; page += 1) {
      const loaded = chat.store.get(id);
      if (loaded.messages.some((m) => m.id === messageId) || !loaded.hasMore) break;
      if (!(await loadOlder(id).catch(() => false))) break;
    }
    // 等这次渲染和「跟到底部」落定后再定位。
    requestAnimationFrame(() => requestAnimationFrame(() => {
      const node = document.querySelector<HTMLElement>(`.chat-pane [data-message-id="${messageId}"]`);
      if (!node) return;
      node.scrollIntoView({ block: "center" });
      node.setAttribute("data-flash", "");
      window.setTimeout(() => node.removeAttribute("data-flash"), 1600);
    }));
  }, [chat.store, loadMessages, loadOlder, seedThread, setView]);
  // 相近报道「引用整组到对话」：放进新对话的输入区，不打断当前对话的草稿。
  const citeGroup = useCallback((citations: CitationView[]) => {
    drafts.addCitations(drafts.threadContext(0), citations);
    setToast({ message: `已把这组 ${citations.length} 篇引用到新对话`, actionLabel: "去对话", onAction: () => selectThread(0) });
  }, [drafts, selectThread]);
  const managePool = useCallback(() => { setSettingsTab("model"); open("settings"); }, [open]);
  // 只在换线程时回收上一条；切栏目不清当前对话，生成中的回复仍归原线程。
  useEffect(() => {
    if (threadId <= 0) return;
    return () => { release(threadId); };
  }, [threadId, release]);
  const send = async (text: string, citations: CitationView[]) => {
    const sentView = view;
    const sentThreadId = threadId;
    try {
      const id = await chat.sendMessage(sentThreadId, text, citations, null, "chat");
      if (current.current.threadId === sentThreadId) {
        setThreadId(id);
        if (current.current.view === sentView) setView("chat");
      }
      return true;
    } catch (err) { setToast({ message: formatNotice("发送失败：", err) }); return false; }
  };

  const webCovered = top !== "web-warning" || commandPaletteOpen || !!webLinks.menu || !!pendingLeave || stack.includes("confirm");
  return <WebLinksContext.Provider value={{ ...webLinks.context, editRule: (domain, url) => { setSiteRuleSeed({ domain, url }); setSettingsTab("web"); open("settings"); } }}><div className="shell" data-web-embedded={webLinks.windows.embedded ? "" : undefined}>
    <Sidebar ref={sidebarRef} view={view} threadId={threadId} inert={trapBackground} feeds={feedsSession} feedsReturn={feedsReturn} onView={setView} onThread={selectThread} onThreadTitle={onThreadTitle}
      onSearch={openSearch} onSettings={openSettings} onAbout={openAbout} onAddFeed={openSubscribe}
      columnId={columnId} onOpenColumn={openColumn} onColumnGone={onColumnGone} notify={setToast} />
    <main className="main" inert={trapBackground}>
      <FocusScreen session={focus} active={view === "focus"} shortcuts={view === "focus" && stack.length === 0 && !commandPaletteOpen && !pendingLeave}
        onOpen={openFocusArticle} onCite={addCitation} onCiteGroup={citeGroup} onRate={rateArticle} onOpenDigest={openDigest} onOpenSearch={openSearch} onOpenFeed={openFeedFromCard} onManageModels={managePool} notify={setToast} />
      {columnId ? <FocusColumnScreen feeds={feedsSession} session={column} active={view === "column"} scope={columnId} name={columnTitle || "栏目"}
        shortcuts={view === "column" && stack.length === 0 && !commandPaletteOpen && !pendingLeave}
        onOpen={openColumnArticle} onCite={addCitation} onCiteGroup={citeGroup} onRate={rateArticle} onOpenDigest={openDigest} onOpenSearch={openSearch} onOpenFeed={openFeedFromCard} onManageModels={managePool} notify={setToast} /> : null}
      {visited.has("feeds") ? <FeedsScreen session={feedsSession} active={view === "feeds"} onOpen={openFeedsArticle} onFetch={focus.refresh} /> : null}
      {visited.has("digest") ? <DigestScreen active={view === "digest"} chat={chat} drafts={drafts} onContext={setDigestContext} discussionRevision={digestThreadRevision}
        topmost={stack.length === 0 && !commandPaletteOpen && !pendingLeave}
        onChange={focus.reload} onOpenUrl={openContentLink} onOpenArticle={openArticleFromMessage} onManagePool={managePool} onFixChat={openChatFix} notify={setToast} /> : null}
      {visited.has("chat") ? <MainViewFrame visible={view === "chat"}>
        <header className="chat-header" data-tauri-drag-region="deep">
          {origin ? <button type="button" className="chat-backchip" onClick={backToArticle} title={`回到原文 · ${origin.title}`}>
            <Icon name="arrowLeft" size={13} /><span>回到原文 ·</span><em>{origin.title}</em><kbd>Esc</kbd>
          </button> : null}<h1 title={threadHeading.id === threadId ? threadHeading.title : "新对话"}>{threadHeading.id === threadId ? threadHeading.title : "新对话"}</h1></header>
        {threadHeading.id === threadId && threadHeading.archived ? <div className="notice chat-archived" role="status">
          <Icon name="inbox" size={14} /><span>这条会话已归档。发送新消息会自动取消归档。</span>
          <button type="button" className="btn sm" onClick={() => void api.unarchiveThread(threadId).then(reloadThreads,
            (err) => setToast({ message: formatNotice("没能取消归档：", err) }))}>取消归档</button>
        </div> : null}
        <ChatPane store={chat.store} turns={chat.turns} threadId={threadId} busy={chat.busyThreadId === threadId} jump={jump}
          onLoadOlder={chat.loadOlder} onOpenUrl={openContentLink} onOpenArticle={openArticleFromMessage} onFix={openChatFix} />
      </MainViewFrame> : null}
      {view !== "digest" ? <Composer drafts={drafts} context={drafts.threadContext(threadId)} ariaLabel="对话消息" placeholder="针对引用提问，或自由探索…"
        otherThreadBusy={chat.busyThreadId !== null && chat.busyThreadId !== threadId} generating={chat.busyThreadId !== null}
        sending={chat.sending} disabled={false} ready collapsible={false} forceExpanded={chat.busyThreadId !== null} showWhenEmpty={view !== "focus" && view !== "column" && view !== "feeds"}
        onSend={send} onStop={() => void api.abort().catch((err) => setToast({ message: formatNotice("停止失败：", err) }))}
        onOpenArticle={openArticleFromMessage} activity={<ChatActivity store={chat.activity} />}
        jump={view === "chat" ? <JumpToLatest controller={jump} /> : null}
        model={<ChatModelChip value={chat.modelFor(threadId, "chat")} thinking={chat.thinkingFor(threadId, "chat")} disabled={chat.sending} onManagePool={managePool}
          onChange={(choice, label) => chat.chooseModel(threadId, "chat", choice, label)}
          onThinking={(level, label) => chat.chooseThinking(threadId, "chat", level, label)} />}
        meter={<ChatContextMeter value={chat.modelFor(threadId, "chat")} usage={chat.contextFor(threadId)} />} /> : null}
    </main>
    <FocusReader onOpenUrl={openContentLink} ref={readerRef} focus={focus} column={column} columnName={columnTitle} feeds={feedsSession} list={readerList} open={stack.includes("reader")} dismissible={top === "reader"}
      shortcuts={top === "reader" && !commandPaletteOpen && !pendingLeave} inert={trapBackground} style={layer("reader")}
      onAsk={(title, message, onOk) => { setConfirm({ title, message, onOk }); open("confirm"); }}
      onTranslateSettings={() => { setSettingsTab("translate"); open("settings"); }}
      onRequestOpen={() => open("reader")} onClose={closeTop} onCite={readerCitations.add} citationTray={readerCitations.tray} onTrayEscape={readerCitations.escape} onContinueAsking={readerCitations.ask} onReturnToThread={returnToThread} onReturnToSearch={returnToSearch} notify={setToast} onRateArticle={rateArticle} onDislikeArticle={dislikeArticle} />
    {stack.includes("settings") && <div className="overlay settings-overlay" style={layer("settings")} inert={top !== "settings" || commandPaletteOpen || !!pendingLeave}>
      <div ref={settingsRootRef} className="drawer settings settings-drawer" role="dialog" aria-modal={modalTop === "settings" && !commandPaletteOpen}
        aria-labelledby="settings-title" aria-describedby="settings-desc">
        <FocusSettings focus={focus} siteRuleSeed={siteRuleSeed} onSiteRuleSeedUsed={() => setSiteRuleSeed(undefined)} navigationHead={<div className="settings-nav-head" data-tauri-drag-region="deep">
          <button type="button" className="tool-btn" data-settings-back onClick={closeTop}><Icon name="arrowLeft" size={16} />返回阅读</button>
          <h2 id="settings-title">设置</h2><span id="settings-desc" hidden>订阅、规则、模型与本机诊断</span>
        </div>} navigationFoot={<div className="settings-theme"><span>外观</span><ChipBar className="segmented" value={theme} options={THEMES} onChange={(next) => {
            setTheme(next); document.documentElement.dataset.theme = next; void api.setSetting("theme_preference", next).catch(() => {});
          }} /></div>} tab={settingsTab} onTab={(next) => {
          if (next !== settingsTab) overlays.requestLeave("settings", () => setSettingsTab(next));
        }} onEditStateChange={onSettingsEditStateChange} onAsk={(title, message, onOk, options) => {
          setConfirm({ title, message, onOk, ...options }); open("confirm");
        }} onOpenArticle={openArticle} />
      </div>
    </div>}
    {stack.includes("about") && <AboutOverlay onClose={closeTop} style={layer("about")} />}
    <FocusTraining ref={trainingRef} session={focus} ratings={ratings} open={stack.includes("training")} style={layer("training")} onOpen={openTraining} onClose={closeTop} notify={setToast} />
    {stack.includes("confirm") && confirm && <ConfirmBar rootRef={confirmRootRef} inert={commandPaletteOpen} title={confirm.title} message={confirm.message}
      okLabel={confirm.okLabel} cancelLabel={confirm.cancelLabel} style={layer("confirm")}
      secondary={confirm.secondary && { label: confirm.secondary.label, onClick: () => { confirm.secondary!.onClick(); closeTop(); } }}
      onOk={() => { confirm.onOk(); closeTop(); }} onCancel={() => { confirm.onCancel?.(); closeTop(); }} />}
    {stack.includes("close-ask") && <CloseAskDialog rootRef={closeAskRootRef} inert={commandPaletteOpen} style={layer("close-ask")} onCancel={closeTop}
      onChoose={(choice, remember) => {
        closeTop();
        void api.windowCloseChoice(choice, remember).then(() => { if (remember) void settingsChanged(queries); })
          .catch((err) => setToast({ message: formatNotice("没能关闭窗口：", err) }));
      }} />}
    {pendingLeave && <LeaveConfirm rootRef={confirmRootRef} inert={commandPaletteOpen} edit={settingsEditState.current} saving={leaveSaving}
      onSave={async () => {
        const save = settingsEditState.current?.save;
        if (!save || leaveSaving) return;
        setLeaveSaving(true);
        const ok = await save().catch(() => false);
        setLeaveSaving(false);
        // 保存失败留在原处，由那一块显示原因。
        if (ok) overlays.confirmLeave(); else overlays.cancelLeave();
      }}
      onDiscard={overlays.confirmLeave} onCancel={overlays.cancelLeave} />}
    {webLinks.menu}
    {stack.includes("web-warning") && (webLinks.warning || webLinks.windows.panel) ? <div className="overlay reader-stage" style={layer("web-warning")} inert={webCovered}>
      <div className="drawer reader" ref={webWarningRootRef} role="dialog" aria-modal="true" aria-label={webLinks.warning ? "链接安全提示" : "原网页"}>
        {webLinks.warning ? <WebSafetyWarning warning={webLinks.warning} onCancel={closeTop} onContinue={webLinks.proceed} />
          : <WebWindowScene controller={webLinks.windows} covered={webCovered} onBack={closeTop} onOpenLink={openContentLink} onProxySettings={() => { setSettingsTab("advanced"); open("settings"); }} />}
      </div>
    </div> : null}
    {!stack.includes("reader") && readerCitations.tray ? <div className="web-citation-tray" inert={trapBackground}>{readerCitations.tray}</div> : null}
    <LinkPreview onOpenArticle={openArticleFromMessage} onCite={addCitation} notify={setToast} />
    <WindowControls />
    {shownToast.value && <Toast message={shownToast.value.message} holdMs={shownToast.value.holdMs} actionLabel={shownToast.value.actionLabel}
      onAction={shownToast.value.onAction} leaving={shownToast.leaving} onDismiss={() => setToast(null)} />}
    <CommandPalette rootRef={searchRootRef} open={commandPaletteOpen} topmost={modalTop === "search"} keepAlive={searchAway} returnTo={searchReturnId}
      region={stack.length === 0 ? "main" : "window"} feeds={feeds}
      onClose={closeSearch} onSearch={focus.search} onHitBlocks={api.searchHitBlocks}
      onReadingMinutes={readPreviewMinutes}
      onOpen={(id, search) => { setSearchAway(true); void openArticle(id, { skipNav: true, search }); }}
      onSearchThreads={api.searchThreads} onOpenThread={(id, messageId) => void openThreadHit(id, messageId)}
      threadFolderName={(folderId) => sidebarRef.current?.getFolders().find((folder) => folder.id === folderId)?.name} />
  </div></WebLinksContext.Provider>;
}

/** 设置里有未保存修改时离开：能保存的给「保存并离开 / 不保存 / 继续编辑」三选，否则二选。 */
/** 搜索预览的阅读时长：读一次文章快照（只读，不标已读），与阅读页同一估时。 */
const readPreviewMinutes = (id: number) => api.reader(id).then((view) => readingMinutes(view.document));

function LeaveConfirm({ edit, saving, onSave, onDiscard, onCancel, rootRef, inert }: {
  edit: SettingsEditState | null;
  saving: boolean;
  onSave: () => void;
  onDiscard: () => void;
  onCancel: () => void;
  rootRef: Ref<HTMLDivElement>;
  inert: boolean;
}) {
  const title = `${edit?.label ?? "设置"}有未保存的修改`;
  return edit?.save
    ? <ConfirmBar rootRef={rootRef} inert={inert} title={title} message={saving ? "正在保存…" : "离开前保存这些修改吗？"}
      okLabel={saving ? "保存中…" : "保存并离开"} secondary={{ label: "不保存", onClick: onDiscard }} cancelLabel="继续编辑"
      style={{ zIndex: 40 }} onOk={onSave} onCancel={onCancel} />
    : <ConfirmBar rootRef={rootRef} inert={inert} title={title} message="离开会丢弃这些修改。"
      okLabel="不保存并离开" cancelLabel="继续编辑" style={{ zIndex: 40 }} onOk={onDiscard} onCancel={onCancel} />;
}
