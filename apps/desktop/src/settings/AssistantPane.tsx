import { useState } from "react";
import { useIsMutating, useQuery, useQueryClient } from "@tanstack/react-query";
import type { McpView, PiToolView } from "../bindings";
import { api, type ModelChoice, type PiInstallView, type PiPage, type PiProbe } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { Trans, useTranslation } from "../i18n";
import { aiMutationOptions } from "./aiQuery";
import { Field, Fold, SettingRow, Toggle } from "./chips";
import { McpDialog, McpImportDialog } from "./McpDialogs";
import { McpSection } from "./McpSection";
import { Dialog } from "./Dialog";
import { readGate, useEditReport, useMergedEditState, useSettingsWrite, type SettingsEditState } from "./editState";
import { piConfigOptions, piKeys, piMutationOptions, piProbeOptions, piQueryOptions, type PiAction, type PiStatus } from "./piQuery";
import { SettingsRows } from "./SettingsRows";
import { installProgressText, piInstalling, usePiInstall } from "./usePiInstall";
import { ChatModelMenu, chatDefault, findEntry, modelName, OwnModelMenu, sameChoice, thinkingLabel, usePoolChoices } from "./modelChoices";
import type { Ask, Navigate } from "./SettingsPanel";
import { Pending } from "../components/Pending";

type Run = (action: PiAction) => Promise<boolean>;
type DialogState = { kind: "tool"; tool: PiToolView } | { kind: "skill"; name?: string } | { kind: "mcp"; server?: McpView } | { kind: "mcpImport" };

