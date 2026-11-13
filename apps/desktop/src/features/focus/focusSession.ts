import { useSyncExternalStore } from "react";
import { api, type ArticleView, type FeedTree, type FeedView, type FocusPage, type SearchQuery } from "../../lib/api";
import { trackEvent } from "../../lib/signals";
import { applyStarToFocusPage, flattenFeeds, focusExcludes, mergeFocusPage, refreshFocusRows, rowKey } from "../../lib/focusUtils";
import { formatNotice } from "../../lib/notice";
import { isTauri, PREVIEW_FOCUS } from "../../lib/preview";
import { FALLBACK_RANKING, type RankingToggle } from "../../components/FocusPane";
import type { SpotAction, SpotReview } from "../../components/SpotCheckReveal";
import { refreshProblem, waitForRefresh } from "./refreshOperation";

// 约一张普通卡片的高度（px）；超过此距离视为已离开列表顶部。
const BROWSING_SCROLL_THRESHOLD = 88;

export type FocusNotice = { message: string; holdMs?: number; actionLabel?: string; onAction?: () => void };
export type FocusSnapshot = {
  spotReviews: Record<number, SpotReview>;
  spotBusy: boolean;
  page: FocusPage | null;
  feeds: FeedView[];
  expandedCluster: number | null;
  loading: boolean;
  loadingMore: boolean;
  moreError: string;
  refreshing: boolean;
  /** 用户换了列表（筛选、范围、刷新等）才加一；回到顶部、键盘选中都跟着它。 */
  revision: number;
  /** 后台就地更新或采用新内容时加一，不清键盘选中。 */
  dataRevision: number;
  /** 后台发现、还没并进列表的新文章数；点顶部提示才重读。 */
  pendingNew: number;
  readerReturn: { id: number; sequence: number } | null;
};

export type FocusSessionOptions = {
  /** 缺省或 `all`：查询不带 scope，与主界面今日焦点一致。栏目传 `folder:<id>`。 */
  scope?: string;
  /** 按范围标已读成功后通知外层刷新未读数。 */
  onMarked?: () => void;
};

