import { useState } from "react";
import { useIsMutating, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ModelBindingsView, ModelChoice, ModelPoolView, ProviderView } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { SourceAvatar } from "../components/ArticleCard";
import { Icon } from "../components/Icon";
import { AddModelsDialog } from "./AddModelsDialog";
import { AutoJobs, AutoSummaryUpgradeNotice, budgetNote, UsageReadouts } from "./AutoJobs";
import { wan } from "./autoSummary";
import { aiKeys, aiMutationOptions, aiQueryOptions, bindingsQueryOptions, poolQueryOptions, presetsQueryOptions, testConnection } from "./aiQuery";
import { explainAiError } from "./aiError";
import { probeWaitLabel, useProbe } from "./aiProbe";
import { Fold } from "./chips";
import { ConnectionDialog, describeTest, Feedback } from "./ConnectionDialog";
import { readGate, useMergedEditState, useSettingsWrite, type SettingsEditState } from "./editState";
import { ManageModels, untested, unused, type ModelFilter } from "./ManageModels";
import { ChatModelMenu, findEntry, lacks, ModelMenu, modelName, sameChoice, taskChoice, type Need } from "./modelChoices";
import { formatMinutes } from "./registry";
import { settingsSchemaOptions } from "./settingsQuery";
import { SettingsRows } from "./SettingsRows";
import type { Ask, Navigate } from "./SettingsPanel";
import { TRANSLATE_BACKENDS, translateQueryOptions } from "./translateQuery";
import { Pending } from "../components/Pending";
import { embedChangeConfirm, VectorSpaceSection, vectorSpaceKey, vectorSpaceQueryOptions } from "./VectorSpaceSection";
import { locale } from "../i18n";

type Result = { tone: "ok" | "err"; text: string; detail?: string };

/** 用途按功能分组显示，存储仍按用途一行（阶段 19-7）；用途常量以后端 `ALL_TASKS` 为准。 */
export const USE_GROUPS: Array<{ id: string; title: string; hint: string; tasks: string[]; need?: Need }> = [
  { id: "chat", title: "对话", hint: "新对话默认用它，对话里可以临时换。只列用于对话的模型。", tasks: ["chat"], need: "tools" },
  { id: "summary", title: "摘要", hint: "本篇摘要、长文分段和相近报道综述；后台自动摘要与自动汇总另在下方「自动任务」里开关。",
    tasks: ["focus_summary", "article_summary", "article_segment", "cluster_summary"], need: "json" },
  { id: "digest", title: "简报", hint: "日报、周报以及它们的选题。",
    tasks: ["digest_daily", "digest_weekly", "digest_pick_daily", "digest_pick_weekly"], need: "json" },
  { id: "memory", title: "记忆整理", hint: "从对话里提取记忆候选、压缩对话情节。", tasks: ["memory_candidate", "episode_compress"], need: "json" },
  { id: "translate", title: "翻译", hint: "", tasks: ["article_translate"] },
  { id: "embed", title: "嵌入", hint: "相似文章与记忆检索用的向量。", tasks: ["embed"] },
];
/** 一键应用默认勾选的组：翻译、嵌入通常要专用模型，对话单独选。 */
const APPLY_DEFAULT = ["summary", "digest", "memory"];