export function AssistantPane({ onAsk, onNavigate, onEditStateChange }: {
  onAsk: Ask;
  onNavigate: Navigate;
  onEditStateChange?: (state: SettingsEditState | null) => void;
}) {
  const { t } = useTranslation("settingsAssistant");
  const client = useQueryClient();
  const { report } = useMergedEditState(onEditStateChange);
  const { writing, run: write } = useSettingsWrite(piMutationOptions(client));
  const install = usePiInstall();
  const page = useQuery({ ...piQueryOptions(), ...readGate(writing) });
  const probe = useQuery({ ...piProbeOptions(), ...readGate(writing) });
  const [status, setStatus] = useState<PiStatus | null>(null);
  const [dialog, setDialog] = useState<DialogState | null>(null);
  const [open, setOpen] = useState<Record<string, boolean>>({});
  const toggle = (key: string) => setOpen((old) => ({ ...old, [key]: !old[key] }));
  useEditReport(report("writes"), t("assistant.editLabel"), false, writing);

  // 写入本身不抛错，失败也作为提示返回；这里只挡掉并发写入并把结果交给页面提示。
  const runOutcome = async (action: PiAction): Promise<boolean | "busy"> => {
    setStatus(null);
    const outcome = await write(action);
    if (outcome.status === "busy") return "busy";
    const result = outcome.status === "done" ? outcome.data
      : { completed: false, status: { tone: "err" as const, text: formatNotice(t("assistant.actionFailed"), outcome.error) } };
    if (outcome.mounted) setStatus(result.status);
    return result.completed;
  };
  const run: Run = async (action) => (await runOutcome(action)) === true;

  const data = page.data;


  return (
    <div className="assistant-pane" aria-busy={writing ? "true" : "false"}>
      <EngineStatus probe={probe.data} loading={probe.isFetching && !probe.data} failed={probe.isError} disabled={writing}
        install={install.view} onInstall={() => void install.start()} onModels={() => onNavigate("model")} onRetry={() => void probe.refetch()} />
      {status && <p className={status.tone === "err" ? "notice error" : "form-status"} role={status.tone === "err" ? "alert" : "status"}>{status.text}</p>}
      {(page.isError) && (
        <div className="notice error" role="alert">
          <span>{formatNotice(t("assistant.loadFailed"), page.error)}</span>
          <button type="button" className="btn sm" disabled={writing} onClick={() => { void page.refetch(); }}>{t("assistant.reload")}</button>
        </div>
      )}

      <section className="settings-group settings-section" aria-labelledby="assistant-abilities">
        <div className="settings-section-meta">
          <div className="settings-group-head"><h4 id="assistant-abilities">{t("assistant.abilities.title")}</h4></div>
          <p className="hint">{t("assistant.abilities.hint")}</p>
        </div>
        <div className="settings-section-body">
          <ChatModel onNavigate={onNavigate} />
          <SettingRow label={t("assistant.abilities.localArticles.label")} hint={t("assistant.abilities.localArticles.hint")}>
            <span className="hint">{t("assistant.abilities.localArticles.always")}</span>
          </SettingRow>
          <SettingRow label={t("assistant.abilities.fetchUrl.label")} hint={t("assistant.abilities.fetchUrl.hint")}>
            <Toggle warning label={t("assistant.abilities.fetchUrl.label")} on={!!page.data?.fetch_url} onLabel="" offLabel=""
              disabled={writing || !page.data} onClick={() => void run({ type: "fetchUrl", enabled: !page.data?.fetch_url })} />
          </SettingRow>
        </div>
      </section>

      <section className="settings-group settings-section">
        <div className="settings-section-meta"><h4>{t("assistant.note.title")}</h4><p className="hint">{t("assistant.note.hint")}</p></div>
        <div className="settings-section-body">
        <SettingsRows className="assistant-note" ids={["pi_assistant_note"]} label={t("assistant.note.rowsLabel")} onEditStateChange={report("note")} />
        {data && (
          <details className="advanced-fold">
            <summary>{t("assistant.note.viewPrompt")}</summary>
            <pre className="system-prompt">{data.system_prompt}</pre>
          </details>
        )}
        </div>
      </section>
      {data && <>
        <section className="settings-group settings-section">
          <div className="settings-section-meta"><h4>{t("assistant.tools.title")} <span className="settings-count">{data.tools.filter((t) => t.enabled).length} / {data.tools.length}</span></h4><p className="hint">{t("assistant.tools.hint")}</p></div>
          <div className="settings-section-body">
          <ul className="settings-list">
            {data.tools.map((tool) => (
              <li key={tool.name} className="settings-list-row">
                <div>
                  <strong>{tool.label}</strong>{tool.confirm && <span className="meta-tag">{t("assistant.tools.confirm")}</span>}
                  {tool.description !== tool.default_description && <span className="meta-tag">{t("assistant.tools.edited")}</span>}
                  <p className="hint">{tool.description}</p>
                </div>
                <button type="button" className="btn sm" disabled={writing} aria-label={t("assistant.tools.editAria", { label: tool.label })}
                  onClick={() => setDialog({ kind: "tool", tool })}>{t("assistant.tools.edit")}</button>
                <Toggle label={t("assistant.tools.toggle", { label: tool.label })} on={tool.enabled} onLabel="" offLabel="" disabled={writing}
                  onClick={() => void run({ type: "tool", name: tool.name, enabled: !tool.enabled })} />
              </li>
            ))}
          </ul>
          </div>
        </section>
        <section className="settings-group settings-section">
          <div className="settings-section-meta"><h4>{t("assistant.extensions")}</h4></div>
          <div className="settings-section-body">
        <Fold title={t("assistant.skills.title")} open={!!open.skills} onToggle={() => toggle("skills")}
          summary={t("assistant.skills.summary", { total: data.skills.length, enabled: data.skills.filter((s) => s.enabled).length })}>
          <ul className="settings-list">
            {data.skills.map((s) => (
              <li key={s.name} className="settings-list-row">
                <div>
                  <strong>{s.name}</strong><span className="meta-tag">{s.bundled ? t("assistant.skills.bundled") : t("assistant.skills.custom")}</span>
                  <p className="hint">{s.description}</p>
                </div>
                {!s.bundled && <button type="button" className="btn sm" disabled={writing} aria-label={t("assistant.skills.editAria", { name: s.name })}
                  onClick={() => setDialog({ kind: "skill", name: s.name })}>{t("action.edit")}</button>}
                <Toggle label={t("assistant.skills.toggle", { name: s.name })} on={s.enabled} onLabel="" offLabel="" disabled={writing}
                  onClick={() => void run({ type: "skill", name: s.name, enabled: !s.enabled })} />
              </li>
            ))}
          </ul>
          <button type="button" className="btn sm" disabled={writing} onClick={() => setDialog({ kind: "skill" })}>{t("assistant.skills.create")}</button>
        </Fold>
        <Fold title={t("assistant.mcpFold.title")} open={!!open.mcp} onToggle={() => toggle("mcp")} summary={data.mcp_servers.length ? t("assistant.mcpFold.count", { count: data.mcp_servers.length }) : t("assistant.mcpFold.none")}>
      {data && <McpSection servers={data.mcp_servers} writing={writing}
        onAdd={() => setDialog({ kind: "mcp" })} onImport={() => setDialog({ kind: "mcpImport" })}
        onEdit={(server) => setDialog({ kind: "mcp", server })}
        onToggle={(m, enabled) => runOutcome({ type: "mcpToggle", id: m.id, enabled })}
        onSave={(input) => run({ type: "mcpSave", input })}
        onLoginFailed={(text) => setStatus({ tone: "err", text })} />}

        </Fold>
          </div>
        </section>
      </>}

      <Engine page={data} probe={probe.data} writing={writing} run={run} report={report} />
      <button type="button" className="link-btn to-advanced" onClick={() => onNavigate("advanced", "advanced-assistant")}>{t("assistant.advanced")}</button>

      {dialog?.kind === "tool" && <ToolDialog tool={dialog.tool} run={run} onAsk={onAsk} onClose={() => setDialog(null)} report={report} />}
      {dialog?.kind === "skill" && <SkillDialog name={dialog.name} run={run} onAsk={onAsk} onClose={() => setDialog(null)} report={report} />}
      {dialog?.kind === "mcp" && <McpDialog server={dialog.server} run={run} status={status} onAsk={onAsk} onClose={() => setDialog(null)}
        onPaste={() => setDialog({ kind: "mcpImport" })} onEditStateChange={report("dialog")} />}
      {dialog?.kind === "mcpImport" && <McpImportDialog run={run} status={status} onAsk={onAsk} onClose={() => setDialog(null)} onEditStateChange={report("dialog")} />}
    </div>
  );
}

