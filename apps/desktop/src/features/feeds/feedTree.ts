import type { FeedCountsView, FeedTree, FeedView } from "../../lib/api";
import { i18n } from "../../i18n";

/** 订阅树的一行。`key` 就是后端认的范围文本：`all` / `folder:<id>` / `feed:<id>`。 */
export type TreeRow = {
  key: string;
  kind: "all" | "folder" | "feed";
  name: string;
  depth: number;
  count: number;
  parent: string | null;
  /** 有子项的文件夹才可展开。 */
  branch: boolean;
  open: boolean;
  feed?: FeedView;
};

type Folder = { id: number; name: string; parent: number | null; position: number; feeds: FeedView[] };

/** `feed_tree` 的文件夹按前序带缩进排列；还原父子关系。 */
function folderIndex(tree: FeedTree): Map<number, Folder> {
  const out = new Map<number, Folder>();
  const stack: { id: number; indent: number }[] = [];
  for (const folder of tree.folders) {
    while (stack.length && stack[stack.length - 1].indent >= folder.indent) stack.pop();
    out.set(folder.id, {
      id: folder.id, name: folder.name, parent: stack[stack.length - 1]?.id ?? null,
      position: folder.position, feeds: folder.feeds,
    });
    stack.push({ id: folder.id, indent: folder.indent });
  }
  return out;
}

type Child = { key: string; position: number; rank: number; id: number; folder?: Folder; feed?: FeedView };

/** 还没有文章的手动收录源不进订阅树（后端只给它标 `empty`）。 */
export function shownFeed(feed: FeedView): boolean {
  return !feed.empty;
}

/** 同一父级的子文件夹与源：按 position，相同则文件夹在前，再按 id。 */
function childrenOf(tree: FeedTree, folders: Map<number, Folder>, parent: number | null): Child[] {
  const slots: Child[] = [];
  for (const folder of folders.values()) {
    if (folder.parent === parent) slots.push({ key: `folder:${folder.id}`, position: folder.position, rank: 0, id: folder.id, folder });
  }
  const feeds = parent === null ? tree.uncategorized : folders.get(parent)?.feeds ?? [];
  for (const feed of feeds) {
    if (!shownFeed(feed)) continue;
    slots.push({ key: `feed:${feed.id}`, position: feed.position, rank: 1, id: feed.id, feed });
  }
  slots.sort((a, b) => a.position - b.position || a.rank - b.rank || a.id - b.id);
  return slots;
}

export function siblingKeys(tree: FeedTree, parent: number | null): string[] {
  return childrenOf(tree, folderIndex(tree), parent).map((slot) => slot.key);
}

export function feedCount(tree: FeedTree): number {
  return tree.folders.reduce((n, folder) => n + folder.feeds.length, tree.uncategorized.length);
}

export function findFeed(tree: FeedTree, id: number): FeedView | undefined {
  return [...tree.folders.flatMap((folder) => folder.feeds), ...tree.uncategorized].find((feed) => feed.id === id);
}

/** 选中项的祖先文件夹 id，进入订阅模式时据此展开到选中项。 */
export function ancestorFolders(tree: FeedTree, scope: string): number[] {
  const folders = folderIndex(tree);
  let start: number | null = null;
  const folder = /^folder:(\d+)$/.exec(scope);
  const feed = /^feed:(\d+)$/.exec(scope);
  if (folder) start = folders.get(Number(folder[1]))?.parent ?? null;
  else if (feed) start = findFeed(tree, Number(feed[1]))?.folder_id || null;
  const out: number[] = [];
  for (let id = start; id !== null && !out.includes(id); id = folders.get(id)?.parent ?? null) out.push(id);
  return out;
}

/** 范围的名称与上级路径，用于时间线标题。找不到时按全部订阅。 */
export function scopeTitle(tree: FeedTree | undefined, scope: string): { name: string; path: string[] } {
  if (!tree) return { name: i18n.t("scope.all", { ns: "feeds" }), path: [] };
  const folders = folderIndex(tree);
  const pathOf = (id: number | null) => {
    const out: string[] = [];
    for (let at = id; at !== null && folders.has(at) && out.length < folders.size; at = folders.get(at)!.parent) out.unshift(folders.get(at)!.name);
    return out;
  };
  const folder = /^folder:(\d+)$/.exec(scope);
  if (folder && folders.has(Number(folder[1]))) {
    const at = folders.get(Number(folder[1]))!;
    return { name: at.name, path: pathOf(at.parent) };
  }
  const feed = /^feed:(\d+)$/.exec(scope);
  const found = feed ? findFeed(tree, Number(feed[1])) : undefined;
  if (found) return { name: found.title, path: pathOf(found.folder_id || null) };
  return { name: i18n.t("scope.all", { ns: "feeds" }), path: [] };
}