export function ModelPane({ onAsk, onNavigate, onEditStateChange }: {
  onAsk: Ask;
  onNavigate: Navigate;
  onEditStateChange?: (state: SettingsEditState | null) => void;
}) {
  const { report } = useMergedEditState(onEditStateChange);
  const writing = useIsMutating({ mutationKey: aiKeys.write }) > 0;
  const page = useQuery({ ...aiQueryOptions(), ...readGate(writing) });
  const bindings = useQuery({ ...bindingsQueryOptions(), ...readGate(writing) });
  const pool = useQuery({ ...poolQueryOptions(), ...readGate(writing) });
  const presets = useQuery(presetsQueryOptions());
  const schema = useQuery(settingsSchemaOptions());
  const [connection, setConnection] = useState<{ provider?: ProviderView } | null>(null);
  const [adding, setAdding] = useState<{ providerId: number; note?: string } | null>(null);
  const [managing, setManaging] = useState<{ filter: ModelFilter; model?: ModelChoice | null } | null>(null);
  const [notice, setNotice] = useState<Result | null>(null);
  const [firstRun, setFirstRun] = useState<ModelChoice | null>(null);
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const toggle = (key: string) => setOpen((old) => ({ ...old, [key]: !old[key] }));

  const providers = page.data?.providers ?? [];
  const presetList = presets.data ?? [];
  const value = (id: string) => schema.data?.values.find((v) => v.id === id)?.value ?? "";
  const tierSpec = schema.data?.specs.find((s) => s.id === "ai_input_tier");
  const tier = tierSpec?.kind.type === "enum" ? tierSpec.kind.options.find((o) => o.id === value("ai_input_tier"))?.label : "";
  const budget = Number(value("ai_daily_budget_tokens"));
  const embedBudget = Number(value("ai_embed_daily_budget_tokens"));
  const autoOn = value("focus_auto_summary") === "1";
  const clusterOn = value("cluster_auto_summary") === "1";
  const minutes = value("digest_generate_minutes");
  const auto = page.data?.auto_summary;
  const usage = page.data?.usage;
  const specDefault = (id: string) => Number(schema.data?.specs.find((s) => s.id === id)?.default ?? 0);
  const summaryChoice = bindings.data ? taskChoice(bindings.data, "focus_summary") : null;
  const summaryEntry = findEntry(pool.data, summaryChoice);
  const summaryModel = summaryEntry ? modelName(summaryEntry) : summaryChoice?.model ?? "";

  const jobsSummary = () => {
    const summary = !autoOn ? "自动摘要关" : !auto ? "自动摘要开"
      : auto.model_ready ? `自动摘要开 · 今日 ${auto.today} / ${auto.daily_limit} 篇` : "自动摘要开 · 未选模型，暂停";
    const cluster = clusterOn && auto ? ` · 汇总 ${auto.cluster_model_ready ? `${auto.cluster_today} / ${auto.cluster_daily_limit} 组` : "暂停"}` : "";
    return `${summary}${cluster} · 简报每天 ${formatMinutes(Number(minutes))}`;
  };

  const reload = () => {
    if (writing) return;
    void page.refetch();
    void bindings.refetch();
    void pool.refetch();
  };
  const manage = (filter: ModelFilter, model?: ModelChoice | null) => { setNotice(null); setManaging({ filter, model }); };
  const addModels = (providerId = providers[0]?.id, note?: string) => { if (providerId) { setNotice(null); setAdding({ providerId, note }); } };
  const showUses = () => {
    setManaging(null);
    requestAnimationFrame(() => document.getElementById("model-uses")?.scrollIntoView({ block: "start" }));
  };
  // 升级提示的「查看自动任务」：展开「自动任务」并定位到自动摘要那一行。
  const showJobs = () => {
    setOpen((old) => ({ ...old, jobs: true }));
    requestAnimationFrame(() => {
      const row = document.getElementById("model-auto-summary");
      row?.scrollIntoView({ block: "center" });
      row?.setAttribute("data-flash", "");
      window.setTimeout(() => row?.removeAttribute("data-flash"), 1600);
    });
  };

  return (
    <div className="model-pane" aria-busy={writing ? "true" : "false"}>
      {(page.isError || bindings.isError || pool.isError) && (
        <div className="notice error" role="alert">
          <span>{formatNotice(page.data ? "刷新模型设置失败，当前显示的是上次读取的内容：" : "读取模型设置失败：",
            page.error ?? bindings.error ?? pool.error)}</span>
          <button type="button" className="btn sm" disabled={writing} onClick={reload}>重新读取</button>
        </div>
      )}
      {notice && <Feedback result={notice} />}
      {auto && <AutoSummaryUpgradeNotice auto={auto} tier={tier ?? ""} onShowJobs={showJobs}
        defaults={{ generation: specDefault("ai_daily_budget_tokens"), embedding: specDefault("ai_embed_daily_budget_tokens") }} />}

      <section className="settings-group settings-section" aria-labelledby="model-connections">
        <div className="settings-section-meta">
          <div className="settings-group-head"><h4 id="model-connections">连接 <span className="settings-count">{providers.length}</span></h4></div>
          <p className="hint">密钥只存在本机。测试会发一条很短的请求。</p>
        </div>
        <div className="settings-section-body">
          {!page.data && !page.isError && <Pending>正在读取连接…</Pending>}
          {page.data && !providers.length && (
            <div className="settings-empty pool-empty">
              <span>还没有连接。添加连接时选服务商、填 API Key，保存后直接挑要用的模型。</span>
              <button type="button" className="btn sm primary" disabled={writing || !presetList.length} onClick={() => setConnection({})}>添加连接</button>
            </div>
          )}
          <ConnectionList providers={providers} pool={pool.data} disabled={writing}
            onEdit={(provider) => { setNotice(null); setConnection({ provider }); }} />
          {providers.length > 0 && <div className="settings-section-actions">
            <button type="button" className="btn sm" disabled={writing || !presetList.length} onClick={() => { setNotice(null); setConnection({}); }}><Icon name="plus" size={14} />添加连接</button>
          </div>}
        </div>
      </section>

      {providers.length > 0 && (
        <ModelsSummary pool={pool.data} disabled={writing} firstRun={firstRun} onFirstRunDone={setFirstRun}
          onAdd={() => addModels()} onManage={manage} />
      )}

      {bindings.data ? (
        <UsesSection view={bindings.data} providers={providers} pool={pool.data} onAsk={onAsk} onNavigate={onNavigate}
          onManage={() => manage("")} />
      ) : !bindings.isError ? <Pending>正在读取模型选择…</Pending> : null}

      {bindings.data && <VectorSpaceSection onAsk={onAsk} />}

      <Fold title="用量" open={!!open.usage} onToggle={() => toggle("usage")}
        summary={schema.data ? `${tier ?? ""}档 · 生成 ${budget ? `${wan(budget)} / 天` : "不限额"} · 嵌入 ${embedBudget ? `${wan(embedBudget)} / 天` : "不限额"}${usage ? ` · 今日生成 ${wan(usage.generation.today)}` : ""}` : ""}>
        <SettingsRows ids={["ai_input_tier", "ai_daily_budget_tokens", "ai_embed_daily_budget_tokens"]} label="用量设置"
          notes={auto ? { ai_daily_budget_tokens: budgetNote({ auto, budget, autoOn, clusterOn, tier: tier ?? "" }) } : undefined}
          onEditStateChange={report("usage")} />
        {usage && <UsageReadouts usage={usage} budget={budget} embedBudget={embedBudget} />}
      </Fold>
      <Fold title="自动任务" open={!!open.jobs} onToggle={() => toggle("jobs")}
        summary={schema.data ? jobsSummary() : ""}>
        {auto && schema.data ? (
          <AutoJobs auto={auto} usage={usage} values={{ autoOn, clusterOn, budget }} tier={tier ?? ""} modelLabel={summaryModel}
            onAsk={onAsk} onShowUses={showUses} report={report} />
        ) : !page.isError ? <Pending>正在读取自动任务…</Pending> : null}
      </Fold>
      <button type="button" className="link-btn to-advanced" onClick={() => onNavigate("advanced", "advanced-model")}>此页的高级设置 →</button>

      {connection && (
        <ConnectionDialog provider={connection.provider} presets={presetList} onAsk={onAsk} onClose={() => setConnection(null)}
          onSaved={(id, message, created) => {
            if (created) addModels(id, `${message}勾选要用的模型。`);
            else setNotice({ tone: "ok", text: message });
          }}
          onEditStateChange={report("dialog")} />
      )}
      {managing && (
        <ManageModels providers={providers} initialFilter={managing.filter} initialModel={managing.model} onAsk={onAsk}
          onClose={() => setManaging(null)} onAddModels={() => addModels()} onShowUses={showUses} onEditStateChange={report("manage")} />
      )}
      {adding && (
        <AddModelsDialog providers={providers} pool={pool.data} initial={adding.providerId} note={adding.note} onAsk={onAsk}
          onClose={() => setAdding(null)} onEditStateChange={report("add")}
          onApplied={(message, first) => {
            setNotice({ tone: "ok", text: `${message}新模型的能力还没测，在「管理模型」里点「测试能力」。` });
            setFirstRun(first);
          }} />
      )}
    </div>
  );
}

