import { useState } from "react";
import { useIsMutating, useQuery, useQueryClient } from "@tanstack/react-query";
import type { McpView, PiToolView } from "../bindings";
import { api, type ModelChoice, type PiInstallView, type PiPage, type PiProbe } from "../lib/api";
import { formatNotice } from "../lib/notice";
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
  useEditReport(report("writes"), "对话助手设置", false, writing);

  // 写入本身不抛错，失败也作为提示返回；这里只挡掉并发写入并把结果交给页面提示。
  const runOutcome = async (action: PiAction): Promise<boolean | "busy"> => {
    setStatus(null);
    const outcome = await write(action);
    if (outcome.status === "busy") return "busy";
    const result = outcome.status === "done" ? outcome.data
      : { completed: false, status: { tone: "err" as const, text: formatNotice("操作失败：", outcome.error) } };
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
          <span>{formatNotice("读取对话助手设置失败：", page.error)}</span>
          <button type="button" className="btn sm" disabled={writing} onClick={() => { void page.refetch(); }}>重新读取</button>
        </div>
      )}

      <section className="settings-group settings-section" aria-labelledby="assistant-abilities">
        <div className="settings-section-meta">
          <div className="settings-group-head"><h4 id="assistant-abilities">模型与能力</h4></div>
          <p className="hint">默认模型和模型页的「对话」是同一项。</p>
        </div>
        <div className="settings-section-body">
          <ChatModel onNavigate={onNavigate} />
          <SettingRow label="查阅本地文章" hint="搜索和阅读已订阅的文章、简报与记忆。">
            <span className="hint">始终可用</span>
          </SettingRow>
          <SettingRow label="读取网页" hint="每次打开链接仍要在对话里确认。">
            <Toggle warning label="读取网页" on={!!page.data?.fetch_url} onLabel="" offLabel=""
              disabled={writing || !page.data} onClick={() => void run({ type: "fetchUrl", enabled: !page.data?.fetch_url })} />
          </SettingRow>
        </div>
      </section>

      <section className="settings-group settings-section">
        <div className="settings-section-meta"><h4>助手说明</h4><p className="hint">每次对话都会带上。适合写回答风格、常用语言这类偏好。</p></div>
        <div className="settings-section-body">
        <SettingsRows className="assistant-note" ids={["pi_assistant_note"]} label="助手说明" onEditStateChange={report("note")} />
        {data && (
          <details className="advanced-fold">
            <summary>查看固定提示</summary>
            <pre className="system-prompt">{data.system_prompt}</pre>
          </details>
        )}
        </div>
      </section>
      {data && <>
        <section className="settings-group settings-section">
          <div className="settings-section-meta"><h4>工具 <span className="settings-count">{data.tools.filter((t) => t.enabled).length} / {data.tools.length}</span></h4><p className="hint">关掉的工具助手看不到。</p></div>
          <div className="settings-section-body">
          <ul className="settings-list">
            {data.tools.map((t) => (
              <li key={t.name} className="settings-list-row">
                <div>
                  <strong>{t.label}</strong>{t.confirm && <span className="meta-tag">需确认</span>}
                  {t.description !== t.default_description && <span className="meta-tag">已改说明</span>}
                  <p className="hint">{t.description}</p>
                </div>
                <button type="button" className="btn sm" disabled={writing} aria-label={`编辑说明 ${t.label}`}
                  onClick={() => setDialog({ kind: "tool", tool: t })}>编辑说明</button>
                <Toggle label={`工具 ${t.label}`} on={t.enabled} onLabel="" offLabel="" disabled={writing}
                  onClick={() => void run({ type: "tool", name: t.name, enabled: !t.enabled })} />
              </li>
            ))}
          </ul>
          </div>
        </section>
        <section className="settings-group settings-section">
          <div className="settings-section-meta"><h4>扩展</h4></div>
          <div className="settings-section-body">
        <Fold title="技能" open={!!open.skills} onToggle={() => toggle("skills")}
          summary={`${data.skills.length} 个 · ${data.skills.filter((s) => s.enabled).length} 个已启用`}>
          <ul className="settings-list">
            {data.skills.map((s) => (
              <li key={s.name} className="settings-list-row">
                <div>
                  <strong>{s.name}</strong><span className="meta-tag">{s.bundled ? "内置" : "自定义"}</span>
                  <p className="hint">{s.description}</p>
                </div>
                {!s.bundled && <button type="button" className="btn sm" disabled={writing} aria-label={`编辑技能 ${s.name}`}
                  onClick={() => setDialog({ kind: "skill", name: s.name })}>编辑</button>}
                <Toggle label={`技能 ${s.name}`} on={s.enabled} onLabel="" offLabel="" disabled={writing}
                  onClick={() => void run({ type: "skill", name: s.name, enabled: !s.enabled })} />
              </li>
            ))}
          </ul>
          <button type="button" className="btn sm" disabled={writing} onClick={() => setDialog({ kind: "skill" })}>新建技能</button>
        </Fold>
        <Fold title="外部工具" open={!!open.mcp} onToggle={() => toggle("mcp")} summary={data.mcp_servers.length ? `${data.mcp_servers.length} 个` : "未添加"}>
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
      <button type="button" className="link-btn to-advanced" onClick={() => onNavigate("advanced", "advanced-assistant")}>此页的高级设置 →</button>

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
  const version = probe?.version.match(/\d+\.\d+\.\d+/)?.[0] ?? "";
  const installing = piInstalling(install);
  const needsInstall = probe?.version_state === "missing" || probe?.version_state === "too_old";
  const installFailed = install?.state === "failed" && needsInstall;
  const pct = install?.total ? Math.min(100, Math.round((install.received / install.total) * 100)) : null;
  const view: { tone: "ok" | "warn" | "err"; text: string; action?: [string, () => void] } =
    installing && install ? { tone: "ok", text: installProgressText(install) }
    : installFailed ? { tone: "err", text: `下载失败：${install.message}`, action: ["重新下载", onInstall] }
    : loading || !probe
      ? failed ? { tone: "err", text: "没能检测对话引擎。", action: ["重新检测", onRetry] } : { tone: "ok", text: "正在检测对话引擎…" }
      : probe.version_state === "missing" ? { tone: "err", text: "未找到对话引擎。", action: ["下载对话引擎", onInstall] }
      : probe.version_state === "too_old" ? { tone: "err", text: `对话引擎版本过低（${version || probe.version}）。`, action: ["升级对话引擎", onInstall] }
      : probe.config_error ? { tone: "err", text: probe.config_error, action: ["去设置对话模型", onModels] }
      : probe.version_state === "unverified"
        ? { tone: "warn", text: "引擎就绪" }
        : { tone: "ok", text: "引擎就绪" };
  return (
    <>
    <div className="engine-status" data-tone={view.tone} role={view.tone === "err" ? "alert" : "status"} aria-busy={installing}>
      {/* 下载中由进度条表示状态，不再叠一个圆点。 */}
      {!installing && <span className="status-dot" data-state={view.tone === "ok" ? (loading ? "busy" : "ok") : view.tone === "warn" ? "warn" : "err"} aria-hidden="true" />}
      <span>{view.text}</span>
      {installing && <progress className="engine-progress" max={100} value={pct ?? undefined} aria-label="对话引擎下载进度">{pct ?? ""}</progress>}
      {version && <span className="engine-version">Pi {version} · {probe?.version_state === "unverified" ? "未验证版本，可能有兼容问题" : probe?.version_state === "ok" ? "已校验" : probe?.version_state === "too_old" ? "版本过低" : ""}</span>}
      <button type="button" className="btn sm ghost engine-recheck" disabled={disabled || loading || installing} onClick={onRetry}>重新检查</button>
      {view.action && <button type="button" className="btn sm" disabled={disabled} onClick={view.action[1]}>{view.action[0]}</button>}
    </div>
    {(installing || needsInstall) && (
      <p className="hint engine-install-note">
        {installing ? "在后台下载，可以离开这页，完成后会提示。" : ""}
        对话引擎会装在 RRSS 自己的目录里{install?.dir ? <>（<code>{install.dir}</code>）</> : null}，不影响你电脑上已有的 Pi。
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
    if (outcome.status === "failed" && outcome.mounted) setError(formatNotice("保存失败：", outcome.error));
  };

  return (
    <>

        <div className="settings-option">
          <div>
            <strong>默认模型</strong>
            <p className="hint">{own ? "已开启「使用我自己的 Pi 配置」，这里列出 Pi 可用的模型。"
              : current ? "新对话默认用它。对话里可以临时换成其他用于对话的模型。" : "还没选对话模型，对话暂停。"}</p>
            {entry?.effective.tool_calling === "unsupported" && (
              <p className="hint warn">这个模型不能调用工具，对话将无法查阅文章或读取网页。</p>
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
            <div><strong>思考强度</strong><p className="hint">在每个模型的设置里调；某条对话想换，可以在输入框旁临时改。</p></div>
            <span className="row">
              <span className="hint">{!entry ? "—" : entry.effective.reasoning === "unsupported" ? "不支持推理"
                : `${modelName(entry)} · ${thinkingLabel(entry.thinking)}`}</span>
              <button type="button" className="link-btn" onClick={() => onNavigate("model", "model-pool")}>在模型设置里改</button>
            </span>
          </div>
        )}
    </>
  );
}

/** 自有 Pi 配置下的对话模型：只列 Pi 的模型，写入 `pi_own_model`。 */
function OwnModel() {
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
    if (outcome.status === "failed") setError(formatNotice("保存失败：", outcome.error));
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
  return (
    <section className="settings-group engine settings-section" aria-labelledby="assistant-engine">
      <div className="settings-section-meta">
        <div className="settings-group-head">
          <h4 id="assistant-engine">对话引擎</h4>
        </div>
      </div>
      <div className="settings-section-body">
        <div className="settings-group-actions">
          <button type="button" className="btn sm" disabled={writing} onClick={() => void run({ type: "restart" })}>重启引擎</button>
        </div>
        <SettingsRows ids={["pi_idle_timeout_secs"]} label="空闲多久释放" onEditStateChange={report("idle")} />
        <dl className="diag-list">
          <div className="diag-row"><dt>版本</dt><dd>{probe?.version || "—"}</dd></div>
          <div className="diag-row"><dt>来源</dt><dd>{page?.manage_status || "—"} · {probe?.binary_source || page?.binary_source || "—"}</dd></div>
        </dl>
        <SettingsRows ids={["pi_auto_download", "pi_auto_update"]} label="对话引擎下载与更新" onEditStateChange={report("engine")} />
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
  const [text, setText] = useState(tool.description);
  const { writing, submit } = useDialogWrite(run);
  return (
    <Dialog title={`编辑说明：${tool.label}`} dirty={text !== tool.description} busy={writing} onAsk={onAsk} onClose={onClose}
      onEditStateChange={report("dialog")}
      leading={<button type="button" className="btn ghost" disabled={writing || text === tool.default_description}
        onClick={() => setText(tool.default_description)}>恢复默认</button>}
      actions={<button type="button" className="btn primary" disabled={writing || !text.trim()}
        onClick={() => void submit({ type: "toolDescription", name: tool.name, description: text }, onClose)}>保存</button>}>
      <Field label="工具说明" hint="助手按这段说明决定何时调用这个工具。">
        <textarea rows={5} value={text} disabled={writing} onChange={(e) => setText(e.target.value)} />
      </Field>
    </Dialog>
  );
}

function SkillDialog({ name, run, onAsk, onClose, report }: DialogProps & { name?: string }) {
  const loaded = useQuery({ queryKey: ["settings", "pi", "skill", name], queryFn: () => api.piSkill(name!), enabled: !!name,
    staleTime: 0, gcTime: 0, retry: false });
  const [draft, setDraft] = useState<{ name: string; description: string; body: string } | null>(name ? null : { name: "", description: "", body: "" });
  const base = name ? loaded.data : { name: "", description: "", body: "" };
  const current = draft ?? base;
  const dirty = !!draft && !!base && (draft.name !== base.name || draft.description !== base.description || draft.body !== base.body);
  const { writing, submit } = useDialogWrite(run);
  const change = (key: "name" | "description" | "body", value: string) => current && setDraft({ ...current, [key]: value });
  const remove = () => name && onAsk("删除技能", `删除「${name}」？删除后不能恢复。`,
    () => void submit({ type: "skillDelete", name }, onClose), { okLabel: "删除" });
  return (
    <Dialog title={name ? "编辑技能" : "新建技能"} dirty={dirty} busy={writing} onAsk={onAsk} onClose={onClose} onEditStateChange={report("dialog")}
      leading={name ? <button type="button" className="btn danger" disabled={writing} onClick={remove}>删除技能</button> : null}
      actions={<button type="button" className="btn primary" disabled={writing || !current?.name.trim() || !current.description.trim()}
        onClick={() => current && void submit({ type: "skillSave", draft: current }, onClose)}>保存</button>}>
      {name && loaded.isError && <p className="notice error" role="alert">{formatNotice("读取技能失败：", loaded.error)}</p>}
      {!current ? <Pending>正在读取…</Pending> : <>
        <Field label="名称" hint="小写字母、数字和连字符，例如 weekly-notes。">
          <input value={current.name} disabled={writing || !!name} onChange={(e) => change("name", e.target.value)} />
        </Field>
        <Field label="什么时候用" hint="一句话说明，助手据此判断是否使用这个技能。">
          <input value={current.description} disabled={writing} onChange={(e) => change("description", e.target.value)} />
        </Field>
        <Field label="内容">
          <textarea rows={8} value={current.body} disabled={writing} onChange={(e) => change("body", e.target.value)} />
        </Field>
      </>}
    </Dialog>
  );
}