/**
 * 可见行：首行「全部订阅」。同一父级的文件夹与源按 position 混排。
 * 有筛选词时：文件夹名命中保留整个子树，否则只保留含命中源的文件夹，并全部展开；不显示「全部订阅」。
 */
export function treeRows(tree: FeedTree, counts: FeedCountsView | null, open: ReadonlySet<number>, filter: string): TreeRow[] {
  const folders = folderIndex(tree);
  const feedCounts = new Map((counts?.feeds ?? []).map((row) => [row.id, row.unread]));
  const folderCounts = new Map((counts?.folders ?? []).map((row) => [row.id, row.unread]));
  const q = filter.trim().toLowerCase();
  const hit = (name: string) => name.toLowerCase().includes(q);
  const keeps = new Map<number, boolean>();
  const keep = (folder: Folder, seen = new Set<number>()): boolean => {
    if (keeps.has(folder.id)) return keeps.get(folder.id)!;
    if (seen.has(folder.id)) return false;
    seen.add(folder.id);
    const result = hit(folder.name) || folder.feeds.some((feed) => shownFeed(feed) && hit(feed.title))
      || childrenOf(tree, folders, folder.id).some((child) => child.folder && keep(child.folder, seen));
    keeps.set(folder.id, result);
    return result;
  };
  const out: TreeRow[] = q ? [] : [{ key: "all", kind: "all", name: i18n.t("scope.all", { ns: "feeds" }), depth: 0, count: counts?.total ?? 0, parent: null, branch: false, open: false }];
  const feedRow = (feed: FeedView, depth: number, parent: string | null): TreeRow =>
    ({ key: `feed:${feed.id}`, kind: "feed", name: feed.title, depth, count: feedCounts.get(feed.id) ?? 0, parent, branch: false, open: false, feed });
  const walk = (parent: number | null, depth: number, whole: boolean, seen: Set<number>) => {
    for (const slot of childrenOf(tree, folders, parent)) {
      if (slot.folder) {
        const folder = slot.folder;
        if (seen.has(folder.id) || (q && !whole && !keep(folder))) continue;
        const branch = childrenOf(tree, folders, folder.id).length > 0;
        const isOpen = q ? true : open.has(folder.id);
        const key = `folder:${folder.id}`;
        out.push({ key, kind: "folder", name: folder.name, depth, count: folderCounts.get(folder.id) ?? 0,
          parent: parent === null ? null : `folder:${parent}`, branch, open: branch && isOpen });
        if (!isOpen) continue;
        walk(folder.id, depth + 1, whole || (q !== "" && hit(folder.name)), new Set([...seen, folder.id]));
      } else if (slot.feed && (!q || whole || hit(slot.feed.title))) {
        out.push(feedRow(slot.feed, depth, parent === null ? null : `folder:${parent}`));
      }
    }
  };
  walk(null, 0, false, new Set());
  return out;
}

export type MovePlan = {
  items: string[];
  parentFolderId: number | null;
  before: string | null;
  name: string;
  where: string;
  /** 放进文件夹时展开它。 */
  expand?: number;
};

function parentOfKey(tree: FeedTree, key: string): number | null {
  if (key.startsWith("folder:")) return folderIndex(tree).get(Number(key.slice(7)))?.parent ?? null;
  const feed = key.startsWith("feed:") ? findFeed(tree, Number(key.slice(5))) : undefined;
  return feed && feed.folder_id > 0 ? feed.folder_id : null;
}

export function itemNameOf(tree: FeedTree, key: string): string | null {
  return itemName(tree, key);
}

function itemName(tree: FeedTree, key: string): string | null {
  if (key.startsWith("folder:")) return folderIndex(tree).get(Number(key.slice(7)))?.name ?? null;
  return key.startsWith("feed:") ? findFeed(tree, Number(key.slice(5)))?.title ?? null : null;
}

