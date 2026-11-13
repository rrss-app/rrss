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
  const { report } = useMergedEditState(onEditStateChange);
  return (
    <div className="advanced-pane">
      <WindowGroup report={report("close-action")} />
      <section className="settings-group settings-section" aria-label="界面调试">
        <div className="settings-section-meta">
          <div className="settings-group-head"><h4>界面</h4></div>
        </div>
        <div className="settings-section-body">
          <SettingsRows ids={["ui_debug_mode"]} label="界面调试" onEditStateChange={report("ui-debug")} />
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
  const [tray, setTray] = useState(true);
  useEffect(() => {
    let disposed = false;
    api.windowChrome().then((chrome) => { if (!disposed && chrome) setTray(chrome.tray); }).catch(() => {});
    return () => { disposed = true; };
  }, []);
  return (
    <section className="settings-group settings-section" aria-label="窗口">
      <div className="settings-section-meta">
        <div className="settings-group-head"><h4>窗口</h4></div>
      </div>
      <div className="settings-section-body">
        <SettingsRows ids={["close_action"]} label="关闭窗口时" onEditStateChange={report}
          notes={tray ? undefined : { close_action: "这台电脑的系统托盘不可用，选「隐藏到托盘」时会改为最小化。" }} />
      </div>
    </section>
  );
}

/** 模型组：各连接的地址、协议与兼容设置在连接弹层的「高级」里（阶段 19-7），这里只剩全局的模型定义覆盖。 */
function ModelGroup({ report }: { report: (part: string) => Report }) {
  return (
    <section className="settings-group settings-section" id="advanced-model" aria-labelledby="advanced-model-title">
      <div className="settings-section-meta">
        <div className="settings-group-head"><h4 id="advanced-model-title">模型</h4></div>
        <p className="hint">各连接的地址、协议和兼容开关在模型页连接的「编辑」里；单个模型的兼容设置在「管理模型」里。</p>
      </div>
      <div className="settings-section-body">
        <OverrideField field="models_override" label="模型定义覆盖" onEditStateChange={report("models-override")}
          hint="补充模型的额外参数，例如上下文长度、图片限制。地址、密钥和模型列表请在模型页修改。"
          placeholder='{ "providers": { "rrss": { "modelOverrides": {} } } }' />
      </div>
    </section>
  );
}

