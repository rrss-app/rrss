import { forwardRef, memo, useEffect, useImperativeHandle, useMemo, useRef, useState, type CSSProperties } from "react";
import { useQuery } from "@tanstack/react-query";
import { api, type ThreadFolderView, type ThreadListView, type ThreadView } from "../lib/api";
import { feedQueryOptions } from "../lib/feedQuery";
import { isTauri, PREVIEW_THREADS } from "../lib/preview";
import { useTranslation } from "../i18n";
import { Icon } from "../components/Icon";
import { BrandMark } from "../components/BrandMark";
import { MenuButton } from "../components/MenuSelect";
import { FeedsTree, useFeedsSession, type FeedsSession, type TreeNotice } from "../features/feeds";
import { ArchivedThreads, ThreadList } from "../features/chat";
import { useFocusColumns, type FocusColumn } from "../features/focus";

export type MainView = "focus" | "column" | "feeds" | "digest" | "chat";

const EMPTY_LIST: ThreadListView = { folders: [], threads: [], archived_count: 0 };
export type SidebarHandle = { reloadThreads: () => void; getThreads: () => ThreadView[]; getFolders: () => ThreadFolderView[] };

/** 主视图名称的语言包键。侧栏导航和订阅模式的返回按钮共用，避免两处各写一份。 */
const VIEW_LABEL = {
  focus: "view.focus",
  column: "view.column",
  feeds: "view.feeds",
  digest: "view.digest",
  chat: "view.chat",
} as const;

