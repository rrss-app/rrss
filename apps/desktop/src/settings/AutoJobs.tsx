import { useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import type { AiUsageView, AutoSummaryView, UsageLine } from "../bindings";
import { api } from "../lib/api";
import { formatNotice } from "../lib/notice";
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
      setError(formatNotice("没能关掉提示：", err));
    } finally {
      setBusy(false);
    }
  };
  const budget = notice.budget_defaulted
    ? <>自动任务现在有每日预算：<strong>生成 {wan(defaults.generation)}、嵌入 {wan(defaults.embedding)} token / 天</strong>，用完当天暂停、次日恢复。你以前没设过预算（不限额），现在按这个默认生效，可在「用量」里改。</>
    : <>你设过的日预算 {wan(notice.generation_budget)} token 保留为「生成日预算」；嵌入另算，默认 {wan(defaults.embedding)} token / 天。</>;
  return (
    <div className="notice auto-summary-notice" role="status">
      <div className="auto-summary-notice-text">
        <strong>自动摘要已关闭</strong>
        <p>后台自动摘要会为每篇新文章调用生成模型，是最主要的持续费用，所以这次升级后统一关闭。阅读时点「本篇摘要」和每日简报照常可用。</p>
        <p>要重新打开：设置 → 模型 → 自动任务。按当前设置（{tier}档）每篇约 {digits(auto.per_article_tokens)} token（估算）。</p>
        <p>{budget}</p>
        {error && <p className="form-status error" role="alert">{error}</p>}
      </div>
      <div className="row auto-summary-notice-actions">
        <button type="button" className="btn sm" onClick={onShowJobs}>查看自动任务</button>
        <button type="button" className="btn sm ghost" disabled={busy} onClick={() => void dismiss()}>知道了</button>
      </div>
    </div>
  );
}

/** 生成日预算下的估算：大于 0 时写「约够 N 篇 / 天」，为 0 时提示建议设上限。 */
export function budgetNote({ auto, budget, autoOn, clusterOn, tier }: {
  auto: AutoSummaryView; budget: number; autoOn: boolean; clusterOn: boolean; tier: string;
}): ReactNode {
  if (budget > 0) {
    return <p className="hint">自动摘要按{tier}档每篇约 {digits(auto.per_article_tokens)} token，{fitText(auto, budget, clusterOn)}（估算）。</p>;
  }
  return <p className="hint warn">现在不限额{autoOn ? `：${unlimitedText(auto, clusterOn)}` : ""}。建议设一个上限，例如 20 万。</p>;
}

