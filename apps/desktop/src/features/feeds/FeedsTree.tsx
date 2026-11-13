import { useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type MouseEvent as ReactMouseEvent } from "react";
import { createPortal } from "react-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type FeedCountsView, type FeedTree, type TreeLayout } from "../../lib/api";
import { feedKeys } from "../../lib/feedQuery";
import { formatNotice } from "../../lib/notice";
import { useTranslation } from "../../i18n";
import { Icon } from "../../components/Icon";
import { FloatingMenu, type MenuAnchor } from "../../components/FloatingMenu";
import { SourceAvatar } from "../../components/ArticleCard";
import { settingsMutationOptions, settingsSchemaOptions } from "../../settings/settingsQuery";
import { FeedPicker } from "./FeedPicker";
import { useTreeDrag, type DragItem, type DropTarget } from "./useTreeDrag";
import {
  ancestorFolders, folderMoveTargets, itemNameOf, liveKeys, orderedKeys, planDrop, planIntoFolder, planShift, planToTop, treeRows,
  type MovePlan, type TreeRow,
} from "./feedTree";

export type TreeNotice = { message: string; holdMs?: number; actionLabel?: string; onAction?: () => void };

type RowMenu = { key: string; mode: "actions" | "move" | "add" | "bulk"; anchor: MenuAnchor };
type Draft = { parent: number | null; picked: string[] | null };
type Picker = { folderId: number; name: string; fresh: boolean };

function Highlight({ text, query }: { text: string; query: string }) {
  const q = query.trim();
  const at = q ? text.toLowerCase().indexOf(q.toLowerCase()) : -1;
  if (at < 0) return <>{text}</>;
  return <>{text.slice(0, at)}<mark>{text.slice(at, at + q.length)}</mark>{text.slice(at + q.length)}</>;
}

