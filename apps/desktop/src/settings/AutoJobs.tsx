import { useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import type { AiUsageView, AutoSummaryView, UsageLine } from "../bindings";
import { api } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { i18n, Trans, useTranslation } from "../i18n";
import { aiKeys } from "./aiQuery";
import { articlesFit, clusterDaily, digits, fitText, tokensToWanText, unlimitedText, wan, wanTextToTokens } from "./autoSummary";
import { SettingRow, Toggle } from "./chips";
import { Dialog } from "./Dialog";
import { useSettingsWrite } from "./editState";
import type { Ask } from "./SettingsPanel";
import { settingsMutationOptions } from "./settingsQuery";
import { SettingsRows } from "./SettingsRows";
import type { SettingsEditState } from "./editState";

/**
 * 模型页的「用量」与「自动任务」里随数据变化的部分（AI 规格 · 自动摘要与预算 · 界面，样板 2026-10-29 定稿；
 * 自动汇总开关 2026-11-08）。开关与预算仍走设置注册表；估算的算法在 `autoSummary.ts`。
 */

type Status = { tone: "ok" | "err"; text: string };

/** 升级后的一次性提示：放在模型页页头下，「查看自动任务」展开并定位到开关，「知道了」后不再出现。 */
export function AutoSummaryUpgradeNotice({ auto, tier, defaults, onShowJobs }: {
  auto: AutoSummaryView;
  tier: string;
  /** 两份预算的注册表默认值（token）。 */
  defaults: { generation: number; embedding: number };
  onShowJobs: () => void;
}) {
  const { t } = useTranslation("settingsJobs");
  const client = useQueryClient();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const notice = auto.notice;
  if (!notice) return null;
  const dismiss = async () => {
    setBusy(true);
    setError("");
    try {
      await api.dismissAutoSummaryNotice();
      await client.invalidateQueries({ queryKey: aiKeys.page });
    } catch (err) {
      setError(formatNotice(t("upgrade.dismissFailed"), err));
    } finally {
      setBusy(false);
    }
  };
  const budget = notice.budget_defaulted
    ? <Trans t={t} i18nKey="upgrade.budgetDefaulted" values={{ generation: wan(defaults.generation), embedding: wan(defaults.embedding) }}
      components={{ strong: <strong /> }} />
    : t("upgrade.budgetKept", { budget: wan(notice.generation_budget), embedding: wan(defaults.embedding) });
  return (
    <div className="notice auto-summary-notice" role="status">
      <div className="auto-summary-notice-text">
        <strong>{t("upgrade.title")}</strong>
        <p>{t("upgrade.body")}</p>
        <p>{t("upgrade.reenable", { tier, tokens: digits(auto.per_article_tokens) })}</p>
        <p>{budget}</p>
        {error && <p className="form-status error" role="alert">{error}</p>}
      </div>
      <div className="row auto-summary-notice-actions">
        <button type="button" className="btn sm" onClick={onShowJobs}>{t("upgrade.showJobs")}</button>
        <button type="button" className="btn sm ghost" disabled={busy} onClick={() => void dismiss()}>{t("upgrade.dismiss")}</button>
      </div>
    </div>
  );
}

/** 生成日预算下的估算：大于 0 时写「约够 N 篇 / 天」，为 0 时提示建议设上限。 */
export function budgetNote({ auto, budget, autoOn, clusterOn, tier }: {
  auto: AutoSummaryView; budget: number; autoOn: boolean; clusterOn: boolean; tier: string;
}): ReactNode {
  if (budget > 0) {
    return <p className="hint">{i18n.t("budgetNote.fit", { ns: "settingsJobs", tier, tokens: digits(auto.per_article_tokens), fit: fitText(auto, budget, clusterOn) })}</p>;
  }
  return <p className="hint warn">{autoOn
    ? i18n.t("budgetNote.unlimitedOn", { ns: "settingsJobs", text: unlimitedText(auto, clusterOn) })
    : i18n.t("budgetNote.unlimitedOff", { ns: "settingsJobs" })}</p>;
}

/** 生成 / 嵌入 / 对话三行用量，同一写法：右侧「今日 X / 预算」与「本月 Y」，用完写明暂停到明天。 */
export function UsageReadouts({ usage, budget, embedBudget }: { usage: AiUsageView; budget: number; embedBudget: number }) {
  const { t } = useTranslation("settingsJobs");
  const readout = (line: UsageLine, cap: number) => (
    <div className="usage-readout">
      {line.paused
        ? <span className="usage-out">{t("usage.todayOut", { today: wan(line.today), cap: wan(cap) })}</span>
        : cap > 0
          ? <span><Trans t={t} i18nKey="usage.todayCap" values={{ today: wan(line.today), cap: wan(cap) }} components={{ strong: <strong /> }} /></span>
          : <span><Trans t={t} i18nKey="usage.today" values={{ today: wan(line.today) }} components={{ strong: <strong /> }} /></span>}
      <span>{t("usage.month", { month: wan(line.month) })}</span>
    </div>
  );
  return <>
    <SettingRow label={t("usage.generation.label")} hint={t("usage.generation.hint")}
      note={usage.generation.paused ? <p className="hint warn">{t("usage.generation.paused")}</p> : null}>
      {readout(usage.generation, budget)}
    </SettingRow>
    <SettingRow label={t("usage.embedding.label")} hint={t("usage.embedding.hint")}
      note={usage.embedding.paused ? <p className="hint warn">{t("usage.embedding.paused")}</p> : null}>
      {readout(usage.embedding, embedBudget)}
    </SettingRow>
    <SettingRow label={t("usage.chat.label")} hint={t("usage.chat.hint")}>{readout(usage.chat, 0)}</SettingRow>
  </>;
}

/** 「自动任务」组：自动摘要（开启要确认）、中性未读额外摘要、自动汇总相近报道（直接开关）、简报自动生成时间。 */
export function AutoJobs({ auto, usage, values, tier, modelLabel, onAsk, onShowUses, report }: {
  auto: AutoSummaryView;
  usage?: AiUsageView;
  values: { autoOn: boolean; clusterOn: boolean; budget: number };
  tier: string;
  /** 「摘要」组焦点摘要用途当前的模型名；没选时为空。 */
  modelLabel: string;
  onAsk: Ask;
  onShowUses: () => void;
  report: (key: string) => (state: SettingsEditState | null) => void;
}) {
  const { t } = useTranslation("settingsJobs");
  const client = useQueryClient();
  const { writing, run } = useSettingsWrite(settingsMutationOptions(client));
  const [confirming, setConfirming] = useState(false);
  const [status, setStatus] = useState<Status | null>(null);
  const { autoOn, clusterOn, budget } = values;
  const paused = !!usage?.generation.paused;

  const write = async (pairs: Array<{ id: string; value: string }>, done: string): Promise<boolean> => {
    setStatus(null);
    const outcome = await run(pairs);
    if (outcome.status === "busy" || !outcome.mounted) return false;
    if (outcome.status === "failed" || outcome.data.failure) {
      const error = outcome.status === "failed" ? outcome.error : outcome.data.failure?.error;
      setStatus({ tone: "err", text: formatNotice(t("notice.saveFailed"), error) });
      return false;
    }
    // 开启会顺手把升级提示标为看过；今日篇数、提示都在模型页查询里。
    void client.invalidateQueries({ queryKey: aiKeys.page });
    setStatus({ tone: "ok", text: done });
    return true;
  };

  const goModels = <button type="button" className="link-btn" onClick={onShowUses}>{t("goModels")}</button>;
  const autoNote = autoOn ? <>
    <p className="hint">{t("auto.on.intro")}</p>
    <ul className="auto-scope">
      <li>{t("auto.on.scopeWindow", { days: auto.window_days })}</li>
      <li>{t("auto.on.scopeLimit", { limit: auto.daily_limit, minutes: auto.batch_minutes, size: auto.batch_size })}</li>
      <li>{budget > 0
        ? t("auto.on.scopeCostBudget", { tier, tokens: digits(auto.per_article_tokens), budget: wan(budget), fit: fitText(auto, budget, clusterOn) })
        : t("auto.on.scopeCostUnlimited", { tier, tokens: digits(auto.per_article_tokens) })}</li>
    </ul>
    {!auto.model_ready ? <p className="hint warn">{t("auto.on.noModel")} · {goModels}</p>
      : paused ? <p className="hint warn">{t("auto.on.pausedToday", { today: auto.today })}</p>
      : <p className="auto-today">{t("auto.on.today", { today: auto.today, limit: auto.daily_limit })}</p>}
  </> : <>
    <p className="hint">{t("auto.off.intro")}</p>
    <p className="hint">{budget > 0
      ? t("auto.off.enableFit", { days: auto.window_days, limit: auto.daily_limit, fit: fitText(auto, budget, clusterOn) })
      : t("auto.off.enableUnlimited", { days: auto.window_days, limit: auto.daily_limit })}</p>
    {!auto.model_ready && <p className="hint warn">{t("auto.off.noModel")}</p>}
  </>;

  const clusterTokens = auto.cluster_daily_limit * auto.per_cluster_tokens;
  const clusterNote = clusterOn ? <>
    <p className="hint">{t("cluster.on.intro")}</p>
    <ul className="auto-scope">
      <li>{t("cluster.on.scopeSources")}</li>
      <li>{t("cluster.on.scopeUpdate")}</li>
      <li>{t("cluster.on.scopeLimit", { limit: auto.cluster_daily_limit, per: digits(auto.per_cluster_tokens), most: wan(clusterTokens) })}</li>
    </ul>
    {!auto.cluster_model_ready ? <p className="hint warn">{t("cluster.on.noModel")} · {goModels}</p>
      : paused ? <p className="hint warn">{t("cluster.on.pausedToday", { today: auto.cluster_today })}</p>
      : <p className="auto-today">{t("cluster.on.today", { today: auto.cluster_today, limit: auto.cluster_daily_limit })}</p>}
  </> : (
    <p className="hint">{t("cluster.off.intro", { limit: auto.cluster_daily_limit, tokens: wan(clusterTokens) })}</p>
  );

  return <>
    <SettingRow id="model-auto-summary" label={t("auto.label")} note={autoNote}>
      <Toggle label={t("auto.label")} on={autoOn} onLabel={t("toggle.on")} offLabel={t("toggle.off")} disabled={writing}
        onClick={() => {
          if (autoOn) void write([{ id: "focus_auto_summary", value: "0" }], t("auto.disabled"));
          else { setStatus(null); setConfirming(true); }
        }} />
    </SettingRow>
    <SettingsRows ids={["focus_summary_neutral_limit"]} label={t("neutral.label")} disabled={!autoOn || writing}
      notes={autoOn ? undefined : { focus_summary_neutral_limit: <p className="hint">{t("neutral.needAuto")}</p> }}
      onEditStateChange={report("jobs-neutral")} />
    <SettingRow id="model-auto-cluster" label={t("cluster.label")} note={clusterNote}>
      <Toggle label={t("cluster.label")} on={clusterOn} onLabel={t("toggle.on")} offLabel={t("toggle.off")} disabled={writing}
        onClick={() => void write([{ id: "cluster_auto_summary", value: clusterOn ? "0" : "1" }],
          clusterOn ? t("cluster.disabled") : t("cluster.enabled"))} />
    </SettingRow>
    <SettingsRows ids={["digest_generate_minutes"]} label={t("digest.label")} disabled={writing} onEditStateChange={report("jobs-digest")} />
    {status && <p className={status.tone === "err" ? "form-status error" : "form-status"} role={status.tone === "err" ? "alert" : "status"}>{status.text}</p>}
    {confirming && (
      <EnableDialog auto={auto} budget={budget} clusterOn={clusterOn} tier={tier} modelLabel={modelLabel} busy={writing} onAsk={onAsk}
        onShowUses={() => { setConfirming(false); onShowUses(); }}
        onClose={() => setConfirming(false)}
        onEnable={async (setBudget) => {
          const pairs = setBudget != null ? [{ id: "ai_daily_budget_tokens", value: String(setBudget) }] : [];
          const ok = await write([...pairs, { id: "focus_auto_summary", value: "1" }],
            setBudget != null
              ? t("auto.enabledWithBudget", { budget: wan(setBudget), minutes: auto.batch_minutes })
              : t("auto.enabled", { minutes: auto.batch_minutes }));
          if (ok) setConfirming(false);
        }} />
    )}
  </>;
}

/**
 * 开启确认：写明持续费用、范围、模型与估算。生成预算为 0 时弹窗里直接给预算输入（默认 20 万），
 * 主按钮「设预算并开启」同时写入预算与开关，次要「不设预算，直接开启」保留。取消不写入任何值。
 */
function EnableDialog({ auto, budget, clusterOn, tier, modelLabel, busy, onAsk, onShowUses, onClose, onEnable }: {
  auto: AutoSummaryView;
  budget: number;
  clusterOn: boolean;
  tier: string;
  modelLabel: string;
  busy: boolean;
  onAsk: Ask;
  onShowUses: () => void;
  onClose: () => void;
  /** 传了数字就同时把生成日预算设为它。 */
  onEnable: (budget: number | null) => Promise<void>;
}) {
  const { t } = useTranslation("settingsJobs");
  const [draft, setDraft] = useState(() => tokensToWanText("200000"));
  const tokens = Number(wanTextToTokens(draft));
  const cluster = clusterDaily(auto, clusterOn);
  const valid = Number.isSafeInteger(tokens) && tokens > 0 && tokens <= 1_000_000_000;
  const estimate = budget > 0 ? (
    <div className="auto-estimate">
      <span className="auto-estimate-tag">{t("dialog.estimateTag")}</span>
      <strong>{budget > auto.digest_daily_tokens + cluster
        ? t("dialog.estimateFit", { per: digits(auto.per_article_tokens), n: Math.min(auto.daily_limit, articlesFit(auto, budget, clusterOn)) })
        : cluster
          ? t("dialog.estimateOnlyDigestCluster", { per: digits(auto.per_article_tokens) })
          : t("dialog.estimateOnlyDigest", { per: digits(auto.per_article_tokens) })}</strong>
      <span>{cluster
        ? t("dialog.deductCluster", { tier, budget: wan(budget), digest: wan(auto.digest_daily_tokens), cluster: wan(cluster) })
        : t("dialog.deduct", { tier, budget: wan(budget), digest: wan(auto.digest_daily_tokens) })}</span>
      <small>{t("dialog.estimateNote")}</small>
    </div>
  ) : (
    <div className="auto-estimate warn">
      <span className="auto-estimate-tag">{t("dialog.noBudgetTag")}</span>
      <span>{t("dialog.unlimitedIntro", { text: unlimitedText(auto, clusterOn) })}</span>
      <div className="row auto-estimate-set">
        <span className="settings-amount">
          <input type="text" inputMode="decimal" aria-label={t("dialog.budgetAria")} value={draft} onChange={(e) => setDraft(e.target.value)} />
          <span className="settings-amount-unit">{t("dialog.budgetUnit")}</span>
        </span>
        <span className="hint" role="status">{valid ? t("estimateSuffix", { text: fitText(auto, tokens, clusterOn) }) : t("dialog.budgetInvalid")}</span>
      </div>
    </div>
  );
  return (
    <Dialog title={t("dialog.title")} dirty={false} busy={busy} onAsk={onAsk} onClose={onClose}
      leading={budget > 0 ? undefined : <button type="button" className="btn ghost" disabled={busy} onClick={() => void onEnable(null)}>{t("dialog.enableNoBudget")}</button>}
      actions={budget > 0
        ? <button type="button" className="btn primary" disabled={busy} onClick={() => void onEnable(null)}>{t("dialog.enable")}</button>
        : <button type="button" className="btn primary" disabled={busy || !valid} onClick={() => void onEnable(tokens)}>{t("dialog.enableWithBudget")}</button>}>
      <p><Trans t={t} i18nKey="dialog.intro" components={{ strong: <strong /> }} /></p>
      <ul className="auto-scope">
        <li>{t("dialog.scopeWindow", { days: auto.window_days })}</li>
        <li>{t("dialog.scopeLimit", { limit: auto.daily_limit, minutes: auto.batch_minutes, size: auto.batch_size })}</li>
        {auto.model_ready && modelLabel
          ? <li><Trans t={t} i18nKey="dialog.model" values={{ model: modelLabel }} components={{ mono: <span className="mono" /> }} /></li>
          : <li className="warn">{t("auto.off.noModel")}<button type="button" className="link-btn" onClick={onShowUses}>{t("goModels")}</button></li>}
      </ul>
      {estimate}
    </Dialog>
  );
}
