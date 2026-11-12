import { useState, type ReactNode } from "react";
import type { ThreadFolderView, ThreadListView, ThreadView } from "../../lib/api";
import { api } from "../../lib/api";
import { formatNotice } from "../../lib/notice";
import { FloatingMenu, type MenuAnchor } from "../../components/FloatingMenu";
import { Icon } from "../../components/Icon";
import { RelativeTime } from "../../components/RelativeTime";

/**
 * 左栏「会话」（阶段 14 · 会话文件夹与归档）：新对话 → 各文件夹（可折叠）→ 没归类的会话，底部「已归档 N」。
 * 置顶的会话在各自分组里排最前；排序以宿主为准（置顶在前、最近更新在后）。行菜单与右键同一份：
 * 重命名就地改、置顶、移到文件夹、归档（提示条可撤销）、删除（菜单里二次确认）。
 */

export type ThreadNotice = { message: string; actionLabel?: string; onAction?: () => void };

type Menu =
  | { kind: "thread"; id: number; step: "root" | "move" | "delete"; anchor: MenuAnchor }
  | { kind: "folder"; id: number; step: "root" | "delete"; anchor: MenuAnchor }
  | { kind: "archived"; id: number; step: "root" | "delete"; anchor: MenuAnchor };

/** 正在改名的：会话 `t:id`、文件夹 `f:id`、新文件夹 `new`（`move` 时建好后把这条会话移进去）。 */
type Editing = { key: string; move?: number } | null;

const TITLE_MAX = 80;
const FOLDER_MAX = 40;

