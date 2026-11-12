import { useRef, useState } from "react";
import { api, type FeedTree, type FeedView, type FolderView } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { Icon } from "../components/Icon";
import { SourceAvatar } from "../components/ArticleCard";
import { downloadText } from "./io";
import { useMergedEditState, type SettingsEditState } from "./editState";
import type { Ask } from "./SettingsPanel";
import { useSubscribeSettings, type SubscribeSettings } from "./useSubscribeSettings";
import { Pending } from "../components/Pending";
import { MenuSelect } from "../components/MenuSelect";
import { SettingsRows } from "./SettingsRows";

/** 保留设置的下拉值：跟随全局、永久保留或自定义天数。 */
type RetentionMode = "follow" | "keep" | "custom";
const RETENTION_MODES: { value: RetentionMode; label: string }[] = [
  { value: "follow", label: "跟随全局设置" },
  { value: "keep", label: "永久保留" },
  { value: "custom", label: "自定义天数" },
];
function retentionMode(days: number | null): RetentionMode {
  return days === null ? "follow" : days === 0 ? "keep" : "custom";
}
function retentionArg(days: number | null) {
  return days === null ? "follow" : days === 0 ? "keep" : String(days);
}
export function SubscribePane({ onAsk, onChanged, onEditStateChange }: {
  onAsk: Ask;
  onChanged?: (tree?: FeedTree) => void;
  onEditStateChange?: (state: SettingsEditState | null) => void;
}) {
  const { report } = useMergedEditState(onEditStateChange);
  const s = useSubscribeSettings(onChanged, report("subscribe"));
  const tree = s.query.data;
  const opmlRef = useRef<HTMLInputElement>(null);
  const feeds = tree ? [...tree.uncategorized, ...tree.folders.flatMap((folder) => folder.feeds)] : [];
  const feedIds = new Set(feeds.map((feed) => feed.id));
  const folderIds = new Set(tree?.folders.map((folder) => folder.id));
  const folders = tree?.folders ?? [];
  // 源健康：只提示，不自动退订；「只看这些」只影响本页列表。
  const issues = feeds.filter((feed) => feed.health);
  const failing = issues.filter((feed) => feed.health === "failing");
  const [onlyIssues, setOnlyIssues] = useState(false);
  const filtering = onlyIssues && issues.length > 0;
  const shown = (list: FeedView[]) => (filtering ? list.filter((feed) => feed.health) : list);
  const askBatch = () => onAsk("退订长期失败的源",
    [...failing.map((feed) => `${feed.title} — ${feed.health_label}`), "这些源的文章会一起删除（收藏的也会删除）。"].join("\n"),
    () => { void s.run("退订", async () => ({ tree: await api.unsubscribeFeeds(failing.map((feed) => feed.id)), message: `已退订 ${failing.length} 个源。` })); },
    { okLabel: `退订 ${failing.length} 个源` });
  return (
    <fieldset className="subscribe-pane" aria-label="订阅设置" aria-busy={s.busy} disabled={s.busy}
      style={{ border: 0, margin: 0, padding: 0, minWidth: 0 }}>
      <section className="subscribe-add settings-card settings-section" aria-labelledby="subscribe-add-title">
        <div className="settings-section-meta">
          <h4 id="subscribe-add-title">添加订阅</h4>
          <p className="hint">粘贴订阅地址，添加你关注的来源。</p>
        </div>
        <div className="settings-section-body">
          <div className="row subscribe-input-row">
            <div className="search-field">
              <Icon name="rss" size={14} />
              <input id="subscribe-url" aria-label="订阅地址" type="url" value={s.url} onChange={(e) => s.setUrl(e.target.value)}
                placeholder="粘贴订阅地址，例如 https://example.com/feed.xml" />
            </div>
            <button type="button" className="btn primary" disabled={!s.url.trim()} onClick={() => {
              if (!s.url.trim()) return;
              void s.run("添加", async () => ({ message: "添加成功：" + (await api.addFeed(s.url.trim())).message }), () => s.setUrl(""));
            }}>添加</button>
          </div>
          <p className="hint">支持 RSS、Atom 和 JSON Feed。</p>
        </div>
      </section>
      <section className="settings-card settings-section" aria-labelledby="subscribe-display-title">
        <div className="settings-section-meta">
          <h4 id="subscribe-display-title">显示</h4>
          <p className="hint">来源在焦点卡片和订阅列表里的样子。</p>
        </div>
        <div className="settings-section-body">
          <SettingsRows ids={["feed_icons_enabled"]} label="网站图标" disabled={s.busy}
            onChanged={() => { void s.reload(); }} />
        </div>
      </section>
      <section className="settings-card settings-section" aria-labelledby="subscribe-fetch-title">
        <div className="settings-section-meta">
          <h4 id="subscribe-fetch-title">抓取</h4>
          <p className="hint">刷新时怎样从各个网站下载订阅。</p>
        </div>
        <div className="settings-section-body">
          <SettingsRows ids={["feed_fetch_per_host"]} label="抓取" disabled={s.busy} onEditStateChange={report("fetch")} />
        </div>
      </section>
      <section className="settings-card settings-section" aria-labelledby="subscribe-folder-title">
        <div className="settings-section-meta">
          <h4 id="subscribe-folder-title">整理</h4>
          <p className="hint">文件夹只影响侧栏分组，不改变排序。</p>
        </div>
        <div className="settings-section-body">
          <div className="settings-option subscribe-folder-create">
            <div><label htmlFor="subscribe-folder">新建文件夹</label></div>
            <input id="subscribe-folder" aria-label="新文件夹名称" value={s.folderName} onChange={(e) => s.setFolderName(e.target.value)} placeholder="例如：技术、设计、生活" />
            <button type="button" className="btn" disabled={!s.folderName.trim()} onClick={() => {
              const name = s.folderName.trim();
              if (!name) return;
              void s.run("新建文件夹", async () => {
                await api.createFolder(name); return { message: `已新建文件夹「${name}」` };
              }, () => s.setFolderName(""));
            }}>新建</button>
          </div>
          <div className="settings-option subscribe-transfer">
            <div><strong>OPML</strong><p className="hint">只包含订阅列表，不含文章正文。</p></div>
            <input ref={opmlRef} type="file" accept=".opml,.xml,text/xml" hidden onChange={(e) => {
              const file = e.target.files?.[0];
              e.target.value = "";
              if (file) void s.run("导入", async () => ({ message: await api.importOpml(await file.text()) }));
            }} />
            <button type="button" className="btn" onClick={() => opmlRef.current?.click()}><Icon name="upload" size={14} />导入</button>
            <button type="button" className="btn" onClick={() => void s.run("导出", async () => { downloadText("rrss.opml", await api.exportOpml(), "text/xml"); return {}; }, undefined, true)}><Icon name="download" size={14} />导出</button>
          </div>
        </div>
      </section>
      {s.status && !s.query.isError && <p className={`subscribe-feedback notice ${s.status.tone === "ok" ? "ok" : "error"}`}
        role={s.status.tone === "ok" ? "status" : "alert"}><Icon name={s.status.tone === "ok" ? "check" : "info"} size={14} />{s.status.text}</p>}
      {s.query.isError && <p className="subscribe-feedback notice error" role="alert">{s.status?.tone === "err"
        ? s.status.text : formatNotice("加载订阅失败，请重新读取后再操作：", s.query.error)}</p>}
      <section className="subscribe-list" aria-labelledby="subscribe-list-title">
        <div className="subscribe-list-heading">
          <h4 id="subscribe-list-title">我的订阅 {tree && <span className="settings-count">{feeds.length}</span>}</h4>
          <p className="hint">按文件夹分组。</p>
          <button type="button" className="btn sm ghost" disabled={s.query.isFetching} onClick={() => void s.reload()}>重新读取</button>
        </div>
        {issues.length > 0 && <div className="health-bar" role="status">
          <Icon name="info" size={14} />
          <span>{issues.length} 个源需要处理：长期失败、长期没有新文章或大部分内容被隐藏。</span>
          <button type="button" className="btn sm" aria-pressed={filtering} onClick={() => setOnlyIssues(!filtering)}>{filtering ? "显示全部" : "只看这些"}</button>
          {failing.length > 0 && <button type="button" className="btn sm danger ghost" onClick={askBatch}>退订长期失败的源…</button>}
        </div>}
        {!tree && s.query.isPending && <Pending className="muted">正在读取订阅…</Pending>}
        {tree && !feeds.length && !folders.length && <div className="empty compact">
          <span className="empty-icon"><Icon name="rss" size={20} /></span>
          <p>还没有订阅。在上方添加一个来源，或导入已有的订阅列表。</p>
        </div>}
        {(filtering ? folders.filter((folder) => folder.feeds.some((feed) => feed.health)) : folders).map((folder) =>
          <FolderBlock key={folder.id} folder={filtering ? { ...folder, feeds: shown(folder.feeds) } : folder} folders={folders} onAsk={onAsk} settings={s} />)}
        {tree && shown(tree.uncategorized).length ? <div className="section-label">未分类 · {shown(tree.uncategorized).length}</div> : null}
        {tree && shown(tree.uncategorized).map((feed) => <FeedItem key={feed.id} feed={feed} folders={folders} onAsk={onAsk} settings={s} />)}
        {/* 外部删除后仍保留可见草稿，不能因列表重挂载而静默丢弃。 */}
        {Object.values(s.feeds).filter(({ baseline }) => !feedIds.has(baseline.id)).map(({ baseline }) =>
          <FeedItem key={baseline.id} feed={baseline} folders={folders} onAsk={onAsk} settings={s} missing />)}
        {Object.values(s.folders).filter(({ baseline }) => !folderIds.has(baseline.id)).map(({ baseline }) =>
          <FolderBlock key={baseline.id} folder={baseline} folders={folders} onAsk={onAsk} settings={s} missing />)}
      </section>
    </fieldset>
  );
}