const ago = (at: number) => {
  const minutes = Math.round((Date.now() - at) / 60_000);
  if (minutes < 1) return "刚刚";
  if (minutes < 60) return `${minutes} 分钟前`;
  if (minutes < 24 * 60) return `${Math.round(minutes / 60)} 小时前`;
  return new Date(at).toLocaleDateString(locale(), { month: "numeric", day: "numeric" });
};

/** 连接列表：状态点反映最近一次测试（记在连接上，重开设置仍在）。 */
function ConnectionList({ providers, pool, disabled, onEdit }: {
  providers: ProviderView[];
  pool?: ModelPoolView;
  disabled: boolean;
  onEdit: (provider: ProviderView) => void;
}) {
  const client = useQueryClient();
  const probe = useProbe();
  const [results, setResults] = useState<Partial<Record<number, Result>>>({});

  const test = async (p: ProviderView) => {
    const set = (result: Result) => setResults((old) => ({ ...old, [p.id]: result }));
    try {
      const outcome = await probe.run(String(p.id), () => testConnection(client, { provider_id: p.id, base_url: p.base_url }));
      if (!outcome.done) { if (outcome.note) set({ tone: "err", text: outcome.note }); return; }
      set(describeTest(outcome.value, []));
    } catch (err) {
      const view = explainAiError(err, []);
      set({ tone: "err", text: `测试失败。${view.title}`, detail: view.detail });
    }
  };

  if (!providers.length) return null;
  return (
    <ul className="connection-list">
      {providers.map((p) => {
        const result = results[p.id];
        const waiting = probe.wait?.key === String(p.id) ? probe.wait : null;
        const last = p.last_test;
        const state = waiting ? "busy" : result?.tone ?? (last ? (last.ok ? "ok" : "err") : "idle");
        let host = p.base_url;
        try { host = new URL(p.base_url).host; } catch { /* 原样显示 */ }
        const count = pool?.pool.filter((e) => e.provider_id === p.id).length ?? 0;
        return (
          <li key={p.id} className="connection-row">
            <SourceAvatar title={p.name} size={24} />
            <div className="connection-main">
              <strong>{p.name}</strong>
              <span className="hint">{!waiting && !result && last && !last.ok ? `测试失败：${last.note}` : `${host} · ${count} 个模型在池中`}</span>
              {p.key === "missing" && <span className="hint warn">需要重新填写 API Key</span>}
              {waiting ? <span className="hint" role="status">{probeWaitLabel(waiting)}</span>
                : result ? <Feedback result={result} />
                : null}
            </div>
            <div className="row connection-actions">
              {waiting ? <button type="button" className="btn sm ghost" onClick={probe.stop}>停止等待</button> : null}
              <button type="button" className="btn sm" disabled={disabled} onClick={() => onEdit(p)} aria-label={`编辑 ${p.name}`}>编辑</button>
              <button type="button" className="btn sm" disabled={!!waiting} aria-label={`测试 ${p.name}`} onClick={() => void test(p)}>测试</button>
            </div>
            <span className="connection-state" data-state={state} title={result?.text ?? (last ? `${ago(last.at)} ${last.ok ? `通过 · 返回 ${last.note} 个模型` : `测试失败：${last.note}`}` : undefined)}>
              {state !== "idle" && <span className="status-dot" data-state={state} aria-hidden="true" />}
              {waiting ? "测试中" : state === "ok" ? "可用" : state === "err" ? "失败" : "未测试"}
            </span>
          </li>
        );
      })}
    </ul>
  );
}