export function ThreadList({ list, active, fresh, onThread, onChanged, onArchived, notify }: {
  list: ThreadListView;
  /** 当前打开的会话；不在对话页时为 0。 */
  active: number;
  /** 刚出现的会话：撑开高度并带一下淡底。 */
  fresh: ReadonlySet<number>;
  onThread: (id: number, saved?: ThreadView) => void;
  /** 写入成功后重读列表。 */
  onChanged: () => void;
  onArchived: () => void;
  notify: (notice: ThreadNotice) => void;
}) {
  const [menu, setMenu] = useState<Menu | null>(null);
  const [editing, setEditing] = useState<Editing>(null);
  // 折叠先在本地生效，写入失败再退回。
  const [collapsed, setCollapsed] = useState<Record<number, boolean>>({});
  const isCollapsed = (folder: ThreadFolderView) => collapsed[folder.id] ?? folder.collapsed;

  const run = async (work: () => Promise<unknown>, failure: string, done?: ThreadNotice) => {
    try {
      await work();
      onChanged();
      if (done) notify(done);
      return true;
    } catch (err) {
      notify({ message: formatNotice(failure, err) });
      onChanged();
      return false;
    }
  };
  const thread = (id: number) => list.threads.find((t) => t.id === id);
  const folder = (id: number) => list.folders.find((f) => f.id === id);
  const openMenu = (next: Menu) => setMenu((old) => (old && old.kind === next.kind && old.id === next.id ? null : next));
  const anchorOf = (el: Element): MenuAnchor => { const r = el.getBoundingClientRect(); return { x: r.left, y: r.bottom + 4 }; };

  const commit = async (value: string) => {
    const edit = editing;
    setEditing(null);
    if (!edit) return;
    const text = value.trim();
    if (edit.key === "new") {
      if (!text) return;
      await run(async () => {
        const created = await api.createThreadFolder(text.slice(0, FOLDER_MAX));
        if (edit.move) await api.setThreadFolder(edit.move, created.id);
      }, "没能新建文件夹：");
    } else if (edit.key.startsWith("t:")) {
      const id = Number(edit.key.slice(2));
      if (text === thread(id)?.title) return;
      // 清空就回到自动标题。
      await run(() => api.renameThread(id, text.slice(0, TITLE_MAX)), "没能重命名：");
    } else {
      const id = Number(edit.key.slice(2));
      if (!text || text === folder(id)?.name) return;
      await run(() => api.renameThreadFolder(id, text.slice(0, FOLDER_MAX)), "没能重命名文件夹：");
    }
  };
  const renameInput = (label: string, initial: string) => (
    <input className="thread-rename" aria-label={label} defaultValue={initial} autoFocus maxLength={editing?.key.startsWith("t:") ? TITLE_MAX : FOLDER_MAX}
      placeholder={editing?.key === "new" ? "文件夹名称" : editing?.key.startsWith("t:") ? "留空恢复自动标题" : undefined}
      onFocus={(event) => event.currentTarget.select()}
      onBlur={(event) => void commit(event.currentTarget.value)}
      onKeyDown={(event) => {
        if (event.nativeEvent.isComposing) return;
        if (event.key === "Enter") { event.preventDefault(); event.currentTarget.blur(); }
        if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); setEditing(null); }
      }} />
  );

  const row = (t: ThreadView, depth: 0 | 1) => {
    const key = `t:${t.id}`;
    return <div key={t.id} className="thread-row" data-depth={depth} data-new={fresh.has(t.id) ? "" : undefined}
      data-menu={menu?.kind === "thread" && menu.id === t.id ? "" : undefined}
      onContextMenu={(event) => { event.preventDefault(); openMenu({ kind: "thread", id: t.id, step: "root", anchor: { x: event.clientX, y: event.clientY } }); }}>
      {editing?.key === key ? <div className="thread-item thread-editing">{renameInput("会话标题", t.title)}</div> : <>
        <button type="button" className="thread-item" title={t.title} data-on={active === t.id ? "1" : "0"} aria-description={t.pinned ? "已置顶" : undefined}
          onClick={() => onThread(t.id, t)}>
          <span className="thread-title">{t.title}</span>
          {t.pinned ? <span className="thread-pin" title="已置顶" aria-hidden="true"><Icon name="pin" size={12} /></span> : null}
        </button>
        <button type="button" className="thread-more" aria-label={`${t.title} 的操作`} aria-haspopup="menu"
          aria-expanded={menu?.kind === "thread" && menu.id === t.id}
          onClick={(event) => openMenu({ kind: "thread", id: t.id, step: "root", anchor: anchorOf(event.currentTarget) })}>⋯</button>
      </>}
    </div>;
  };

  const folders = list.folders.map((f) => {
    const items = list.threads.filter((t) => t.folder_id === f.id);
    const shut = isCollapsed(f);
    const key = `f:${f.id}`;
    return <div key={key} className="thread-folder-group" role="group" aria-label={f.name}>
      <div className="thread-row thread-folder-row" data-menu={menu?.kind === "folder" && menu.id === f.id ? "" : undefined}
        onContextMenu={(event) => { event.preventDefault(); openMenu({ kind: "folder", id: f.id, step: "root", anchor: { x: event.clientX, y: event.clientY } }); }}>
        {editing?.key === key ? <div className="thread-item thread-folder thread-editing"><Icon name="folder" size={14} />{renameInput("文件夹名称", f.name)}</div> : <>
          <button type="button" className="thread-item thread-folder" aria-expanded={!shut}
            onClick={() => {
              setCollapsed((old) => ({ ...old, [f.id]: !shut }));
              void api.setThreadFolderCollapsed(f.id, !shut).catch((err) => {
                setCollapsed((old) => ({ ...old, [f.id]: shut }));
                notify({ message: formatNotice("没能折叠文件夹：", err) });
              });
            }}>
            <Icon name="chevronDown" size={12} className="thread-folder-twist" /><Icon name="folder" size={14} />
            <span className="thread-title">{f.name}</span><span className="thread-folder-count">{items.length}</span>
          </button>
          <button type="button" className="thread-more" aria-label={`${f.name} 文件夹的操作`} aria-haspopup="menu"
            aria-expanded={menu?.kind === "folder" && menu.id === f.id}
            onClick={(event) => openMenu({ kind: "folder", id: f.id, step: "root", anchor: anchorOf(event.currentTarget) })}>⋯</button>
        </>}
      </div>
      {shut ? null : items.length ? items.map((t) => row(t, 1)) : <p className="thread-empty">空文件夹</p>}
    </div>;
  });

  return <>
    <div className="section-label thread-section">
      <span>会话</span>
      <button type="button" className="thread-section-add" title="新建文件夹" aria-label="新建文件夹" onClick={() => { setMenu(null); setEditing({ key: "new" }); }}>
        <Icon name="folder" size={13} /><Icon name="plus" size={9} />
      </button>
    </div>
    <div className="thread-list">
      <button type="button" className="thread-item new-thread" onClick={() => onThread(0)}><Icon name="plus" size={14} />新对话</button>
      {folders}
      {editing?.key === "new" ? <div className="thread-row"><div className="thread-item thread-folder thread-editing"><Icon name="folder" size={14} />{renameInput("新文件夹名称", "")}</div></div> : null}
      {list.threads.filter((t) => t.folder_id == null || !folder(t.folder_id)).map((t) => row(t, 0))}
      {list.archived_count > 0 ? (
        <button type="button" className="thread-item thread-archived-entry" onClick={onArchived}>
          <Icon name="inbox" size={14} /><span className="thread-title">已归档</span><span className="thread-folder-count">{list.archived_count}</span>
        </button>
      ) : null}
    </div>
    {menu ? <FloatingMenu key={`${menu.kind}:${menu.id}:${menu.step}`} anchor={menu.anchor} placement="below"
      role={menu.step === "delete" ? "dialog" : "menu"} label={menu.step === "delete" ? "确认删除" : menu.kind === "folder" ? "文件夹操作" : "会话操作"}
      className="reader-context-menu menu-list thread-menu" onClose={() => setMenu(null)}>
      {menu.kind === "thread" ? threadMenu(menu) : menu.kind === "folder" ? folderMenu(menu) : null}
    </FloatingMenu> : null}
  </>;

  function threadMenu(m: Extract<Menu, { kind: "thread" }>): ReactNode {
    const t = thread(m.id);
    if (!t) return null;
    const close = () => setMenu(null);
    if (m.step === "delete") return <DeleteConfirm title={t.title} onCancel={() => setMenu({ ...m, step: "root" })}
      onDelete={() => { close(); void run(async () => { await api.deleteThread(t.id); if (active === t.id) onThread(0); }, "没能删除：", { message: `已删除「${t.title}」` }); }} />;
    if (m.step === "move") return <>
      <button type="button" role="menuitem" className="thread-menu-back" onClick={() => setMenu({ ...m, step: "root" })}><Icon name="chevronLeft" size={13} />移到文件夹</button>
      <hr />
      {list.folders.map((f) => (
        <button key={f.id} type="button" role="menuitem" disabled={t.folder_id === f.id}
          onClick={() => { close(); void run(() => api.setThreadFolder(t.id, f.id), "没能移动：", { message: `已移到「${f.name}」` }); }}>
          <Icon name="folder" size={14} />{f.name}{t.folder_id === f.id ? <Icon name="check" size={13} className="thread-menu-check" /> : null}
        </button>
      ))}
      <button type="button" role="menuitem" onClick={() => { close(); setEditing({ key: "new", move: t.id }); }}><Icon name="plus" size={14} />新建文件夹并移入</button>
      {t.folder_id != null ? <><hr /><button type="button" role="menuitem"
        onClick={() => { close(); void run(() => api.setThreadFolder(t.id, null), "没能移动：", { message: "已移出文件夹" }); }}>移出文件夹</button></> : null}
    </>;
    return <>
      <button type="button" role="menuitem" onClick={() => { close(); setEditing({ key: `t:${t.id}` }); }}><Icon name="edit" size={14} />重命名</button>
      <button type="button" role="menuitem" onClick={() => { close(); void run(() => api.setThreadPinned(t.id, !t.pinned), t.pinned ? "没能取消置顶：" : "没能置顶："); }}>
        <Icon name="pin" size={14} />{t.pinned ? "取消置顶" : "置顶"}</button>
      <button type="button" role="menuitem" onClick={() => setMenu({ ...m, step: "move" })}><Icon name="folder" size={14} />移到文件夹…</button>
      <hr />
      <button type="button" role="menuitem" onClick={() => {
        close();
        void run(() => api.archiveThread(t.id), "没能归档：", {
          message: `已归档「${t.title}」`, actionLabel: "撤销",
          onAction: () => void run(() => api.unarchiveThread(t.id), "没能撤销归档："),
        });
      }}><Icon name="inbox" size={14} />归档</button>
      <button type="button" role="menuitem" className="danger" onClick={() => setMenu({ ...m, step: "delete" })}><Icon name="trash" size={14} />删除…</button>
    </>;
  }

  function folderMenu(m: Extract<Menu, { kind: "folder" }>): ReactNode {
    const f = folder(m.id);
    if (!f) return null;
    const index = list.folders.indexOf(f);
    const close = () => setMenu(null);
    const count = list.threads.filter((t) => t.folder_id === f.id).length;
    const move = (delta: -1 | 1) => {
      const ids = list.folders.map((x) => x.id);
      [ids[index], ids[index + delta]] = [ids[index + delta], ids[index]];
      close();
      void run(() => api.reorderThreadFolders(ids), "没能调整顺序：");
    };
    if (m.step === "delete") return <DeleteConfirm title={f.name} folder
      note={count ? `里面的 ${count} 条会话回到未归类，不会删除。` : "这是空文件夹。"}
      onCancel={() => setMenu({ ...m, step: "root" })}
      onDelete={() => { close(); void run(() => api.deleteThreadFolder(f.id), "没能删除文件夹：", { message: `已删除文件夹「${f.name}」${count ? "，里面的会话回到未归类" : ""}` }); }} />;
    return <>
      <button type="button" role="menuitem" onClick={() => { close(); setEditing({ key: `f:${f.id}` }); }}><Icon name="edit" size={14} />重命名</button>
      <button type="button" role="menuitem" disabled={index === 0} onClick={() => move(-1)}>上移</button>
      <button type="button" role="menuitem" disabled={index === list.folders.length - 1} onClick={() => move(1)}>下移</button>
      <hr />
      <button type="button" role="menuitem" className="danger" onClick={() => setMenu({ ...m, step: "delete" })}><Icon name="trash" size={14} />删除文件夹…</button>
    </>;
  }
}