function FolderBlock({ folder, folders, onAsk, settings: s, missing = false }: {
  folder: FolderView; folders: FolderView[]; onAsk: Ask; settings: SubscribeSettings; missing?: boolean;
}) {
  const editor = s.folders[folder.id];
  return <div className="folder-block">
    {editor ? <div className="feed-edit">
      <label className="field"><span>文件夹名称</span><input value={editor.draft.name}
        onChange={(e) => s.editFolder(folder.id, { ...editor, draft: { ...editor.draft, name: e.target.value } })} /></label>
      {missing && <p className="notice warn">此文件夹已不在订阅列表中，未保存的输入仍保留。</p>}
      <div className="row form-actions">
        <button type="button" className="btn primary sm" disabled={missing || !editor.draft.name.trim()} onClick={() =>
          void s.run("保存", async () => { await api.renameFolder(folder.id, editor.draft.name); return {}; }, () => s.editFolder(folder.id))
        }>保存修改</button>
        <button type="button" className="btn sm" onClick={() => s.editFolder(folder.id)}>取消</button>
        <span className="toolbar-spacer" />
        <button type="button" className="btn sm danger ghost" disabled={missing} onClick={() =>
          onAsk("删除文件夹", `删除「${folder.name}」？其中的订阅会移到「未分类」。`, () => {
            void s.run("删除", async () => { await api.deleteFolder(folder.id); return {}; }, () => s.editFolder(folder.id));
          })
        }>删除</button>
      </div>
    </div> : <div className="list-row folder-row">
      <div className="folder-name"><span>{folder.label} · {folder.feeds.length}</span></div>
      <button type="button" className="btn sm ghost" onClick={() => s.editFolder(folder.id, { baseline: folder, draft: folder })}>编辑</button>
    </div>}
    {!missing && <div className="folder-feeds">{folder.feeds.map((feed) =>
      <FeedItem key={feed.id} feed={feed} folders={folders} onAsk={onAsk} settings={s} />)}</div>}
  </div>;
}