/** 模型页只放概况；列表与每个模型的设置在「管理模型」弹出页里。 */
function ModelsSummary({ pool, disabled, firstRun, onFirstRunDone, onAdd, onManage }: {
  pool?: ModelPoolView;
  disabled: boolean;
  firstRun: ModelChoice | null;
  onFirstRunDone: (next: null) => void;
  onAdd: () => void;
  onManage: (filter: ModelFilter) => void;
}) {
  const client = useQueryClient();
  const { writing, run } = useSettingsWrite(aiMutationOptions(client));
  const [status, setStatus] = useState<Result | null>(null);
  const entries = pool?.pool ?? [];
  const chats = entries.filter((e) => e.chat);
  const stats: Array<{ filter: ModelFilter; n: number; label: string; attn?: boolean }> = [
    { filter: "", n: entries.length, label: "个模型" },
    { filter: "chat", n: chats.length, label: "用于对话" },
    { filter: "untested", n: entries.filter(untested).length, label: "能力未测", attn: entries.some(untested) },
    { filter: "unused", n: entries.filter(unused).length, label: "没在用" },
  ];
  const first = firstRun && findEntry(pool, firstRun);
  const applyAll = async () => {
    if (!firstRun) return;
    const tasks = USE_GROUPS.filter((g) => g.id !== "embed" && (g.id !== "chat" || first?.chat)).flatMap((g) => g.tasks);
    const outcome = await run({ type: "bindings", args: { bindings: tasks.map((task) => ({ task, ...firstRun })) } });
    if (outcome.status === "busy" || !outcome.mounted) return;
    if (outcome.status === "failed") { setStatus({ tone: "err", text: formatNotice("没有应用：", outcome.error) }); return; }
    setStatus({ tone: "ok", text: `已把 ${tasks.length} 项用途设为 ${first ? modelName(first) : firstRun.model}。嵌入需要专门的嵌入模型，请单独选。` });
    onFirstRunDone(null);
  };
  return (
    <section className="settings-group settings-section" id="model-pool" aria-labelledby="model-pool-title">
      <div className="settings-section-meta">
        <div className="settings-group-head"><h4 id="model-pool-title">模型</h4></div>
      </div>
      <div className="settings-section-body">
        {entries.length ? <>
          <div className="model-stats">
            {stats.map((s) => (
              <button key={s.label} type="button" className="model-stat" data-attn={s.attn ? "1" : undefined}
                aria-label={`${s.label} ${s.n}`} onClick={() => onManage(s.filter)}>
                <span>{s.n}</span><small>{s.label}</small>
              </button>
            ))}
          </div>
          <p className="hint">对话可选：{chats.length ? chats.map(modelName).join("、") : "还没有，在「管理模型」里打开「用于对话」"}。</p>
        </> : (
          <div className="settings-empty pool-empty">
            <span>还没有模型。先挑几个常用的加进来，之后摘要、简报、对话选模型时只会列出这些。</span>
            <button type="button" className="btn sm primary" disabled={disabled} onClick={onAdd}>添加模型</button>
          </div>
        )}
        <div className="settings-section-actions">
          <button type="button" className="btn sm" onClick={() => onManage("")}>管理模型</button>
          {entries.length > 0 && <button type="button" className="btn sm ghost" disabled={disabled} onClick={onAdd}>添加模型</button>}
        </div>
        {first && (
          <div className="first-run" role="status">
            <span>要把 <span className="mono">{modelName(first)}</span> 用在全部用途上吗？以后可以逐项改。</span>
            <button type="button" className="btn sm primary" disabled={writing} onClick={() => void applyAll()}>全部用它</button>
            <button type="button" className="btn sm ghost" onClick={() => onFirstRunDone(null)}>稍后自己选</button>
          </div>
        )}
        {status && <Feedback result={status} />}
      </div>
    </section>
  );
}

