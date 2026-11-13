import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type PiConfigView } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { Field, SaveBar, SettingRow, Toggle } from "./chips";
import { jsonObject, prettyJson, sameJson } from "./compat";
import { Feedback } from "./ConnectionDialog";
import { MenuSelect } from "../components/MenuSelect";
import { Icon } from "../components/Icon";
import { Pending } from "../components/Pending";
import { Dialog } from "./Dialog";
import { readGate, useEditReport, useMergedEditState, useSettingsWrite, type SettingsEditState } from "./editState";
import { generatedConfigOptions, piConfigOptions, piMutationOptions, piQueryOptions, type PiAction, type PiStatus } from "./piQuery";
import { useTranslation } from "../i18n";
import { SettingsRows } from "./SettingsRows";
import type { Ask } from "./SettingsPanel";
import { NetworkProxyGroup } from "./NetworkProxyGroup";

type Report = (state: SettingsEditState | null) => void;
type Result = { tone: "ok" | "err"; text: string; detail?: string };

/**
 * 「高级」分类（L2）：会改变默认隔离或兼容行为的项，按领域分组，基本页用锚点直达。
 * 每块自己读写、自己上报离开状态；写入通道与模型页、对话助手页共用。
 */
export function AdvancedPane({ onAsk, onEditStateChange }: {
  onAsk: Ask;
  onEditStateChange?: Report;
}) {
  const { t } = useTranslation("settingsSystem");
  const { report } = useMergedEditState(onEditStateChange);
  return (
    <div className="advanced-pane">
      <WindowGroup report={report("close-action")} />
      <section className="settings-group settings-section" aria-label={t("advanced.ui.aria")}>
        <div className="settings-section-meta">
          <div className="settings-group-head"><h4>{t("advanced.ui.head")}</h4></div>
        </div>
        <div className="settings-section-body">
          <SettingsRows ids={["ui_debug_mode"]} label={t("advanced.ui.label")} onEditStateChange={report("ui-debug")} />
        </div>
      </section>
      <NetworkProxyGroup report={report("network-proxy")} />
      <EngineGroup onAsk={onAsk} report={report} />
      <ModelGroup report={report} />
    </div>
  );
}

/** 窗口组：关闭窗口时怎么做（阶段 14 · 后台驻留）。托盘不可用时提示「隐藏到托盘」会改为最小化。 */
function WindowGroup({ report }: { report: Report }) {
  const { t } = useTranslation("settingsSystem");
  const [tray, setTray] = useState(true);
  useEffect(() => {
    let disposed = false;
    api.windowChrome().then((chrome) => { if (!disposed && chrome) setTray(chrome.tray); }).catch(() => {});
    return () => { disposed = true; };
  }, []);
  return (
    <section className="settings-group settings-section" aria-label={t("advanced.window.head")}>
      <div className="settings-section-meta">
        <div className="settings-group-head"><h4>{t("advanced.window.head")}</h4></div>
      </div>
      <div className="settings-section-body">
        <SettingsRows ids={["close_action"]} label={t("advanced.window.label")} onEditStateChange={report}
          notes={tray ? undefined : { close_action: t("advanced.window.trayUnavailable") }} />
      </div>
    </section>
  );
}

/** 模型组：各连接的地址、协议与兼容设置在连接弹层的「高级」里（阶段 19-7），这里只剩全局的模型定义覆盖。 */
function ModelGroup({ report }: { report: (part: string) => Report }) {
  const { t } = useTranslation("settingsSystem");
  return (
    <section className="settings-group settings-section" id="advanced-model" aria-labelledby="advanced-model-title">
      <div className="settings-section-meta">
        <div className="settings-group-head"><h4 id="advanced-model-title">{t("advanced.model.head")}</h4></div>
        <p className="hint">{t("advanced.model.hint")}</p>
      </div>
      <div className="settings-section-body">
        <OverrideField field="models_override" label={t("advanced.model.overrideLabel")} onEditStateChange={report("models-override")}
          hint={t("advanced.model.overrideHint")}
          placeholder='{ "providers": { "rrss": { "modelOverrides": {} } } }' />
      </div>
    </section>
  );
}