/** 订阅树（`role="tree"`）。展开状态只在本次进入订阅模式内有效。 */
export function FeedsTree({ tree, counts, scope, onSelect, onAddFeed, onSettings, notify }: {
  tree: FeedTree;
  counts: FeedCountsView | null;
  scope: string;
  onSelect: (scope: string) => void;
  onAddFeed: () => void;
  onSettings: () => void;
  notify: (notice: TreeNotice) => void;
}) {
  const { t } = useTranslation("feeds");
  const client = useQueryClient();
  const [open, setOpen] = useState<ReadonlySet<number>>(() => new Set(ancestorFolders(tree, scope)));
  const [filter, setFilter] = useState("");
  const [cursor, setCursor] = useState(scope);
  const [menu, setMenu] = useState<RowMenu | null>(null);
  const [editing, setEditingState] = useState<string | null>(null);
  // 回车、Esc 与失焦都会结束改名；移除输入框时可能再补发一次失焦，只认第一次。
  const editingRef = useRef<string | null>(null);
  const setEditing = (key: string | null) => { editingRef.current = key; setEditingState(key); };
  const [picking, setPicking] = useState(false);
  const [picked, setPicked] = useState<ReadonlySet<string>>(() => new Set());
  const [draft, setDraftState] = useState<Draft | null>(null);
  const [picker, setPicker] = useState<Picker | null>(null);
  const draftRef = useRef<Draft | null>(null);
  const anchorRef = useRef<string | null>(null);
  const setDraft = (value: Draft | null) => { draftRef.current = value; setDraftState(value); };
  const schema = useQuery(settingsSchemaOptions());
  const saveSetting = useMutation(settingsMutationOptions(client));
  const pinned = useMemo(() => liveKeys(schema.data?.values.find((item) => item.id === "feeds_pinned")?.value ?? "", tree, false, 10), [schema.data, tree]);
  const columns = useMemo(() => liveKeys(schema.data?.values.find((item) => item.id === "focus_columns")?.value ?? "", tree, true, 5), [schema.data, tree]);
  const rowRefs = useRef(new Map<string, HTMLDivElement>());
  const pendingFocus = useRef<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  // 树写入按发起顺序提交：宿主命令可能落到不同阻塞线程，并发提交时先发的写入可能后落库。
  const treeWrites = useRef({ tail: Promise.resolve(), pending: 0 });

  useEffect(() => {
    const up = ancestorFolders(tree, scope);
    setOpen((prev) => up.every((id) => prev.has(id)) ? prev : new Set([...prev, ...up]));
  }, [tree, scope]);
  useEffect(() => { setCursor(scope); }, [scope]);

  const rows = useMemo(() => treeRows(tree, counts, open, filter), [tree, counts, open, filter]);
  const active = rows.some((row) => row.key === cursor) ? cursor : rows.find((row) => row.key === scope)?.key ?? rows[0]?.key;

  useLayoutEffect(() => {
    const key = pendingFocus.current;
    if (key === null) return;
    pendingFocus.current = null;
    const el = rowRefs.current.get(key);
    el?.focus();
    el?.scrollIntoView?.({ block: "nearest" });
  });

  const moveTo = (row: TreeRow | undefined) => {
    if (!row) return;
    setCursor(row.key);
    pendingFocus.current = row.key;
    if (row.key === active) rowRefs.current.get(row.key)?.focus();
  };
  const toggle = (row: TreeRow, next: boolean) => {
    const id = Number(row.key.slice("folder:".length));
    setOpen((prev) => {
      const out = new Set(prev);
      if (next) out.add(id); else out.delete(id);
      return out;
    });
    setCursor(row.key);
    pendingFocus.current = row.key;
  };

  const refreshTree = () => client.invalidateQueries({ queryKey: feedKeys.tree });
  /** 写入连同之后的回读排在前一次树写入之后；没有在途写入时立即开始。 */
  const serialWrite = <T,>(work: () => Promise<T>): Promise<T> => {
    const writes = treeWrites.current;
    const run = writes.pending ? writes.tail.then(work) : work();
    const done = run.then(() => undefined, () => undefined);
    writes.pending += 1;
    writes.tail = done;
    void done.then(() => { writes.pending -= 1; });
    return run;
  };
  // 撤销与改名写的是绝对值，排队执行；移动计划按当前显示的树计算，有树写入在途时放弃，除非计划与位置无关（新建文件夹后移入）。
  const runMove = async (plan: MovePlan | null, queue = false) => {
    if (!plan || (!queue && treeWrites.current.pending)) return;
    setMenu(null);
    try {
      const layout = await serialWrite(async () => {
        const layout = await api.moveTreeItems(plan.items, plan.parentFolderId, plan.before);
        if (plan.expand != null) setOpen((prev) => new Set(prev).add(plan.expand!));
        await refreshTree();
        return layout;
      });
      notify({
        message: plan.items.length > 1 ? t("move.doneMany", { count: plan.items.length, where: plan.where }) : t("move.doneOne", { name: plan.name, where: plan.where }),
        holdMs: 4000,
        actionLabel: t("move.undo"),
        onAction: () => { void undo(layout); },
      });
      exitPick();
    } catch (err) {
      notify({ message: formatNotice(t("tree.error.move"), err) });
    }
  };
  const exitPick = () => { setPicking(false); setPicked(new Set()); anchorRef.current = null; };
  const writeSetting = async (id: "feeds_pinned" | "focus_columns", value: string) => {
    const result = await saveSetting.mutateAsync([{ id, value }]);
    if (result.failure) throw result.failure.error;
  };
  const finishDraft = async (raw: string) => {
    const current = draftRef.current;
    if (!current) return;
    draftRef.current = null;
    setDraftState(null);
    const name = raw.trim();
    if (!name) return;
    try {
      const id = await serialWrite(async () => {
        const id = await api.createFolder(name, current.parent);
        setOpen((prev) => new Set([...prev, id, ...(current.parent == null ? [] : [current.parent])]));
        await refreshTree();
        return id;
      });
      onSelect(`folder:${id}`);
      if (current.picked?.length) {
        const items = orderedKeys(tree, current.picked);
        exitPick();
        await runMove({ items, parentFolderId: id, before: null, name: itemNameOf(tree, items[0]) ?? name, where: t("move.where.folder", { name }), expand: id }, true);
      } else setPicker({ folderId: id, name, fresh: true });
    } catch (err) {
      notify({ message: formatNotice(t("tree.error.newFolder"), err) });
    }
  };
  const undo = async (layout: TreeLayout[]) => {
    try {
      await serialWrite(async () => {
        await api.restoreTreeLayout(layout);
        await refreshTree();
      });
    } catch (err) {
      notify({ message: formatNotice(t("tree.error.undo"), err) });
    }
  };
  const reveal = (key: string) => {
    setMenu(null);
    setOpen((prev) => new Set([...prev, ...ancestorFolders(tree, key)]));
    setCursor(key);
    pendingFocus.current = key;
    onSelect(key);
  };
  const togglePin = async (key: string) => {
    setMenu(null);
    try {
      if (pinned.includes(key)) await writeSetting("feeds_pinned", pinned.filter((item) => item !== key).join(","));
      else if (pinned.length >= 10) notify({ message: t("tree.limit.pinned") });
      else await writeSetting("feeds_pinned", [...pinned, key].join(","));
    } catch (err) { notify({ message: formatNotice(t("tree.error.pin"), err) }); }
  };
  const setColumn = async (key: string) => {
    setMenu(null);
    const on = columns.includes(key);
    if (!on && columns.length >= 5) { notify({ message: t("tree.limit.columns") }); return; }
    try { await writeSetting("focus_columns", (on ? columns.filter((item) => item !== key) : [...columns, key]).join(",")); }
    catch (err) { notify({ message: formatNotice(t("tree.error.column"), err) }); }
  };
  const commitRename = async (row: TreeRow, raw: string) => {
    if (editingRef.current !== row.key) return;
    const name = raw.trim();
    setEditing(null);
    if (!name || name === row.name) return;
    try {
      await serialWrite(async () => {
        if (row.kind === "folder") await api.renameFolder(Number(row.key.slice(7)), name);
        else await api.saveFeed({ id: Number(row.key.slice(5)), title: name, url: null, folder_id: null, retention: null });
        await refreshTree();
      });
    } catch (err) {
      notify({ message: formatNotice(t("tree.error.rename"), err) });
    }
  };

  const dropPin = async (item: DragItem, target: DropTarget) => {
    if (target.zone === "into") return;
    const moving = item.fromPin ? [item.key.slice(4)] : item.items;
    if (moving.includes(target.targetKey)) return;
    const rest = pinned.filter((key) => !moving.includes(key));
    if (rest.length + moving.length > 10) { notify({ message: t("tree.limit.pinned") }); return; }
    let index = rest.indexOf(target.targetKey);
    if (index < 0) index = rest.length;
    if (target.zone === "after") index += 1;
    try { await writeSetting("feeds_pinned", [...rest.slice(0, index), ...moving, ...rest.slice(index)].slice(0, 10).join(",")); }
    catch (err) { notify({ message: formatNotice(t("tree.error.pin"), err) }); }
  };
  const openMenu = (key: string, anchor: MenuAnchor, mode: RowMenu["mode"] = "actions") => setMenu({ key, mode, anchor });
  const pickedItems = (key: string) => picking && picked.has(key) && picked.size > 1 ? orderedKeys(tree, [...picked]) : [key];
  const { drag, dragMoved } = useTreeDrag(scrollRef, {
    pick: (row) => {
      if (!row.dataset.key || row.dataset.key === "all") return null;
      const fromPin = row.dataset.zone === "pin";
      const real = fromPin ? row.dataset.target ?? "" : row.dataset.key;
      return { key: row.dataset.key, name: row.querySelector(".feeds-row-name")?.textContent ?? "", items: fromPin ? [real] : pickedItems(real), fromPin };
    },
    target: (el, y, item) => {
      const rect = el.getBoundingClientRect();
      const rel = rect.height ? (y - rect.top) / rect.height : 0.5;
      // 置顶区内只排前后；树里的行可以拖进置顶区，捷径不能拖回树里。
      if (el.dataset.zone === "pin") {
        const key = el.dataset.target;
        return key && !item.items.includes(key) ? { targetKey: key, zone: rel < 0.5 ? "before" : "after", pinDrop: true } : null;
      }
      const row = item.fromPin ? undefined : rows.find((entry) => entry.key === el.dataset.key);
      if (!row || row.kind === "all") return null;
      const zone = row.kind === "folder" && rel > 0.25 && rel < 0.75 ? "into" : rel < 0.5 ? "before" : "after";
      return planDrop(tree, item.items, row, zone) ? { targetKey: row.key, zone, pinDrop: false } : null;
    },
    drop: (item, target) => {
      if (target.pinDrop) { void dropPin(item, target); return; }
      const row = rows.find((entry) => entry.key === target.targetKey);
      const plan = row ? planDrop(tree, item.items, row, target.zone) : null;
      if (plan) void runMove(plan);
    },
    start: () => setMenu(null),
    contextMenu: (key, anchor) => openMenu(key, anchor),
  });

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    if (target.closest("button, input")) return;
    const key = target.closest<HTMLElement>("[role=treeitem]")?.dataset.key;
    const index = rows.findIndex((row) => row.key === key);
    const row = rows[index];
    if (!row) return;
    if (event.altKey && (event.key === "ArrowUp" || event.key === "ArrowDown") && row.kind !== "all") {
      event.preventDefault();
      void runMove(planShift(tree, row.key, event.key === "ArrowUp" ? -1 : 1));
      return;
    }
    const handled = () => event.preventDefault();
    switch (event.key) {
      case "ArrowDown": handled(); moveTo(rows[Math.min(index + 1, rows.length - 1)]); break;
      case "ArrowUp": handled(); if (index === 0 && inputRef.current) inputRef.current.focus(); else moveTo(rows[Math.max(index - 1, 0)]); break;
      case "Home": handled(); moveTo(rows[0]); break;
      case "End": handled(); moveTo(rows[rows.length - 1]); break;
      case "ArrowRight":
        handled();
        if (row.branch && !row.open) toggle(row, true);
        else if (row.branch) moveTo(rows[index + 1]);
        break;
      case "ArrowLeft":
        handled();
        if (row.branch && row.open && !filter.trim()) toggle(row, false);
        else moveTo(rows.find((item) => item.key === row.parent));
        break;
      case "Enter":
      case " ":
        handled(); setCursor(row.key); onSelect(row.key); break;
    }
  };

  useEffect(() => {
    if (!picking && !picker) return;
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key !== "Escape" || event.isComposing) return;
      if (event.target instanceof HTMLElement && event.target.classList.contains("feeds-rename")) return;
      event.preventDefault();
      event.stopPropagation();
      if (picker) setPicker(null);
      else { setPicking(false); setPicked(new Set()); anchorRef.current = null; }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [picking, picker]);
  const showPins = pinned.length > 0 && !filter.trim();
  const draftAt = (() => {
    if (!draft) return -1;
    if (draft.parent == null) {
      const at = rows.findIndex((row) => row.kind !== "all");
      return at < 0 ? rows.length : at;
    }
    const at = rows.findIndex((row) => row.key === `folder:${draft.parent}`);
    return at < 0 ? -1 : at + 1;
  })();
  const draftDepth = draft?.parent == null ? 0 : (rows.find((row) => row.key === `folder:${draft.parent}`)?.depth ?? 0) + 1;
  const menuRow = menu && !menu.key.startsWith("pin:") && menu.key !== "add" && menu.key !== "bulk" ? rows.find((row) => row.key === menu.key) : undefined;
  const menuKeys = menu?.mode === "bulk" ? [...picked] : menu ? [menu.key] : [];
  const pickRow = (row: TreeRow, event: ReactMouseEvent) => {
    const visible = rows.filter((item) => item.kind !== "all").map((item) => item.key);
    setPicking(true);
    if (event.shiftKey) {
      const anchor = anchorRef.current ?? row.key;
      const a = visible.indexOf(anchor);
      const b = visible.indexOf(row.key);
      const [lo, hi] = a < b ? [a, b] : [b, a];
      setPicked((prev) => {
        const next = new Set(prev);
        if (a < 0 || b < 0) next.add(row.key); else for (const item of visible.slice(lo, hi + 1)) next.add(item);
        return next;
      });
      return;
    }
    setPicked((prev) => {
      const next = new Set(picking || event.ctrlKey || event.metaKey ? prev : []);
      if (next.has(row.key)) next.delete(row.key); else next.add(row.key);
      return next;
    });
    anchorRef.current = row.key;
  };
  const editor = draft && draftAt >= 0 ? <div key="draft" className="feeds-row" style={{ "--depth": draftDepth } as CSSProperties}>
    <span className="feeds-twist" data-leaf="1" aria-hidden="true"><Icon name="chevronDown" size={14} /></span>
    <Icon name="folder" size={16} />
    <input className="feeds-rename" aria-label={t("tree.newFolderLabel")} placeholder={t("tree.folderName")} autoFocus
      onClick={(event) => event.stopPropagation()}
      onBlur={(event) => void finishDraft(event.currentTarget.value)}
      onKeyDown={(event) => {
        if (event.key === "Enter") { event.preventDefault(); event.stopPropagation(); void finishDraft(event.currentTarget.value); }
        if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); setDraft(null); }
      }} />
  </div> : null;
  return <>
    <div className="feeds-tools">
      <label className="feeds-filter">
        <Icon name="search" size={14} />
        <input ref={inputRef} value={filter} placeholder={t("tree.filterPlaceholder")} aria-label={t("tree.filterLabel")} autoComplete="off"
          onChange={(event) => { setFilter(event.target.value); setCursor(""); }}
          onKeyDown={(event) => {
            if (event.key === "ArrowDown" && rows.length) { event.preventDefault(); moveTo(rows[0]); }
            if (event.key === "Escape" && filter) { event.preventDefault(); setFilter(""); }
          }} />
      </label>
      <button type="button" className="feeds-add" aria-label={t("tree.menu.add")} aria-haspopup="menu" aria-expanded={menu?.key === "add"}
        onClick={(event) => {
          const rect = event.currentTarget.getBoundingClientRect();
          openMenu("add", { x: rect.left, y: rect.bottom + 4 }, "add");
        }}><Icon name="plus" size={16} /></button>
    </div>
    <div className="feeds-tree-scroll" ref={scrollRef} data-dragging={drag ? "1" : undefined}>
      {drag?.line ? <div className="feeds-drop-line" style={{ top: drag.line.top, left: drag.line.left }} /> : null}
      {showPins ? <div className="feeds-sec">{t("tree.secPinned")}</div> : null}
      {showPins ? pinned.map((key) => {
        const name = itemNameOf(tree, key) ?? "";
        const count = key.startsWith("folder:")
          ? counts?.folders.find((item) => item.id === Number(key.slice(7)))?.unread ?? 0
          : counts?.feeds.find((item) => item.id === Number(key.slice(5)))?.unread ?? 0;
        return <div key={`pin:${key}`} className="feeds-row" data-key={`pin:${key}`} data-target={key} data-zone="pin" data-kind={key.startsWith("folder:") ? "folder" : "feed"}
          role="button" tabIndex={-1} aria-selected={scope === key} data-dragging={drag?.key === `pin:${key}` ? "1" : undefined}
          onClick={() => { if (dragMoved.current) { dragMoved.current = false; return; } onSelect(key); }}>
          <span className="feeds-twist" data-leaf="1" aria-hidden="true"><Icon name="chevronDown" size={14} /></span>
          {key.startsWith("feed:") ? <SourceAvatar title={name} /> : <Icon name="folder" size={16} />}
          <span className="feeds-row-name">{name}</span>
          <Icon name="pin" size={13} />
          <span className="feeds-row-count">{count || ""}</span>
          <button type="button" className="feeds-row-more" aria-label={t("tree.rowActions", { name })} aria-haspopup="menu"
            onClick={(event) => { event.stopPropagation(); const rect = event.currentTarget.getBoundingClientRect(); openMenu(`pin:${key}`, { x: rect.left, y: rect.bottom + 4 }); }}>⋯</button>
        </div>;
      }) : null}
      {showPins ? <div className="feeds-sec">{t("tree.secAll")}</div> : null}
      {rows.length === 0 ? <div className="feeds-tree-empty">{t("tree.filterEmpty", { query: filter.trim() })}</div>
        : <div className="feeds-tree" role="tree" aria-label={t("tree.label")} onKeyDown={onKeyDown}>
          {rows.map((row, index) => {
            const selected = row.key === scope;
            const renaming = editing === row.key;
            const node = <div key={row.key} ref={(el) => { if (el) rowRefs.current.set(row.key, el); else rowRefs.current.delete(row.key); }}
              role="treeitem" className="feeds-row" data-key={row.key} data-kind={row.kind} aria-level={row.depth + 1} aria-selected={selected}
              aria-checked={picking && row.kind !== "all" ? picked.has(row.key) : undefined}
              aria-expanded={row.branch ? row.open : undefined} tabIndex={row.key === active ? 0 : -1}
              data-noisy={row.feed?.noisy ? "1" : undefined} data-empty={row.kind !== "all" && row.count === 0 ? "1" : undefined}
              data-picked={picking && picked.has(row.key) ? "1" : undefined}
              data-dragging={drag?.items.includes(row.key) ? "1" : undefined}
              data-drop={drag?.targetKey === row.key && drag.zone === "into" ? "into" : undefined}
              style={{ "--depth": row.depth } as CSSProperties}
              onClick={(event) => {
                if (dragMoved.current) { dragMoved.current = false; return; }
                if (row.kind !== "all" && (picking || event.metaKey || event.ctrlKey || event.shiftKey)) { pickRow(row, event); return; }
                setCursor(row.key); onSelect(row.key);
              }}>
              {picking && row.kind !== "all" ? <span className="feeds-check" data-on={picked.has(row.key) ? "1" : "0"} aria-hidden="true" />
                : <span className="feeds-twist" data-leaf={row.branch ? undefined : "1"} aria-hidden="true"
                  onClick={row.branch ? (event) => { event.stopPropagation(); toggle(row, !row.open); } : undefined}>
                  <Icon name="chevronDown" size={14} />
                </span>}
              {row.kind === "feed" ? <SourceAvatar title={row.name} iconHref={row.feed?.icon_href} />
                : <Icon name={row.kind === "all" ? "rss" : "folder"} size={16} />}
              {renaming ? <input className="feeds-rename" aria-label={t("tree.renameLabel", { name: row.name })} defaultValue={row.name} autoFocus
                onClick={(event) => event.stopPropagation()}
                onFocus={(event) => event.currentTarget.select()}
                onBlur={(event) => void commitRename(row, event.currentTarget.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") { event.preventDefault(); event.stopPropagation(); void commitRename(row, event.currentTarget.value); }
                  if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); setEditing(null); }
                }} />
                : <span className="feeds-row-name"><Highlight text={row.name} query={filter} /></span>}
              {row.feed?.error ? <span className="feeds-row-flag" title={row.feed.error} aria-label={t("tree.fetchError", { error: row.feed.error })} /> : null}
              <span className="feeds-row-count">{row.kind === "folder" && row.open ? "" : row.count || ""}</span>
              {row.kind === "all" || renaming ? null : <button type="button" className="feeds-row-more" aria-label={t("tree.rowActions", { name: row.name })} aria-haspopup="menu"
                aria-expanded={menu?.key === row.key}
                onClick={(event) => {
                  event.stopPropagation();
                  const rect = event.currentTarget.getBoundingClientRect();
                  openMenu(row.key, { x: rect.left, y: rect.bottom + 4 });
                }}>⋯</button>}
            </div>;
            return <span key={row.key} style={{ display: "contents" }}>{index === draftAt ? editor : null}{node}</span>;
          })}
          {draftAt === rows.length ? editor : null}
        </div>}
    </div>
    {picking ? <div className="sidebar-foot feeds-select-foot">
      <div className="feeds-select-lead"><span>{t("tree.picked", { count: picked.size })}</span><button type="button" onClick={exitPick}>{t("tree.done")}</button></div>
      <div className="feeds-select-acts">
        <button type="button" disabled={picked.size === 0} onClick={(event) => { const rect = event.currentTarget.getBoundingClientRect(); openMenu("bulk", { x: rect.left, y: rect.top - 8 }, "bulk"); }}>{t("tree.moveTo")}</button>
        <button type="button" disabled={picked.size === 0} onClick={() => { setMenu(null); setDraft({ parent: null, picked: [...picked] }); }}>{t("tree.menu.newFolder")}</button>
      </div>
    </div> : <div className="sidebar-foot">
      <button type="button" onClick={() => { setPicking(true); setPicked(new Set()); }}><Icon name="folder" size={15} />{t("tree.organize")}</button>
      <button type="button" onClick={onSettings}><Icon name="settings" size={15} />{t("tree.settings")}</button>
    </div>}
    {menu ? <FloatingMenu key={menu.mode + menu.key} anchor={menu.anchor} label={menu.mode === "add" ? t("tree.menu.add") : menu.mode === "move" || menu.mode === "bulk" ? t("tree.menu.moveFolder") : t("tree.menu.row")} className="feeds-row-menu" onClose={() => setMenu(null)}>
      {menu.key.startsWith("pin:") ? <>
        <button type="button" role="menuitem" onClick={() => { setMenu(null); void writeSetting("feeds_pinned", pinned.filter((item) => item !== menu.key.slice(4)).join(",")).catch((err) => notify({ message: formatNotice(t("tree.error.pin"), err) })); }}>{t("tree.menu.unpin")}</button>
        <button type="button" role="menuitem" onClick={() => reveal(menu.key.slice(4))}>{t("tree.menu.reveal")}</button>
      </> : menu.mode === "add" ? <>
        <button type="button" role="menuitem" onClick={() => { setMenu(null); setDraft({ parent: null, picked: null }); }}>{t("tree.menu.newFolder")}</button>
        <button type="button" role="menuitem" onClick={() => { setMenu(null); onAddFeed(); }}>{t("tree.menu.addFeed")}</button>
      </> : menu.mode === "move" || menu.mode === "bulk" ? <>
        <button type="button" role="menuitem" onClick={() => void runMove(planIntoFolder(tree, menuKeys, null))}>{t("tree.menu.noFolder")}</button>
        <hr />
        {folderMoveTargets(tree, menuKeys).map((folder) => <button key={folder.id} type="button" role="menuitem" style={{ paddingLeft: 8 + folder.depth * 16 }}
          onClick={() => void runMove(planIntoFolder(tree, menuKeys, folder.id))}>{folder.name}</button>)}
      </> : menuRow ? <>
        {menuRow.kind === "folder" ? <button type="button" role="menuitem" onClick={() => { setMenu(null); setPicker({ folderId: Number(menu.key.slice(7)), name: menuRow.name, fresh: false }); }}>{t("tree.menu.addSource")}</button> : null}
        {menuRow.kind === "folder" ? <button type="button" role="menuitem" onClick={() => { setMenu(null); setOpen((prev) => new Set(prev).add(Number(menu.key.slice(7)))); setDraft({ parent: Number(menu.key.slice(7)), picked: null }); }}>{t("tree.menu.newSubfolder")}</button> : null}
        {menuRow.kind === "folder" ? <hr /> : null}
        <button type="button" role="menuitem" disabled={!planToTop(tree, menu.key)} onClick={() => void runMove(planToTop(tree, menu.key))}>{t("tree.menu.toTop")}</button>
        <button type="button" role="menuitem" disabled={!planShift(tree, menu.key, -1)} onClick={() => void runMove(planShift(tree, menu.key, -1))}>{t("tree.menu.up")}<kbd>Alt ↑</kbd></button>
        <button type="button" role="menuitem" disabled={!planShift(tree, menu.key, 1)} onClick={() => void runMove(planShift(tree, menu.key, 1))}>{t("tree.menu.down")}<kbd>Alt ↓</kbd></button>
        <button type="button" role="menuitem" onClick={() => setMenu({ ...menu, mode: "move" })}>{t("tree.menu.toFolder")}</button>
        <button type="button" role="menuitem" onClick={() => void togglePin(menu.key)}>{pinned.includes(menu.key) ? t("tree.menu.unpin") : t("tree.menu.pin")}</button>
        <hr />
        <button type="button" role="menuitem" onClick={() => { setMenu(null); setEditing(menu.key); }}>{t("tree.menu.rename")}</button>
        {menuRow.kind === "folder" ? <button type="button" role="menuitem" disabled={!columns.includes(menu.key) && columns.length >= 5} title={!columns.includes(menu.key) && columns.length >= 5 ? t("tree.limit.columns") : undefined}
          onClick={() => void setColumn(menu.key)}>{columns.includes(menu.key) ? t("tree.menu.unsetColumn") : t("tree.menu.setColumn")}</button> : null}
      </> : null}
    </FloatingMenu> : null}
    {picker ? createPortal(<FeedPicker tree={tree} folderId={picker.folderId} folderName={picker.name} fresh={picker.fresh} onClose={() => setPicker(null)}
      onApply={(ids) => { setPicker(null); void runMove(planIntoFolder(tree, ids.map((id) => `feed:${id}`), picker.folderId)); }} />, document.body) : null}
    {drag ? createPortal(<div className="feeds-drag-ghost" style={{ left: drag.x + 12, top: drag.y + 8 }}>{drag.name}{drag.extra ? ` +${drag.extra}` : ""}</div>, document.body) : null}
  </>;
}
