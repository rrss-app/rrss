import { feedAlert, findFeed, feedHealthKind, feedHealthLabel, feedHealthDetail, formatFeedError, formatRefreshTime } from "../features/feeds";
import { useEffect, useRef, useState } from "react";
import { api, type AddFeedNotice, type FeedTree, type FeedView, type FolderView } from "../lib/api";
import { formatError } from "../lib/errors";
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
import { i18n, useTranslation } from "../i18n";

/** 保留设置的下拉值：跟随全局、永久保留或自定义天数。 */
type RetentionMode = "follow" | "keep" | "custom";
const RETENTION_MODES: RetentionMode[] = ["follow", "keep", "custom"];
function retentionMode(days: number | null): RetentionMode {
  return days === null ? "follow" : days === 0 ? "keep" : "custom";
}
function retentionArg(days: number | null) {
  return days === null ? "follow" : days === 0 ? "keep" : String(days);
}

/** 刷新间隔的下拉值：跟随全局或自定义分钟数（下限 1，不设档位）。 */
type IntervalMode = "follow" | "custom";
const INTERVAL_MODES: IntervalMode[] = ["follow", "custom"];
const DEFAULT_CUSTOM_MINUTES = 60;
function intervalArg(minutes: number | null) {
  return minutes === null ? "follow" : String(minutes);
}

/** 编辑框里的上次 / 下次刷新；从未排期（含手动收录源）时不显示。 */
function refreshLine(feed: FeedView, rescheduled: boolean, now = Date.now()): string | null {
  const { last_fetched_at: last, next_fetch_at: next } = feed;
  if (last === null && next === null) return null;
  const t = (key: "refreshTimes" | "refreshDue" | "refreshPending" | "refreshRescheduled" | "refreshLimited", params: { last?: string; next?: string; until?: string } = {}) =>
    i18n.t(`subscribe.feed.${key}`, { ns: "settingsContent", ...params });
  const due = next === null || next <= now;
  if (last === null) return t("refreshPending");
  if (rescheduled) return t("refreshRescheduled", { last: formatRefreshTime(last, now) });
  const alert = feedAlert(feed, now);
  if (alert?.kind === "limited") return t("refreshLimited", { last: formatRefreshTime(last, now), until: formatRefreshTime(alert.until, now) });
  return due ? t("refreshDue", { last: formatRefreshTime(last, now) })
    : t("refreshTimes", { last: formatRefreshTime(last, now), next: formatRefreshTime(next, now) });
}

