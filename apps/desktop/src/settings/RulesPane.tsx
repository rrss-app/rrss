import { useRef, useState } from "react";
import { api, type PresetView, type RuleView, type ScopeOption } from "../lib/api";
import { Icon } from "../components/Icon";
import { ThumbIcon } from "../components/ThumbIcon";
import { ChipBar, Field, SettingRow, Toggle } from "./chips";
import { Stepper } from "./Stepper";
import type { Ask } from "./SettingsPanel";
import type { SettingsEditState } from "./editState";
import { useRulesSettings } from "./useRulesSettings";
import { Pending } from "../components/Pending";
import { MenuSelect } from "../components/MenuSelect";
import { useQuery } from "@tanstack/react-query";
import { HiddenListDialog } from "../features/focus";
import { formatPublished, useMinuteClock } from "../lib/time";
import { settingsSchemaOptions } from "./settingsQuery";
import { readGate } from "./editState";

const POLARITY = [
  { id: "like", label: "加" },
  { id: "dislike", label: "减" },
];
const DIMENSIONS = [
  { id: "title_kw", label: "标题" },
  { id: "title_re", label: "标题正则" },
  { id: "author", label: "作者" },
  { id: "tag", label: "标签" },
  { id: "url_phrase", label: "URL" },
  { id: "url_re", label: "URL 正则" },
  { id: "body_phrase", label: "正文" },
  { id: "body_re", label: "正文正则" },
];
const SCOPES = [
  { id: "global", label: "全局" },
  { id: "folder", label: "文件夹" },
  { id: "feed", label: "订阅源" },
];
const FILTERS = [
  { id: "all", label: "全部" },
  { id: "like", label: "加分" },
  { id: "dislike", label: "减分" },
  { id: "pending", label: "待确认" },
];

function ruleVerb(dimensionId: string): string {
  return dimensionId.endsWith("_re") ? "匹配正则" : "包含";
}