/** 生成 / 嵌入 / 对话三行用量，同一写法：右侧「今日 X / 预算」与「本月 Y」，用完写明暂停到明天。 */
export function UsageReadouts({ usage, budget, embedBudget }: { usage: AiUsageView; budget: number; embedBudget: number }) {
  const readout = (line: UsageLine, cap: number) => (
    <div className="usage-readout">
      {line.paused
        ? <span className="usage-out">今日 {wan(line.today)} / {wan(cap)} token · 已用完</span>
        : <span>今日 <strong>{wan(line.today)}</strong>{cap > 0 ? ` / ${wan(cap)}` : ""} token</span>}
      <span>本月 {wan(line.month)} token</span>
    </div>
  );
  return <>
    <SettingRow label="生成用量" hint="只读。自动与手动的摘要、简报、记忆整理都计入；按本地日计。"
      note={usage.generation.paused ? <p className="hint warn">今天的生成预算已用完，自动摘要和自动简报暂停到明天。手动摘要、手动简报照常。</p> : null}>
      {readout(usage.generation, budget)}
    </SettingRow>
    <SettingRow label="嵌入用量" hint="只读。只计嵌入请求。"
      note={usage.embedding.paused ? <p className="hint warn">今天的嵌入预算已用完，新文章暂时只靠全文检索找到，明天恢复。摘要和简报不受影响。</p> : null}>
      {readout(usage.embedding, embedBudget)}
    </SettingRow>
    <SettingRow label="对话用量" hint="只读。对话不计入任何预算。">{readout(usage.chat, 0)}</SettingRow>
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
      setStatus({ tone: "err", text: formatNotice("没有保存：", error) });
      return false;
    }
    // 开启会顺手把升级提示标为看过；今日篇数、提示都在模型页查询里。
    void client.invalidateQueries({ queryKey: aiKeys.page });
    setStatus({ tone: "ok", text: done });
    return true;
  };

  const goModels = <button type="button" className="link-btn" onClick={onShowUses}>去选模型</button>;
  const autoNote = autoOn ? <>
    <p className="hint">在后台为新文章写摘要，焦点卡片直接显示。这是持续费用。</p>
    <ul className="auto-scope">
      <li>只处理入库 {auto.window_days} 天内、评分为正的文章；更早的文章需要时点「本篇摘要」。</li>
      <li>每天最多 {auto.daily_limit} 篇（手动摘要不计入），每 {auto.batch_minutes} 分钟一批 {auto.batch_size} 篇。</li>
      <li>按{tier}档每篇约 {digits(auto.per_article_tokens)} token；{budget > 0 ? `生成日预算 ${wan(budget)}，${fitText(auto, budget, clusterOn)}（估算）` : "生成日预算不限额"}。</li>
    </ul>
    {!auto.model_ready ? <p className="hint warn">「摘要」还没选模型，自动摘要暂停 · {goModels}</p>
      : paused ? <p className="hint warn">今日已摘要 {auto.today} 篇 · 生成预算已用完，明天继续</p>
      : <p className="auto-today">今日已摘要 {auto.today} / {auto.daily_limit} 篇</p>}
  </> : <>
    <p className="hint">关闭时不在后台自动摘要。阅读时点「本篇摘要」照常可用；每日简报也照常生成，只是没有摘要的文章选题时只看标题，内容可能少一些。</p>
    <p className="hint">开启后只处理入库 {auto.window_days} 天内的正分文章，每天最多 {auto.daily_limit} 篇，{budget > 0 ? `${fitText(auto, budget, clusterOn)}（估算）` : "生成日预算现在不限额"}。</p>
    {!auto.model_ready && <p className="hint warn">「摘要」还没选模型：可以先开启，选好模型后才开始。</p>}
  </>;

  const clusterTokens = auto.cluster_daily_limit * auto.per_cluster_tokens;
  const clusterNote = clusterOn ? <>
    <p className="hint">在后台为焦点里的相近报道写综述，展开时直接看到，和你点「汇总」生成的一样。计入生成日预算。</p>
    <ul className="auto-scope">
      <li>只汇总至少 2 个来源、最新一篇在 3 天内的组；还没有综述的先做。</li>
      <li>已有综述的组，来了新报道、且距上次生成超过 24 小时才更新。</li>
      <li>每天最多 {auto.cluster_daily_limit} 组，排在自动摘要之后；每组约 {digits(auto.per_cluster_tokens)} token，每天最多约 {wan(clusterTokens)}（估算）。</li>
    </ul>
    {!auto.cluster_model_ready ? <p className="hint warn">「相近报道综述」还没选模型，自动汇总暂停 · {goModels}</p>
      : paused ? <p className="hint warn">今日已汇总 {auto.cluster_today} 组 · 生成预算已用完，明天继续</p>
      : <p className="auto-today">今日已汇总 {auto.cluster_today} / {auto.cluster_daily_limit} 组</p>}
  </> : (
    <p className="hint">关闭时只在你点「汇总」时生成综述。开启后在后台为多来源的相近报道写综述，每天最多 {auto.cluster_daily_limit} 组，约 {wan(clusterTokens)} token / 天（估算），计入生成日预算。</p>
  );

  return <>
    <SettingRow id="model-auto-summary" label="自动摘要" note={autoNote}>
      <Toggle label="自动摘要" on={autoOn} onLabel="已开启" offLabel="已关闭" disabled={writing}
        onClick={() => {
          if (autoOn) void write([{ id: "focus_auto_summary", value: "0" }], "已关闭自动摘要。手动「本篇摘要」和简报照常可用。");
          else { setStatus(null); setConfirming(true); }
        }} />
    </SettingRow>
    <SettingsRows ids={["focus_summary_neutral_limit"]} label="中性未读额外摘要" disabled={!autoOn || writing}
      notes={autoOn ? undefined : { focus_summary_neutral_limit: <p className="hint">需先开启自动摘要。</p> }}
      onEditStateChange={report("jobs-neutral")} />
    <SettingRow id="model-auto-cluster" label="自动汇总相近报道" note={clusterNote}>
      <Toggle label="自动汇总相近报道" on={clusterOn} onLabel="已开启" offLabel="已关闭" disabled={writing}
        onClick={() => void write([{ id: "cluster_auto_summary", value: clusterOn ? "0" : "1" }],
          clusterOn ? "已关闭自动汇总。点「汇总」照常可以生成。" : "已开启自动汇总相近报道。下一批排在自动摘要之后。")} />
    </SettingRow>
    <SettingsRows ids={["digest_generate_minutes"]} label="简报自动生成" disabled={writing} onEditStateChange={report("jobs-digest")} />
    {status && <p className={status.tone === "err" ? "form-status error" : "form-status"} role={status.tone === "err" ? "alert" : "status"}>{status.text}</p>}
    {confirming && (
      <EnableDialog auto={auto} budget={budget} clusterOn={clusterOn} tier={tier} modelLabel={modelLabel} busy={writing} onAsk={onAsk}
        onShowUses={() => { setConfirming(false); onShowUses(); }}
        onClose={() => setConfirming(false)}
        onEnable={async (setBudget) => {
          const pairs = setBudget != null ? [{ id: "ai_daily_budget_tokens", value: String(setBudget) }] : [];
          const ok = await write([...pairs, { id: "focus_auto_summary", value: "1" }],
            `已开启自动摘要${setBudget != null ? `，生成日预算设为 ${wan(setBudget)} token` : ""}。下一批在 ${auto.batch_minutes} 分钟内开始。`);
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
  const [draft, setDraft] = useState(() => tokensToWanText("200000"));
  const tokens = Number(wanTextToTokens(draft));
  const cluster = clusterDaily(auto, clusterOn);
  const valid = Number.isSafeInteger(tokens) && tokens > 0 && tokens <= 1_000_000_000;
  const estimate = budget > 0 ? (
    <div className="auto-estimate">
      <span className="auto-estimate-tag">估算</span>
      <strong>每篇约 {digits(auto.per_article_tokens)} token，{budget > auto.digest_daily_tokens + cluster
        ? `约够 ${Math.min(auto.daily_limit, articlesFit(auto, budget, clusterOn))} 篇 / 天` : `预算只够${cluster ? "自动简报和自动汇总" : "自动简报"}`}</strong>
      <span>按当前输入档位（{tier}）粗算；当前生成日预算 {wan(budget)} token，先扣掉自动简报约 {wan(auto.digest_daily_tokens)}{cluster ? `、自动汇总约 ${wan(cluster)}` : ""} / 天。</span>
      <small>实际随文章长短与模型而变。预算用完当天就停，自动简报也用这份预算，会一起停到次日。</small>
    </div>
  ) : (
    <div className="auto-estimate warn">
      <span className="auto-estimate-tag">还没有生成日预算</span>
      <span>现在是不限额。开启后{unlimitedText(auto, clusterOn)}。建议先设一个上限：</span>
      <div className="row auto-estimate-set">
        <span className="settings-amount">
          <input type="text" inputMode="decimal" aria-label="生成日预算（万 token）" value={draft} onChange={(e) => setDraft(e.target.value)} />
          <span className="settings-amount-unit">万 token / 天</span>
        </span>
        <span className="hint" role="status">{valid ? `${fitText(auto, tokens, clusterOn)}（估算）` : "填一个大于 0 的数；以后可在「用量」里改。"}</span>
      </div>
    </div>
  );
  return (
    <Dialog title="开启自动摘要？" dirty={false} busy={busy} onAsk={onAsk} onClose={onClose}
      leading={budget > 0 ? undefined : <button type="button" className="btn ghost" disabled={busy} onClick={() => void onEnable(null)}>不设预算，直接开启</button>}
      actions={budget > 0
        ? <button type="button" className="btn primary" disabled={busy} onClick={() => void onEnable(null)}>开启自动摘要</button>
        : <button type="button" className="btn primary" disabled={busy || !valid} onClick={() => void onEnable(tokens)}>设预算并开启</button>}>
      <p>开启后，RRSS 会在后台为新文章调用生成模型写摘要。<strong>这是持续费用</strong>：只要应用开着，每天都会产生用量。</p>
      <ul className="auto-scope">
        <li>只处理入库 {auto.window_days} 天内、评分为正的文章。</li>
        <li>每天最多 {auto.daily_limit} 篇，每 {auto.batch_minutes} 分钟一批 {auto.batch_size} 篇；手动摘要不计入。</li>
        {auto.model_ready && modelLabel
          ? <li>用「摘要」用途的模型：<span className="mono">{modelLabel}</span>。</li>
          : <li className="warn">「摘要」还没选模型：可以先开启，选好模型后才开始。<button type="button" className="link-btn" onClick={onShowUses}>去选模型</button></li>}
      </ul>
      {estimate}
    </Dialog>
  );
}