/** 菜单里的二次确认：删除会话连同消息与 Pi 会话记录；删除文件夹只把会话放回未归类。 */
export function DeleteConfirm({ title, folder = false, note, onCancel, onDelete }: {
  title: string; folder?: boolean; note?: string; onCancel: () => void; onDelete: () => void;
}) {
  return <div className="thread-confirm">
    <p>删除{folder ? "文件夹" : ""}「{title}」？</p>
    <small>{note ?? "消息和 Pi 会话记录一起删掉，不能恢复。只是不想在侧栏看到，可以归档。"}</small>
    <div className="row thread-confirm-actions">
      <button type="button" className="btn sm ghost" autoFocus onClick={onCancel}>取消</button>
      <button type="button" className="btn sm danger" onClick={onDelete}>{folder ? "删除文件夹" : "删除"}</button>
    </div>
  </div>;
}

/**
 * 「已归档」：整列侧栏换成归档列表（做法同「订阅」），左上角回到会话。归档的会话不在侧栏、不进搜索；
 * 打开后可以照常看，发新消息时宿主自动取消归档。
 */
export function ArchivedThreads({ threads, active, onBack, onThread, onChanged, notify }: {
  threads: ThreadView[] | null;
  active: number;
  onBack: () => void;
  onThread: (id: number, saved?: ThreadView) => void;
  onChanged: () => void;
  notify: (notice: ThreadNotice) => void;
}) {
  const [menu, setMenu] = useState<Extract<Menu, { kind: "archived" }> | null>(null);
  const run = async (work: () => Promise<unknown>, failure: string, done: string) => {
    try { await work(); notify({ message: done }); } catch (err) { notify({ message: formatNotice(failure, err) }); }
    onChanged();
  };
  const target = menu ? threads?.find((t) => t.id === menu.id) : undefined;
  return <>
    <div className="feeds-mode-head thread-archived-head" data-tauri-drag-region="deep">
      <button type="button" className="feeds-back" onClick={onBack} title="回到会话（Esc）"><Icon name="chevronLeft" size={14} />会话</button>
      <strong>已归档</strong>
    </div>
    <div className="thread-list" aria-busy={threads ? undefined : "true"}>
      {threads?.map((t) => (
        <div key={t.id} className="thread-row" data-menu={menu?.id === t.id ? "" : undefined}
          onContextMenu={(event) => { event.preventDefault(); setMenu({ kind: "archived", id: t.id, step: "root", anchor: { x: event.clientX, y: event.clientY } }); }}>
          <button type="button" className="thread-item" title={t.title} aria-label={t.title} data-on={active === t.id ? "1" : "0"} onClick={() => onThread(t.id, t)}>
            <span className="thread-title">{t.title}</span>
            {t.archived_at ? <RelativeTimeLabel ms={t.archived_at} /> : null}
          </button>
          <button type="button" className="thread-more" aria-label={`${t.title} 的操作`} aria-haspopup="menu" aria-expanded={menu?.id === t.id}
            onClick={(event) => { const r = event.currentTarget.getBoundingClientRect(); setMenu((old) => old?.id === t.id ? null : { kind: "archived", id: t.id, step: "root", anchor: { x: r.left, y: r.bottom + 4 } }); }}>⋯</button>
        </div>
      ))}
      {threads && !threads.length ? <p className="thread-empty">没有归档的会话。</p> : null}
      <p className="thread-empty">归档的会话不在侧栏、不进搜索。打开后发新消息会自动取消归档。</p>
    </div>
    {menu && target ? <FloatingMenu key={`${menu.id}:${menu.step}`} anchor={menu.anchor} placement="below" role={menu.step === "delete" ? "dialog" : "menu"}
      label={menu.step === "delete" ? "确认删除" : "会话操作"} className="reader-context-menu menu-list thread-menu" onClose={() => setMenu(null)}>
      {menu.step === "delete" ? <DeleteConfirm title={target.title} onCancel={() => setMenu({ ...menu, step: "root" })}
        onDelete={() => { setMenu(null); void run(async () => { await api.deleteThread(target.id); if (active === target.id) onThread(0); }, "没能删除：", `已删除「${target.title}」`); }} /> : <>
        <button type="button" role="menuitem" onClick={() => { setMenu(null); void run(() => api.unarchiveThread(target.id), "没能取消归档：", `已取消归档「${target.title}」`); }}>
          <Icon name="inbox" size={14} />取消归档</button>
        <hr />
        <button type="button" role="menuitem" className="danger" onClick={() => setMenu({ ...menu, step: "delete" })}><Icon name="trash" size={14} />删除…</button>
      </>}
    </FloatingMenu> : null}
  </>;
}

function RelativeTimeLabel({ ms }: { ms: number }) {
  return <RelativeTime className="thread-when" ms={ms} />;
}