export function RulesPane({ onChanged, onEditStateChange, onAsk, onOpenArticle }: {
  onAsk?: Ask;
  onOpenArticle?: (id: number) => void;
  onChanged?: () => void;
  onEditStateChange?: (state: SettingsEditState | null) => void;
}) {
  const { query, filter, changeFilter, editId, spec, setSpec, begin, closeForm, save, threshold, setThreshold,
    saveThreshold, status, busy, run, refresh } = useRulesSettings(onChanged, onEditStateChange);
  const stats = useQuery({ queryKey: ["rules", "stats"], queryFn: api.ruleStats, retry: false, refetchOnWindowFocus: false, ...readGate(busy) });
  const schema = useQuery({ ...settingsSchemaOptions(), ...readGate(busy) });
  const rate = schema.data?.values.find(value => value.id === "hidden_spot_check_rate")?.value ?? "3";
  const [hidden, setHidden] = useState(false);
  const now = useMinuteClock();
  const data = query.data;
  const spot = stats.data?.spot_check;
  const patternRef = useRef<HTMLInputElement>(null);
  // 编辑沿用顶部的一句话构造器：载入后把它带进视野并聚焦，长列表底部点「编辑」也看得到。
  const pick = (rule: RuleView) => {
    begin(rule.id, {
      polarity: rule.polarity, dimension: rule.dimension_id, pattern: rule.pattern, weight: rule.weight,
      scope_kind: rule.scope_kind, scope_id: rule.scope_id,
    });
    requestAnimationFrame(() => {
      patternRef.current?.scrollIntoView({ block: "center" });
      patternRef.current?.focus({ preventScroll: true });
    });
  };
  return <div className="rules-pane">
    {query.isPending && !data && <Pending>正在读取规则…</Pending>}
    {query.isError && <div className="notice error" role="alert">读取规则失败，当前表单未被覆盖：{String(query.error)} <button type="button" className="btn sm" disabled={busy} onClick={() => void refresh()}>重试读取</button></div>}
    {status && <p className={`form-status${status.tone === "err" ? " error" : ""}`} role={status.tone === "err" ? "alert" : "status"}>{status.text}</p>}
    <fieldset className="settings-fields" disabled={busy} aria-busy={busy}>
      <section className="settings-card settings-section">
        <div className="settings-section-meta">
          <h4>{editId ? "修改规则" : "新建规则"}</h4>
          <p className="hint">读起来像一句话：当什么包含什么，就加减多少分。</p>
        </div>
        <div className="settings-section-body">
          <div className="rule-composer">
            <span>当</span>
            <MenuSelect label="匹配位置" value={spec.dimension} onChange={(dimension) => setSpec((s) => ({ ...s, dimension }))}
              options={DIMENSIONS.map((d) => ({ value: d.id, label: d.label }))} />
            <span>{ruleVerb(spec.dimension)}</span>
            <input ref={patternRef} className="rule-pattern" aria-label="匹配内容" value={spec.pattern} placeholder="词或短语"
              onChange={(e) => setSpec((s) => ({ ...s, pattern: e.target.value }))} />
            <ChipBar className="segmented" value={spec.polarity} options={POLARITY} onChange={(polarity) => setSpec((s) => ({ ...s, polarity }))} />
            <Stepper label="评分权重" min={1} value={String(spec.weight)} onChange={(weight) => setSpec((s) => ({ ...s, weight: Number(weight) || 0 }))} />
            <span>分</span>
            <button type="button" className="btn primary" onClick={save}>{editId ? "保存修改" : "添加"}</button>
          </div>
          <details className="settings-more" open={editId > 0 || undefined}>
            <summary>更多</summary>
            <Field label="生效范围">
              <ChipBar value={spec.scope_kind} options={SCOPES} onChange={(scope_kind) => setSpec((s) => ({ ...s, scope_kind, scope_id: scope_kind === "global" ? 0 : s.scope_id }))} />
            </Field>
            {spec.scope_kind !== "global" && <ScopeList options={spec.scope_kind === "folder" ? data?.folders : data?.feeds} failed={query.isError}
              value={spec.scope_id} onChange={(scope_id) => setSpec((s) => ({ ...s, scope_id }))} />}
          </details>
          {editId > 0 && <button type="button" className="btn sm" onClick={closeForm}>取消</button>}
        </div>
      </section>
      {[{ title: "建议规则", suggested: true }, { title: "已有规则", suggested: false }].map(group => {
        const rules = data?.rules.filter(rule => rule.pending === group.suggested) ?? [];
        if (group.suggested && !rules.length) return null;
        return <section key={group.title} className="settings-card settings-section settings-section-list">
        <div className="settings-section-meta">
          <h4>{group.title} {data && <span className="settings-count">{rules.length}</span>}</h4>
          <p className="hint">{group.suggested ? "建议只在这里等待确认，不会自动启用。" : "停用的规则保留，可以随时再打开。"}</p>
        </div>
        <div className="settings-section-body">
          {!group.suggested && <details className="rule-filter"><summary>筛选规则</summary><ChipBar value={filter} options={FILTERS} onChange={changeFilter} /></details>}
          {data && !rules.length && <p className="settings-empty">{filter === "all" ? "还没有规则。可以新建规则，或启用下方预设。" : "此分类下没有规则。可以切换到「全部」查看其他规则。"}</p>}
          <div className="rule-list">
            {rules.map((r) => {
              const stat = stats.data?.rules.find(stat => stat.rule_id === r.id);
              return <div key={r.id} data-rule-id={r.id} data-pending={r.pending} className={`rule-row${r.enabled || r.pending ? "" : " off"}`}>
              <span aria-hidden="true" className={`rule-score ${r.polarity === "like" ? "up" : "down"}`}><ThumbIcon down={r.polarity !== "like"} /></span>
              <div className="rule-pick">
                <span className="rule-summary">{r.summary}</span>
                <span className="muted rule-meta">{r.dimension} · {r.pattern} · {r.scope_label} · {r.pack_id ? "来自预设" : ["manual", "user"].includes(r.source) ? "手动添加" : r.source === "suggest" ? "建议" : r.source}
                  {r.pending ? " · 待确认建议" : ""}{!r.enabled && !r.pending ? " · 已停用" : ""}</span>
                {stat && <span className="muted rule-stats">近 30 天命中 {stat.hits_30d} 篇 · 最近 {formatPublished(stat.last_hit_at, now) || "暂无"}{stat.hidden_count > 0 ? ` · 经它隐藏 ${stat.hidden_count} 篇` : ""}</span>}
                {stat && r.enabled && stat.advice !== "none" && <div className="rule-advice">
                  <span className="meta-tag warn">{stat.advice === "stale" ? "90 天没有命中，可以清理" : "命中较宽"}</span>
                  <button type="button" className="btn sm ghost" onClick={() => stat.advice === "stale"
                    ? void run("停用规则", () => api.setRuleAction(r.id, 2)) : pick(r)}>{stat.advice === "stale" ? "停用" : "收窄…"}</button>
                </div>}
              </div>
              <div className="row rule-actions">
                <button type="button" className="btn sm ghost rule-edit" aria-label={`编辑规则 ${r.pattern}`} onClick={() => pick(r)}>编辑</button>
                {r.pending ? <>
                  <button type="button" className="btn primary sm" onClick={() => void run("确认规则", () => api.confirmRule(r.id))}>确认</button>
                  <button type="button" className="btn sm danger" onClick={() => void run("丢弃规则", () => api.discardRule(r.id))}>丢弃</button>
                </> : <>
                  {onAsk && <button type="button" className="btn sm danger rule-edit" onClick={() => onAsk("删除规则", `删除「${r.pattern}」这条规则？`, () => void run("删除规则", () => api.discardRule(r.id)))}>删除</button>}
                  <Toggle label={`启用规则 ${r.pattern}`} on={r.enabled} onLabel="" offLabel=""
                    onClick={() => void run("更新规则", () => api.setRuleAction(r.id, r.enabled ? 2 : r.polarity === "like" ? 0 : 1))} />
                </>}
              </div>
            </div>; })}
          </div>
        </div>
      </section>; })}
      <section className="settings-card rules-advanced settings-section">
        <div className="settings-section-meta"><h4>隐藏</h4></div>
        <div className="settings-section-body">
          <SettingRow label="隐藏阈值" hint="分数低于这个值的文章不出现在焦点里。">
            <Stepper label="隐藏阈值" value={threshold} onChange={setThreshold} onCommit={saveThreshold} />
          </SettingRow>
          <SettingRow label="隐藏抽查" hint="随机放回少量本该隐藏的文章，看规则有没有误杀。">
            <ChipBar className="segmented" value={rate} disabled={!schema.data} options={[{ id: "0", label: "关" }, { id: "1", label: "1%" }, { id: "3", label: "3%" }, { id: "5", label: "5%" }]}
              onChange={value => void run("保存隐藏抽查比例", () => api.setSetting("hidden_spot_check_rate", value))} />
          </SettingRow>
          {stats.isPending && <Pending>正在读取隐藏抽查统计…</Pending>}
          {(stats.isError || schema.isError) && <p className="notice error" role="alert">读取抽查设置或统计失败。<button className="btn sm" onClick={() => { void stats.refetch(); void schema.refetch(); }}>重试读取</button></p>}
          {spot && <p className="rule-spot-stats">近 30 天放回 {spot.exposed} 篇：你打开或赞了 {spot.liked} 篇，踩了 {spot.disliked} 篇。
            {spot.insufficient ? "样本还少，暂不估计误杀率" : `估计误杀约 ${Math.round(spot.liked / Math.max(1, spot.exposed) * 100)}%`}
            {spot.liked > 0 && onOpenArticle && <button className="link-btn" onClick={() => setHidden(true)}>查看你赞过的 {spot.liked} 篇</button>}
          </p>}
          <p className="hint">放回的文章和普通文章看起来一样；打开「推荐详情」后才标出「抽查」并在反馈后说明原因。</p>
          <SettingRow label="历史文章" hint="修改规则后，可重新计算已有文章的评分。">
            <button type="button" className="btn" onClick={() => void run("重新评分", api.rescore)}><Icon name="refresh" size={14} />重新评分</button>
          </SettingRow>
        </div>
      </section>
      <details className="advanced-fold"><summary>预设</summary><PresetList presets={data?.presets ?? []} run={run} /></details>
    </fieldset>
    {hidden && onOpenArticle && <HiddenListDialog initialFilter="spot_liked" onOpenArticle={onOpenArticle}
      onClose={() => setHidden(false)} onChanged={onChanged} />}
  </div>;
}

