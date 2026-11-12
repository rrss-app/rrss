import type { ArticleView, FeedTree, FeedView, FocusPage } from "./api";

export function flattenFeeds(tree: FeedTree): FeedView[] {
  return [...tree.uncategorized, ...tree.folders.flatMap((f) => f.feeds)];
}

export function focusExcludes(articles: ArticleView[]): {
  excludeIds: number[];
  excludeClusterIds: number[];
} {
  const excludeIds: number[] = [];
  const clusters = new Set<number>();
  for (const a of articles) {
    if (a.row_kind === 0) excludeIds.push(a.id);
    if (a.cluster_id) clusters.add(a.cluster_id);
  }
  return { excludeIds, excludeClusterIds: [...clusters] };
}

export function rowKey(row: ArticleView): string {
  return `${row.row_kind}-${row.id}-${row.cluster_id}`;
}

export function mergeFocusPage(prev: FocusPage | null, more: FocusPage): FocusPage {
  if (!prev) return more;
  const seen = new Set(prev.articles.map(rowKey));
  const appended = more.articles.filter((a) => !seen.has(rowKey(a)));
  return {
    ...prev,
    articles: appended.length ? [...prev.articles, ...appended] : prev.articles,
    has_more: appended.length === 0 ? false : more.has_more,
  };
}

/**
 * 后台更新（阶段 17 · 后台更新不动视口）：保持当前行的集合与先后，只换已显示行的内容；
 * 已读、收藏由本地写入维护，以当前行为准。返回第一页里当前列表还没有的文章数。
 */
export function refreshFocusRows(current: FocusPage, latest: FocusPage, skip: (id: number) => boolean = () => false): { page: FocusPage; added: number } {
  const byKey = new Map(latest.articles.map((row) => [rowKey(row), row]));
  let changed = false;
  const articles = current.articles.map((row) => {
    const next = byKey.get(rowKey(row));
    if (!next) return row;
    const merged = { ...next, read: row.read, starred: row.starred };
    if (JSON.stringify(merged) === JSON.stringify(row)) return row;
    changed = true;
    return merged;
  });
  const shown = new Set(current.articles.filter((row) => row.row_kind === 0).map((row) => row.id));
  const added = new Set(latest.articles.filter((row) => row.row_kind === 0 && !shown.has(row.id) && !skip(row.id)).map((row) => row.id)).size;
  return { page: changed ? { ...current, articles } : current, added };
}

/** 虚拟列表中行下标：仅 `row_kind === 0` 的文章卡。 */
export function navigableRowIndices(articles: ArticleView[]): number[] {
  const out: number[] = [];
  articles.forEach((a, i) => {
    if (a.row_kind === 0) out.push(i);
  });
  return out;
}

export function navigableArticleIds(page: FocusPage | null): number[] {
  return (page?.articles ?? []).filter((a) => a.row_kind === 0).map((a) => a.id);
}

/** Drop cluster header / more rows whose article cards are gone; keep cluster_size in sync. */
export function pruneEmptyClusters(articles: ArticleView[]): ArticleView[] {
  const remaining = new Map<number, number>();
  for (const a of articles) {
    if (a.row_kind === 0 && a.cluster_id) {
      remaining.set(a.cluster_id, (remaining.get(a.cluster_id) ?? 0) + 1);
    }
  }
  return articles
    .filter((a) => {
      if (a.row_kind === 0 || !a.cluster_id) return true;
      return (remaining.get(a.cluster_id) ?? 0) > 0;
    })
    .map((a) => {
      if (a.row_kind === 0 || !a.cluster_id) return a;
      const n = remaining.get(a.cluster_id) ?? 0;
      return n === a.cluster_size ? a : { ...a, cluster_size: n };
    });
}

export function applyStarToFocusPage(page: FocusPage, id: number, starred: boolean): FocusPage {
  let articles = page.articles.map((a) =>
    a.row_kind === 0 && a.id === id ? { ...a, starred } : a,
  );
  if (page.list_scope === "starred" && !starred) {
    articles = pruneEmptyClusters(articles.filter((a) => !(a.row_kind === 0 && a.id === id)));
  }
  return { ...page, articles };
}
