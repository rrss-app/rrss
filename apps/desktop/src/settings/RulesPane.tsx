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
import { useTranslation } from "../i18n";

const POLARITY = ["like", "dislike"] as const;
const DIMENSIONS = ["title_kw", "title_re", "author", "tag", "url_phrase", "url_re", "body_phrase", "body_re"] as const;
const SCOPES = ["global", "folder", "feed"] as const;
const FILTERS = ["all", "like", "dislike", "pending"] as const;

export function RulesPane({ onChanged, onEditStateChange, onAsk, onOpenArticle }: {
  onAsk?: Ask;
  onOpenArticle?: (id: number) => void;
  onChanged?: () => void;
  onEditStateChange?: (state: SettingsEditState | null) => void;
}) {
  const { t } = useTranslation("settingsContent");
  const ruleVerb = (dimensionId: string): string => dimensionId.endsWith("_re") ? t("rules.verb.regex") : t("rules.verb.contains");
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
    {query.isPending && !data && <Pending>{t("rules.loading")}</Pending>}
    {query.isError && <div className="notice error" role="alert">{t("rules.readFailed", { error: String(query.error) })} <button type="button" className="btn sm" disabled={busy} onClick={() => void refresh()}>{t("rules.retryRead")}</button></div>}
    {status && <p className={`form-status${status.tone === "err" ? " error" : ""}`} role={status.tone === "err" ? "alert" : "status"}>{status.text}</p>}
    <fieldset className="settings-fields" disabled={busy} aria-busy={busy}>
      <section className="settings-card settings-section">
        <div className="settings-section-meta">
          <h4>{editId ? t("rules.composer.editTitle") : t("rules.composer.newTitle")}</h4>
          <p className="hint">{t("rules.composer.hint")}</p>
        </div>
        <div className="settings-section-body">
          <div className="rule-composer">
            <span>{t("rules.composer.when")}</span>
            <MenuSelect label={t("rules.composer.position")} value={spec.dimension} onChange={(dimension) => setSpec((s) => ({ ...s, dimension }))}
              options={DIMENSIONS.map((d) => ({ value: d, label: t(`rules.dimension.${d}`) }))} />
            <span>{ruleVerb(spec.dimension)}</span>
            <input ref={patternRef} className="rule-pattern" aria-label={t("rules.composer.pattern")} value={spec.pattern} placeholder={t("rules.composer.patternPlaceholder")}
              onChange={(e) => setSpec((s) => ({ ...s, pattern: e.target.value }))} />
            <ChipBar className="segmented" value={spec.polarity} options={POLARITY.map((id) => ({ id, label: t(`rules.polarity.${id}`) }))} onChange={(polarity) => setSpec((s) => ({ ...s, polarity }))} />
            <Stepper label={t("rules.composer.weight")} min={1} value={String(spec.weight)} onChange={(weight) => setSpec((s) => ({ ...s, weight: Number(weight) || 0 }))} />
            <span>{t("rules.composer.points")}</span>
            <button type="button" className="btn primary" onClick={save}>{editId ? t("rules.composer.saveEdit") : t("rules.composer.add")}</button>
          </div>
          <details className="settings-more" open={editId > 0 || undefined}>
            <summary>{t("rules.composer.more")}</summary>
            <Field label={t("rules.composer.scope")}>
              <ChipBar value={spec.scope_kind} options={SCOPES.map((id) => ({ id, label: t(`rules.scopeKind.${id}`) }))} onChange={(scope_kind) => setSpec((s) => ({ ...s, scope_kind, scope_id: scope_kind === "global" ? 0 : s.scope_id }))} />
            </Field>
            {spec.scope_kind !== "global" && <ScopeList options={spec.scope_kind === "folder" ? data?.folders : data?.feeds} failed={query.isError}
              value={spec.scope_id} onChange={(scope_id) => setSpec((s) => ({ ...s, scope_id }))} />}
          </details>
          {editId > 0 && <button type="button" className="btn sm" onClick={closeForm}>{t("rules.composer.cancel")}</button>}
        </div>
      </section>
      {([{ key: "suggested", suggested: true }, { key: "existing", suggested: false }] as const).map(group => {
        const rules = data?.rules.filter(rule => rule.pending === group.suggested) ?? [];
        if (group.suggested && !rules.length) return null;
        return <section key={group.key} className="settings-card settings-section settings-section-list">
        <div className="settings-section-meta">
          <h4>{t(`rules.group.${group.key}.title`)} {data && <span className="settings-count">{rules.length}</span>}</h4>
          <p className="hint">{t(`rules.group.${group.key}.hint`)}</p>
        </div>
        <div className="settings-section-body">
          {!group.suggested && <details className="rule-filter"><summary>{t("rules.filter.summary")}</summary><ChipBar value={filter} options={FILTERS.map((id) => ({ id, label: t(`rules.filter.${id}`) }))} onChange={changeFilter} /></details>}
          {data && !rules.length && <p className="settings-empty">{filter === "all" ? t("rules.empty.all") : t("rules.empty.filtered")}</p>}
          <div className="rule-list">
            {rules.map((r) => {
              const stat = stats.data?.rules.find(stat => stat.rule_id === r.id);
              return <div key={r.id} data-rule-id={r.id} data-pending={r.pending} className={`rule-row${r.enabled || r.pending ? "" : " off"}`}>
              <span aria-hidden="true" className={`rule-score ${r.polarity === "like" ? "up" : "down"}`}><ThumbIcon down={r.polarity !== "like"} /></span>
              <div className="rule-pick">
                <span className="rule-summary">{r.summary}</span>
                <span className="muted rule-meta">{r.dimension} · {r.pattern} · {r.scope_label} · {r.pack_id ? t("rules.meta.preset") : ["manual", "user"].includes(r.source) ? t("rules.meta.manual") : r.source === "suggest" ? t("rules.meta.suggest") : r.source}
                  {r.pending ? ` · ${t("rules.meta.pending")}` : ""}{!r.enabled && !r.pending ? ` · ${t("rules.meta.disabled")}` : ""}</span>
                {stat && <span className="muted rule-stats">{t(stat.hidden_count > 0 ? "rules.stats.withHidden" : "rules.stats.hits", { hits: stat.hits_30d, last: formatPublished(stat.last_hit_at, now) || t("rules.stats.none"), hidden: stat.hidden_count })}</span>}
                {stat && r.enabled && stat.advice !== "none" && <div className="rule-advice">
                  <span className="meta-tag warn">{stat.advice === "stale" ? t("rules.advice.stale") : t("rules.advice.broad")}</span>
                  <button type="button" className="btn sm ghost" onClick={() => stat.advice === "stale"
                    ? void run(t("rules.action.disable"), () => api.setRuleAction(r.id, 2)) : pick(r)}>{stat.advice === "stale" ? t("rules.advice.disable") : t("rules.advice.narrow")}</button>
                </div>}
              </div>
              <div className="row rule-actions">
                <button type="button" className="btn sm ghost rule-edit" aria-label={t("rules.row.editAria", { pattern: r.pattern })} onClick={() => pick(r)}>{t("rules.row.edit")}</button>
                {r.pending ? <>
                  <button type="button" className="btn primary sm" onClick={() => void run(t("rules.action.confirm"), () => api.confirmRule(r.id))}>{t("rules.row.confirm")}</button>
                  <button type="button" className="btn sm danger" onClick={() => void run(t("rules.action.discard"), () => api.discardRule(r.id))}>{t("rules.row.discard")}</button>
                </> : <>
                  {onAsk && <button type="button" className="btn sm danger rule-edit" onClick={() => onAsk(t("rules.action.delete"), t("rules.row.deleteConfirm", { pattern: r.pattern }), () => void run(t("rules.action.delete"), () => api.discardRule(r.id)))}>{t("rules.row.delete")}</button>}
                  <Toggle label={t("rules.row.enableAria", { pattern: r.pattern })} on={r.enabled} onLabel="" offLabel=""
                    onClick={() => void run(t("rules.action.update"), () => api.setRuleAction(r.id, r.enabled ? 2 : r.polarity === "like" ? 0 : 1))} />
                </>}
              </div>
            </div>; })}
          </div>
        </div>
      </section>; })}
      <section className="settings-card rules-advanced settings-section">
        <div className="settings-section-meta"><h4>{t("rules.hidden.title")}</h4></div>
        <div className="settings-section-body">
          <SettingRow label={t("rules.hidden.threshold")} hint={t("rules.hidden.thresholdHint")}>
            <Stepper label={t("rules.hidden.threshold")} value={threshold} onChange={setThreshold} onCommit={saveThreshold} />
          </SettingRow>
          <SettingRow label={t("rules.hidden.spot")} hint={t("rules.hidden.spotHint")}>
            <ChipBar className="segmented" value={rate} disabled={!schema.data} options={[{ id: "0", label: t("rules.hidden.spotOff") }, { id: "1", label: "1%" }, { id: "3", label: "3%" }, { id: "5", label: "5%" }]}
              onChange={value => void run(t("rules.action.saveSpotRate"), () => api.setSetting("hidden_spot_check_rate", value))} />
          </SettingRow>
          {stats.isPending && <Pending>{t("rules.hidden.statsLoading")}</Pending>}
          {(stats.isError || schema.isError) && <p className="notice error" role="alert">{t("rules.hidden.statsFailed")}<button className="btn sm" onClick={() => { void stats.refetch(); void schema.refetch(); }}>{t("rules.retryRead")}</button></p>}
          {spot && <p className="rule-spot-stats">{t("rules.hidden.spotStats", { exposed: spot.exposed, liked: spot.liked, disliked: spot.disliked })}
            {spot.insufficient ? t("rules.hidden.insufficient") : t("rules.hidden.estimate", { percent: Math.round(spot.liked / Math.max(1, spot.exposed) * 100) })}
            {spot.liked > 0 && onOpenArticle && <button className="link-btn" onClick={() => setHidden(true)}>{t("rules.hidden.viewLiked", { liked: spot.liked })}</button>}
          </p>}
          <p className="hint">{t("rules.hidden.spotNote")}</p>
          <SettingRow label={t("rules.hidden.history")} hint={t("rules.hidden.historyHint")}>
            <button type="button" className="btn" onClick={() => void run(t("rules.action.rescore"), api.rescore)}><Icon name="refresh" size={14} />{t("rules.hidden.rescore")}</button>
          </SettingRow>
        </div>
      </section>
      <details className="advanced-fold"><summary>{t("rules.presets.summary")}</summary><PresetList presets={data?.presets ?? []} run={run} /></details>
    </fieldset>
    {hidden && onOpenArticle && <HiddenListDialog initialFilter="spot_liked" onOpenArticle={onOpenArticle}
      onClose={() => setHidden(false)} onChanged={onChanged} />}
  </div>;
}