function PresetList({ presets, run }: {
  presets: PresetView[];
  run: (label: string, action: () => Promise<unknown>, onSuccess?: (value?: unknown) => void) => Promise<void>;
}) {
  if (presets.length === 0) return <p className="muted">没有可用预设。</p>;
  return (
    <div className="preset-list">
      {presets.map((p) => (
        <div key={p.id} className={`card preset-card${p.installed ? " on" : ""}`}>
          <div className="preset-body">
            <strong>{p.name}</strong>
            <p className="muted">{p.description}</p>
            <p className="hint">
              {p.installed ? `已启用 ${p.enabled_count} / ${p.rule_count} 条` : `${p.rule_count} 条规则，未安装`}
            </p>
          </div>
          <button
            type="button"
            className={p.installed ? "btn sm danger" : "btn sm primary"}
            onClick={() => { void run(p.installed ? "移除预设" : "启用预设", () => p.installed ? api.removePreset(p.id) : api.applyPreset(p.id)); }}
          >
            {p.installed ? "移除" : "启用"}
          </button>
        </div>
      ))}
    </div>
  );
}

function ScopeList({
  options,
  failed,
  value,
  onChange,
}: {
  options: ScopeOption[] | undefined;
  failed: boolean;
  value: number;
  onChange: (id: number) => void;
}) {
  if (!options) return <p className="hint" role="status">{failed ? "生效范围读取失败，请重试读取规则。" : "正在读取生效范围…"}</p>;
  if (options.length === 0) return <p className="muted">暂无可选范围，请先在「订阅」中添加订阅源或文件夹。</p>;
  return (
    <div className="scope-list" role="group" aria-label="选择生效范围">
      {options.map((o) => (
        <button
          key={o.id}
          type="button"
          className="chip-btn"
          data-on={value === o.id ? "1" : "0"}
          aria-pressed={value === o.id}
          onClick={() => onChange(o.id)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}