/**
 * 状态行：正常时一句话；出问题时一句原因加一个操作。下载在后台进行，这里只显示进度；
 * 缺引擎或下载中时说明装到哪里、不动本机 Pi 的配置。
 */
function EngineStatus({ probe, loading, failed, disabled, install, onInstall, onModels, onRetry }: {
  probe?: PiProbe;
  loading: boolean;
  failed: boolean;
  disabled: boolean;
  install: PiInstallView | null;
  onInstall: () => void;
  onModels: () => void;
  onRetry: () => void;
}) {
  const { t } = useTranslation("settingsAssistant");
  const version = probe?.version.match(/\d+\.\d+\.\d+/)?.[0] ?? "";
  const installing = piInstalling(install);
  const needsInstall = probe?.version_state === "missing" || probe?.version_state === "too_old";
  const installFailed = install?.state === "failed" && needsInstall;
  const pct = install?.total ? Math.min(100, Math.round((install.received / install.total) * 100)) : null;
  const view: { tone: "ok" | "warn" | "err"; text: string; action?: [string, () => void] } =
    installing && install ? { tone: "ok", text: installProgressText(install) }
    : installFailed ? { tone: "err", text: t("engine.downloadFailed", { message: install.message }), action: [t("engine.redownload"), onInstall] }
    : loading || !probe
      ? failed ? { tone: "err", text: t("engine.detectFailed"), action: [t("engine.redetect"), onRetry] } : { tone: "ok", text: t("engine.detecting") }
      : probe.version_state === "missing" ? { tone: "err", text: t("engine.missing"), action: [t("engine.download"), onInstall] }
      : probe.version_state === "too_old" ? { tone: "err", text: t("engine.tooOld", { version: version || probe.version }), action: [t("engine.upgrade"), onInstall] }
      : probe.config_error ? { tone: "err", text: probe.config_error, action: [t("engine.setModel"), onModels] }
      : probe.version_state === "unverified"
        ? { tone: "warn", text: t("engine.ready") }
        : { tone: "ok", text: t("engine.ready") };
  return (
    <>
    <div className="engine-status" data-tone={view.tone} role={view.tone === "err" ? "alert" : "status"} aria-busy={installing}>
      {/* 下载中由进度条表示状态，不再叠一个圆点。 */}
      {!installing && <span className="status-dot" data-state={view.tone === "ok" ? (loading ? "busy" : "ok") : view.tone === "warn" ? "warn" : "err"} aria-hidden="true" />}
      <span>{view.text}</span>
      {installing && <progress className="engine-progress" max={100} value={pct ?? undefined} aria-label={t("engine.progressLabel")}>{pct ?? ""}</progress>}
      {version && <span className="engine-version">{t(probe?.version_state === "unverified" ? "engine.versionNote.unverified" : probe?.version_state === "ok" ? "engine.versionNote.ok" : probe?.version_state === "too_old" ? "engine.versionNote.tooOld" : "engine.versionNote.other", { version })}</span>}
      <button type="button" className="btn sm ghost engine-recheck" disabled={disabled || loading || installing} onClick={onRetry}>{t("engine.recheck")}</button>
      {view.action && <button type="button" className="btn sm" disabled={disabled} onClick={view.action[1]}>{view.action[0]}</button>}
    </div>
    {(installing || needsInstall) && (
      <p className="hint engine-install-note">
        {installing ? t("engine.installBackground") : ""}
        {install?.dir ? <Trans t={t} i18nKey="engine.installNoteDir" values={{ dir: install.dir }} components={{ code: <code /> }} /> : t("engine.installNote")}
      </p>
    )}
    </>
  );
}

