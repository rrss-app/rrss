import { useSyncExternalStore } from "react";
import { api, type ArticleView, type FeedCountsView, type FeedsCursor, type FeedsPage } from "../../lib/api";
import { formatNotice } from "../../lib/notice";

export type FeedsNotice = { message: string };

export type FeedsSnapshot = {
  scope: string;
  listScope: string;
  articles: ArticleView[];
  hasMore: boolean;
  nextCursor: FeedsCursor | null;
  hiddenCount: number;
  counts: FeedCountsView | null;
  loading: boolean;
  loadingMore: boolean;
  moreError: string;
  revision: number;
  /** 换范围后列表应回到的位置：命中缓存时是上次离开的位置，否则是顶部。 */
  restoreTop: number;
};

const EMPTY: FeedsSnapshot = {
  scope: "all", listScope: "all", articles: [], hasMore: false, nextCursor: null, hiddenCount: 0,
  counts: null, loading: false, loadingMore: false, moreError: "", revision: 0, restoreTop: 0,
};

type CachedView = Pick<FeedsSnapshot, "articles" | "hasMore" | "nextCursor" | "hiddenCount"> & { scrollTop: number };
/** 最近看过的范围数；每个只存已加载的行和滚动位置。 */
const CACHE_LIMIT = 12;
const viewKey = (scope: string, listScope: string) => `${scope}\n${listScope}`;

/** 订阅域的唯一快照。切走不销毁；迟到响应按代次丢弃。
 * 换范围时先显示该范围的缓存（行与滚动位置），同时重取首页核对：首页没变就保留已续的页，变了才整体替换并回到顶部。 */
