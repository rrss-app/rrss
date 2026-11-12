import { useEffect, useRef, useState } from "react";
import { api, type InterestHitView, type RankingSettings } from "../lib/api";
import type { RankingToggle } from "../components/FocusPane";
import { Icon } from "../components/Icon";
import { Segmented } from "../components/Segmented";
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
      if (gen === hitsGen.current) { setHitsFor(0); setError("读取相关文章失败：" + String(err)); }
    } finally {
      if (gen === hitsGen.current) setLoadingHitsFor(0);
    }
  };
  const setRanking = (key: RankingToggle, value: boolean) => void run("更新排序设置", async () => {
    await onRanking(key, value);
  }, { readOnly: true, quiet: true }); // 焦点域负责串行保存、错误反馈和重读。

  return (
    <div className="memory-pane">
      {sub === "memory" && query.isPending && !busy && <Pending className="form-status">正在读取记忆…</Pending>}
      {sub === "memory" && query.isError && <div className="notice error" role="alert">
        {status?.tone === "err" ? status.text : "读取记忆失败：" + String(query.error)}
        <button type="button" className="btn sm" disabled={busy || query.isFetching} onClick={() => void refresh()}>重试读取</button>
      </div>}
      {sub === "interest" && interestQuery.isError && <div className="notice error" role="alert">
        {"读取兴趣失败：" + String(interestQuery.error)}
        <button type="button" className="btn sm" disabled={busy || interestQuery.isFetching} onClick={() => void refresh()}>重试读取</button>
      </div>}
      {error && <p className="notice error" role="alert">{error}</p>}
      {status && !(sub === "memory" && query.isError) && <p className={status.tone === "err" ? "notice error" : "form-status"} role={status.tone === "err" ? "alert" : "status"}>{status.text}</p>}
      {busy && <p className="form-status" role="status">正在处理操作…</p>}
      <fieldset className="settings-fields" disabled={busy}>
        <section className="settings-card settings-section settings-section-list memory-content-section">
          <div className="settings-section-meta"><h4>内容</h4><p className="hint">对话里提到的偏好会先放在「待确认」，确认后才生效。</p></div>
          <div className="settings-section-body">
            <div className="row memory-toolbar">
              <Segmented className="segmented memory-tabs" role="group" aria-label="记忆设置分类">
                <button type="button" data-on={sub === "memory" ? "1" : "0"} aria-pressed={sub === "memory"} onClick={() => setSub("memory")}>
                  记忆{data && <span className="settings-count">{data.memories.length}</span>}
                </button>
                <button type="button" data-on={sub === "interest" ? "1" : "0"} aria-pressed={sub === "interest"} onClick={() => setSub("interest")}>
                  兴趣{interestsData && <span className="settings-count">{interestsData.interests.length}</span>}
                </button>
              </Segmented>
              {sub === "memory" && <div className="search-field">
                <Icon name="search" size={14} />
                <input value={search} onChange={(e) => setSearch(e.target.value)}
                  onKeyDown={(e) => { if (e.key === "Enter" && !e.nativeEvent.isComposing) submitSearch(); }}
                  placeholder="搜索记忆，回车查找" aria-label="搜索记忆" />
              </div>}
            </div>
            {sub === "memory" && <>
              {data && visibleMemories.length === 0 && <div className="empty compact">
                <span className="empty-icon"><Icon name="memory" size={20} /></span>
                <p>{filter ? "没有匹配的记忆。" : "还没有记忆。对话中确认的偏好会出现在这里。"}</p>
              </div>}
              {visibleMemories.map((m) => (
                <div key={m.id} className={`card memory-card${m.pending ? " pending" : ""}`}>
                  <div className="memory-main">
                  {editing?.baseline.id === m.id ? <>
                    <textarea rows={3} aria-label="记忆内容" value={editing.content}
                      onChange={(e) => setEditing({ ...editing, content: e.target.value })} />
                    <div className="row">
                      <button type="button" className="btn primary sm" disabled={!editing.content.trim()} onClick={() => void saveEdit()}>保存</button>
                      <button type="button" className="btn sm" onClick={() => setEditing(null)}>取消</button>
                    </div>
                  </> : <div className="memory-head"><p className="memory-content">{m.content}</p>{m.pending && <span className="meta-tag warn">待确认</span>}</div>}
                  <div className="memory-meta">
                    <span className="meta-tag">{m.kind}</span>
                    {m.pinned && <span className="meta-tag accent"><Icon name="bookmark" size={11} />置顶</span>}
                    {m.scope && <span className="muted">{m.scope}</span>}
                  </div>
                  </div>
                  <div className="row memory-actions" hidden={editing?.baseline.id === m.id}>
                    <button type="button" className="btn sm ghost" onClick={() => void run("更新置顶", () => api.pinMemory(m.id, !m.pinned))}>{m.pinned ? "取消置顶" : "置顶"}</button>
                    <button type="button" className="btn sm ghost" onClick={() => beginEdit(m)}><Icon name="edit" size={13} />编辑</button>
                    {m.pending ? <>
                      <button type="button" className="btn sm" onClick={() => void run("添加为兴趣", () => api.memoryAsInterest(m.id), { interests: true, onSuccess: clearHits })}>添加为兴趣</button>
                      <button type="button" className="btn sm ghost memory-ignore" onClick={() => void run("丢弃记忆", () => api.discardMemory(m.id))}>忽略</button>
                      <button type="button" className="btn primary sm" onClick={() => void run("确认记忆", () => api.confirmMemory(m.id))}>确认</button>
                    </> : <button type="button" className="btn sm danger" onClick={() => onAsk("删除记忆", `删除「${m.content}」？`, () => void run("删除记忆", () => api.discardMemory(m.id)))}>删除</button>}
                  </div>
                </div>
              ))}
            </>}
            {sub === "interest" && <>
              <div className="row interest-add">
                <input value={label} aria-label="新兴趣标签" onChange={(e) => setLabel(e.target.value)}
                  onKeyDown={(e) => { if (e.key === "Enter" && !e.nativeEvent.isComposing) void addInterest(); }}
                  placeholder="新兴趣标签，例如「本地优先」" />
                <button type="button" className="btn primary" disabled={!label.trim()} onClick={() => void addInterest()}><Icon name="plus" size={14} />添加</button>
              </div>
              {!!interestsData?.interests.length && <div className="section-label">已确认兴趣</div>}
              {interestsData && interestsData.interests.length === 0 && <div className="empty compact">
                <span className="empty-icon"><Icon name="star" size={20} /></span>
                <p>还没有兴趣标签。可以在上方添加，也可以将待确认的记忆添加为兴趣。</p>
              </div>}
              {interestsData?.interests.map((i) => (
                <div key={i.id} className={`list-row interest-row${i.enabled ? "" : " off"}`}>
                  <div><span className={i.enabled ? "" : "muted"}>{i.label}</span>
                    <span className="muted interest-meta">{i.has_embedding ? "语义匹配已就绪" : "语义匹配尚未就绪"} · {i.hit_count} 篇相关内容</span>
                  </div>
                  <div className="settings-actions">
                    <button type="button" className="btn sm ghost" aria-expanded={hitsFor === i.id}
                      aria-controls={hitsFor === i.id ? "interest-hits" : undefined} onClick={() => void showHits(i.id)}>
                      {loadingHitsFor === i.id ? "读取中…" : hitsFor === i.id ? "收起文章" : "相关文章"}
                    </button>
                    <Toggle label={"兴趣 " + i.label} on={i.enabled} onLabel="已启用" offLabel="已停用"
                      onClick={() => void run("更新兴趣", () => api.setInterestEnabled(i.id, !i.enabled), { interests: true, onSuccess: clearHits })} />
                    <button type="button" className="btn sm danger ghost" aria-label={`删除兴趣 ${i.label}`}
                      onClick={() => void run("删除兴趣", () => api.deleteInterest(i.id), { interests: true, onSuccess: clearHits })}><Icon name="trash" size={13} />删除</button>
                  </div>
                </div>
              ))}
              {hitsFor > 0 && <section id="interest-hits" className="settings-card settings-section settings-section-list" aria-label="兴趣相关文章">
                <div className="settings-section-meta">
                  <h4>{interestsData?.interests.find((interest) => interest.id === hitsFor)?.label} · 相关文章</h4>
                </div>
                <div className="settings-section-body">
                  {loadingHitsFor ? <p className="hint" role="status">正在读取相关文章…</p> : hits.length === 0 ? <p className="hint" role="status">暂时没有匹配的文章。</p> : null}
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
            <summary>导出与清空记忆</summary>
            <p className="hint">导出为 JSON 文件以便留存。清空会永久删除全部记忆，操作前会再次确认。</p>
            <div className="settings-actions">
              <button type="button" className="btn" onClick={() => void run("导出记忆", () => api.exportMemories(), {
                readOnly: true, onSuccess: (text) => downloadText("rrss-memories.json", text, "application/json"),
              })}><Icon name="download" size={14} />导出记忆</button>
              <button type="button" className="btn danger" onClick={() => onAsk("清空记忆", "删除全部记忆条目？此操作不可恢复。", () => {
                void run("清空记忆", () => api.clearMemories(), { onSuccess: () => setEditing(null) });
              })}><Icon name="trash" size={14} />清空全部记忆</button>
            </div>
          </details>
        </>}
        <section className="settings-card settings-section">
          <div className="settings-section-meta">
            <h4>排序</h4>
            <p className="hint">只影响焦点里的先后，不会隐藏文章。</p>
          </div>
          <div className="settings-section-body">
            <SettingRow label="兴趣加分" hint="为与你已确认兴趣相关的可见文章加分。">
              <Toggle label="兴趣加分" on={ranking.interest_boost} onLabel="" offLabel="" onClick={() => setRanking("interest_boost", !ranking.interest_boost)} />
            </SettingRow>
            <SettingRow label="用兴趣重排焦点" hint="关闭后只按时间和规则分数排。">
              <Toggle label="用兴趣重排焦点" on={ranking.rerank} onLabel="" offLabel="" onClick={() => setRanking("rerank", !ranking.rerank)} />
            </SettingRow>
            <p className="hint">{ranking.ranking_note || "兴趣只影响可见文章的评分与排序，不会取消规则隐藏。"}</p>
          </div>
        </section>
      </fieldset>
    </div>
  );
}