/**
 * 对话模型：从用于对话的模型里选；和模型页「对话」是同一项、同一查询键，改完立即保存。
 * 思考强度跟着模型走（阶段 19-7），这里只读显示，到「管理模型」里改。
 */
function ChatModel({ onNavigate }: { onNavigate: Navigate }) {
  const { t } = useTranslation("settingsAssistant");
  const client = useQueryClient();
  const { writing, run } = useSettingsWrite(aiMutationOptions(client));
  const { pool, providers, bindings } = usePoolChoices(readGate(writing));
  const [error, setError] = useState("");
  const current = chatDefault(bindings);
  const entry = findEntry(pool, current);
  const own = !!bindings?.chat_uses_own_config;

  const choose = async (choice: ModelChoice) => {
    if (sameChoice(choice, current)) return;
    setError("");
    const outcome = await run({ type: "bindings", args: { bindings: [{ task: "chat", ...choice }] } });
    if (outcome.status === "failed" && outcome.mounted) setError(formatNotice(t("chatModel.saveFailed"), outcome.error));
  };

  return (
    <>

        <div className="settings-option">
          <div>
            <strong>{t("chatModel.defaultModel")}</strong>
            <p className="hint">{own ? t("chatModel.hint.own")
              : current ? t("chatModel.hint.chosen") : t("chatModel.hint.none")}</p>
            {entry?.effective.tool_calling === "unsupported" && (
              <p className="hint warn">{t("chatModel.noTools")}</p>
            )}
            {error && <p className="form-status error" role="alert">{error}</p>}
          </div>
          {own ? <OwnModel /> : (
            <ChatModelMenu value={current} onPick={(c) => void choose(c)} providers={providers} pool={pool} bindings={bindings}
              disabled={writing || !bindings} onManage={() => onNavigate("model", "model-pool")} />
          )}
        </div>
        {!own && (
          <div className="settings-option">
            <div><strong>{t("chatModel.thinking")}</strong><p className="hint">{t("chatModel.thinkingHint")}</p></div>
            <span className="row">
              <span className="hint">{!entry ? "—" : entry.effective.reasoning === "unsupported" ? t("chatModel.reasoningUnsupported")
                : `${modelName(entry)} · ${thinkingLabel(entry.thinking)}`}</span>
              <button type="button" className="link-btn" onClick={() => onNavigate("model", "model-pool")}>{t("chatModel.changeInModels")}</button>
            </span>
          </div>
        )}
    </>
  );
}

/** 自有 Pi 配置下的对话模型：只列 Pi 的模型，写入 `pi_own_model`。 */
function OwnModel() {
  const { t } = useTranslation("settingsAssistant");
  const client = useQueryClient();
  const { writing, run } = useSettingsWrite(piMutationOptions(client));
  const config = useQuery({ ...piConfigOptions(), ...readGate(writing) });
  const [error, setError] = useState("");
  const current = config.data?.own_model ?? "";

  const choose = async (own_model: string) => {
    if (own_model === current) return;
    setError("");
    const outcome = await run({ type: "config", args: { own_model } });
    if (outcome.status === "busy" || !outcome.mounted) return;
    if (outcome.status === "failed") setError(formatNotice(t("chatModel.saveFailed"), outcome.error));
    else if (!outcome.data.completed) setError(outcome.data.status.text);
  };

  return (
    <span className="advanced-inline">
      <OwnModelMenu value={current} ownModel={current} onPick={(m) => void choose(m)} disabled={writing || !config.data} />
      {error && <span className="form-status error" role="alert">{error}</span>}
    </span>
  );
}

/** 页底的对话引擎分区；路径、开发者工具等在「高级」。 */
function Engine({ page, probe, writing, run, report }: {
  page?: PiPage;
  probe?: PiProbe;
  writing: boolean;
  run: Run;
  report: (part: string) => (state: SettingsEditState | null) => void;
}) {
  const { t } = useTranslation("settingsAssistant");
  return (
    <section className="settings-group engine settings-section" aria-labelledby="assistant-engine">
      <div className="settings-section-meta">
        <div className="settings-group-head">
          <h4 id="assistant-engine">{t("engine.title")}</h4>
        </div>
      </div>
      <div className="settings-section-body">
        <div className="settings-group-actions">
          <button type="button" className="btn sm" disabled={writing} onClick={() => void run({ type: "restart" })}>{t("engine.restart")}</button>
        </div>
        <SettingsRows ids={["pi_idle_timeout_secs"]} label={t("engine.idle")} onEditStateChange={report("idle")} />
        <dl className="diag-list">
          <div className="diag-row"><dt>{t("engine.version")}</dt><dd>{probe?.version || "—"}</dd></div>
          <div className="diag-row"><dt>{t("engine.source")}</dt><dd>{page?.manage_status || "—"} · {probe?.binary_source || page?.binary_source || "—"}</dd></div>
        </dl>
        <SettingsRows ids={["pi_auto_download", "pi_auto_update"]} label={t("engine.autoUpdate")} onEditStateChange={report("engine")} />
      </div>
    </section>
  );
}