/** 目标父级是被移动文件夹自身或子孙。 */
function blocksParent(tree: FeedTree, key: string, parent: number | null): boolean {
  if (parent === null || !key.startsWith("folder:")) return false;
  const moving = Number(key.slice(7));
  const folders = folderIndex(tree);
  for (let at: number | null = parent, hops = 0; at !== null && hops <= folders.size; at = folders.get(at)?.parent ?? null, hops++) {
    if (at === moving) return true;
  }
  return false;
}

function whereParent(tree: FeedTree, parent: number | null): string {
  if (parent === null) return i18n.t("move.where.root", { ns: "feeds" });
  return i18n.t("move.where.folder", { ns: "feeds", name: folderIndex(tree).get(parent)?.name ?? "" });
}

function keysOf(key: string | string[]): string[] {
  return Array.isArray(key) ? key : [key];
}

/** 多个项按当前树的前序排列，拖动和批量移动都按这个顺序。 */
export function orderedKeys(tree: FeedTree, keys: string[]): string[] {
  const rank = new Map<string, number>();
  const walk = (parent: number | null, cursor: { n: number }) => {
    for (const item of siblingKeys(tree, parent)) {
      rank.set(item, cursor.n++);
      if (item.startsWith("folder:")) walk(Number(item.slice(7)), cursor);
    }
  };
  walk(null, { n: 0 });
  return [...new Set(keys)].filter((item) => itemName(tree, item)).sort((a, b) => (rank.get(a) ?? 0) - (rank.get(b) ?? 0));
}

function plan(tree: FeedTree, key: string | string[], parent: number | null, before: string | null, where: string, expand?: number): MovePlan | null {
  const items = orderedKeys(tree, keysOf(key));
  if (!items.length || items.some((item) => blocksParent(tree, item, parent)) || (before != null && items.includes(before))) return null;
  const name = itemName(tree, items[0]);
  if (!name) return null;
  return { items, parentFolderId: parent, before, name, where, ...(expand === undefined ? {} : { expand }) };
}

/** 上移 = 放到前一个兄弟之前；下移 = 放到后一个兄弟的后一个之前，没有则末尾。 */
export function planShift(tree: FeedTree, key: string, dir: -1 | 1): MovePlan | null {
  const parent = parentOfKey(tree, key);
  const sibs = siblingKeys(tree, parent);
  const index = sibs.indexOf(key);
  if (index < 0) return null;
  if (dir < 0) return index === 0 ? null : plan(tree, key, parent, sibs[index - 1], whereParent(tree, parent));
  if (index >= sibs.length - 1) return null;
  return plan(tree, key, parent, sibs[index + 2] ?? null, whereParent(tree, parent));
}

/** 移到根层最前。已经是根层第一项时无处可移。 */
export function planToTop(tree: FeedTree, key: string): MovePlan | null {
  const root = siblingKeys(tree, null);
  if (parentOfKey(tree, key) === null && root[0] === key) return null;
  const before = root[0] === key ? root[1] ?? null : root[0] ?? null;
  return plan(tree, key, null, before, i18n.t("move.where.top", { ns: "feeds" }));
}

/** `folderId` 为空：不放文件夹，放到根层最前。 */
export function planIntoFolder(tree: FeedTree, key: string | string[], folderId: number | null): MovePlan | null {
  const moving = new Set(keysOf(key));
  if (folderId === null) {
    const before = siblingKeys(tree, null).find((item) => !moving.has(item)) ?? null;
    return plan(tree, key, null, before, i18n.t("move.where.rootTop", { ns: "feeds" }));
  }
  const name = folderIndex(tree).get(folderId)?.name;
  if (!name) return null;
  return plan(tree, key, folderId, null, i18n.t("move.where.folder", { ns: "feeds", name }), folderId);
}

export function planDrop(tree: FeedTree, key: string | string[], row: TreeRow, zone: "before" | "after" | "into"): MovePlan | null {
  const moving = new Set(keysOf(key));
  if (row.kind === "all" || moving.has(row.key)) return null;
  if (zone === "into") return row.kind === "folder" ? planIntoFolder(tree, key, Number(row.key.slice(7))) : null;
  const parent = row.parent ? Number(row.parent.slice(7)) : null;
  const sibs = siblingKeys(tree, parent);
  const index = sibs.indexOf(row.key);
  const before = zone === "before" ? row.key : sibs.slice(index + 1).find((item) => !moving.has(item)) ?? null;
  return plan(tree, key, parent, before, whereParent(tree, parent));
}

