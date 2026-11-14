import { settingOptions } from "./registry";
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
import { i18n, locale, Trans, useTranslation } from "../i18n";

type Result = { tone: "ok" | "err"; text: string; detail?: string };

/** 用途按功能分组显示，存储仍按用途一行（阶段 19-7）；用途常量以后端 `ALL_TASKS` 为准。 */
type UseGroupId = "chat" | "summary" | "digest" | "memory" | "translate" | "embed";
export const USE_GROUPS: Array<{ id: UseGroupId; tasks: string[]; need?: Need }> = [
  { id: "chat", tasks: ["chat"], need: "tools" },
  { id: "summary", tasks: ["focus_summary", "article_summary", "article_segment", "cluster_summary"], need: "json" },
  { id: "digest", tasks: ["digest_daily", "digest_weekly", "digest_pick_daily", "digest_pick_weekly"], need: "json" },
  { id: "memory", tasks: ["memory_candidate", "episode_compress"], need: "json" },
  { id: "translate", tasks: ["article_translate"] },
  { id: "embed", tasks: ["embed"] },
];
/** 一键应用默认勾选的组：翻译、嵌入通常要专用模型，对话单独选。 */
const APPLY_DEFAULT = ["summary", "digest", "memory"];

export function ModelPane({ onAsk, onNavigate, onEditStateChange }: {
  onAsk: Ask;
  onNavigate: Navigate;
  onEditStateChange?: (state: SettingsEditState | null) => void;
}) {
  const { t } = useTranslation("settingsModels");
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
  const tier = tierSpec?.kind.type === "enum" ? settingOptions("ai_input_tier", tierSpec.kind.options).find((o) => o.id === value("ai_input_tier"))?.label : "";
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
    const summary = !autoOn ? t("pane.jobs.off") : !auto ? t("pane.jobs.on")
      : auto.model_ready ? t("pane.jobs.onToday", { today: auto.today, limit: auto.daily_limit }) : t("pane.jobs.onPaused");
    const cluster = clusterOn && auto
      ? auto.cluster_model_ready ? t("pane.jobs.cluster", { today: auto.cluster_today, limit: auto.cluster_daily_limit }) : t("pane.jobs.clusterPaused")
      : "";
    return [summary, cluster, t("pane.jobs.digest", { time: formatMinutes(Number(minutes)) })].filter(Boolean).join(" · ");
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
          <span>{formatNotice(page.data ? t("pane.refreshFailed") : t("pane.loadFailed"),
            page.error ?? bindings.error ?? pool.error)}</span>
          <button type="button" className="btn sm" disabled={writing} onClick={reload}>{t("pane.reload")}</button>
        </div>
      )}
      {notice && <Feedback result={notice} />}
      {auto && <AutoSummaryUpgradeNotice auto={auto} tier={tier ?? ""} onShowJobs={showJobs}
        defaults={{ generation: specDefault("ai_daily_budget_tokens"), embedding: specDefault("ai_embed_daily_budget_tokens") }} />}

      <section className="settings-group settings-section" aria-labelledby="model-connections">
        <div className="settings-section-meta">
          <div className="settings-group-head"><h4 id="model-connections">{t("pane.connections")} <span className="settings-count">{providers.length}</span></h4></div>
          <p className="hint">{t("pane.connectionsHint")}</p>
        </div>
        <div className="settings-section-body">
          {!page.data && !page.isError && <Pending>{t("pane.loadingConnections")}</Pending>}
          {page.data && !providers.length && (
            <div className="settings-empty pool-empty">
              <span>{t("pane.noConnections")}</span>
              <button type="button" className="btn sm primary" disabled={writing || !presetList.length} onClick={() => setConnection({})}>{t("pane.addConnection")}</button>
            </div>
          )}
          <ConnectionList providers={providers} pool={pool.data} disabled={writing}
            onEdit={(provider) => { setNotice(null); setConnection({ provider }); }} />
          {providers.length > 0 && <div className="settings-section-actions">
            <button type="button" className="btn sm" disabled={writing || !presetList.length} onClick={() => { setNotice(null); setConnection({}); }}><Icon name="plus" size={14} />{t("pane.addConnection")}</button>
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
      ) : !bindings.isError ? <Pending>{t("pane.loadingBindings")}</Pending> : null}

      {bindings.data && <VectorSpaceSection onAsk={onAsk} />}

      <Fold title={t("pane.usage")} open={!!open.usage} onToggle={() => toggle("usage")}
        summary={schema.data ? t(usage ? "pane.usageSummaryToday" : "pane.usageSummary", {
          tier: tier ?? "",
          gen: budget ? t("pane.perDay", { amount: wan(budget) }) : t("pane.unlimited"),
          embed: embedBudget ? t("pane.perDay", { amount: wan(embedBudget) }) : t("pane.unlimited"),
          today: usage ? wan(usage.generation.today) : "",
        }) : ""}>
        <SettingsRows ids={["ai_input_tier", "ai_daily_budget_tokens", "ai_embed_daily_budget_tokens"]} label={t("pane.usageSettings")}
          notes={auto ? { ai_daily_budget_tokens: budgetNote({ auto, budget, autoOn, clusterOn, tier: tier ?? "" }) } : undefined}
          onEditStateChange={report("usage")} />
        {usage && <UsageReadouts usage={usage} budget={budget} embedBudget={embedBudget} />}
      </Fold>
      <Fold title={t("pane.jobs.title")} open={!!open.jobs} onToggle={() => toggle("jobs")}
        summary={schema.data ? jobsSummary() : ""}>
        {auto && schema.data ? (
          <AutoJobs auto={auto} usage={usage} values={{ autoOn, clusterOn, budget }} tier={tier ?? ""} modelLabel={summaryModel}
            onAsk={onAsk} onShowUses={showUses} report={report} />
        ) : !page.isError ? <Pending>{t("pane.loadingJobs")}</Pending> : null}
      </Fold>
      <button type="button" className="link-btn to-advanced" onClick={() => onNavigate("advanced", "advanced-model")}>{t("pane.advanced")}</button>

      {connection && (
        <ConnectionDialog provider={connection.provider} presets={presetList} onAsk={onAsk} onClose={() => setConnection(null)}
          onSaved={(id, message, created) => {
            if (created) addModels(id, t("pane.pickModels", { message }));
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
            setNotice({ tone: "ok", text: t("pane.afterApply", { message }) });
            setFirstRun(first);
          }} />
      )}
    </div>
  );
}