/** 侧栏独立读取低频会话列表；只将选中标题的变化上报根壳，不订阅流式消息。 */
export const Sidebar = memo(forwardRef<SidebarHandle, {
  view: MainView; threadId: number; inert: boolean;
  feeds: FeedsSession;
  /** 进入订阅模式之前的视图；返回按钮回到这里。 */
  feedsReturn: MainView;
  onView: (view: MainView) => void;
  /** 选中会话；已有会话带上它保存的模型选择。 */
  onThread: (id: number, saved?: ThreadView) => void;
  /** 选中会话的标题与是否已归档（对话页顶部据此提示「已归档」）。 */
  onThreadTitle: (id: number, title: string, archived: boolean) => void;
  onSearch: () => void; onSettings: () => void; onAbout: () => void;
  /** 打开设置 → 订阅，用来添加订阅。 */
  onAddFeed: () => void;
  /** 当前栏目的 `folder:<id>`；不在栏目页时仍传，文件夹被删后用来退回。 */
  columnId: string | null;
  onOpenColumn: (id: string) => void;
  onColumnGone: () => void;
  notify: (notice: TreeNotice) => void;
}>(function Sidebar({ view, threadId, inert, feeds, feedsReturn, onView, onThread, onThreadTitle, onSearch, onSettings, onAbout, onAddFeed, columnId, onOpenColumn, onColumnGone, notify }, ref) {
  const [list, setList] = useState<ThreadListView>(EMPTY_LIST);
  // 归档列表只在打开「已归档」、或当前会话不在主列表里（刚归档 / 从归档打开）时读取。
  const [archived, setArchived] = useState<ThreadView[] | null>(null);
  const [archiveMode, setArchiveMode] = useState(false);
  const [revision, setRevision] = useState(0);
  const [loaded, setLoaded] = useState(false);
  const threads = list.threads;
  const known = useMemo(() => (archived ? [...threads, ...archived] : threads), [archived, threads]);
  useImperativeHandle(ref, () => ({ reloadThreads: () => setRevision((value) => value + 1), getThreads: () => known, getFolders: () => list.folders }), [known, list.folders]);
  // 首次读回之后才出现的会话（新对话发出后）：撑开高度、强调淡底随之褪去，只播这一次。
  const seenIds = useRef<Set<number> | null>(null);
  const [fresh, setFresh] = useState<ReadonlySet<number>>(() => new Set());
  useEffect(() => {
    if (fresh.size === 0) return;
    const timer = window.setTimeout(() => setFresh(new Set()), 400);
    return () => window.clearTimeout(timer);
  }, [fresh]);
  useEffect(() => {
    let cancelled = false;
    void api.threadList().then((next) => {
      if (cancelled) return;
      const seen = seenIds.current;
      seenIds.current = new Set(next.threads.map((thread) => thread.id));
      const added = seen ? next.threads.filter((thread) => !seen.has(thread.id)).map((thread) => thread.id) : [];
      if (added.length) setFresh(new Set(added));
      setList(next);
      setLoaded(true);
    }).catch(() => {
      if (!cancelled && !isTauri()) { setList({ ...EMPTY_LIST, threads: PREVIEW_THREADS }); setLoaded(true); }
    });
    return () => { cancelled = true; };
  }, [revision]);
  // 当前会话不在主列表里：多半刚归档、或是从归档列表打开的，读归档列表拿它的标题与状态。
  const missing = loaded && list.archived_count > 0 && threadId > 0 && !threads.some((thread) => thread.id === threadId);
  useEffect(() => {
    if (!archiveMode && !missing) return;
    let cancelled = false;
    void api.archivedThreads().then((rows) => { if (!cancelled) setArchived(rows); }, () => undefined);
    return () => { cancelled = true; };
  }, [revision, archiveMode, missing]);
  const { t } = useTranslation("feeds");
  const current = known.find(thread => thread.id === threadId);
  const title = current?.title ?? t("sidebar.newThread");
  const isArchived = !!current?.archived_at && !threads.some((thread) => thread.id === threadId);
  useEffect(() => { onThreadTitle(threadId, title, isArchived); }, [threadId, title, isArchived, onThreadTitle]);
  const reload = () => setRevision((value) => value + 1);
  const feedsState = useFeedsSession(feeds);
  // 「订阅」项的未读总数：离开订阅模式时也刷新，标已读与阅读后不会停在旧数。
  useEffect(() => { if (view !== "feeds") void feeds.refreshCounts(); }, [feeds, view]);
  if (view === "feeds") return <FeedsSidebar feeds={feeds} inert={inert} feedsReturn={feedsReturn} onBack={() => onView(feedsReturn)} onAddFeed={onAddFeed} onSettings={onSettings} notify={notify} />;
  if (archiveMode) return <aside className="sidebar thread-archive-mode" inert={inert} aria-label={t("sidebar.archivedLabel")}
    onKeyDown={(event) => { if (event.key === "Escape" && !event.defaultPrevented) { event.preventDefault(); setArchiveMode(false); } }}>
    <ArchivedThreads threads={archived} active={view === "chat" ? threadId : 0} onBack={() => setArchiveMode(false)}
      onThread={onThread} onChanged={reload} notify={notify} />
    <div className="sidebar-foot">
      <button type="button" onClick={onSettings}><Icon name="settings" size={15} />{t("sidebar.settings")}</button>
      <button type="button" onClick={onAbout}><Icon name="info" size={15} />{t("sidebar.about")}</button>
    </div>
  </aside>;
  return <aside className="sidebar" inert={inert}>
    <div className="sidebar-top" data-tauri-drag-region="deep"><button type="button" className="brand" onClick={onAbout} title={t("sidebar.aboutTitle")}>
      <BrandMark /><span className="brand-name">RRSS</span>
    </button></div>
    <button type="button" className="sidebar-search" onClick={onSearch} title={t("sidebar.searchTitle")}>
      <Icon name="search" size={15} /><span>{t("sidebar.searchLabel")}</span><kbd className="kbd">Ctrl K</kbd>
    </button>
    <nav className="nav" aria-label={t("sidebar.navLabel")}>
      <button type="button" className="nav-item" data-on={view === "focus" ? "1" : "0"} onClick={() => onView("focus")}><Icon name="focus" size={16} />{t(VIEW_LABEL.focus)}</button>
      <FocusColumns view={view} columnId={columnId} counts={feedsState.counts} onOpen={onOpenColumn} onGone={onColumnGone} notify={(message) => notify({ message })} />
      <button type="button" className="nav-item" data-on="0" onClick={() => onView("feeds")}><Icon name="rss" size={16} />{t(VIEW_LABEL.feeds)}
        {feedsState.counts?.total ? <span className="nav-count" aria-label={t("sidebar.unreadCount", { count: feedsState.counts.total })}>{feedsState.counts.total}</span> : null}</button>
      <button type="button" className="nav-item" data-on={view === "digest" ? "1" : "0"} onClick={() => onView("digest")}><Icon name="digest" size={16} />{t(VIEW_LABEL.digest)}</button>
      <button type="button" className="nav-item" data-on={view === "chat" ? "1" : "0"} onClick={() => onView("chat")}><Icon name="chat" size={16} />{t(VIEW_LABEL.chat)}</button>
    </nav>
    <ThreadList list={list} active={view === "chat" ? threadId : 0} fresh={fresh} onThread={onThread} onChanged={reload}
      onArchived={() => { setArchived(null); setArchiveMode(true); }} notify={notify} />
    <div className="sidebar-foot">
      <button type="button" onClick={onSettings}><Icon name="settings" size={15} />{t("sidebar.settings")}</button>
      <button type="button" onClick={onAbout}><Icon name="info" size={15} />{t("sidebar.about")}</button>
    </div>
  </aside>;
}));