/** 移到文件夹的二级列表：全部文件夹按缩进，排除自身与子孙。 */
export function folderMoveTargets(tree: FeedTree, key: string | string[]): { id: number; name: string; depth: number }[] {
  const blocked = new Set<number>();
  const folders = folderIndex(tree);
  const walk = (id: number) => {
    if (blocked.has(id)) return;
    blocked.add(id);
    for (const folder of folders.values()) if (folder.parent === id) walk(folder.id);
  };
  for (const item of keysOf(key)) if (item.startsWith("folder:")) walk(Number(item.slice(7)));
  return tree.folders.filter((folder) => !blocked.has(folder.id)).map((folder) => ({ id: folder.id, name: folder.name, depth: folder.indent }));
}

export type PickerFeed = { id: number; title: string; locked: boolean; loose: boolean };
export type PickerGroup = { label: string; feeds: PickerFeed[] };

/** 「放进订阅」按当前位置分组。已在目标文件夹（含子文件夹）里的源锁定。 */
export function pickerGroups(tree: FeedTree, folderId: number, query: string, looseOnly: boolean): PickerGroup[] {
  const inside = new Set(feedsInScope(tree, `folder:${folderId}`).map((feed) => feed.id));
  const q = query.trim().toLowerCase();
  const groups = new Map<string, PickerFeed[]>();
  const all = [...tree.uncategorized, ...tree.folders.flatMap((folder) => folder.feeds)].filter(shownFeed);
  for (const feed of all) {
    if (q && !feed.title.toLowerCase().includes(q)) continue;
    const loose = feed.folder_id <= 0;
    if (looseOnly && !loose) continue;
    const path = scopeTitle(tree, `feed:${feed.id}`).path;
    const label = loose || path.length === 0 ? i18n.t("picker.loose", { ns: "feeds" }) : path.join(" › ");
    const list = groups.get(label) ?? [];
    list.push({ id: feed.id, title: feed.title, locked: inside.has(feed.id), loose });
    groups.set(label, list);
  }
  return [...groups].map(([label, feeds]) => ({ label, feeds }));
}

/** 设置里的 `folder:<id>` / `feed:<id>` 列表，丢掉树里已经没有的项。 */
export function liveKeys(raw: string, tree: FeedTree, foldersOnly: boolean, limit: number): string[] {
  const out: string[] = [];
  for (const part of raw.split(",")) {
    const key = part.trim();
    if (!/^(folder|feed):\d+$/.test(key) || (foldersOnly && !key.startsWith("folder:"))) continue;
    if (out.includes(key) || !itemName(tree, key)) continue;
    out.push(key);
    if (out.length >= limit) break;
  }
  return out;
}

/** 范围内的源（文件夹含子孙）。 */
export function feedsInScope(tree: FeedTree, scope: string): FeedView[] {
  const all = [...tree.folders.flatMap((folder) => folder.feeds), ...tree.uncategorized];
  const feed = /^feed:(\d+)$/.exec(scope);
  if (feed) return all.filter((item) => item.id === Number(feed[1]));
  const folder = /^folder:(\d+)$/.exec(scope);
  if (!folder) return all;
  const folders = folderIndex(tree);
  const under = (id: number) => {
    for (let at: number | null = id, hops = 0; at !== null && hops <= folders.size; at = folders.get(at)?.parent ?? null, hops++) {
      if (at === Number(folder[1])) return true;
    }
    return false;
  };
  return all.filter((item) => item.folder_id > 0 && under(item.folder_id));
}

/** 范围的可见未读数，与侧栏同源。 */
export function scopeUnread(counts: FeedCountsView | null, scope: string): number {
  if (!counts) return 0;
  if (scope === "all") return counts.total;
  const [kind, id] = scope.split(":");
  const rows = kind === "folder" ? counts.folders : kind === "feed" ? counts.feeds : [];
  return rows.find((row) => row.id === Number(id))?.unread ?? 0;
}