type DialogProps = { run: Run; onAsk: Ask; onClose: () => void; report: (part: string) => (state: SettingsEditState | null) => void };

function useDialogWrite(run: Run) {
  const writing = useIsMutating({ mutationKey: piKeys.write }) > 0;
  const submit = async (action: PiAction, done: () => void) => { if (await run(action)) done(); };
  return { writing, submit };
}

function ToolDialog({ tool, run, onAsk, onClose, report }: DialogProps & { tool: PiToolView }) {
  const { t } = useTranslation("settingsAssistant");
  const [text, setText] = useState(tool.description);
  const { writing, submit } = useDialogWrite(run);
  return (
    <Dialog title={t("toolDialog.title", { label: tool.label })} dirty={text !== tool.description} busy={writing} onAsk={onAsk} onClose={onClose}
      onEditStateChange={report("dialog")}
      leading={<button type="button" className="btn ghost" disabled={writing || text === tool.default_description}
        onClick={() => setText(tool.default_description)}>{t("toolDialog.restoreDefault")}</button>}
      actions={<button type="button" className="btn primary" disabled={writing || !text.trim()}
        onClick={() => void submit({ type: "toolDescription", name: tool.name, description: text }, onClose)}>{t("action.save")}</button>}>
      <Field label={t("toolDialog.field")} hint={t("toolDialog.hint")}>
        <textarea rows={5} value={text} disabled={writing} onChange={(e) => setText(e.target.value)} />
      </Field>
    </Dialog>
  );
}

function SkillDialog({ name, run, onAsk, onClose, report }: DialogProps & { name?: string }) {
  const { t } = useTranslation("settingsAssistant");
  const loaded = useQuery({ queryKey: ["settings", "pi", "skill", name], queryFn: () => api.piSkill(name!), enabled: !!name,
    staleTime: 0, gcTime: 0, retry: false });
  const [draft, setDraft] = useState<{ name: string; description: string; body: string } | null>(name ? null : { name: "", description: "", body: "" });
  const base = name ? loaded.data : { name: "", description: "", body: "" };
  const current = draft ?? base;
  const dirty = !!draft && !!base && (draft.name !== base.name || draft.description !== base.description || draft.body !== base.body);
  const { writing, submit } = useDialogWrite(run);
  const change = (key: "name" | "description" | "body", value: string) => current && setDraft({ ...current, [key]: value });
  const remove = () => name && onAsk(t("skillDialog.delete"), t("skillDialog.confirmMessage", { name }),
    () => void submit({ type: "skillDelete", name }, onClose), { okLabel: t("action.delete") });
  return (
    <Dialog title={name ? t("skillDialog.edit") : t("skillDialog.create")} dirty={dirty} busy={writing} onAsk={onAsk} onClose={onClose} onEditStateChange={report("dialog")}
      leading={name ? <button type="button" className="btn danger" disabled={writing} onClick={remove}>{t("skillDialog.delete")}</button> : null}
      actions={<button type="button" className="btn primary" disabled={writing || !current?.name.trim() || !current.description.trim()}
        onClick={() => current && void submit({ type: "skillSave", draft: current }, onClose)}>{t("action.save")}</button>}>
      {name && loaded.isError && <p className="notice error" role="alert">{formatNotice(t("skillDialog.loadFailed"), loaded.error)}</p>}
      {!current ? <Pending>{t("skillDialog.loading")}</Pending> : <>
        <Field label={t("skillDialog.name")} hint={t("skillDialog.nameHint")}>
          <input value={current.name} disabled={writing || !!name} onChange={(e) => change("name", e.target.value)} />
        </Field>
        <Field label={t("skillDialog.when")} hint={t("skillDialog.whenHint")}>
          <input value={current.description} disabled={writing} onChange={(e) => change("description", e.target.value)} />
        </Field>
        <Field label={t("skillDialog.body")}>
          <textarea rows={8} value={current.body} disabled={writing} onChange={(e) => change("body", e.target.value)} />
        </Field>
      </>}
    </Dialog>
  );
}