function PresetList({ presets, run }: {
  presets: PresetView[];
  run: (label: string, action: () => Promise<unknown>, onSuccess?: (value?: unknown) => void) => Promise<void>;
}) {
  const { t } = useTranslation("settingsContent");
  if (presets.length === 0) return <p className="muted">{t("rules.presets.none")}</p>;
  return (
    <div className="preset-list">
      {presets.map((p) => (
        <div key={p.id} className={`card preset-card${p.installed ? " on" : ""}`}>
          <div className="preset-body">
            <strong>{p.name}</strong>
            <p className="muted">{p.description}</p>
            <p className="hint">
              {p.installed ? t("rules.presets.enabledOf", { enabled: p.enabled_count, total: p.rule_count }) : t("rules.presets.notInstalled", { count: p.rule_count })}
            </p>
          </div>
          <button
            type="button"
            className={p.installed ? "btn sm danger" : "btn sm primary"}
            onClick={() => { void run(p.installed ? t("rules.action.removePreset") : t("rules.action.applyPreset"), () => p.installed ? api.removePreset(p.id) : api.applyPreset(p.id)); }}
          >
            {p.installed ? t("rules.presets.remove") : t("rules.presets.apply")}
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
  const { t } = useTranslation("settingsContent");
  if (!options) return <p className="hint" role="status">{failed ? t("rules.scopeList.failed") : t("rules.scopeList.loading")}</p>;
  if (options.length === 0) return <p className="muted">{t("rules.scopeList.empty")}</p>;
  return (
    <div className="scope-list" role="group" aria-label={t("rules.scopeList.aria")}>
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