const ago = (at: number) => {
  const minutes = Math.round((Date.now() - at) / 60_000);
  if (minutes < 1) return i18n.t("pane.ago.now", { ns: "settingsModels" });
  if (minutes < 60) return i18n.t("pane.ago.minutes", { ns: "settingsModels", count: minutes });
  if (minutes < 24 * 60) return i18n.t("pane.ago.hours", { ns: "settingsModels", count: Math.round(minutes / 60) });
  return new Date(at).toLocaleDateString(locale(), { month: "numeric", day: "numeric" });
};

/** 连接列表：状态点反映最近一次测试（记在连接上，重开设置仍在）。 */
function ConnectionList({ providers, pool, disabled, onEdit }: {
  providers: ProviderView[];
  pool?: ModelPoolView;
  disabled: boolean;
  onEdit: (provider: ProviderView) => void;
}) {
  const { t } = useTranslation("settingsModels");
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
      set({ tone: "err", text: t("connection.testFailedTitle", { title: view.title }), detail: view.detail });
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
              <span className="hint">{!waiting && !result && last && !last.ok ? t("pane.conn.lastFailed", { note: last.note }) : t("pane.conn.summary", { host, count })}</span>
              {p.key === "missing" && <span className="hint warn">{t("pane.conn.keyMissing")}</span>}
              {waiting ? <span className="hint" role="status">{probeWaitLabel(waiting)}</span>
                : result ? <Feedback result={result} />
                : null}
            </div>
            <div className="row connection-actions">
              {waiting ? <button type="button" className="btn sm ghost" onClick={probe.stop}>{t("connection.stopWaiting")}</button> : null}
              <button type="button" className="btn sm" disabled={disabled} onClick={() => onEdit(p)} aria-label={t("pane.conn.editAria", { name: p.name })}>{t("pane.conn.edit")}</button>
              <button type="button" className="btn sm" disabled={!!waiting} aria-label={t("pane.conn.testAria", { name: p.name })} onClick={() => void test(p)}>{t("pane.conn.test")}</button>
            </div>
            <span className="connection-state" data-state={state} title={result?.text ?? (last ? t(last.ok ? "pane.conn.lastOk" : "pane.conn.lastFailedAt", { ago: ago(last.at), note: last.note }) : undefined)}>
              {state !== "idle" && <span className="status-dot" data-state={state} aria-hidden="true" />}
              {t(waiting ? "pane.conn.stateBusy" : state === "ok" ? "pane.conn.stateOk" : state === "err" ? "pane.conn.stateErr" : "pane.conn.stateIdle")}
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
  const { t } = useTranslation("settingsModels");
  const client = useQueryClient();
  const { writing, run } = useSettingsWrite(aiMutationOptions(client));
  const [status, setStatus] = useState<Result | null>(null);
  const entries = pool?.pool ?? [];
  const chats = entries.filter((e) => e.chat);
  const stats: Array<{ filter: ModelFilter; n: number; attn?: boolean }> = [
    { filter: "", n: entries.length },
    { filter: "chat", n: chats.length },
    { filter: "untested", n: entries.filter(untested).length, attn: entries.some(untested) },
    { filter: "unused", n: entries.filter(unused).length },
  ];
  const first = firstRun && findEntry(pool, firstRun);
  const applyAll = async () => {
    if (!firstRun) return;
    const tasks = USE_GROUPS.filter((g) => g.id !== "embed" && (g.id !== "chat" || first?.chat)).flatMap((g) => g.tasks);
    const outcome = await run({ type: "bindings", args: { bindings: tasks.map((task) => ({ task, ...firstRun })) } });
    if (outcome.status === "busy" || !outcome.mounted) return;
    if (outcome.status === "failed") { setStatus({ tone: "err", text: formatNotice(t("pane.applyFailed"), outcome.error) }); return; }
    setStatus({ tone: "ok", text: t("pane.appliedAll", { count: tasks.length, name: first ? modelName(first) : firstRun.model }) });
    onFirstRunDone(null);
  };
  return (
    <section className="settings-group settings-section" id="model-pool" aria-labelledby="model-pool-title">
      <div className="settings-section-meta">
        <div className="settings-group-head"><h4 id="model-pool-title">{t("pane.models")}</h4></div>
      </div>
      <div className="settings-section-body">
        {entries.length ? <>
          <div className="model-stats">
            {stats.map((s) => (
              <button key={s.filter} type="button" className="model-stat" data-attn={s.attn ? "1" : undefined}
                aria-label={`${t(`pane.stat.${s.filter || "all"}`)} ${s.n}`} onClick={() => onManage(s.filter)}>
                <span>{s.n}</span><small>{t(`pane.stat.${s.filter || "all"}`)}</small>
              </button>
            ))}
          </div>
          <p className="hint">{chats.length ? t("pane.chatChoices", { list: chats.map(modelName).join(t("pane.listSep")) }) : t("pane.chatChoicesNone")}</p>
        </> : (
          <div className="settings-empty pool-empty">
            <span>{t("pane.noModels")}</span>
            <button type="button" className="btn sm primary" disabled={disabled} onClick={onAdd}>{t("pane.addModels")}</button>
          </div>
        )}
        <div className="settings-section-actions">
          <button type="button" className="btn sm" onClick={() => onManage("")}>{t("pane.manage")}</button>
          {entries.length > 0 && <button type="button" className="btn sm ghost" disabled={disabled} onClick={onAdd}>{t("pane.addModels")}</button>}
        </div>
        {first && (
          <div className="first-run" role="status">
            <span><Trans t={t} i18nKey="pane.firstRun" values={{ name: modelName(first) }} components={{ mono: <span className="mono" /> }} /></span>
            <button type="button" className="btn sm primary" disabled={writing} onClick={() => void applyAll()}>{t("pane.useAll")}</button>
            <button type="button" className="btn sm ghost" onClick={() => onFirstRunDone(null)}>{t("pane.later")}</button>
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
  const { t } = useTranslation("settingsModels");
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
  const title = (task: string) => view.tasks.find((x) => x.task === task)?.title ?? task;

  const save = async (tasks: string[], choice: ModelChoice, done: string) => {
    setStatus(null);
    const outcome = await run({ type: "bindings", args: { bindings: tasks.map((task) => ({ task, ...choice })) } });
    if (outcome.status === "busy" || !outcome.mounted) return;
    if (outcome.status === "failed") {
      setStatus({ tone: "err", text: formatNotice(t("pane.saveFailed"), outcome.error) });
    }
    else {
      setStatus({ tone: "ok", text: done });
      if (tasks.includes("embed")) void client.invalidateQueries({ queryKey: vectorSpaceKey });
    }
  };
  const pick = (tasks: string[], choice: ModelChoice) => {
    const e = findEntry(pool, choice);
    const label = e ? modelName(e) : choice.model;
    const done = tasks.length > 1 ? t("pane.savedMany", { count: tasks.length, label }) : t("pane.savedOne", { title: title(tasks[0]), label });
    const embed = taskChoice(view, "embed");
    if (tasks.includes("embed") && !(embed && sameChoice(embed, choice))) {
      const confirm = embedChangeConfirm(vectors.data, label);
      if (!confirm) {
        setStatus({ tone: "err", text: t("pane.embedBlocked", { from: vectors.data?.effective?.model, to: label }) });
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
        <div className="settings-group-head"><h4 id="model-uses-title">{t("pane.uses")}</h4></div>
        <p className="hint">{t("pane.usesHint")}</p>
      </div>
      <div className="settings-section-body">
        {entries.length > 0 && (
          <div className="apply-bar" role="group" aria-label={t("pane.applyBar")}>
            <div className="row">
              <span>{t("pane.applyPre")}</span>
              <ModelMenu label={t("pane.applyModel")} value={applyModel} providers={providers} pool={pool} onManage={onManage}
                disabled={writing} onPick={setApplyTo} />
              <span>{t("pane.applyPost")}</span>
              <button type="button" className="btn sm primary" disabled={writing || !applyModel || !scopedTasks.length}
                onClick={() => applyModel && pick(scopedTasks, applyModel)}>{t("pane.applyTo", { count: scopedTasks.length })}</button>
            </div>
            <div className="row apply-scope">
              {USE_GROUPS.filter((g) => g.id !== "chat").map((g) => (
                <label key={g.id}><input type="checkbox" checked={!!scope[g.id]} disabled={writing}
                  onChange={(e) => setScope((old) => ({ ...old, [g.id]: e.target.checked }))} />{t(`pane.use.${g.id}.title`)}</label>
              ))}
            </div>
          </div>
        )}
        {USE_GROUPS.map((g) => {
          const choices = g.tasks.map((task) => taskChoice(view, task));
          const groupTitle = t(`pane.use.${g.id}.title`);
          const keys = new Set(choices.map((c) => (c ? `${c.provider_id}|${c.model}` : "")));
          const mixed = keys.size > 1 ? t("pane.mixed", { count: keys.size }) : undefined;
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
                <strong>{groupTitle}</strong>
                {g.id !== "translate" && <p className="hint">{t(`pane.use.${g.id}.hint`)}</p>}
                {backend && <p className="hint">{t("pane.translateBackend", { backend })}
                  <button type="button" className="link-btn" onClick={() => onNavigate("translate")}>{t("pane.translateSettings")}</button></p>}
                {!ownConfig && (unset ? <p className="hint warn">{unset === g.tasks.length ? t("pane.unsetAll", { title: groupTitle }) : t("pane.unsetSome", { count: unset })}</p>
                  : warn ? <p className="hint warn">{t(g.need === "tools" ? "pane.warnTools" : "pane.warnJson")}</p> : null)}
              </div>
              <div className="row use-control">
                {ownConfig ? (
                  <p className="hint"><Trans t={t} i18nKey="pane.ownConfig" components={{ link: <button type="button" className="link-btn" onClick={() => onNavigate("assistant")} /> }} /></p>
                ) : g.id === "chat" ? (
                  <ChatModelMenu value={value} providers={providers} pool={pool} bindings={view} disabled={writing} onManage={onManage}
                    onPick={(c) => pick(["chat"], c)} />
                ) : (
                  <ModelMenu label={t("pane.useModel", { title: groupTitle })} value={value} mixed={mixed} need={g.need} providers={providers} pool={pool}
                    disabled={writing} onManage={onManage} onPick={(c) => pick(g.tasks, c)} />
                )}
                {multi && <button type="button" className="link-btn" aria-expanded={!!subs[g.id]}
                  onClick={() => setSubs((old) => ({ ...old, [g.id]: !old[g.id] }))}>{subs[g.id] ? t("pane.collapse") : t("pane.expand", { count: g.tasks.length })}</button>}
              </div>
              {multi && subs[g.id] && (
                <div className="use-subs">
                  {g.tasks.map((task) => (
                    <div key={task} className="settings-option">
                      <div><strong>{title(task)}</strong></div>
                      <ModelMenu label={t("pane.useModel", { title: title(task) })} value={taskChoice(view, task)} need={g.need} providers={providers} pool={pool}
                        disabled={writing} onManage={onManage} onPick={(c) => pick([task], c)} />
                    </div>
                  ))}
                </div>
              )}
            </div>
          );
        })}
        {status && <Feedback result={status} />}
        {!entries.length && <p className="hint">{t("pane.addFirst")}</p>}
      </div>
    </section>
  );
}