function FeedItem({ feed, folders, onAsk, settings: s, missing = false }: {
  feed: FeedView; folders: FolderView[]; onAsk: Ask; settings: SubscribeSettings; missing?: boolean;
}) {
  const editor = s.feeds[feed.id];
  const [open, setOpen] = useState(false);
  const unsubscribe = () => onAsk("删除订阅", `退订「${feed.title}」？相关文章会一并删除。`, () => {
    void s.run("删除", async () => { await api.unsubscribe(feed.id); return {}; }, () => s.editFeed(feed.id));
  });
  if (!editor) return <div className="list-row feed-row" data-feed-health={feed.health || undefined}>
    <SourceAvatar title={feed.title} size={20} iconHref={feed.icon_href} />
    <div>
      <span className="feed-title">{feed.title}</span>
      <span className="url" title={feed.url}>{feed.url}</span>
      {feed.health && open && <span className="feed-health-detail">{feed.health_detail}</span>}
    </div>
    <div className="feed-row-actions">
      {feed.health ? <>
        <button type="button" className="btn sm ghost" onClick={() => void s.run("忽略", async () => ({ tree: await api.dismissFeedHealth(feed.id), message: `已忽略「${feed.title}」的提示，30 天内不再提示。` }))}>忽略</button>
      </> : null}
      <button type="button" className="btn sm ghost" onClick={() => s.editFeed(feed.id, { baseline: feed, draft: feed })}>编辑</button>
      <button type="button" className="btn sm danger" onClick={unsubscribe}>退订</button>
      {feed.health && <button type="button" className={`health-badge ${feed.health === "failing" ? "danger" : "warn"}`}
        aria-expanded={open} onClick={() => setOpen(!open)}>{feed.health_label}</button>}
      {!feed.health && feed.error && <span className="health-badge danger" title={feed.error}>刷新失败</span>}
    </div>
  </div>;
  const { baseline, draft } = editor;
  const change = (patch: Partial<FeedView>) => s.editFeed(feed.id, { ...editor, draft: { ...draft, ...patch } });
  const badDays = draft.retention_days !== null && !(draft.retention_days >= 0);
  return <div className="feed-edit">
    <div className="field-grid">
      <label className="field"><span>订阅名称</span><input value={draft.title} onChange={(e) => change({ title: e.target.value })} placeholder="留空使用来源的默认名称" /></label>
      <label className="field"><span>所属文件夹</span>
        <MenuSelect label="所属文件夹" value={draft.folder_id} onChange={(folder_id) => change({ folder_id })} options={[
          { value: 0, label: "未分类" },
          ...(draft.folder_id !== 0 && !folders.some((folder) => folder.id === draft.folder_id) ? [{ value: draft.folder_id, label: "原文件夹已不可用" }] : []),
          ...folders.map((folder) => ({ value: folder.id, label: folder.label })),
        ]} />
      </label>
    </div>
    <label className="field"><span>订阅地址</span><input value={draft.url} onChange={(e) => change({ url: e.target.value })} placeholder="https://example.com/feed.xml" /></label>
    <div className="field-grid">
      <label className="field"><span>保留文章正文</span>
        <MenuSelect label="保留文章正文" value={retentionMode(draft.retention_days)} options={RETENTION_MODES}
          onChange={(mode) => change({ retention_days: mode === "follow" ? null : mode === "keep" ? 0 : baseline.retention_days || 90 })} />
      </label>
      {retentionMode(draft.retention_days) === "custom" && <label className="field"><span>保留天数</span>
        <input type="number" min={1} inputMode="numeric" value={Number.isNaN(draft.retention_days) ? "" : draft.retention_days ?? ""}
          onChange={(e) => change({ retention_days: e.target.value === "" ? Number.NaN : Math.trunc(Number(e.target.value)) })} />
      </label>}
    </div>
    <p className="hint">收录满这么多天的文章会清掉正文，只留标题、链接和阅读记录。所有源的默认值在「存储」里设置。</p>
    {feed.error && <p className="notice warn"><Icon name="info" size={14} />{feed.error}</p>}
    {missing && <p className="notice warn">此订阅已不在列表中，未保存的输入仍保留。</p>}
    <div className="row form-actions">
      <button type="button" className="btn primary sm" disabled={missing || badDays} onClick={() => void s.run("保存", async () => ({
        tree: await api.saveFeed({ id: feed.id, title: draft.title.trim() !== baseline.title ? draft.title : null,
          url: draft.url !== baseline.url ? draft.url : null, folder_id: draft.folder_id !== baseline.folder_id ? draft.folder_id : null,
          retention: draft.retention_days !== baseline.retention_days ? retentionArg(draft.retention_days) : null }),
      }), () => s.editFeed(feed.id))}>保存修改</button>
      <button type="button" className="btn sm" onClick={() => s.editFeed(feed.id)}>取消</button>
      <span className="toolbar-spacer" />
      <button type="button" className="btn sm danger ghost" disabled={missing} onClick={unsubscribe}>删除</button>
    </div>
  </div>;
}