/** 添加订阅后的抓取安排；延期或未安排时带上原因。 */
function addFeedNotice(notice: AddFeedNotice): string {
  const reason = notice.code === "deferred" || notice.code === "not_scheduled" ? formatError(notice.params.error) : "";
  return i18n.t(`subscribe.add.notice.${notice.code}`, { ns: "settingsContent", reason });
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
  const failing = issues.filter((feed) => feed.health?.code === "feed_health_failing");
  const [onlyIssues, setOnlyIssues] = useState(false);
  const filtering = onlyIssues && issues.length > 0;
  const shown = (list: FeedView[]) => (filtering ? list.filter((feed) => feed.health) : list);
  const askBatch = () => onAsk(t("subscribe.batch.title"),
    [...failing.map((feed) => `${feed.title} — ${feedHealthLabel(feed.health)}`), t("subscribe.batch.warning")].join("\n"),
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
              void s.run(t("subscribe.action.add"), async () => ({ message: t("subscribe.add.success", { message: addFeedNotice((await api.addFeed(s.url.trim())).notice) }) }), () => s.setUrl(""));
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
          <SettingsRows ids={["feed_fetch_interval_minutes", "feed_fetch_per_host"]} label={t("subscribe.fetch.label")} disabled={s.busy} onEditStateChange={report("fetch")} />
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
              if (file) void s.run(t("subscribe.action.import"), async () => {
                const report = await api.importOpml(await file.text());
                return { message: t("subscribe.opml.imported", { imported: report.imported, skipped: report.skipped_same_url, folders: report.folders }) };
              });
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

export function QuickFeedSettings({ feed, active, onAsk, onChanged, onClose, onEditStateChange }: {
  active: boolean;
  feed: FeedView; onAsk: Ask; onChanged: (tree?: FeedTree) => void; onClose: () => void;
  onEditStateChange: (state: SettingsEditState | null) => void;
}) {
  const { t } = useTranslation("feeds");
  const s = useSubscribeSettings(onChanged, onEditStateChange, feed);
  const [finished, setFinished] = useState(false);
  useEffect(() => { if (finished && active) onClose(); }, [finished, active, onClose]);
  const tree = s.query.data;
  const live = tree && findFeed(tree, feed.id);
  return <>
    <h3 id="feed-edit-title">{t("quickSettings")} · {feed.title}</h3>
    <fieldset className="quick-feed-fields" disabled={s.busy} aria-busy={s.busy}>
      <FeedItem feed={live ?? feed} folders={tree?.folders ?? []} onAsk={onAsk} settings={s}
        missing={!live} onFinish={() => setFinished(true)} />
    </fieldset>
    {s.status && <p className="form-status" role="status" data-tone={s.status.tone}>{s.status.text}</p>}
  </>;
}

function FeedItem({ feed, folders, onAsk, settings: s, missing = false, onFinish }: {
  feed: FeedView; folders: FolderView[]; onAsk: Ask; settings: SubscribeSettings; missing?: boolean; onFinish?: () => void;
}) {
  const { t } = useTranslation("settingsContent");
  const editor = s.feeds[feed.id];
  const [open, setOpen] = useState(false);
  const [limitOpen, setLimitOpen] = useState(false);
  const finish = () => { s.editFeed(feed.id); onFinish?.(); };
  const unsubscribe = () => onAsk(t("subscribe.feed.deleteTitle"), t("subscribe.feed.deleteConfirm", { title: feed.title }), () => {
    void s.run(t("subscribe.action.delete"), async () => { await api.unsubscribe(feed.id); return {}; }, finish);
  });
  const alert = feedAlert(feed);
  const limited = alert?.kind === "limited" ? t("subscribe.feed.limitedDetail", { until: formatRefreshTime(alert.until) }) : "";
  if (!editor) return <div className="list-row feed-row" data-feed-health={feedHealthKind(feed.health) || undefined}>
    <SourceAvatar title={feed.title} size={20} iconHref={feed.icon_href} />
    <div>
      <span className="feed-title">{feed.title}</span>
      <span className="url" title={feed.url}>{feed.url}</span>
      {feed.health && open && <span className="feed-health-detail">{feedHealthDetail(feed)}</span>}
      {limited && limitOpen && <span className="feed-health-detail">{limited}</span>}
    </div>
    <div className="feed-row-actions">
      {feed.health ? <>
        <button type="button" className="btn sm ghost" onClick={() => void s.run(t("subscribe.action.dismiss"), async () => ({ tree: await api.dismissFeedHealth(feed.id), message: t("subscribe.feed.dismissed", { title: feed.title }) }))}>{t("subscribe.feed.dismiss")}</button>
      </> : null}
      <button type="button" className="btn sm ghost" onClick={() => s.editFeed(feed.id, { baseline: feed, draft: feed })}>{t("subscribe.edit")}</button>
      <button type="button" className="btn sm danger" onClick={unsubscribe}>{t("subscribe.feed.unsubscribe")}</button>
      {feed.health && <button type="button" className={`health-badge ${feed.health?.code === "feed_health_failing" ? "danger" : "warn"}`}
        aria-expanded={open} onClick={() => setOpen(!open)}>{feedHealthLabel(feed.health)}</button>}
      {alert?.kind === "limited" && <button type="button" className="health-badge warn" aria-expanded={limitOpen} onClick={() => setLimitOpen(!limitOpen)}>
        {t("subscribe.feed.limited", { until: formatRefreshTime(alert.until) })}</button>}
      {!feed.health && alert?.kind === "failed" && feed.error && <span className="health-badge danger" title={formatFeedError(feed.error)}>{t("subscribe.feed.refreshFailed")}</span>}
    </div>
  </div>;
  const { baseline, draft } = editor;
  const change = (patch: Partial<FeedView>) => s.editFeed(feed.id, { ...editor, draft: { ...draft, ...patch } });
  const badDays = draft.retention_days !== null && !(draft.retention_days >= 0);
  const badInterval = draft.interval_minutes !== null && !(draft.interval_minutes >= 1);
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
          onChange={(e) => { const days = e.target.valueAsNumber; change({ retention_days: Number.isInteger(days) && days > 0 ? days : Number.NaN }); }} />
      </label>}
    </div>
    <p className="hint">{t("subscribe.feed.retentionHint")}</p>
    <div className="field-grid">
      <label className="field"><span>{t("subscribe.feed.interval")}</span>
        <MenuSelect label={t("subscribe.feed.interval")} value={draft.interval_minutes === null ? "follow" : "custom"}
          options={INTERVAL_MODES.map((value) => ({ value, label: t(`subscribe.intervalMode.${value}`) }))}
          onChange={(mode) => change({ interval_minutes: mode === "follow" ? null : baseline.interval_minutes ?? DEFAULT_CUSTOM_MINUTES })} />
      </label>
      {draft.interval_minutes !== null && <label className="field"><span>{t("subscribe.feed.intervalMinutes")}</span>
        <input type="number" min={1} inputMode="numeric" value={Number.isNaN(draft.interval_minutes) ? "" : draft.interval_minutes}
          onChange={(e) => { const minutes = e.target.valueAsNumber; change({ interval_minutes: Number.isInteger(minutes) && minutes >= 1 ? minutes : Number.NaN }); }} />
      </label>}
    </div>
    {(() => {
      const line = refreshLine(feed, !badInterval && draft.interval_minutes !== baseline.interval_minutes);
      return line && <p className="hint feed-refresh">{line}</p>;
    })()}
    {limited ? <p className="notice warn"><Icon name="info" size={14} />{limited}</p>
      : feed.error && <p className="notice warn"><Icon name="info" size={14} />{formatFeedError(feed.error)}</p>}
    {missing && <p className="notice warn">{t("subscribe.feed.missing")}</p>}
    <div className="row form-actions">
      <button type="button" className="btn primary sm" disabled={missing || badDays || badInterval} onClick={() => void s.run(t("subscribe.action.save"), async () => ({
        tree: await api.saveFeed({ id: feed.id, title: draft.title.trim() !== baseline.title ? draft.title : null,
          url: draft.url !== baseline.url ? draft.url : null, folder_id: draft.folder_id !== baseline.folder_id ? draft.folder_id : null,
          retention: draft.retention_days !== baseline.retention_days ? retentionArg(draft.retention_days) : null,
          interval: draft.interval_minutes !== baseline.interval_minutes ? intervalArg(draft.interval_minutes) : null }),
      }), finish)}>{t("subscribe.saveEdit")}</button>
      <button type="button" className="btn sm" onClick={finish}>{t("subscribe.cancel")}</button>
      <span className="toolbar-spacer" />
      <button type="button" className="btn sm danger ghost" disabled={missing} onClick={unsubscribe}>{t("subscribe.delete")}</button>
    </div>
  </div>;
}