/** 各用途用哪个模型：6 组，从加进来的模型里选，选完即存；组内各项不同时显示「N 种模型」，「细分」逐项改。 */
function UsesSection({ view, providers, pool, onAsk, onNavigate, onManage }: {
  view: ModelBindingsView;
  providers: ProviderView[];
  pool?: ModelPoolView;
  onAsk: Ask;
  onNavigate: Navigate;
  onManage: () => void;
}) {
  const client = useQueryClient();
  const { writing, run } = useSettingsWrite(aiMutationOptions(client));
  const translate = useQuery({ ...translateQueryOptions(), select: (p) => p.backend });
  const vectors = useQuery(vectorSpaceQueryOptions());
  const [status, setStatus] = useState<Result | null>(null);
  const [subs, setSubs] = useState<Record<string, boolean>>({});
  const [applyTo, setApplyTo] = useState<ModelChoice | null>(null);
  const [scope, setScope] = useState<Record<string, boolean>>(() => Object.fromEntries(APPLY_DEFAULT.map((g) => [g, true])));
  const entries = pool?.pool ?? [];
  const applyModel = applyTo && findEntry(pool, applyTo) ? applyTo : entries[0] ?? null;
  const title = (task: string) => view.tasks.find((t) => t.task === task)?.title ?? task;

  const save = async (tasks: string[], choice: ModelChoice, done: string) => {
    setStatus(null);
    const outcome = await run({ type: "bindings", args: { bindings: tasks.map((task) => ({ task, ...choice })) } });
    if (outcome.status === "busy" || !outcome.mounted) return;
    if (outcome.status === "failed") {
      const error = outcome.error;
      const message = error && typeof error === "object" && "message" in error ? error.message : error;
      setStatus({ tone: "err", text: formatNotice("没有保存：", message) });
    }
    else {
      setStatus({ tone: "ok", text: done });
      if (tasks.includes("embed")) void client.invalidateQueries({ queryKey: vectorSpaceKey });
    }
  };
  const pick = (tasks: string[], choice: ModelChoice) => {
    const e = findEntry(pool, choice);
    const label = e ? modelName(e) : choice.model;
    const done = `已保存：${tasks.length > 1 ? `${tasks.length} 项` : title(tasks[0])}改用 ${label}。`;
    const embed = taskChoice(view, "embed");
    if (tasks.includes("embed") && !(embed && sameChoice(embed, choice))) {
      const confirm = embedChangeConfirm(vectors.data, label);
      if (!confirm) {
        setStatus({ tone: "err", text: `没有更换。本库的向量由 ${vectors.data?.effective?.model} 生成，换成 ${label} 需要重建向量库：先打开下方「允许更换嵌入模型」，再改选。` });
        document.getElementById("vector-space")?.scrollIntoView({ block: "nearest" });
        return;
      }
      onAsk(confirm.title, confirm.message, () => void save(tasks, choice, done), { okLabel: confirm.okLabel });
    } else void save(tasks, choice, done);
  };
  const scoped = USE_GROUPS.filter((g) => g.id !== "chat" && scope[g.id]);
  const scopedTasks = scoped.flatMap((g) => g.tasks);

  return (
    <section className="settings-group settings-section" id="model-uses" aria-labelledby="model-uses-title">
      <div className="settings-section-meta">
        <div className="settings-group-head"><h4 id="model-uses-title">各用途</h4></div>
        <p className="hint">对话里可以临时换模型；这里是默认值。</p>
      </div>
      <div className="settings-section-body">
        {entries.length > 0 && (
          <div className="apply-bar" role="group" aria-label="一键应用">
            <div className="row">
              <span>把</span>
              <ModelMenu label="一键应用的模型" value={applyModel} providers={providers} pool={pool} onManage={onManage}
                disabled={writing} onPick={setApplyTo} />
              <span>用于</span>
              <button type="button" className="btn sm primary" disabled={writing || !applyModel || !scopedTasks.length}
                onClick={() => applyModel && pick(scopedTasks, applyModel)}>应用到 {scopedTasks.length} 项</button>
            </div>
            <div className="row apply-scope">
              {USE_GROUPS.filter((g) => g.id !== "chat").map((g) => (
                <label key={g.id}><input type="checkbox" checked={!!scope[g.id]} disabled={writing}
                  onChange={(e) => setScope((old) => ({ ...old, [g.id]: e.target.checked }))} />{g.title}</label>
              ))}
            </div>
          </div>
        )}
        {USE_GROUPS.map((g) => {
          const choices = g.tasks.map((t) => taskChoice(view, t));
          const keys = new Set(choices.map((c) => (c ? `${c.provider_id}|${c.model}` : "")));
          const mixed = keys.size > 1 ? `${keys.size} 种模型` : undefined;
          const value = mixed ? null : choices[0];
          const unset = choices.filter((c) => !c).length;
          const ownConfig = g.id === "chat" && view.chat_uses_own_config;
          const multi = g.tasks.length > 1;
          const warn = lacks(findEntry(pool, value), g.need);
          const backend = g.id === "translate" && translate.data && translate.data !== "complete"
            ? TRANSLATE_BACKENDS.find((b) => b.id === translate.data)?.label ?? translate.data : "";
          return (
            <div key={g.id} className="settings-option use-row" data-group={g.id}>
              <div>
                <strong>{g.title}</strong>
                {g.hint && <p className="hint">{g.hint}</p>}
                {backend && <p className="hint">现在用 {backend} 翻译，改成「大模型」后才会用这里的模型。
                  <button type="button" className="link-btn" onClick={() => onNavigate("translate")}>翻译设置</button></p>}
                {!ownConfig && (unset ? <p className="hint warn">{unset === g.tasks.length ? `还没选模型，${g.title}暂停。` : `${unset} 项没选模型，这几项暂停。`}</p>
                  : warn ? <p className="hint warn">{g.need === "tools" ? "这个模型不能调用工具，对话将无法查阅文章或读取网页。" : "这个模型不支持结构化输出，结果可能格式错乱。"}</p> : null)}
              </div>
              <div className="row use-control">
                {ownConfig ? (
                  <p className="hint">由你的 Pi 配置决定 · <button type="button" className="link-btn" onClick={() => onNavigate("assistant")}>对话助手</button></p>
                ) : g.id === "chat" ? (
                  <ChatModelMenu value={value} providers={providers} pool={pool} bindings={view} disabled={writing} onManage={onManage}
                    onPick={(c) => pick(["chat"], c)} />
                ) : (
                  <ModelMenu label={`${g.title}的模型`} value={value} mixed={mixed} need={g.need} providers={providers} pool={pool}
                    disabled={writing} onManage={onManage} onPick={(c) => pick(g.tasks, c)} />
                )}
                {multi && <button type="button" className="link-btn" aria-expanded={!!subs[g.id]}
                  onClick={() => setSubs((old) => ({ ...old, [g.id]: !old[g.id] }))}>{subs[g.id] ? "收起细分" : `细分（${g.tasks.length}）`}</button>}
              </div>
              {multi && subs[g.id] && (
                <div className="use-subs">
                  {g.tasks.map((task) => (
                    <div key={task} className="settings-option">
                      <div><strong>{title(task)}</strong></div>
                      <ModelMenu label={`${title(task)}的模型`} value={taskChoice(view, task)} need={g.need} providers={providers} pool={pool}
                        disabled={writing} onManage={onManage} onPick={(c) => pick([task], c)} />
                    </div>
                  ))}
                </div>
              )}
            </div>
          );
        })}
        {status && <Feedback result={status} />}
        {!entries.length && <p className="hint">先添加模型，再给每件事选模型。</p>}
      </div>
    </section>
  );
}
