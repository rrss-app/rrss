import { useMemo, useState } from "react";
import type { FeedTree } from "../../lib/api";
import { Icon } from "../../components/Icon";
import { SourceAvatar } from "../../components/ArticleCard";
import { pickerGroups } from "./feedTree";

/** 把源放进一个文件夹。新建文件夹后取消按钮叫「跳过」。 */
export function FeedPicker({ tree, folderId, folderName, fresh, onClose, onApply }: {
  tree: FeedTree;
  folderId: number;
  folderName: string;
  fresh: boolean;
  onClose: () => void;
  onApply: (ids: number[]) => void;
}) {
  const [query, setQuery] = useState("");
  const [looseOnly, setLooseOnly] = useState(false);
  const [checked, setChecked] = useState<ReadonlySet<number>>(() => new Set());
  const groups = useMemo(() => pickerGroups(tree, folderId, query, looseOnly), [tree, folderId, query, looseOnly]);
  const moving = [...checked].filter((id) => {
    const feed = [...tree.uncategorized, ...tree.folders.flatMap((folder) => folder.feeds)].find((item) => item.id === id);
    return !!feed && feed.folder_id > 0;
  }).length;
  const toggle = (id: number) => setChecked((prev) => {
    const next = new Set(prev);
    if (next.has(id)) next.delete(id); else next.add(id);
    return next;
  });
  const toggleGroup = (ids: number[]) => setChecked((prev) => {
    const next = new Set(prev);
    const all = ids.every((id) => next.has(id));
    for (const id of ids) if (all) next.delete(id); else next.add(id);
    return next;
  });
  return <div className="feed-picker" role="dialog" aria-label={`把订阅放进${folderName}`} data-scroll-boundary>
    <div className="feed-picker-head">
      <div>
        <h2>把订阅放进「{folderName}」</h2>
        <p>{fresh ? "文件夹已建好。勾选要放进去的源，也可以稍后把源拖到文件夹上。" : "勾选后一次放进去，原来所在的文件夹里就不再有它们。"}</p>
      </div>
      <button type="button" className="feed-picker-close" aria-label="关闭" onClick={onClose}><Icon name="close" size={14} /></button>
    </div>
    <label className="feed-picker-search">
      <Icon name="search" size={14} />
      <input value={query} placeholder="搜索源" aria-label="搜索要放进文件夹的源" autoFocus autoComplete="off" onChange={(event) => setQuery(event.target.value)} />
    </label>
    <div className="feed-picker-opts">
      <button type="button" className="chip-btn" data-on={looseOnly ? "1" : "0"} aria-pressed={looseOnly} onClick={() => setLooseOnly((value) => !value)}>只看没放进文件夹的</button>
    </div>
    <div className="feed-picker-body">
      {groups.length === 0 ? <div className="feed-picker-empty">没有名称含「{query.trim()}」的源</div> : groups.map((group) => {
        const free = group.feeds.filter((feed) => !feed.locked).map((feed) => feed.id);
        const all = free.length > 0 && free.every((id) => checked.has(id));
        return <div key={group.label}>
          <div className="feed-picker-group">
            <span>{group.label}</span>
            {free.length > 1 ? <button type="button" onClick={() => toggleGroup(free)}>{all ? "全不选" : "全选"}</button> : null}
          </div>
          {group.feeds.map((feed) => feed.locked
            ? <div key={feed.id} className="feed-picker-row" aria-disabled="true"><span className="feeds-check" data-on="1" /><SourceAvatar title={feed.title} /><span className="feed-picker-name">{feed.title}</span><span className="feed-picker-note">已在这里</span></div>
            : <button key={feed.id} type="button" className="feed-picker-row" role="checkbox" aria-checked={checked.has(feed.id)} onClick={() => toggle(feed.id)}>
              <span className="feeds-check" data-on={checked.has(feed.id) ? "1" : "0"} />
              <SourceAvatar title={feed.title} /><span className="feed-picker-name">{feed.title}</span>
            </button>)}
        </div>;
      })}
    </div>
    <div className="feed-picker-foot">
      <span>{checked.size ? `已选 ${checked.size} 个${moving ? `，其中 ${moving} 个会从原文件夹移出` : ""}` : "还没选"}</span>
      <button type="button" onClick={onClose}>{fresh ? "跳过" : "取消"}</button>
      <button type="button" className="primary" disabled={checked.size === 0} onClick={() => onApply([...checked])}>放进 {checked.size || ""} 个</button>
    </div>
  </div>;
}