/** 「今日焦点」下的栏目。选中时用导航纸签，此时「今日焦点」本身不选中。 */
function FocusColumns({ view, columnId, counts, onOpen, onGone, notify }: {
  view: MainView;
  columnId: string | null;
  counts: ReturnType<typeof useFeedsSession>["counts"];
  onOpen: (id: string) => void;
  onGone: () => void;
  notify: (message: string) => void;
}) {
  const { t } = useTranslation("feeds");
  const columns = useFocusColumns(counts, columnId, onGone, notify);
  const menu = (item: FocusColumn, index: number, close: () => void) => <>
    <button type="button" role="menuitem" disabled={index === 0} onClick={() => { columns.move(item.id, -1); close(); }}>{t("column.moveUp")}</button>
    <button type="button" role="menuitem" disabled={index === columns.columns.length - 1} onClick={() => { columns.move(item.id, 1); close(); }}>{t("column.moveDown")}</button>
    <button type="button" role="menuitem" onClick={() => { columns.remove(item.id); close(); }}>{t("column.remove")}</button>
  </>;
  return <>
    {columns.columns.map((item, index) => <div key={item.id} className="nav-item nav-column" data-on={view === "column" && columnId === item.id ? "1" : "0"}
      onContextMenu={(event) => { event.preventDefault(); event.currentTarget.querySelector<HTMLButtonElement>(".nav-column-more")?.click(); }}>
      <button type="button" className="nav-column-hit" onClick={() => onOpen(item.id)}>
        <Icon name="hash" size={13} /><span className="nav-column-name">{item.name}</span>
        {item.unread ? <span className="nav-count" aria-label={t("sidebar.unreadCount", { count: item.unread })}>{item.unread}</span> : null}
      </button>
      <MenuButton className="nav-column-more" label={t("column.menuLabel", { name: item.name })} title={t("column.menuTitle")} face="⋯">
        {(close) => menu(item, index, close)}
      </MenuButton>
    </div>)}
    {columns.canAdd ? <MenuButton className="nav-item nav-column nav-column-add" label={t("column.add")} face={<><Icon name="plus" size={13} />{t("column.add")}</>}>
      {(close) => columns.choices.map((folder) => <button key={folder.id} type="button" role="menuitem" className="nav-column-pick" disabled={folder.added}
        style={{ "--depth": folder.depth } as CSSProperties} onClick={() => { if (!folder.added) { columns.add(folder.id); close(); } }}>
        {folder.name}{folder.added ? t("column.added") : ""}
      </button>)}
    </MenuButton> : null}
  </>;
}

/** 订阅模式：整列侧栏换成订阅树，只在进入时滑入一次。 */
function FeedsSidebar({ feeds, inert, feedsReturn, onBack, onAddFeed, onSettings, notify }: {
  feeds: FeedsSession; inert: boolean; feedsReturn: MainView;
  onBack: () => void; onAddFeed: () => void; onSettings: () => void;
  notify: (notice: TreeNotice) => void;
}) {
  const { t } = useTranslation("feeds");
  const state = useFeedsSession(feeds);
  const tree = useQuery(feedQueryOptions()).data;
  const backName = t(VIEW_LABEL[feedsReturn]);
  useEffect(() => { feeds.start(); }, [feeds]);
  return <aside className="sidebar feeds-mode" inert={inert} aria-label={t("sidebar.feedsLabel")}>
    <div className="feeds-mode-head" data-tauri-drag-region="deep">
      <button type="button" className="feeds-back" onClick={onBack} title={t("sidebar.back", { name: backName })}><Icon name="chevronLeft" size={14} />{backName}</button>
      <strong>{t("view.feeds")}</strong>
    </div>
    {tree ? <FeedsTree tree={tree} counts={state.counts} scope={state.scope} notify={notify} onAddFeed={onAddFeed} onSettings={onSettings} onSelect={(scope) => { if (scope !== state.scope) void feeds.setScope(scope); }} />
      : <div className="feeds-tree-scroll" aria-busy="true" />}
  </aside>;
}
