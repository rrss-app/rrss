import type { FeedTree, FeedView, TreeLayout } from "../bindings";

export type FeedTreeOrderArgs = {
  items?: string[];
  parentFolderId?: number | null;
  before?: string | null;
  layout?: TreeLayout[];
};

type FlatFolder = { id: number; name: string; label: string; parent: number | null; position: number };
type Model = { folders: Map<number, FlatFolder>; feeds: FeedView[]; combo: string[] };
type Parsed = { kind: "folder" | "feed"; id: number; key: string };

/**
 * 与 `rrss_core::tree_order` 相同的移动与撤销。
 * 函数体不引用外部绑定，`toString()` 后可在模拟宿主里执行。
 */
export function applyFeedTreeOrder(tree: FeedTree, op: "move" | "restore", args: FeedTreeOrderArgs): { tree: FeedTree; layout: TreeLayout[] } {
  function parseKey(raw: string): Parsed {
    const [kind, id] = String(raw).split(":");
    return { kind: kind === "folder" ? "folder" : "feed", id: Number(id), key: `${kind}:${Number(id)}` };
  }
  function index(source: FeedTree): Model {
    const folders = new Map<number, FlatFolder>();
    const stack: { id: number; indent: number }[] = [];
    for (const folder of source.folders) {
      while (stack.length && stack[stack.length - 1].indent >= folder.indent) stack.pop();
      folders.set(folder.id, {
        id: folder.id, name: folder.name, label: folder.label,
        parent: stack.length ? stack[stack.length - 1].id : null, position: folder.position,
      });
      stack.push({ id: folder.id, indent: folder.indent });
    }
    const feeds: FeedView[] = [];
    for (const folder of source.folders) for (const feed of folder.feeds) feeds.push({ ...feed });
    for (const feed of source.uncategorized) feeds.push({ ...feed });
    return { folders, feeds, combo: source.folder_combo.slice() };
  }
  /** 文件夹与源的父级、顺序读写合成一处；找不到返回 null（撤销时跳过已删除的项）。 */
  function node(model: Model, key: string) {
    const item = parseKey(key);
    const folder = item.kind === "folder" ? model.folders.get(item.id) : undefined;
    const feed = item.kind === "feed" ? model.feeds.find((row) => row.id === item.id) : undefined;
    if (!folder && !feed) return null;
    return {
      parent: (): number | null => folder ? folder.parent : feed!.folder_id > 0 ? feed!.folder_id : null,
      place(parent: number | null, position?: number) {
        if (folder) { folder.parent = parent; if (position !== undefined) folder.position = position; }
        else { feed!.folder_id = parent ?? 0; if (position !== undefined) feed!.position = position; }
      },
      position(value: number) { if (folder) folder.position = value; else feed!.position = value; },
    };
  }
  function childKeys(model: Model, parent: number | null): { key: string; position: number }[] {
    const rows: { key: string; rank: number; id: number; position: number }[] = [];
    for (const folder of model.folders.values()) {
      if (folder.parent === parent) rows.push({ key: `folder:${folder.id}`, rank: 0, id: folder.id, position: folder.position });
    }
    for (const feed of model.feeds) {
      if ((feed.folder_id || 0) === (parent ?? 0)) rows.push({ key: `feed:${feed.id}`, rank: 1, id: feed.id, position: feed.position });
    }
    return rows.sort((a, b) => a.position - b.position || a.rank - b.rank || a.id - b.id);
  }
  function rebuild(model: Model): FeedTree {
    const byPosition = <T extends { position: number; id: number }>(a: T, b: T) => a.position - b.position || a.id - b.id;
    const folders: FeedTree["folders"] = [];
    const walk = (parent: number | null, indent: number) => {
      for (const folder of [...model.folders.values()].filter((item) => item.parent === parent).sort(byPosition)) {
        const feeds = model.feeds.filter((feed) => feed.folder_id === folder.id).sort(byPosition);
        folders.push({ id: folder.id, name: folder.name, label: folder.label, indent, position: folder.position, feeds });
        walk(folder.id, indent + 1);
      }
    };
    walk(null, 0);
    const uncategorized = model.feeds.filter((feed) => !feed.folder_id).sort(byPosition);
    return { folders, uncategorized, folder_combo: model.combo };
  }
  /** 只做移动本身：越界的请求前端在发出前已排除，这里不复刻后端的校验。 */
  function move(model: Model, input: FeedTreeOrderArgs): TreeLayout[] {
    const moved = (input.items ?? []).map((raw) => parseKey(raw).key);
    const parent = input.parentFolderId ?? null;
    const before = input.before ? parseKey(input.before).key : null;
    const parents: (number | null)[] = [parent];
    for (const key of moved) {
      const origin = node(model, key)?.parent() ?? null;
      if (!parents.includes(origin)) parents.push(origin);
    }
    const layout = parents.flatMap((group) => childKeys(model, group).map((row) => ({ item: row.key, parent_folder_id: group, position: row.position })));
    for (const key of moved) node(model, key)?.place(parent);
    for (const group of parents) {
      const order = childKeys(model, group).map((row) => row.key).filter((key) => !moved.includes(key));
      if (group === parent) order.splice(before == null ? order.length : order.indexOf(before), 0, ...moved);
      order.forEach((key, position) => node(model, key)?.position(position));
    }
    return layout;
  }

  const model = index(tree);
  if (op === "restore") {
    for (const slot of args.layout ?? []) node(model, slot.item)?.place(slot.parent_folder_id, slot.position);
    return { tree: rebuild(model), layout: [] };
  }
  const layout = move(model, args);
  return { tree: rebuild(model), layout };
}