export function createFeedsSession(notify: (notice: FeedsNotice) => void) {
  let state: FeedsSnapshot = { ...EMPTY, articles: [] };
  const cache = new Map<string, CachedView>();
  let scrollTop = 0;
  const listeners = new Set<() => void>();
  let generation = 0;
  let started = false;
  let initialLoad: Promise<void> = Promise.resolve();
  let moreRequest: Promise<void> | null = null;
  let writes: Promise<void> = Promise.resolve();

  const publish = (patch: Partial<FeedsSnapshot>) => {
    state = { ...state, ...patch };
    listeners.forEach((listener) => listener());
  };
  const current = (gen: number) => gen === generation;

  function persist(id: "feeds_scope" | "feeds_list_scope", value: string) {
    const gen = generation;
    writes = writes.catch(() => undefined).then(async () => {
      if (!current(gen)) return;
      try {
        await api.setSetting(id, value);
      } catch (err) {
        if (current(gen)) notify({ message: formatNotice("设置失败：", err) });
      }
    });
  }

  function arm(patch: Partial<FeedsSnapshot>) {
    const gen = ++generation;
    moreRequest = null;
    publish({ loading: true, loadingMore: false, moreError: "", revision: state.revision + 1, ...patch });
    return gen;
  }

  /** 记下当前范围的已加载行与位置；空白的加载中视图不记。 */
  function remember() {
    if (!state.articles.length && state.loading) return;
    const key = viewKey(state.scope, state.listScope);
    cache.delete(key);
    cache.set(key, { articles: state.articles, hasMore: state.hasMore, nextCursor: state.nextCursor, hiddenCount: state.hiddenCount, scrollTop });
    while (cache.size > CACHE_LIMIT) cache.delete(cache.keys().next().value!);
  }

  function switchTo(scope: string, listScope: string) {
    remember();
    const hit = cache.get(viewKey(scope, listScope));
    scrollTop = hit?.scrollTop ?? 0;
    return arm({
      scope, listScope, articles: hit?.articles ?? [], hasMore: hit?.hasMore ?? false,
      nextCursor: hit?.nextCursor ?? null, hiddenCount: hit?.hiddenCount ?? 0, restoreTop: scrollTop,
    });
  }

  async function loadPage(gen: number) {
    const scope = state.scope;
    const listScope = state.listScope;
    try {
      const page: FeedsPage = await api.feedsPage(scope, listScope, null);
      if (!current(gen)) return;
      if (page.scope !== scope) persist("feeds_scope", page.scope);
      const shown = state.articles;
      const unchanged = shown.length > page.articles.length && page.articles.every((row, i) => shown[i].id === row.id);
      if (unchanged) {
        // 首页与正在显示的一致：换上新行，保留已续的页和位置。
        publish({ scope: page.scope, articles: [...page.articles, ...shown.slice(page.articles.length)], hiddenCount: page.hidden_count });
      } else {
        const moved = shown.length > 0 && (shown.length !== page.articles.length || page.articles.some((row, i) => shown[i].id !== row.id));
        publish({
          scope: page.scope, articles: page.articles, hasMore: page.has_more,
          nextCursor: page.next_cursor, hiddenCount: page.hidden_count,
          ...(moved && scrollTop > 0 ? { revision: state.revision + 1, restoreTop: 0 } : {}),
        });
        if (moved) scrollTop = 0;
      }
    } catch (err) {
      if (current(gen)) notify({ message: formatNotice("加载失败：", err) });
    } finally {
      if (current(gen)) publish({ loading: false });
    }
  }

  async function loadCounts(gen: number) {
    try {
      const counts = await api.feedCounts();
      if (current(gen)) publish({ counts });
    } catch (err) {
      if (current(gen)) notify({ message: formatNotice("加载失败：", err) });
    }
  }

  /** `scope` 给定时（从卡片来源名进入）用它取代保存的范围，列表范围仍读设置。 */
  async function boot(gen: number, scope?: string) {
    try {
      const schema = await api.settingsSchema();
      if (!current(gen)) return;
      const value = (id: string) => schema.values.find((item) => item.id === id)?.value || "all";
      publish({ scope: scope ?? value("feeds_scope"), listScope: value("feeds_list_scope") });
    } catch (err) {
      if (!current(gen)) return;
      notify({ message: formatNotice("加载失败：", err) });
      publish({ scope: scope ?? "all", listScope: "all" });
    }
    await Promise.all([loadPage(gen), loadCounts(gen)]);
  }

  function loadMore(): Promise<void> {
    if (moreRequest) return moreRequest;
    const cursor = state.nextCursor;
    if (state.loading || !state.hasMore || cursor === null) return Promise.resolve();
    const gen = generation;
    const scope = state.scope;
    const listScope = state.listScope;
    publish({ loadingMore: true, moreError: "" });
    const request = (async () => {
      try {
        const page = await api.feedsPage(scope, listScope, cursor);
        if (!current(gen)) return;
        const seen = new Set(state.articles.map((row) => row.id));
        publish({
          articles: [...state.articles, ...page.articles.filter((row) => !seen.has(row.id))],
          hasMore: page.has_more, nextCursor: page.next_cursor, hiddenCount: page.hidden_count,
        });
      } catch (err) {
        if (current(gen)) publish({ moreError: formatNotice("加载失败：", err) });
      } finally {
        if (current(gen)) { moreRequest = null; publish({ loadingMore: false }); }
      }
    })();
    moreRequest = request;
    return request;
  }

  function setScope(scope: string) {
    const gen = switchTo(scope, state.listScope);
    persist("feeds_scope", scope);
    if (started) return loadPage(gen);
    // 还没进过订阅模式：直接以这个范围启动，免得随后的 start 读回旧设置把它盖掉。
    started = true;
    initialLoad = boot(gen, scope);
    return initialLoad;
  }

  function setListScope(listScope: string) {
    const gen = switchTo(state.scope, listScope);
    persist("feeds_list_scope", listScope);
    return loadPage(gen);
  }

  /** 抓取或改动隐藏后重读：其他范围的缓存都可能过期，只留当前视图等首页核对。 */
  function refresh() {
    cache.clear();
    const gen = arm({ restoreTop: scrollTop });
    return Promise.all([loadPage(gen), loadCounts(gen)]).then(() => undefined);
  }

  async function markScopeRead() {
    const gen = generation;
    const scope = state.scope;
    try {
      const ids = new Set(await api.markScopeRead(scope));
      if (!current(gen)) return;
      patchCached((row) => ids.has(row.id) ? { ...row, read: true } : row);
      publish({ articles: state.articles.map((row) => ids.has(row.id) ? { ...row, read: true } : row) });
      await loadCounts(gen);
    } catch (err) {
      if (current(gen)) notify({ message: formatNotice("标记已读失败：", err) });
    }
  }

  /** 已读、收藏在所有缓存的范围里同步，切回去时不显示旧状态。 */
  const patchCached = (update: (row: ArticleView) => ArticleView) => {
    for (const view of cache.values()) view.articles = view.articles.map(update);
  };
  const patchRow = (id: number, patch: Partial<ArticleView>, differs: (row: ArticleView) => boolean) => {
    patchCached((row) => row.id === id ? { ...row, ...patch } : row);
    if (state.articles.some((row) => row.id === id && differs(row))) {
      publish({ articles: state.articles.map((row) => row.id === id ? { ...row, ...patch } : row) });
    }
  };

  /** 阅读器打开 / 改回未读后的本地回写；真正的写入由阅读器现有流程负责。 */
  function setRead(id: number, read: boolean) {
    const gen = generation;
    patchRow(id, { read }, (row) => row.read !== read);
    return loadCounts(gen);
  }

  function setStarred(id: number, starred: boolean) {
    patchRow(id, { starred }, (row) => row.starred !== starred);
  }

  function start() {
    if (started) return;
    started = true;
    initialLoad = boot(arm({}));
  }

  return {
    getSnapshot: () => state,
    subscribe: (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; },
    start, setScope, setListScope, loadMore, refresh, markScopeRead, setRead, setStarred,
    /** 列表滚动时上报位置，换走时连同已加载的行一起缓存。 */
    noteScroll: (top: number) => { scrollTop = top; },
    /** 侧栏「订阅」项的总数在进入订阅模式前也要有，只刷新计数、不取文章。 */
    refreshCounts: () => loadCounts(generation),
    invalidate: async () => {
      if (!started) return loadCounts(generation);
      // A feed event must not cancel the first read of the saved scope/filter.
      await initialLoad;
      return refresh();
    },
  };
}

export type FeedsSession = ReturnType<typeof createFeedsSession>;

export function useFeedsSession(session: FeedsSession) {
  return useSyncExternalStore(session.subscribe, session.getSnapshot);
}