function EngineGroup({ onAsk, report }: { onAsk: Ask; report: (part: string) => Report }) {
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
      : { completed: false, status: { tone: "err" as const, text: formatNotice("操作失败：", outcome.error) } };
    if (outcome.mounted) setStatus(result.status);
    return result.completed;
  };
  // 两项都会越过默认边界：开启前确认，关闭（回到默认）直接生效。
  const setOwn = () => own ? void run({ type: "config", args: { use_own_config: false } })
    : onAsk("使用我自己的 Pi 配置", "对话将改用 ~/.pi 里的模型、密钥和扩展，RRSS 的模型设置对对话不再生效。关闭后恢复原样，之前的选择会保留。",
      () => void run({ type: "config", args: { use_own_config: true } }), { okLabel: "改用我的配置" });
  const setDeveloper = () => developer ? void run({ type: "developerTools", enabled: false })
    : onAsk("开启开发者工具", "助手将可以在工作目录里运行命令、读写文件。请确认你清楚这样做的后果再开启。",
      () => void run({ type: "developerTools", enabled: true }), { okLabel: "开启" });
  const savePath = (value: string) => run({ type: "savePath", path: value }).then((ok) => { if (ok) setPath(null); return ok; });
  useEditReport(report("engine"), "对话引擎路径", pathDirty, writing, () => savePath((path ?? "").trim()));

  return (
    <section className="settings-group settings-section" id="advanced-assistant" aria-labelledby="advanced-assistant-title">
      <div className="settings-section-meta">
        <div className="settings-group-head"><h4 id="advanced-assistant-title">对话引擎与安全</h4></div>
        <p className="hint">这些开关会放宽助手能做的事，一般保持默认就好。</p>
      </div>
      <div className="settings-section-body">
        {(page.isError || config.isError) && (
          <p className="notice error" role="alert">
            {formatNotice("读取对话引擎设置失败：", page.error ?? config.error)}
            <button type="button" className="btn sm" onClick={() => { void page.refetch(); void config.refetch(); }}>重新读取</button>
          </p>
        )}
        <SettingRow label="使用我自己的 Pi 配置" hint="改用 ~/.pi 里的模型和密钥。">
          <Toggle warning label="使用我自己的 Pi 配置" on={own} onLabel="" offLabel="" disabled={writing || !config.data} onClick={setOwn} />
        </SettingRow>
        <SettingRow label="开发者工具" hint="允许助手读写文件、运行命令。默认关闭。">
          <Toggle warning label="开发者工具" on={developer} onLabel="" offLabel="" disabled={writing || !page.data} onClick={setDeveloper} />
        </SettingRow>
        <SettingRow label="Pi 可执行文件路径" hint="留空时先用应用下载的版本，没有再用系统里安装的。">
          <span className="advanced-inline">
            <input aria-label="Pi 可执行文件路径" value={path ?? savedPath} disabled={writing || !page.data} spellCheck={false}
              placeholder="留空使用应用下载的版本" onChange={(e) => setPath(e.target.value)} />
          </span>
        </SettingRow>
        {pathDirty && <SaveBar label="Pi 路径" busy={writing} onSave={() => savePath((path ?? "").trim())} onUndo={() => setPath(null)} />}
        <SettingsRows ids={["pi_cache_warming"]} label="缓存保温" disabled={writing} onEditStateChange={report("warming")} />
        <OverrideField field="settings_override" label="Pi 设置覆盖" onEditStateChange={report("settings-override")}
          hint="补充界面上还没有的对话引擎设置。和应用自己管理的项冲突时，以应用为准。"
          placeholder='{ "compaction": { "reserveTokens": 16000 } }' />
        <SettingRow label="会话目录" hint="对话记录保存的位置。">
          <code className="advanced-path">{page.data?.session_dir || "—"}</code>
        </SettingRow>
        <SettingRow label="生成的配置" hint="对话引擎启动时用到的参数和配置，密钥已隐藏。">
          <button type="button" className="btn sm" onClick={() => setViewing(true)}>查看</button>
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
  const client = useQueryClient();
  const { writing, run } = useSettingsWrite(piMutationOptions(client));
  const config = useQuery({ ...piConfigOptions(), ...readGate(writing) });
  const saved = shownOverride(config.data, field);
  const [draft, setDraft] = useState<string | null>(null);
  const [result, setResult] = useState<Result | null>(null);
  const dirty = draft !== null && !sameJson(draft || "{}", saved || "{}");
  const save = async (text: string): Promise<boolean> => {
    if (jsonObject(text) === null) { setResult({ tone: "err", text: `${label}不是有效的 JSON 对象，未保存；修改后再试。` }); return false; }
    setResult(null);
    const outcome = await run({ type: "config", args: { [field]: text } });
    if (outcome.status === "busy") return false;
    if (!outcome.mounted) return outcome.status === "done" && outcome.data.completed;
    if (outcome.status === "failed") { setResult({ tone: "err", text: formatNotice("保存失败：", outcome.error) }); return false; }
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
            {!config.data ? "尚未读取" : jsonObject(draft ?? saved) === null ? "格式错误" : "格式正确"}
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
  const query = useQuery(generatedConfigOptions());
  const data = query.data;
  return (
    <Dialog title="生成的配置" dirty={false} busy={false} onAsk={onAsk} onClose={onClose} onEditStateChange={onEditStateChange} actions={null} cancelLabel="关闭">
      {query.isPending && <p className="hint" role="status">正在生成…</p>}
      {query.isError && <p className="form-status error" role="alert">{formatNotice("没能生成配置：", query.error)}</p>}
      {data && (
        <div className="generated-config">
          {data.own_config && <p className="hint">正在使用你自己的 Pi 配置：模型、密钥和扩展由 ~/.pi 决定，这里只列出 RRSS 传入的部分。</p>}
          {data.ignored.length > 0 && (
            <p className="notice warn" role="status">这些覆盖项由 RRSS 管理，已忽略：{data.ignored.map((key) => <code key={key}>{key}</code>)}</p>
          )}
          <div><strong>启动参数</strong><pre>{[data.program, ...data.args].join(" ")}</pre></div>
          {data.env.length > 0 && <div><strong>环境变量</strong><pre>{data.env.map(([key, value]) => `${key}=${value}`).join("\n")}</pre></div>}
          {data.files.map(([path, text]) => <div key={path}><strong>{path}</strong><pre>{text}</pre></div>)}
        </div>
      )}
    </Dialog>
  );
}