/** 焦点域的唯一快照。根壳只装配实例，不订阅列表；切页不销毁当前范围。 */
export function createFocusSession(notify: (notice: FocusNotice) => void, readFeeds: () => Promise<FeedTree> = api.feeds, rulesChanged: () => Promise<void> = async () => {}, options?: FocusSessionOptions) {
  let state: FocusSnapshot = {
    spotReviews: {}, spotBusy: false,
    page: null, feeds: [], expandedCluster: null,
    loading: false, loadingMore: false, moreError: "", refreshing: false, revision: 0, dataRevision: 0, pendingNew: 0, readerReturn: null,
  };
  const listeners = new Set<() => void>();
  let generation = 0;
  let returnSequence = 0;
  let pageGeneration = -1;
  let requestedCluster: number | null = null;
  let expandTicket = 0;
  // 展开前标题行后面的那篇代表；`null` 表示当时没有代表文章。
  const representatives = new Map<number, ArticleView | null>();
  let feedsGeneration = 0;
  let active = false;
  let browsingStarted = false;
  let viewport: { readTop: () => number; resetTop: () => void } | undefined;
  function startBrowsing() {
    if (active && !state.loading && state.page) browsingStarted = true;
  }
  let refreshController: AbortController | null = null;
  // 空字符串表示全部订阅：请求里不放 scope 键。
  let feedScope = options?.scope && options.scope !== "all" ? options.scope : "";
  const focusQuery = (expanded: number | null, page?: { excludeIds: number[]; excludeClusterIds: number[]; more: true }) => {
    const scope = feedScope || undefined;
    if (!page && !scope) return api.focus(expanded);
    return api.focus(expanded, { ...page, ...(scope ? { scope } : {}) });
  };
  let writes: Promise<void> = Promise.resolve();
  let moreRequest: Promise<void> | null = null;
  const dismissedIds = new Set<number>();
  let navigationReset = 0;
  const visiblePage = (page: FocusPage | null): FocusPage | null => page ? {
    ...page,
    articles: arrangeExpanded(page.articles.filter((row) => row.row_kind !== 0 || !dismissedIds.has(row.id)), requestedCluster),
  } : page;
  const resetNavigation = () => { dismissedIds.clear(); navigationReset += 1; };
  // 阅读器正在看的文章：阅读期间任何重读都把它留在原位，上下篇、踩和篇目才找得到当前篇。
  let readingId: number | null = null;
  let keptReading = false;
  const keepReading = (page: FocusPage, previous: FocusPage | null): FocusPage => {
    keptReading = false;
    const reading = readingId;
    if (reading === null || page.articles.some((row) => row.row_kind === 0 && row.id === reading)) return page;
    const old = previous?.articles ?? [];
    const at = old.findIndex((row) => row.row_kind === 0 && row.id === reading);
    if (at < 0) return page;
    const key = (row: ArticleView) => `${row.row_kind}:${row.id}`;
    const present = new Set(page.articles.map(key));
    const anchor = old.slice(0, at).reverse().find((row) => present.has(key(row)));
    const insert = anchor ? page.articles.findIndex((row) => key(row) === key(anchor)) + 1 : 0;
    keptReading = true;
    return { ...page, articles: [...page.articles.slice(0, insert), old[at], ...page.articles.slice(insert)] };
  };
  const publish = (patch: Partial<FocusSnapshot>) => {
    state = { ...state, ...patch };
    listeners.forEach((listener) => listener());
  };
  const owns = (gen: number) => active && gen === generation;
  const clusterHeader = (articles: ArticleView[], clusterId: number) =>
    articles.findIndex((row) => row.row_kind === 1 && row.cluster_id === clusterId);
  const shownRepresentative = (articles: ArticleView[], clusterId: number): ArticleView | null => {
    const header = clusterHeader(articles, clusterId);
    const next = header < 0 ? undefined : articles[header + 1];
    return next && next.row_kind === 0 && next.cluster_id === clusterId ? next : null;
  };
  const spliceCluster = (articles: ArticleView[], clusterId: number, replacement: ArticleView[]) => {
    const header = clusterHeader(articles, clusterId);
    if (header < 0) return null;
    let end = header + 1;
    while (end < articles.length && articles[end].row_kind === 0 && articles[end].cluster_id === clusterId) end += 1;
    return [...articles.slice(0, header + 1), ...replacement, ...articles.slice(end)];
  };
  /** 收起时放回的代表：取列表里的当前那一行，展开期间标的已读、收藏不丢。 */
  const restoreCard = (articles: ArticleView[], clusterId: number) => {
    if (!representatives.has(clusterId)) return shownRepresentative(articles, clusterId);
    const rep = representatives.get(clusterId) ?? null;
    return rep && (articles.find((row) => row.row_kind === 0 && row.id === rep.id) ?? rep);
  };
  /** 展开的组：代表报道固定在组首（收起时显示的那篇），其余按发布时间倒序。 */
  function arrangeExpanded(articles: ArticleView[], clusterId: number | null): ArticleView[] {
    if (clusterId == null) return articles;
    const header = clusterHeader(articles, clusterId);
    if (header < 0) return articles;
    let end = header + 1;
    while (end < articles.length && articles[end].row_kind === 0 && articles[end].cluster_id === clusterId) end += 1;
    const members = articles.slice(header + 1, end);
    if (members.length < 2) return articles;
    const repId = representatives.get(clusterId)?.id;
    const rep = members.find((row) => row.id === repId);
    // 同一时刻发布的保持宿主给的先后（排序是稳定的）。
    const rest = members.filter((row) => row !== rep).sort((a, b) => (b.published_at ?? 0) - (a.published_at ?? 0));
    return [...articles.slice(0, header + 1), ...(rep ? [rep] : []), ...rest, ...articles.slice(end)];
  }

  const saveReview = (id: number, review: SpotReview) => publish({ spotReviews: { ...state.spotReviews, [id]: review } });
  function revealSpotCheck(id: number, up: boolean) {
    const old = state.spotReviews[id];
    const article = old?.article ?? state.page?.articles.find(row => row.id === id && row.row_kind === 0 && row.spot_check);
    if (active && article) saveReview(id, { ...old, article, up });
  }
  async function decideSpotCheck(id: number, action: SpotAction) {
    const review = state.spotReviews[id];
    if (!active || !review || state.spotBusy) return;
    if (action === "keep") { saveReview(id, { ...review, result: "已保持不变。", undo: undefined }); return; }
    publish({ spotBusy: true });
    try {
      let result: string | undefined;
      let undo: SpotReview["undo"];
      if (action === "undo") await review.undo?.();
      else {
        const reason = [...(review.article.hide_reasons ?? [])].sort((a, b) => a.weight - b.weight)[0];
        if (!reason) throw new Error("没有可操作的隐藏规则。");
        if (action === "allow") {
          const added = await api.allowRuleForFeed(reason.rule_id, review.article.feed_id);
          undo = () => api.discardRule(added);
          result = `已对「${review.article.feed_title}」放行。`;
        } else {
          const rule = (await api.rules()).rules.find(r => r.id === reason.rule_id);
          if (!rule?.enabled) throw new Error("这条规则已停用或删除，请刷新焦点。");
          if (action === "disable") {
            await api.setRuleAction(rule.id, 2);
            undo = () => api.setRuleAction(rule.id, rule.polarity === "like" ? 0 : 1);
            result = `已停用「${reason.label}」。`;
          } else {
            const original = { polarity: rule.polarity, dimension: rule.dimension_id, pattern: rule.pattern,
              weight: rule.weight, scope_kind: rule.scope_kind, scope_id: rule.scope_id };
            const weight = Math.max(1, Math.round(rule.weight / 2));
            await api.updateRule(rule.id, { ...original, weight });
            undo = () => api.updateRule(rule.id, original);
            result = `已把「${reason.label}」的权重减半。`;
          }
        }
      }
      saveReview(id, { ...(state.spotReviews[id] ?? review), result, undo });
      await rulesChanged();
    } catch (err) { notify({ message: formatNotice("规则操作失败：", err) }); }
    finally { publish({ spotBusy: false }); }
  }

  async function reload(resetBrowsing = true) {
    const gen = ++generation;
    if (resetBrowsing) browsingStarted = false;
    moreRequest = null;
    publish({ readerReturn: null, loading: true, loadingMore: false, moreError: "", revision: state.revision + 1, pendingNew: 0 });
    // 筛选存于宿主设置：先等本实例排队的写入完成，再读取实际生效范围。
    await writes;
    if (!owns(gen)) return;
    try {
      const page = await focusQuery(requestedCluster);
      if (owns(gen)) {
        pageGeneration = gen;
        publish({ page: visiblePage(keepReading(page, state.page)), expandedCluster: requestedCluster });
      }
    } catch (err) {
      if (!owns(gen)) return;
      if (!isTauri()) publish({ page: state.page ?? PREVIEW_FOCUS });
      else notify({ message: formatNotice("加载失败：", err), actionLabel: "重试", onAction: () => void reload() });
    } finally {
      if (owns(gen)) publish({ loading: false });
    }
  }

  function loadMore(): Promise<void> {
    if (moreRequest) return moreRequest;
    const page = state.page;
    if (!active || state.loading || pageGeneration !== generation || !page?.has_more) return Promise.resolve();
    const gen = generation;
    const { excludeIds, excludeClusterIds } = focusExcludes(page.articles);
    publish({ loadingMore: true, moreError: "" });
    const request = (async () => {
      try {
        const more = await focusQuery(state.expandedCluster, { excludeIds, excludeClusterIds, more: true });
        if (owns(gen)) publish({ page: visiblePage(mergeFocusPage(state.page, more)) });
      } catch (err) {
        if (owns(gen)) publish({ moreError: formatNotice("加载失败：", err) });
      } finally {
        // 旧范围结束不能释放新范围的分页锁，也不能清掉它的 busy 状态。
        if (owns(gen)) { moreRequest = null; publish({ loadingMore: false }); }
      }
    })();
    moreRequest = request;
    return request;
  }

  function changeSetting(write: () => Promise<unknown>) {
    writes = writes.then(async () => { await write(); }).catch((err) => {
      if (active) notify({ message: formatNotice("设置失败：", err) });
    });
    return reload();
  }

  function applyFeeds(tree: FeedTree) {
    publish({ feeds: flattenFeeds(tree) });
  }

  async function reloadFeeds() {
    const gen = ++feedsGeneration;
    try {
      const tree = await readFeeds();
      if (!active || gen !== feedsGeneration) return;
      applyFeeds(tree);
    } catch { /* 失败或被写入取消的读取不清空现有来源。 */ }
  }

  function feedsChanged(tree?: FeedTree) {
    ++feedsGeneration; // 保存前启动的读取不再拥有来源快照。
    if (!active) return;
    if (tree) applyFeeds(tree);
    // 改名、移动和退订也改变文章标题、规则作用域或列表内容。
    return reload();
  }

  /**
   * 后台静默取第一页：未开始浏览时直接采用新内容；开始后就地更新并提示。
   */
  async function backgroundChanged(tree?: FeedTree) {
    ++feedsGeneration;
    if (!active) return;
    if (tree) applyFeeds(tree);
    // 正在读的那一轮会带回最新数据；还没有列表时没有视口可保。
    if (state.loading) return;
    if (!state.page?.articles.length) return reload();
    const gen = generation;
    let latest: FocusPage;
    try { latest = await focusQuery(requestedCluster); } catch { return; }
    if (!owns(gen) || pageGeneration !== gen || !state.page || state.loading) return;
    // 查询期间也可能开始浏览；离顶部超过约一行时兜底，不能只看请求发出时的状态。
    if ((viewport?.readTop() ?? 0) > BROWSING_SCROLL_THRESHOLD) browsingStarted = true;
    const { page, added } = refreshFocusRows(state.page, latest, (id) => dismissedIds.has(id));
    if (added > 0 && !browsingStarted) {
      // 替换分页基线，旧列表的迟到续页不能再追加；保留本地的已读 / 收藏写入。
      pageGeneration = ++generation;
      moreRequest = null;
      const updated = new Map(page.articles.map(row => [rowKey(row), row]));
      viewport?.resetTop();
      publish({ page: visiblePage({ ...latest, articles: latest.articles.map(row => updated.get(rowKey(row)) ?? row) }),
        dataRevision: state.dataRevision + 1, pendingNew: 0, loadingMore: false, moreError: "" });
      return;
    }
    const patch: Partial<FocusSnapshot> = {};
    if (page !== state.page) Object.assign(patch, { page, dataRevision: state.dataRevision + 1 });
    if (added !== state.pendingNew) patch.pendingNew = added;
    if (Object.keys(patch).length) publish(patch);
  }

  /**
   * 从托盘唤回（阶段 14 · 后台驻留）：当作重新开始，直接采用最新的第一页并回到顶部，
   * 不显示「有 N 篇新内容」；不出骨架屏。本地已读 / 收藏写入先落库再读。
   */
  async function adoptLatest() {
    if (!active) return;
    browsingStarted = false;
    if (state.loading || !state.page?.articles.length) return reload();
    const gen = generation;
    await writes;
    let latest: FocusPage;
    try { latest = await focusQuery(requestedCluster); } catch { return; }
    if (!owns(gen) || pageGeneration !== gen || state.loading) return;
    pageGeneration = ++generation;
    moreRequest = null;
    viewport?.resetTop();
    publish({ page: visiblePage(latest), dataRevision: state.dataRevision + 1, pendingNew: 0, loadingMore: false, moreError: "" });
  }

  async function toggleStar(id: number, starred: boolean) {
    const next = !starred;
    try {
      await api.starArticle(id, next);
      trackEvent(id, next ? "star" : "unstar");
      if (!active) return true;
      if (state.page) publish({ page: applyStarToFocusPage(state.page, id, next) });
      if (state.page?.list_scope === "starred" || (next && !state.page?.articles.some((row) => row.row_kind === 0 && row.id === id))) await reload();
      notify({ message: next ? "已收藏" : "已取消收藏" });
      return true;
    } catch (err) {
      if (active) notify({ message: formatNotice("收藏失败：", err) });
      return false;
    }
  }

  function setRead(id: number, read: boolean) {
    if (read) revealSpotCheck(id, true);
    const page = state.page;
    if (page?.articles.some((row) => row.id === id && row.read !== read)) {
      publish({ page: { ...page, articles: page.articles.map((row) => row.id === id ? { ...row, read } : row) } });
    }
  }

  async function markAllRead() {
    const requestedGeneration = generation;
    await writes;
    if (!owns(requestedGeneration) || state.loading || pageGeneration !== generation) return;
    try {
      const ids = await api.markScopeRead(feedScope || "all");
      if (!active) return;
      if (!ids.length) { notify({ message: "没有未读条目" }); return; }
      options?.onMarked?.();
      await reload();
      notify({ message: `已标记 ${ids.length} 篇为已读`, actionLabel: "撤销", onAction: () => {
        void api.unmarkRead(ids).then(() => reload()).catch((err) => notify({ message: formatNotice("撤销失败：", err) }));
      } });
    } catch (err) { if (active) notify({ message: formatNotice("标记已读失败：", err) }); }
  }

  /** 组尾「全部标为已读」：整组在范围内可见的未读一次标完，直接生效、不带撤销（2026-11-06 定稿）。 */
  async function markGroupRead(clusterId: number) {
    const gen = generation;
    if (!active) return;
    let ids: number[];
    try {
      ids = await (feedScope ? api.markClusterRead(clusterId, feedScope) : api.markClusterRead(clusterId));
    } catch (err) {
      if (owns(gen)) notify({ message: formatNotice("标记已读失败：", err) });
      return;
    }
    if (!owns(gen)) return;
    if (!ids.length) { notify({ message: "这组已经都读过了" }); return; }
    options?.onMarked?.();
    const marked = new Set(ids);
    const page = state.page;
    if (page?.list_scope === "unread") await reload();
    else if (page) publish({ page: { ...page, articles: page.articles.map((row) => row.row_kind === 0 && marked.has(row.id) ? { ...row, read: true } : row) } });
    notify({ message: `已把这组 ${ids.length} 篇标为已读` });
  }

  const refresh = () => refreshCurrent(false);
  async function refreshCurrent(resume: boolean) {
    if (state.refreshing || state.spotBusy) return;
    refreshController?.abort();
    const controller = new AbortController();
    refreshController = controller;
    if (!resume) publish({ refreshing: true, spotReviews: {} });
    try {
      const result = await waitForRefresh(async () => {
        if (!resume) return api.refresh();
        const snapshot = await api.schedulerStatus();
        const running = snapshot.operations.find(item => !item.done);
        return running ? { run_id: snapshot.run_id, operation_id: running.id } : null;
      }, controller.signal, () => { if (active && refreshController === controller) publish({ refreshing: true }); });
      if (!active || !result || refreshController !== controller) return;
      resetNavigation();
      const problem = refreshProblem(result, state.feeds);
      if (problem) notify({ message: problem, actionLabel: "重试", onAction: () => void refresh() });
      else if (result.inserted > 0) notify({ message: `刷新完成，新入库 ${result.inserted} 篇` });
    } catch (err) {
      if (active && !controller.signal.aborted && !resume) notify({ message: formatNotice("刷新失败：", err), actionLabel: "重试", onAction: () => void refresh() });
    } finally {
      if (active && refreshController === controller) {
        if (state.refreshing) {
          if (resume) await backgroundChanged();
          else await reload();
        }
        if (active && refreshController === controller) publish({ refreshing: false });
      }
    }
  }

  async function readerClosed(id?: number | null, dismissed: number[] = []) {
    const sequence = ++returnSequence;
    readingId = null;
    // 返回目标按收起前的顺序找（当前篇可能刚被踩掉）；本次踩掉的行与入口无关（含搜索打开），随后一并收起。
    const before = state.page?.articles.filter(row => row.row_kind === 0) ?? [];
    if (dismissed.length) {
      dismissed.forEach((articleId) => dismissedIds.add(articleId));
      if (state.page) publish({ page: visiblePage(state.page) });
    }
    const at = before.findIndex(row => row.id === id);
    const unread = state.page?.list_scope === "unread";
    // 用关闭前的顺序找幸存者；不能拿删行后的同一下标代替原篇。
    const forward = at < 0 ? [] : before.slice(at).filter(row => !unread || !row.read).map(row => row.id);
    const backward = before.slice(0, at).reverse().map(row => row.id);
    const known = new Set(before.map(row => row.id));
    const hadMore = state.page?.has_more;
    const nextTarget = () => forward.find(candidate => state.page?.articles.some(row => row.row_kind === 0 && row.id === candidate))
      ?? (hadMore ? state.page?.articles.find(row => row.row_kind === 0 && !known.has(row.id))?.id : undefined);
    // 阅读期间为当前篇保留过的行不再属于宿主范围，关闭时一并重读。
    const pending = unread || keptReading ? reload(false) : Promise.resolve();
    const gen = generation;
    await pending;
    const current = () => owns(gen) && pageGeneration === gen && sequence === returnSequence;
    if (!current() || at < 0) return;
    // 返回远处的未读行时，重读可能只给第一页。沿原分页接口找目标，失败不循环重试。
    while (nextTarget() === undefined && state.page?.has_more) {
      const count = state.page.articles.length;
      await loadMore();
      if (!current() || state.moreError) return;
      if (state.page.articles.length === count) break;
    }
    const rows = state.page?.articles.filter(row => row.row_kind === 0) ?? [];
    const target = nextTarget() ?? backward.find(candidate => rows.some(row => row.id === candidate)) ?? rows[Math.min(at, rows.length - 1)]?.id;
    publish({ readerReturn: target === undefined ? null : { id: target, sequence } });
  }

  return {
    getSnapshot: () => state,
    getRanking: () => state.page?.ranking ?? FALLBACK_RANKING,
    subscribe: (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; },
    start() { active = true; publish({ refreshing: false }); void reload(); void reloadFeeds(); if (isTauri() && !options?.scope) void refreshCurrent(true); },
    stop() { active = false; refreshController?.abort(); ++generation; ++feedsGeneration; moreRequest = null; },
    reload, reloadFeeds, feedsChanged, backgroundChanged, adoptLatest, loadMore, toggleStar, setRead, markAllRead, markGroupRead, refresh, revealSpotCheck, decideSpotCheck,
    startBrowsing,
    bindViewport(readTop: () => number, resetTop: () => void) {
      const bound = { readTop, resetTop };
      viewport = bound;
      return () => { if (viewport === bound) viewport = undefined; };
    },
    setFilter: (filter: string) => { resetNavigation(); return changeSetting(() => api.setSetting("focus_filter", filter)); },
    setListScope: (scope: string) => { resetNavigation(); return changeSetting(() => api.setSetting("list_scope", scope)); },
    setExpanded(id: number | null) {
      if (!active || (id !== null && id === state.expandedCluster)) return Promise.resolve();
      const ticket = ++expandTicket;
      const gen = generation;
      if (id == null) {
        const current = state.expandedCluster;
        requestedCluster = null;
        if (current == null || !state.page) return Promise.resolve();
        const rep = restoreCard(state.page.articles, current);
        representatives.delete(current);
        const articles = spliceCluster(state.page.articles, current, rep ? [rep] : []) ?? state.page.articles;
        publish({ page: visiblePage({ ...state.page, articles }), expandedCluster: null });
        return Promise.resolve();
      }
      if (!state.page) {
        requestedCluster = id;
        return Promise.resolve();
      }
      const clusterId = id;
      return (async () => {
        try {
          const members = (await (feedScope ? api.focusClusterMembers(clusterId, feedScope) : api.focusClusterMembers(clusterId))).filter((row) => row.row_kind === 0);
          if (!owns(gen) || ticket !== expandTicket || !state.page || members.length === 0) return;
          let base = state.page.articles;
          const previous = state.expandedCluster;
          if (previous != null && previous !== clusterId) {
            const rep = restoreCard(base, previous);
            base = spliceCluster(base, previous, rep ? [rep] : []) ?? base;
          }
          const articles = spliceCluster(base, clusterId, members);
          if (!articles) return;
          if (previous != null && previous !== clusterId) representatives.delete(previous);
          representatives.set(clusterId, shownRepresentative(base, clusterId));
          requestedCluster = clusterId;
          publish({ page: visiblePage({ ...state.page, articles }), expandedCluster: clusterId });
        } catch (err) {
          if (!owns(gen) || ticket !== expandTicket) return;
          notify({ message: formatNotice("加载失败：", err) });
        }
      })();
    },
    setRanking(key: RankingToggle, value: boolean) {
      resetNavigation();
      return changeSetting(async () => {
        const id = { cluster_group: "cluster_group_enabled", interest_boost: "interest_boost_enabled",
          show_details: "focus_show_details",
          rerank: "focus_rerank_enabled", hide_noisy: "focus_hide_noisy", rank_features: "rank_features_enabled",
          rank_source: "rank_source_enabled", rank_coverage: "rank_coverage_enabled", rank_freshness: "rank_freshness_enabled", rank_affinity: "rank_affinity_enabled" } as const;
        await api.setSetting(id[key], value ? "1" : "0");
        if (key === "cluster_group" && !value) requestedCluster = null;
      });
    },
    /** 综述刚生成：组头的「新 N」随之清掉，不等重读。 */
    clearClusterFresh(clusterId: number) {
      const page = state.page;
      if (!page?.articles.some((row) => row.row_kind === 1 && row.cluster_id === clusterId && row.cluster_fresh)) return;
      publish({ page: { ...page, articles: page.articles.map((row) => row.row_kind === 1 && row.cluster_id === clusterId ? { ...row, cluster_fresh: undefined } : row) } });
    },
    findArticle: (id: number): ArticleView | undefined => state.page?.articles.find((row) => row.row_kind === 0 && row.id === id),
    setStarred(id: number, starred: boolean) {
      const page = state.page;
      if (page?.articles.some((row) => row.id === id && row.starred !== starred)) {
        publish({ page: { ...page, articles: page.articles.map((row) => row.id === id ? { ...row, starred } : row) } });
      }
    },
    /** 换栏目范围：重载首页。未启动时只记下范围，等 start 再读。 */
    setFeedScope(next: string) {
      const normalized = next && next !== "all" ? next : "";
      if (normalized === feedScope) return Promise.resolve();
      feedScope = normalized;
      requestedCluster = null;
      representatives.clear();
      resetNavigation();
      if (!active) return Promise.resolve();
      return reload();
    },
    getNavigation: () => ({ expandedCluster: state.expandedCluster, articles: state.page?.articles ?? [], hasMore: !state.loading && pageGeneration === generation && !!state.page?.has_more, loadingMore: state.loadingMore, scope: state.page?.list_scope, revision: state.revision, reset: navigationReset }),
    readerClosed,
    readerOpened() { startBrowsing(); ++returnSequence; publish({ readerReturn: null }); },
    /** 阅读器显示了列表里的某篇；`null` 表示当前篇不来自列表（简报、对话）。 */
    readerAt(id: number | null) { readingId = id; },
    clearReaderReturn() { publish({ readerReturn: null }); },
    async search(query: SearchQuery) {
      try { return await api.searchArticles(query); }
      catch (err) {
        if (isTauri()) throw err;
        const q = query.query.toLowerCase();
        return (state.page ?? PREVIEW_FOCUS).articles.filter((row) => row.row_kind === 0 && row.title.toLowerCase().includes(q)).slice(0, 20).map((row) => ({
          id: row.id, title: row.title, feed_title: row.feed_title, feed_icon_href: row.feed_icon_href, published_at: row.published_at, score: row.score, snippet: "", read: row.read, starred: row.starred, title_match: true, body_match: false,
        }));
      }
    },
  };
}

export type FocusSession = ReturnType<typeof createFocusSession>;
export const useFocusSnapshot = (session: FocusSession) => useSyncExternalStore(session.subscribe, session.getSnapshot);
export const useFocusRanking = (session: FocusSession) => useSyncExternalStore(session.subscribe, session.getRanking);
