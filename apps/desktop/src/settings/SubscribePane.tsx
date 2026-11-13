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
import { useTranslation } from "../i18n";

/** 保留设置的下拉值：跟随全局、永久保留或自定义天数。 */
type RetentionMode = "follow" | "keep" | "custom";
const RETENTION_MODES: RetentionMode[] = ["follow", "keep", "custom"];
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
  const { t } = useTranslation("settingsContent");
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
  const askBatch = () => onAsk(t("subscribe.batch.title"),
    [...failing.map((feed) => `${feed.title} — ${feed.health_label}`), t("subscribe.batch.warning")].join("\n"),
    () => { void s.run(t("subscribe.action.unsubscribe"), async () => ({ tree: await api.unsubscribeFeeds(failing.map((feed) => feed.id)), message: t("subscribe.batch.done", { count: failing.length }) })); },
    { okLabel: t("subscribe.batch.ok", { count: failing.length }) });
  return (
    <fieldset className="subscribe-pane" aria-label={t("subscribe.aria")} aria-busy={s.busy} disabled={s.busy}
      style={{ border: 0, margin: 0, padding: 0, minWidth: 0 }}>
      <section className="subscribe-add settings-card settings-section" aria-labelledby="subscribe-add-title">
        <div className="settings-section-meta">
          <h4 id="subscribe-add-title">{t("subscribe.add.title")}</h4>
          <p className="hint">{t("subscribe.add.hint")}</p>
        </div>
        <div className="settings-section-body">
          <div className="row subscribe-input-row">
            <div className="search-field">
              <Icon name="rss" size={14} />
              <input id="subscribe-url" aria-label={t("subscribe.add.urlAria")} type="url" value={s.url} onChange={(e) => s.setUrl(e.target.value)}
                placeholder={t("subscribe.add.urlPlaceholder")} />
            </div>
            <button type="button" className="btn primary" disabled={!s.url.trim()} onClick={() => {
              if (!s.url.trim()) return;
              void s.run(t("subscribe.action.add"), async () => ({ message: t("subscribe.add.success", { message: (await api.addFeed(s.url.trim())).message }) }), () => s.setUrl(""));
            }}>{t("subscribe.add.button")}</button>
          </div>
          <p className="hint">{t("subscribe.add.formats")}</p>
        </div>
      </section>
      <section className="settings-card settings-section" aria-labelledby="subscribe-display-title">
        <div className="settings-section-meta">
          <h4 id="subscribe-display-title">{t("subscribe.display.title")}</h4>
          <p className="hint">{t("subscribe.display.hint")}</p>
        </div>
        <div className="settings-section-body">
          <SettingsRows ids={["feed_icons_enabled"]} label={t("subscribe.display.icons")} disabled={s.busy}
            onChanged={() => { void s.reload(); }} />
        </div>
      </section>
      <section className="settings-card settings-section" aria-labelledby="subscribe-fetch-title">
        <div className="settings-section-meta">
          <h4 id="subscribe-fetch-title">{t("subscribe.fetch.title")}</h4>
          <p className="hint">{t("subscribe.fetch.hint")}</p>
        </div>
        <div className="settings-section-body">
          <SettingsRows ids={["feed_fetch_per_host"]} label={t("subscribe.fetch.label")} disabled={s.busy} onEditStateChange={report("fetch")} />
        </div>
      </section>
      <section className="settings-card settings-section" aria-labelledby="subscribe-folder-title">
        <div className="settings-section-meta">
          <h4 id="subscribe-folder-title">{t("subscribe.organize.title")}</h4>
          <p className="hint">{t("subscribe.organize.hint")}</p>
        </div>
        <div className="settings-section-body">
          <div className="settings-option subscribe-folder-create">
            <div><label htmlFor="subscribe-folder">{t("subscribe.organize.newFolder")}</label></div>
            <input id="subscribe-folder" aria-label={t("subscribe.organize.folderNameAria")} value={s.folderName} onChange={(e) => s.setFolderName(e.target.value)} placeholder={t("subscribe.organize.folderPlaceholder")} />
            <button type="button" className="btn" disabled={!s.folderName.trim()} onClick={() => {
              const name = s.folderName.trim();
              if (!name) return;
              void s.run(t("subscribe.action.newFolder"), async () => {
                await api.createFolder(name); return { message: t("subscribe.organize.created", { name }) };
              }, () => s.setFolderName(""));
            }}>{t("subscribe.organize.create")}</button>
          </div>
          <div className="settings-option subscribe-transfer">
            <div><strong>OPML</strong><p className="hint">{t("subscribe.opml.hint")}</p></div>
            <input ref={opmlRef} type="file" accept=".opml,.xml,text/xml" hidden onChange={(e) => {
              const file = e.target.files?.[0];
              e.target.value = "";
              if (file) void s.run(t("subscribe.action.import"), async () => ({ message: await api.importOpml(await file.text()) }));
            }} />
            <button type="button" className="btn" onClick={() => opmlRef.current?.click()}><Icon name="upload" size={14} />{t("subscribe.opml.import")}</button>
            <button type="button" className="btn" onClick={() => void s.run(t("subscribe.action.export"), async () => { downloadText("rrss.opml", await api.exportOpml(), "text/xml"); return {}; }, undefined, true)}><Icon name="download" size={14} />{t("subscribe.opml.export")}</button>
          </div>
        </div>
      </section>
      {s.status && !s.query.isError && <p className={`subscribe-feedback notice ${s.status.tone === "ok" ? "ok" : "error"}`}
        role={s.status.tone === "ok" ? "status" : "alert"}><Icon name={s.status.tone === "ok" ? "check" : "info"} size={14} />{s.status.text}</p>}
      {s.query.isError && <p className="subscribe-feedback notice error" role="alert">{s.status?.tone === "err"
        ? s.status.text : formatNotice(t("subscribe.loadFailed"), s.query.error)}</p>}
      <section className="subscribe-list" aria-labelledby="subscribe-list-title">
        <div className="subscribe-list-heading">
          <h4 id="subscribe-list-title">{t("subscribe.list.title")} {tree && <span className="settings-count">{feeds.length}</span>}</h4>
          <p className="hint">{t("subscribe.list.hint")}</p>
          <button type="button" className="btn sm ghost" disabled={s.query.isFetching} onClick={() => void s.reload()}>{t("subscribe.list.reload")}</button>
        </div>
        {issues.length > 0 && <div className="health-bar" role="status">
          <Icon name="info" size={14} />
          <span>{t("subscribe.health.issues", { count: issues.length })}</span>
          <button type="button" className="btn sm" aria-pressed={filtering} onClick={() => setOnlyIssues(!filtering)}>{filtering ? t("subscribe.health.showAll") : t("subscribe.health.onlyThese")}</button>
          {failing.length > 0 && <button type="button" className="btn sm danger ghost" onClick={askBatch}>{t("subscribe.health.unsubscribeFailing")}</button>}
        </div>}
        {!tree && s.query.isPending && <Pending className="muted">{t("subscribe.list.loading")}</Pending>}
        {tree && !feeds.length && !folders.length && <div className="empty compact">
          <span className="empty-icon"><Icon name="rss" size={20} /></span>
          <p>{t("subscribe.list.empty")}</p>
        </div>}
        {(filtering ? folders.filter((folder) => folder.feeds.some((feed) => feed.health)) : folders).map((folder) =>
          <FolderBlock key={folder.id} folder={filtering ? { ...folder, feeds: shown(folder.feeds) } : folder} folders={folders} onAsk={onAsk} settings={s} />)}
        {tree && shown(tree.uncategorized).length ? <div className="section-label">{t("subscribe.uncategorized")} · {shown(tree.uncategorized).length}</div> : null}
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
  const { t } = useTranslation("settingsContent");
  const editor = s.folders[folder.id];
  return <div className="folder-block">
    {editor ? <div className="feed-edit">
      <label className="field"><span>{t("subscribe.folder.name")}</span><input value={editor.draft.name}
        onChange={(e) => s.editFolder(folder.id, { ...editor, draft: { ...editor.draft, name: e.target.value } })} /></label>
      {missing && <p className="notice warn">{t("subscribe.folder.missing")}</p>}
      <div className="row form-actions">
        <button type="button" className="btn primary sm" disabled={missing || !editor.draft.name.trim()} onClick={() =>
          void s.run(t("subscribe.action.save"), async () => { await api.renameFolder(folder.id, editor.draft.name); return {}; }, () => s.editFolder(folder.id))
        }>{t("subscribe.saveEdit")}</button>
        <button type="button" className="btn sm" onClick={() => s.editFolder(folder.id)}>{t("subscribe.cancel")}</button>
        <span className="toolbar-spacer" />
        <button type="button" className="btn sm danger ghost" disabled={missing} onClick={() =>
          onAsk(t("subscribe.folder.deleteTitle"), t("subscribe.folder.deleteConfirm", { name: folder.name }), () => {
            void s.run(t("subscribe.action.delete"), async () => { await api.deleteFolder(folder.id); return {}; }, () => s.editFolder(folder.id));
          })
        }>{t("subscribe.delete")}</button>
      </div>
    </div> : <div className="list-row folder-row">
      <div className="folder-name"><span>{folder.label} · {folder.feeds.length}</span></div>
      <button type="button" className="btn sm ghost" onClick={() => s.editFolder(folder.id, { baseline: folder, draft: folder })}>{t("subscribe.edit")}</button>
    </div>}
    {!missing && <div className="folder-feeds">{folder.feeds.map((feed) =>
      <FeedItem key={feed.id} feed={feed} folders={folders} onAsk={onAsk} settings={s} />)}</div>}
  </div>;
}