function EngineGroup({ onAsk, report }: { onAsk: Ask; report: (part: string) => Report }) {
  const { t } = useTranslation("settingsSystem");
  const client = useQueryClient();
  const { writing, run: write } = useSettingsWrite(piMutationOptions(client));
  const page = useQuery({ ...piQueryOptions(), ...readGate(writing) });
  const config = useQuery({ ...piConfigOptions(), ...readGate(writing) });
  const [status, setStatus] = useState<PiStatus | null>(null);
  const [path, setPath] = useState<string | null>(null);
  const [viewing, setViewing] = useState(false);
  const savedPath = page.data?.pi_path ?? "";
  const pathDirty = path !== null && path !== savedPath;
  const own = !!config.data?.use_own_config;
  const developer = !!page.data?.developer_tools;

  const run = async (action: PiAction) => {
    setStatus(null);
    const outcome = await write(action);
    if (outcome.status === "busy") return false;
    const result = outcome.status === "done" ? outcome.data
      : { completed: false, status: { tone: "err" as const, text: formatNotice(t("advanced.engine.opFailed"), outcome.error) } };
    if (outcome.mounted) setStatus(result.status);
    return result.completed;
  };
  // 两项都会越过默认边界：开启前确认，关闭（回到默认）直接生效。
  const setOwn = () => own ? void run({ type: "config", args: { use_own_config: false } })
    : onAsk(t("advanced.engine.own.title"), t("advanced.engine.own.confirm"),
      () => void run({ type: "config", args: { use_own_config: true } }), { okLabel: t("advanced.engine.own.okLabel") });
  const setDeveloper = () => developer ? void run({ type: "developerTools", enabled: false })
    : onAsk(t("advanced.engine.dev.confirmTitle"), t("advanced.engine.dev.confirm"),
      () => void run({ type: "developerTools", enabled: true }), { okLabel: t("advanced.engine.dev.okLabel") });
  const savePath = (value: string) => run({ type: "savePath", path: value }).then((ok) => { if (ok) setPath(null); return ok; });
  useEditReport(report("engine"), t("advanced.engine.pathReport"), pathDirty, writing, () => savePath((path ?? "").trim()));

  return (
    <section className="settings-group settings-section" id="advanced-assistant" aria-labelledby="advanced-assistant-title">
      <div className="settings-section-meta">
        <div className="settings-group-head"><h4 id="advanced-assistant-title">{t("advanced.engine.head")}</h4></div>
        <p className="hint">{t("advanced.engine.hint")}</p>
      </div>
      <div className="settings-section-body">
        {(page.isError || config.isError) && (
          <p className="notice error" role="alert">
            {formatNotice(t("advanced.engine.readFailed"), page.error ?? config.error)}
            <button type="button" className="btn sm" onClick={() => { void page.refetch(); void config.refetch(); }}>{t("advanced.engine.reload")}</button>
          </p>
        )}
        <SettingRow label={t("advanced.engine.own.title")} hint={t("advanced.engine.own.hint")}>
          <Toggle warning label={t("advanced.engine.own.title")} on={own} onLabel="" offLabel="" disabled={writing || !config.data} onClick={setOwn} />
        </SettingRow>
        <SettingRow label={t("advanced.engine.dev.label")} hint={t("advanced.engine.dev.hint")}>
          <Toggle warning label={t("advanced.engine.dev.label")} on={developer} onLabel="" offLabel="" disabled={writing || !page.data} onClick={setDeveloper} />
        </SettingRow>
        <SettingRow label={t("advanced.engine.piPath.label")} hint={t("advanced.engine.piPath.hint")}>
          <span className="advanced-inline">
            <input aria-label={t("advanced.engine.piPath.label")} value={path ?? savedPath} disabled={writing || !page.data} spellCheck={false}
              placeholder={t("advanced.engine.piPath.placeholder")} onChange={(e) => setPath(e.target.value)} />
          </span>
        </SettingRow>
        {pathDirty && <SaveBar label={t("advanced.engine.piPath.saveLabel")} busy={writing} onSave={() => savePath((path ?? "").trim())} onUndo={() => setPath(null)} />}
        <SettingsRows ids={["pi_cache_warming"]} label={t("advanced.engine.warming")} disabled={writing} onEditStateChange={report("warming")} />
        <OverrideField field="settings_override" label={t("advanced.engine.settingsOverrideLabel")} onEditStateChange={report("settings-override")}
          hint={t("advanced.engine.settingsOverrideHint")}
          placeholder='{ "compaction": { "reserveTokens": 16000 } }' />
        <SettingRow label={t("advanced.engine.sessionDir.label")} hint={t("advanced.engine.sessionDir.hint")}>
          <code className="advanced-path">{page.data?.session_dir || "—"}</code>
        </SettingRow>
        <SettingRow label={t("advanced.engine.generated.label")} hint={t("advanced.engine.generated.hint")}>
          <button type="button" className="btn sm" onClick={() => setViewing(true)}>{t("advanced.engine.generated.view")}</button>
        </SettingRow>
        {status && <p className={status.tone === "err" ? "form-status error" : "form-status"} role={status.tone === "err" ? "alert" : "status"}>{status.text}</p>}
        {viewing && <GeneratedConfigDialog onAsk={onAsk} onClose={() => setViewing(false)} onEditStateChange={report("generated")} />}
      </div>
    </section>
  );
}

