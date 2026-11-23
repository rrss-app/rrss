import type { ArticleView, FeedTree, FeedView, FocusPage } from "./api";
import { i18n } from "../i18n";

/** 排序说明码（interest_boost_inactive / interest_not_ready）的文字；没有说明时为空。 */
export function rankingNoteText(code: string): string {
  if (!code) return "";
  const key = `rankingNote.${code}` as "rankingNote.interest_not_ready";
  return i18n.exists(key, { ns: "focus" }) ? String(i18n.t(key, { ns: "focus" })) : "";
}

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

export function focusBlocks(rows: ArticleView[]): ArticleView[][] {
  const blocks: ArticleView[][] = [];
  for (const row of rows) {
    const block = blocks.at(-1);
    if (row.row_kind !== 1 && block?.[0].row_kind === 1 && block[0].cluster_id === row.cluster_id) block.push(row);
    else blocks.push([row]);
  }
  return blocks;
}

export function mergeLowerArrivals(current: FocusPage, latest: FocusPage, newIds: Set<number>, visibleEnd: number): FocusPage {
  // 位置只按已显示的行和新文章算：不会插进来的旧文章不能把新文章挤到阅读位置之下。
  const shownKeys = new Set(current.articles.map(rowKey));
  const ranked = focusBlocks(latest.articles)
    .map(block => block.filter(row => row.row_kind !== 0 || shownKeys.has(rowKey(row)) || newIds.has(row.id)))
    .filter(block => block.some(row => row.row_kind === 0));
  const placed = ranked.flat();
  if (!placed.slice(visibleEnd + 1).some(row => row.row_kind === 0 && newIds.has(row.id))) return current;
  const blocks = focusBlocks(current.articles);
  let offset = 0;
  let firstSafe = 0;
  const byKey = new Map(blocks.map((block, index) => {
    const start = offset;
    offset += block.length;
    if (start <= visibleEnd) firstSafe = index + 1;
    return [rowKey(block[0]), { block, start }];
  }));
  let start = placed.length;
  let anchor = blocks.length;
  let changed = false;
  for (const block of ranked.reverse()) {
    start -= block.length;
    const existing = byKey.get(rowKey(block[0]));
    if (existing) anchor = blocks.indexOf(existing.block);
    const safeFrom = Math.max(0, visibleEnd + 1 - Math.min(start, existing?.start ?? start));
    if (!existing && safeFrom > 0) continue;
    if (!block.slice(safeFrom).some(row => row.row_kind === 0 && newIds.has(row.id))) continue;
    if (existing) {
      const keep = Math.max(0, visibleEnd + 1 - existing.start);
      const prefix = new Set(existing.block.slice(0, keep).map(rowKey));
      const local = new Map(existing.block.map(row => [rowKey(row), row]));
      const tail = block.filter((row, index) => !prefix.has(rowKey(row)) &&
        (row.row_kind !== 0 || local.has(rowKey(row)) || (index >= safeFrom && newIds.has(row.id))));
      existing.block.splice(keep, existing.block.length, ...tail.map(row => local.get(rowKey(row)) ?? row));
    } else {
      anchor = Math.max(firstSafe, anchor);
      blocks.splice(anchor, 0, block.filter(row => row.row_kind !== 0 || newIds.has(row.id)));
    }
    changed = true;
  }
  return changed ? { ...current, articles: blocks.flat() } : current;
}

/**
 * 「查看 N 篇新内容」：本次提示的新文章连同所属报道组按推荐先后放到最前，
 * 其余行保持当前列表的先后，排序上升的旧文章不借机挤到前面。已显示的行沿用本地的已读、收藏。
 */
export function promoteArrivals(current: FocusPage, latest: FocusPage, newIds: Set<number>): FocusPage {
  const local = new Map(current.articles.map(row => [rowKey(row), row]));
  const moved = focusBlocks(latest.articles)
    .filter(block => block.some(row => row.row_kind === 0 && newIds.has(row.id)))
    .map(block => block.map(row => {
      const shown = local.get(rowKey(row));
      return shown && row.row_kind === 0 ? { ...row, read: shown.read, starred: shown.starred } : row;
    }));
  if (!moved.length) return current;
  const movedKeys = new Set(moved.flat().map(rowKey));
  const rest = focusBlocks(current.articles)
    .filter(block => !movedKeys.has(rowKey(block[0])))
    .flatMap(block => block.filter(row => !movedKeys.has(rowKey(row))));
  return { ...current, articles: [...moved.flat(), ...rest] };
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
 * 后台更新（保持当前视口不动）：保持当前行的集合与先后，只换已显示行的内容；
 * 已读、收藏由本地写入维护，以当前行为准。
 */
export function refreshFocusRows(current: FocusPage, latest: FocusPage): FocusPage {
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
  return changed ? { ...current, articles } : current;
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