function FeedItem({ feed, folders, onAsk, settings: s, missing = false }: {
  feed: FeedView; folders: FolderView[]; onAsk: Ask; settings: SubscribeSettings; missing?: boolean;
}) {
  const { t } = useTranslation("settingsContent");
  const editor = s.feeds[feed.id];
  const [open, setOpen] = useState(false);
  const unsubscribe = () => onAsk(t("subscribe.feed.deleteTitle"), t("subscribe.feed.deleteConfirm", { title: feed.title }), () => {
    void s.run(t("subscribe.action.delete"), async () => { await api.unsubscribe(feed.id); return {}; }, () => s.editFeed(feed.id));
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
        <button type="button" className="btn sm ghost" onClick={() => void s.run(t("subscribe.action.dismiss"), async () => ({ tree: await api.dismissFeedHealth(feed.id), message: t("subscribe.feed.dismissed", { title: feed.title }) }))}>{t("subscribe.feed.dismiss")}</button>
      </> : null}
      <button type="button" className="btn sm ghost" onClick={() => s.editFeed(feed.id, { baseline: feed, draft: feed })}>{t("subscribe.edit")}</button>
      <button type="button" className="btn sm danger" onClick={unsubscribe}>{t("subscribe.feed.unsubscribe")}</button>
      {feed.health && <button type="button" className={`health-badge ${feed.health === "failing" ? "danger" : "warn"}`}
        aria-expanded={open} onClick={() => setOpen(!open)}>{feed.health_label}</button>}
      {!feed.health && feed.error && <span className="health-badge danger" title={feed.error}>{t("subscribe.feed.refreshFailed")}</span>}
    </div>
  </div>;
  const { baseline, draft } = editor;
  const change = (patch: Partial<FeedView>) => s.editFeed(feed.id, { ...editor, draft: { ...draft, ...patch } });
  const badDays = draft.retention_days !== null && !(draft.retention_days >= 0);
  return <div className="feed-edit">
    <div className="field-grid">
      <label className="field"><span>{t("subscribe.feed.name")}</span><input value={draft.title} onChange={(e) => change({ title: e.target.value })} placeholder={t("subscribe.feed.namePlaceholder")} /></label>
      <label className="field"><span>{t("subscribe.feed.folder")}</span>
        <MenuSelect label={t("subscribe.feed.folder")} value={draft.folder_id} onChange={(folder_id) => change({ folder_id })} options={[
          { value: 0, label: t("subscribe.uncategorized") },
          ...(draft.folder_id !== 0 && !folders.some((folder) => folder.id === draft.folder_id) ? [{ value: draft.folder_id, label: t("subscribe.feed.folderGone") }] : []),
          ...folders.map((folder) => ({ value: folder.id, label: folder.label })),
        ]} />
      </label>
    </div>
    <label className="field"><span>{t("subscribe.feed.url")}</span><input value={draft.url} onChange={(e) => change({ url: e.target.value })} placeholder="https://example.com/feed.xml" /></label>
    <div className="field-grid">
      <label className="field"><span>{t("subscribe.feed.retention")}</span>
        <MenuSelect label={t("subscribe.feed.retention")} value={retentionMode(draft.retention_days)} options={RETENTION_MODES.map((value) => ({ value, label: t(`subscribe.retentionMode.${value}`) }))}
          onChange={(mode) => change({ retention_days: mode === "follow" ? null : mode === "keep" ? 0 : baseline.retention_days || 90 })} />
      </label>
      {retentionMode(draft.retention_days) === "custom" && <label className="field"><span>{t("subscribe.feed.retentionDays")}</span>
        <input type="number" min={1} inputMode="numeric" value={Number.isNaN(draft.retention_days) ? "" : draft.retention_days ?? ""}
          onChange={(e) => change({ retention_days: e.target.value === "" ? Number.NaN : Math.trunc(Number(e.target.value)) })} />
      </label>}
    </div>
    <p className="hint">{t("subscribe.feed.retentionHint")}</p>
    {feed.error && <p className="notice warn"><Icon name="info" size={14} />{feed.error}</p>}
    {missing && <p className="notice warn">{t("subscribe.feed.missing")}</p>}
    <div className="row form-actions">
      <button type="button" className="btn primary sm" disabled={missing || badDays} onClick={() => void s.run(t("subscribe.action.save"), async () => ({
        tree: await api.saveFeed({ id: feed.id, title: draft.title.trim() !== baseline.title ? draft.title : null,
          url: draft.url !== baseline.url ? draft.url : null, folder_id: draft.folder_id !== baseline.folder_id ? draft.folder_id : null,
          retention: draft.retention_days !== baseline.retention_days ? retentionArg(draft.retention_days) : null }),
      }), () => s.editFeed(feed.id))}>{t("subscribe.saveEdit")}</button>
      <button type="button" className="btn sm" onClick={() => s.editFeed(feed.id)}>{t("subscribe.cancel")}</button>
      <span className="toolbar-spacer" />
      <button type="button" className="btn sm danger ghost" disabled={missing} onClick={unsubscribe}>{t("subscribe.delete")}</button>
    </div>
  </div>;
}