/** Pi 设置 / 模型定义覆盖：JSON 草稿，保存前预检；非法时不提交、保留草稿，后端仍会再校验。 */
function OverrideField({ field, label, hint, placeholder, onEditStateChange }: {
  field: "settings_override" | "models_override";
  label: string;
  hint: string;
  placeholder: string;
  onEditStateChange: Report;
}) {
  const { t } = useTranslation("settingsSystem");
  const client = useQueryClient();
  const { writing, run } = useSettingsWrite(piMutationOptions(client));
  const config = useQuery({ ...piConfigOptions(), ...readGate(writing) });
  const saved = shownOverride(config.data, field);
  const [draft, setDraft] = useState<string | null>(null);
  const [result, setResult] = useState<Result | null>(null);
  const dirty = draft !== null && !sameJson(draft || "{}", saved || "{}");
  const save = async (text: string): Promise<boolean> => {
    if (jsonObject(text) === null) { setResult({ tone: "err", text: t("advanced.override.invalid", { label }) }); return false; }
    setResult(null);
    const outcome = await run({ type: "config", args: { [field]: text } });
    if (outcome.status === "busy") return false;
    if (!outcome.mounted) return outcome.status === "done" && outcome.data.completed;
    if (outcome.status === "failed") { setResult({ tone: "err", text: formatNotice(t("advanced.override.saveFailed"), outcome.error) }); return false; }
    setResult(outcome.data.status);
    if (outcome.data.completed) setDraft(null);
    return outcome.data.completed;
  };
  useEditReport(onEditStateChange, label, dirty, writing, () => save(draft ?? ""));

  return (
    <div className="settings-option stacked">
      <div>
        <strong>{label}</strong>
        <p className="hint">{hint}</p>
      </div>
      <div className="settings-code">
        <div className="settings-code-head">
          <code>{field === "settings_override" ? "settings.json" : "models.json"}</code>
          <span className={`code-validation${jsonObject(draft ?? saved) === null ? " error" : ""}`}>
            {!config.data ? t("advanced.override.notRead") : jsonObject(draft ?? saved) === null ? t("advanced.override.badFormat") : t("advanced.override.goodFormat")}
          </span>
        </div>
        <textarea className="code" rows={4} aria-label={label} aria-invalid={jsonObject(draft ?? saved) === null} value={draft ?? saved} placeholder={placeholder} spellCheck={false}
          disabled={writing || !config.data} onChange={(e) => { setDraft(e.target.value); setResult(null); }} />
      </div>
      {dirty && <SaveBar label={label} busy={writing} onSave={() => void save(draft ?? "")} onUndo={() => { setDraft(null); setResult(null); }} />}
      {result && <Feedback result={result} />}
    </div>
  );
}

function shownOverride(config: PiConfigView | undefined, field: "settings_override" | "models_override") {
  const raw = config?.[field] ?? "";
  // 库里空覆盖读出来是 `{}`，编辑框里显示为空，让占位示例可见。
  return sameJson(raw || "{}", "{}") ? "" : prettyJson(raw);
}

function GeneratedConfigDialog({ onAsk, onClose, onEditStateChange }: { onAsk: Ask; onClose: () => void; onEditStateChange: Report }) {
  const { t } = useTranslation("settingsSystem");
  const query = useQuery(generatedConfigOptions());
  const data = query.data;
  return (
    <Dialog title={t("advanced.generated.title")} dirty={false} busy={false} onAsk={onAsk} onClose={onClose} onEditStateChange={onEditStateChange} actions={null} cancelLabel={t("advanced.generated.close")}>
      {query.isPending && <p className="hint" role="status">{t("advanced.generated.generating")}</p>}
      {query.isError && <p className="form-status error" role="alert">{formatNotice(t("advanced.generated.failed"), query.error)}</p>}
      {data && (
        <div className="generated-config">
          {data.own_config && <p className="hint">{t("advanced.generated.own")}</p>}
          {data.ignored.length > 0 && (
            <p className="notice warn" role="status">{t("advanced.generated.ignored")}{data.ignored.map((key) => <code key={key}>{key}</code>)}</p>
          )}
          <div><strong>{t("advanced.generated.args")}</strong><pre>{[data.program, ...data.args].join(" ")}</pre></div>
          {data.env.length > 0 && <div><strong>{t("advanced.generated.env")}</strong><pre>{data.env.map(([key, value]) => `${key}=${value}`).join("\n")}</pre></div>}
          {data.files.map(([path, text]) => <div key={path}><strong>{path}</strong><pre>{text}</pre></div>)}
        </div>
      )}
    </Dialog>
  );
}
