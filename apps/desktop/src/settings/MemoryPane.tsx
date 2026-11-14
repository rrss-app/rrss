import { formatError } from "../lib/errors";
import { useEffect, useRef, useState } from "react";
import { api, type InterestHitView, type RankingSettings } from "../lib/api";
import type { RankingToggle } from "../components/FocusPane";
import { Icon } from "../components/Icon";
import { Segmented } from "../components/Segmented";
import { useTranslation } from "../i18n";
import { SettingRow, Toggle } from "./chips";
import { downloadText } from "./io";
import type { SettingsEditState } from "./editState";
import { useMemorySettings } from "./useMemorySettings";
import { Pending } from "../components/Pending";

export function MemoryPane({ ranking, onRanking, onAsk, onOpenArticle, onChanged, onEditStateChange }: {
  ranking: RankingSettings;
  onRanking: (key: RankingToggle, value: boolean) => void | Promise<void>;
  onAsk: (title: string, message: string, onOk: () => void) => void;
  onOpenArticle: (id: number) => void;
  onChanged?: () => void;
  onEditStateChange?: (state: SettingsEditState | null) => void;
}) {
  const { t } = useTranslation("settingsAssistant");
  const s = useMemorySettings(onAsk, onChanged, onEditStateChange);
  const { query, interestQuery, sub, setSub, search, setSearch, filter, submitSearch, refresh, label, setLabel, editing, setEditing,
    beginEdit, saveEdit, addInterest, status, busy, run, visibleMemories } = s;
  const data = query.data;
  const interestsData = interestQuery.data;
  const [hits, setHits] = useState<InterestHitView[]>([]);
  const [hitsFor, setHitsFor] = useState(0);
  const [error, setError] = useState("");
  const [loadingHitsFor, setLoadingHitsFor] = useState(0);
  const hitsGen = useRef(0);
  useEffect(() => {
    const generation = hitsGen;
    return () => { ++generation.current; };
  }, []);

  const clearHits = () => {
    ++hitsGen.current;
    setHits([]);
    setHitsFor(0);
    setLoadingHitsFor(0);
  };
  const showHits = async (id: number) => {
    const gen = ++hitsGen.current;
    setError("");
    setHits([]);
    if (hitsFor === id) { setHitsFor(0); setLoadingHitsFor(0); return; }
    setHitsFor(id);
    setLoadingHitsFor(id);
    try {
      const result = await api.interestHits(id);
      if (gen === hitsGen.current) setHits(result);
    } catch (err) {
      if (gen === hitsGen.current) { setHitsFor(0); setError(t("memory.hitsFailed") + formatError(err)); }
    } finally {
      if (gen === hitsGen.current) setLoadingHitsFor(0);
    }
  };
  const setRanking = (key: RankingToggle, value: boolean) => void run(t("memory.op.updateRanking"), async () => {
    await onRanking(key, value);
  }, { readOnly: true, quiet: true }); // 焦点域负责串行保存、错误反馈和重读。

  return (
    <div className="memory-pane">
      {sub === "memory" && query.isPending && !busy && <Pending className="form-status">{t("memory.loading")}</Pending>}
      {sub === "memory" && query.isError && <div className="notice error" role="alert">
        {status?.tone === "err" ? status.text : t("memory.loadFailed") + formatError(query.error)}
        <button type="button" className="btn sm" disabled={busy || query.isFetching} onClick={() => void refresh()}>{t("memory.retry")}</button>
      </div>}
      {sub === "interest" && interestQuery.isError && <div className="notice error" role="alert">
        {t("memory.interestLoadFailed") + formatError(interestQuery.error)}
        <button type="button" className="btn sm" disabled={busy || interestQuery.isFetching} onClick={() => void refresh()}>{t("memory.retry")}</button>
      </div>}
      {error && <p className="notice error" role="alert">{error}</p>}
      {status && !(sub === "memory" && query.isError) && <p className={status.tone === "err" ? "notice error" : "form-status"} role={status.tone === "err" ? "alert" : "status"}>{status.text}</p>}
      {busy && <p className="form-status" role="status">{t("memory.processing")}</p>}
      <fieldset className="settings-fields" disabled={busy}>
        <section className="settings-card settings-section settings-section-list memory-content-section">
          <div className="settings-section-meta"><h4>{t("memory.content.title")}</h4><p className="hint">{t("memory.content.hint")}</p></div>
          <div className="settings-section-body">
            <div className="row memory-toolbar">
              <Segmented className="segmented memory-tabs" role="group" aria-label={t("memory.tabsLabel")}>
                <button type="button" data-on={sub === "memory" ? "1" : "0"} aria-pressed={sub === "memory"} onClick={() => setSub("memory")}>
                  {t("memory.tab.memory")}{data && <span className="settings-count">{data.memories.length}</span>}
                </button>
                <button type="button" data-on={sub === "interest" ? "1" : "0"} aria-pressed={sub === "interest"} onClick={() => setSub("interest")}>
                  {t("memory.tab.interest")}{interestsData && <span className="settings-count">{interestsData.interests.length}</span>}
                </button>
              </Segmented>
              {sub === "memory" && <div className="search-field">
                <Icon name="search" size={14} />
                <input value={search} onChange={(e) => setSearch(e.target.value)}
                  onKeyDown={(e) => { if (e.key === "Enter" && !e.nativeEvent.isComposing) submitSearch(); }}
                  placeholder={t("memory.searchPlaceholder")} aria-label={t("memory.searchLabel")} />
              </div>}
            </div>
            {sub === "memory" && <>
              {data && visibleMemories.length === 0 && <div className="empty compact">
                <span className="empty-icon"><Icon name="memory" size={20} /></span>
                <p>{filter ? t("memory.noMatch") : t("memory.empty")}</p>
              </div>}
              {visibleMemories.map((m) => (
                <div key={m.id} className={`card memory-card${m.pending ? " pending" : ""}`}>
                  <div className="memory-main">
                  {editing?.baseline.id === m.id ? <>
                    <textarea rows={3} aria-label={t("memory.editLabel")} value={editing.content}
                      onChange={(e) => setEditing({ ...editing, content: e.target.value })} />
                    <div className="row">
                      <button type="button" className="btn primary sm" disabled={!editing.content.trim()} onClick={() => void saveEdit()}>{t("memory.save")}</button>
                      <button type="button" className="btn sm" onClick={() => setEditing(null)}>{t("memory.cancel")}</button>
                    </div>
                  </> : <div className="memory-head"><p className="memory-content">{m.content}</p>{m.pending && <span className="meta-tag warn">{t("memory.pendingTag")}</span>}</div>}
                  <div className="memory-meta">
                    <span className="meta-tag">{m.kind}</span>
                    {m.pinned && <span className="meta-tag accent"><Icon name="bookmark" size={11} />{t("memory.pinned")}</span>}
                    {m.scope && <span className="muted">{m.scope}</span>}
                  </div>
                  </div>
                  <div className="row memory-actions" hidden={editing?.baseline.id === m.id}>
                    <button type="button" className="btn sm ghost" onClick={() => void run(t("memory.op.pin"), () => api.pinMemory(m.id, !m.pinned))}>{m.pinned ? t("memory.unpin") : t("memory.pin")}</button>
                    <button type="button" className="btn sm ghost" onClick={() => beginEdit(m)}><Icon name="edit" size={13} />{t("memory.edit")}</button>
                    {m.pending ? <>
                      <button type="button" className="btn sm" onClick={() => void run(t("memory.op.memoryAsInterest"), () => api.memoryAsInterest(m.id), { interests: true, onSuccess: clearHits })}>{t("memory.addAsInterest")}</button>
                      <button type="button" className="btn sm ghost memory-ignore" onClick={() => void run(t("memory.op.discard"), () => api.discardMemory(m.id))}>{t("memory.ignore")}</button>
                      <button type="button" className="btn primary sm" onClick={() => void run(t("memory.op.confirm"), () => api.confirmMemory(m.id))}>{t("memory.confirm")}</button>
                    </> : <button type="button" className="btn sm danger" onClick={() => onAsk(t("memory.deleteConfirm.title"), t("memory.deleteConfirm.message", { content: m.content }), () => void run(t("memory.op.delete"), () => api.discardMemory(m.id)))}>{t("memory.delete")}</button>}
                  </div>
                </div>
              ))}
            </>}
            {sub === "interest" && <>
              <div className="row interest-add">
                <input value={label} aria-label={t("memory.interest.newLabel")} onChange={(e) => setLabel(e.target.value)}
                  onKeyDown={(e) => { if (e.key === "Enter" && !e.nativeEvent.isComposing) void addInterest(); }}
                  placeholder={t("memory.interest.newPlaceholder")} />
                <button type="button" className="btn primary" disabled={!label.trim()} onClick={() => void addInterest()}><Icon name="plus" size={14} />{t("memory.interest.add")}</button>
              </div>
              {!!interestsData?.interests.length && <div className="section-label">{t("memory.interest.confirmed")}</div>}
              {interestsData && interestsData.interests.length === 0 && <div className="empty compact">
                <span className="empty-icon"><Icon name="star" size={20} /></span>
                <p>{t("memory.interest.empty")}</p>
              </div>}
              {interestsData?.interests.map((i) => (
                <div key={i.id} className={`list-row interest-row${i.enabled ? "" : " off"}`}>
                  <div><span className={i.enabled ? "" : "muted"}>{i.label}</span>
                    <span className="muted interest-meta">{i.has_embedding ? t("memory.interest.embeddingReady") : t("memory.interest.embeddingNot")} · {t("memory.interest.hits", { count: i.hit_count })}</span>
                  </div>
                  <div className="settings-actions">
                    <button type="button" className="btn sm ghost" aria-expanded={hitsFor === i.id}
                      aria-controls={hitsFor === i.id ? "interest-hits" : undefined} onClick={() => void showHits(i.id)}>
                      {loadingHitsFor === i.id ? t("memory.interest.loadingHits") : hitsFor === i.id ? t("memory.interest.collapse") : t("memory.interest.related")}
                    </button>
                    <Toggle label={t("memory.interest.toggle", { label: i.label })} on={i.enabled} onLabel={t("memory.interest.on")} offLabel={t("memory.interest.off")}
                      onClick={() => void run(t("memory.op.updateInterest"), () => api.setInterestEnabled(i.id, !i.enabled), { interests: true, onSuccess: clearHits })} />
                    <button type="button" className="btn sm danger ghost" aria-label={t("memory.interest.deleteAria", { label: i.label })}
                      onClick={() => void run(t("memory.op.deleteInterest"), () => api.deleteInterest(i.id), { interests: true, onSuccess: clearHits })}><Icon name="trash" size={13} />{t("memory.interest.delete")}</button>
                  </div>
                </div>
              ))}
              {hitsFor > 0 && <section id="interest-hits" className="settings-card settings-section settings-section-list" aria-label={t("memory.interest.hitsAria")}>
                <div className="settings-section-meta">
                  <h4>{t("memory.interest.hitsTitle", { label: interestsData?.interests.find((interest) => interest.id === hitsFor)?.label ?? "" })}</h4>
                </div>
                <div className="settings-section-body">
                  {loadingHitsFor ? <p className="hint" role="status">{t("memory.interest.loadingHitsStatus")}</p> : hits.length === 0 ? <p className="hint" role="status">{t("memory.interest.noHits")}</p> : null}
                  {hits.map((h) => <button key={h.id} type="button" className="hit-row" onClick={() => onOpenArticle(h.id)}>
                    <span className="hit-title">{h.title}</span><span className="muted">{h.feed_title}</span>
                  </button>)}
                </div>
              </section>}
            </>}
          </div>
        </section>
        {sub === "memory" && <>
          <details className="advanced-fold">
            <summary>{t("memory.exportFold.summary")}</summary>
            <p className="hint">{t("memory.exportFold.hint")}</p>
            <div className="settings-actions">
              <button type="button" className="btn" onClick={() => void run(t("memory.op.export"), () => api.exportMemories(), {
                readOnly: true, onSuccess: (text) => downloadText("rrss-memories.json", text, "application/json"),
              })}><Icon name="download" size={14} />{t("memory.exportFold.export")}</button>
              <button type="button" className="btn danger" onClick={() => onAsk(t("memory.exportFold.clearTitle"), t("memory.exportFold.clearMessage"), () => {
                void run(t("memory.op.clear"), () => api.clearMemories(), { onSuccess: () => setEditing(null) });
              })}><Icon name="trash" size={14} />{t("memory.exportFold.clear")}</button>
            </div>
          </details>
        </>}
        <section className="settings-card settings-section">
          <div className="settings-section-meta">
            <h4>{t("memory.ranking.title")}</h4>
            <p className="hint">{t("memory.ranking.hint")}</p>
          </div>
          <div className="settings-section-body">
            <SettingRow label={t("memory.ranking.boost")} hint={t("memory.ranking.boostHint")}>
              <Toggle label={t("memory.ranking.boost")} on={ranking.interest_boost} onLabel="" offLabel="" onClick={() => setRanking("interest_boost", !ranking.interest_boost)} />
            </SettingRow>
            <SettingRow label={t("memory.ranking.rerank")} hint={t("memory.ranking.rerankHint")}>
              <Toggle label={t("memory.ranking.rerank")} on={ranking.rerank} onLabel="" offLabel="" onClick={() => setRanking("rerank", !ranking.rerank)} />
            </SettingRow>
            <p className="hint">{ranking.ranking_note || t("memory.ranking.note")}</p>
          </div>
        </section>
      </fieldset>
    </div>
  );
}
