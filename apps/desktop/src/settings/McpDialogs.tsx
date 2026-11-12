import { useEffect, useId, useMemo, useRef, useState } from "react";
import { useIsMutating } from "@tanstack/react-query";
import type { McpImportEntry, McpInput, McpProbeView, McpView } from "../bindings";
import { api } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { Icon } from "../components/Icon";
import { ChipBar, Field } from "./chips";
import { Dialog } from "./Dialog";
import type { SettingsEditState } from "./editState";
import { McpPanel } from "./McpSection";
import { clause, connectionKey, forgetProbe, guessHostName, includeFrom, openTools, probeDraft, startLogin } from "./mcpRuntime";
import { guessName, joinCommand, parseCommand } from "./mcpCommand";
import { piKeys, type PiAction, type PiStatus } from "./piQuery";
import type { Ask } from "./SettingsPanel";

type Props = {
  run: (action: PiAction) => Promise<boolean>;
  onAsk: Ask;
  status?: PiStatus | null;
  onClose: () => void;
  onEditStateChange: (state: SettingsEditState | null) => void;
};
const TYPE: Record<string, string> = { stdio: "本地命令", http: "远程 HTTP", sse: "旧式 SSE" };
const AUTHS = [{ id: "none", label: "不需要" }, { id: "token", label: "访问令牌" }, { id: "oauth", label: "浏览器登录" }];
const SAVED = "已保存，留空不变";

type Kind = "stdio" | "http";
type Auth = "none" | "token" | "oauth";
type Row = { name: string; value: string; saved: boolean };
type Draft = {
  kind: Kind; name: string; line: string; env: Row[];
  url: string; auth: Auth; token: string; headers: Row[];
};
type Probe = { busy: boolean; result: McpProbeView | null; open: boolean };

const isAuthorization = (name: string) => name.trim().toLowerCase() === "authorization";
const bearer = (token: string) => /^bearer\s/i.test(token.trim()) ? token.trim() : `Bearer ${token.trim()}`;
const rows = (items: { name: string; value: string }[]) => items.filter((r) => r.name.trim()).map((r) => ({ name: r.name.trim(), value: r.value }));
const blank = (): Row => ({ name: "", value: "", saved: false });

function draftFrom(server?: McpView): Draft {
  const kind: Kind = server && server.transport !== "stdio" ? "http" : "stdio";
  const auth: Auth = server?.auth === "oauth" ? "oauth" : server?.headers.length ? "token" : "none";
  return {
    kind, name: server?.name ?? "",
    line: server?.transport === "stdio" ? joinCommand([server.command, ...server.args]) : "",
    env: server?.env.length ? server.env.map((e) => ({ name: e.name, value: "", saved: e.has_value })) : [blank()],
    url: server?.url ?? "", auth, token: "",
    headers: server?.headers.filter((h) => !isAuthorization(h.name)).map((h) => ({ name: h.name, value: "", saved: h.has_value })) ?? [],
  };
}

/**
 * 添加 / 编辑外部工具（阶段 19 · 内置 MCP 的界面）：两张类型卡片，左标签右字段，输入框只用占位示例。
 * 「检测连接」用还没保存的内容连一次，不保存、不开启；检测后勾好的工具随保存写入。浏览器登录的服务保存后直接开始登录。
 */
export function McpDialog({ server, run, status, onPaste, ...props }: Props & { server?: McpView; onPaste: () => void }) {
  const [base] = useState(() => draftFrom(server));
  const [d, setDraft] = useState(base);
  const [probe, setProbe] = useState<Probe>({ busy: false, result: null, open: false });
  // 检测后勾选的工具；没检测过为 null，保存时沿用原来的开放范围。
  const [chosen, setChosen] = useState<Set<string> | null>(null);
  const ticket = useRef(0);
  const panelId = useId();
  const ids = { name: useId(), line: useId(), url: useId(), token: useId(), auth: useId() };
  const [showToken, setShowToken] = useState(false);
  const writing = useIsMutating({ mutationKey: piKeys.write }) > 0;
  const legacy = server?.transport === "sse";
  const local = d.kind === "stdio";
  const oauth = !local && d.auth === "oauth";
  const authorization = server?.headers.find((h) => isAuthorization(h.name));
  const command = useMemo(() => parseCommand(d.line), [d.line]);
  const parts = command.parts;
  const guessed = local ? guessName(parts) : guessHostName(d.url);

  const build = (draft: Draft, picked: Set<string> | null): McpInput => {
    const isLocal = draft.kind === "stdio";
    const parts = parseCommand(draft.line).parts;
    const tokenRow = draft.token.trim() ? [{ name: authorization?.name ?? "Authorization", value: bearer(draft.token) }]
      : authorization ? [{ name: authorization.name, value: "" }] : [];
    const tools = probe.result?.ok ? probe.result.tools : [];
    return {
      id: server?.id ?? 0,
      name: draft.name.trim() || (isLocal ? guessName(parts) : guessHostName(draft.url)),
      transport: isLocal ? "stdio" : legacy && draft.url.trim() === server?.url ? "sse" : "http",
      command: isLocal ? parts[0] ?? "" : "",
      args: isLocal ? parts.slice(1) : [],
      url: isLocal ? "" : draft.url.trim(),
      env: isLocal ? rows(draft.env) : [],
      headers: !isLocal && draft.auth === "token" ? [...tokenRow, ...rows(draft.headers)] : [],
      auth: isLocal ? "none" : draft.auth,
      include_tools: picked && tools.length ? includeFrom(tools, picked) : server?.include_tools ?? [],
    };
  };
  const input = build(d, chosen);
  const dirty = JSON.stringify(input) !== JSON.stringify(build(base, null));
  const valid = !!input.name && (local ? !!input.command && command.complete : !!input.url);
  // 已登录的服务只改名字等不用重新登录；换了地址或第一次选浏览器登录才「保存并登录」。
  const loginAfterSave = oauth && !(server?.auth === "oauth" && server.logged_in && d.url.trim() === server.url);

  const patch = (next: Partial<Draft>) => {
    const draft = { ...d, ...next };
    setDraft(draft);
    if (connectionKey(input) !== connectionKey(build(draft, null)) || (draft.kind === "stdio" && !parseCommand(draft.line).complete)) {
      ticket.current++;
      setProbe({ busy: false, result: null, open: false });
      setChosen(null);
    }
  };
  useEffect(() => () => { ticket.current++; }, []);
  const runProbe = () => {
    const current = ++ticket.current;
    setProbe((old) => ({ ...old, busy: true }));
    void probeDraft(input).then((result) => {
      if (current !== ticket.current) return;
      setProbe((old) => ({ busy: false, result, open: old.open }));
      if (!result.ok) return;
      // 新服务：会改文件的默认不勾（全是会改文件的就全勾）；已有服务按当前保存的开放范围。
      const safe = result.tools.filter((t) => !t.writes).map((t) => t.name);
      setChosen((old) => old ?? (server ? openTools(result.tools, server.include_tools) : new Set(safe.length ? safe : result.tools.map((t) => t.name))));
    });
  };
  const save = async () => {
    if (!valid) return false;
    let id = server?.id ?? 0;
    const ok = await run({ type: "mcpSave", input, saved: (saved) => { id = saved; } });
    if (!ok) return false;
    props.onClose();
    if (server) forgetProbe(server.id);
    if (loginAfterSave && id > 0) startLogin(id);
    return true;
  };
  const paste = () => {
    if (!dirty) return onPaste();
    props.onAsk("添加外部工具有未保存的修改", "换成粘贴 JSON 后，这里填的内容不会保存。", onPaste, { okLabel: "不保存并继续", cancelLabel: "继续编辑" });
  };
  const title = server ? "编辑外部工具" : "添加外部工具";

  return <Dialog {...props} className="wide mcp-dialog" title={title} dirty={dirty} busy={writing} onSave={save}
    subtitle={!server && <>有现成的 JSON 配置？<button type="button" className="link-btn" disabled={writing} onClick={paste}>直接粘贴</button></>}
    leading={<>
      {server && <button type="button" className="btn ghost danger" disabled={writing} onClick={() => props.onAsk("删除外部工具服务", `删除「${server.name}」？`,
        () => { void run({ type: "mcpDelete", id: server.id }).then((ok) => { if (ok) props.onClose(); }); }, { okLabel: "删除" })}>删除</button>}
      {!oauth && <button type="button" className="btn" disabled={writing || probe.busy || !valid} aria-busy={probe.busy || undefined} onClick={runProbe}>检测连接</button>}
    </>}
    actions={<button type="button" className="btn primary" disabled={writing || !valid} onClick={() => void save()}>{loginAfterSave ? "保存并登录" : "保存"}</button>}>
    {status?.tone === "err" && <p className="notice error" role="alert">{status.text}</p>}
    {legacy && <div className="notice warn mcp-legacy" role="note">
      <div><strong>这个地址已经连不上</strong>多数服务另有新地址，通常是把结尾的 <code>/sse</code> 换成 <code>/mcp</code>，以服务说明为准。改好后保存，再回列表里开启。</div>
      <button type="button" className="btn sm" disabled={writing || !/\/sse\/?$/.test(d.url.trim())}
        onClick={() => patch({ url: d.url.trim().replace(/\/sse\/?$/, "/mcp") })}>换成 …/mcp</button>
    </div>}
    <div className="mcp-kinds" role="radiogroup" aria-label="类型">
      {([["stdio", "terminal", "本机程序", "在你电脑上运行的命令，例如 npx、uvx"], ["http", "globe", "在线服务", "通过网址连接"]] as const).map(([kind, icon, label, desc]) =>
        <button key={kind} type="button" role="radio" className="mcp-kind" aria-checked={d.kind === kind} disabled={writing}
          onClick={() => { if (d.kind !== kind) patch({ kind, auth: "none" }); }}>
          <Icon name={icon} size={18} /><span><strong>{label}</strong><small>{desc}</small></span>
        </button>)}
    </div>
    <div className="mcp-form">
      <label className="mcp-label" htmlFor={ids.name}>名称</label>
      <input id={ids.name} value={d.name} readOnly={writing} placeholder={guessed || (local ? "filesystem" : "linear")} onChange={(e) => patch({ name: e.target.value })} />
      {local ? <>
        <label className="mcp-label" htmlFor={ids.line}>启动命令</label>
        <div>
          <input id={ids.line} className="mono" value={d.line} readOnly={writing} spellCheck={false} autoComplete="off" aria-invalid={!command.complete || undefined}
            placeholder="npx -y @modelcontextprotocol/server-filesystem ~/notes" onChange={(e) => patch({ line: e.target.value })} />
          {!command.complete && <p className="hint warn">启动命令的引号尚未闭合。</p>}
        </div>
        <span className="mcp-label">环境变量</span>
        <KeyValues title="环境变量" example="ALLOWED_DIRS" items={d.env} busy={writing} addLabel="添加变量" onChange={(env) => patch({ env })} />
      </> : <>
        <label className="mcp-label" htmlFor={ids.url}>地址</label>
        <input id={ids.url} className="mono" value={d.url} readOnly={writing} spellCheck={false} autoComplete="off"
          placeholder="https://mcp.example.com/mcp" onChange={(e) => patch({ url: e.target.value })} />
        <span className="mcp-label" id={ids.auth}>登录</span>
        <div role="group" aria-labelledby={ids.auth}>
          <ChipBar className="segmented" value={d.auth} options={AUTHS} disabled={writing} onChange={(auth) => patch({ auth: auth as Auth })} />
        </div>
        {d.auth === "token" && <>
          <label className="mcp-label" htmlFor={ids.token}>令牌</label>
          <div className="mcp-token">
            <div className="mcp-secret">
              <input id={ids.token} className="mono" type={showToken ? "text" : "password"} autoComplete="off" spellCheck={false} value={d.token} readOnly={writing}
                placeholder={authorization?.has_value ? SAVED : "访问令牌"} onChange={(e) => patch({ token: e.target.value })} />
              <button type="button" className="icon-btn" aria-pressed={showToken} aria-label={showToken ? "遮盖令牌" : "显示令牌"} onClick={() => setShowToken((v) => !v)}><Icon name="eye" size={14} /></button>
            </div>
            {d.headers.length > 0
              ? <KeyValues title="请求头" example="X-Workspace" items={d.headers} busy={writing} addLabel="再加一个" onChange={(headers) => patch({ headers })} />
              : <button type="button" className="mcp-add" disabled={writing} onClick={() => patch({ headers: [blank()] })}><Icon name="plus" size={12} />其他请求头</button>}
          </div>
        </>}
      </>}
    </div>
    {!oauth && (probe.busy || probe.result) && <div className="mcp-probe">
      <div className="mcp-probe-bar">
        <p className="mcp-health" role="status" data-tone={probe.busy ? "busy" : probe.result?.ok ? "ok" : "bad"}>
          {probe.busy ? "正在连接…" : probe.result?.ok ? `能连上 · ${probe.result.tools.length} 个工具 · ${(probe.result.elapsed_ms / 1000).toFixed(1)} 秒`
            : `连不上 · ${clause(probe.result?.error ?? "") || "没有连上"}`}
        </p>
        {probe.result && !probe.busy && <button type="button" className="btn sm ghost" aria-expanded={probe.open} aria-controls={panelId}
          onClick={() => setProbe((old) => ({ ...old, open: !old.open }))}>
          {probe.result.ok ? (probe.open ? "收起工具" : "查看工具") : (probe.open ? "收起详情" : "查看详情")}</button>}
      </div>
      {probe.result && <McpPanel id={panelId} open={probe.open} result={probe.result} disabled={writing}
        include={probe.result.ok && chosen ? includeFrom(probe.result.tools, chosen) : server?.include_tools ?? []}
        onInclude={(include) => probe.result && setChosen(openTools(probe.result.tools, include))} />}
    </div>}
  </Dialog>;
}

/** 键值行：一行一对，值默认遮盖，眼睛图标显示。宿主按名称保留旧值，已保存的行不能改名（删掉重加）。 */
function KeyValues({ title, example, items, busy, addLabel, onChange }: {
  title: string; example: string; items: Row[]; busy: boolean; addLabel: string; onChange: (items: Row[]) => void;
}) {
  const [shown, setShown] = useState<Record<number, boolean>>({});
  const change = (index: number, key: "name" | "value", value: string) => onChange(items.map((item, i) => i === index ? { ...item, [key]: value } : item));
  return <div className="mcp-kv">
    {items.map((item, index) => <div className="mcp-kv-row" key={index}>
      <input className="mono" aria-label={`${title}名称 ${index + 1}`} placeholder={example} value={item.name} readOnly={busy || item.saved}
        spellCheck={false} autoComplete="off" onChange={(e) => change(index, "name", e.target.value)} />
      <div className="mcp-secret">
        <input className="mono" aria-label={`${title}值 ${index + 1}`} type={shown[index] ? "text" : "password"} autoComplete="off" spellCheck={false}
          value={item.value} readOnly={busy} placeholder={item.saved ? SAVED : "值"} onChange={(e) => change(index, "value", e.target.value)} />
        <button type="button" className="icon-btn" disabled={busy} aria-pressed={!!shown[index]} aria-label={`${shown[index] ? "遮盖" : "显示"}${title}值 ${index + 1}`}
          onClick={() => setShown({ ...shown, [index]: !shown[index] })}><Icon name="eye" size={14} /></button>
      </div>
      <button type="button" className="icon-btn mcp-del" disabled={busy} aria-label={`删除${title} ${index + 1}`}
        onClick={() => { setShown({}); onChange(items.filter((_, i) => i !== index)); }}><Icon name="close" size={14} /></button>
    </div>)}
    <button type="button" className="mcp-add" disabled={busy} onClick={() => { setShown({}); onChange([...items, blank()]); }}><Icon name="plus" size={12} />{addLabel}</button>
  </div>;
}

/** 粘贴即预览（停顿后交给宿主解析），预览与当前文本一致时才能导入。 */
export function McpImportDialog({ run, status, ...props }: Props) {
  const [raw, setRaw] = useState("");
  const [preview, setPreview] = useState<McpImportEntry[] | null>(null);
  const [error, setError] = useState("");
  const ticket = useRef(0);
  const writing = useIsMutating({ mutationKey: piKeys.write }) > 0;
  useEffect(() => {
    const current = ++ticket.current;
    setPreview(null); setError("");
    if (!raw.trim()) return;
    const timer = setTimeout(() => {
      api.previewMcpImport(raw).then((result) => { if (current === ticket.current) setPreview(result); },
        (e) => { if (current === ticket.current) setError(formatNotice("", e)); });
    }, 300);
    return () => clearTimeout(timer);
  }, [raw]);
  const adds = preview?.filter((e) => !e.skipped).length ?? 0;
  const apply = async () => { if (await run({ type: "mcpImport", json: raw })) props.onClose(); };
  return <Dialog {...props} className="wide" title="从 JSON 导入外部工具" dirty={!!raw} busy={writing}
    actions={<button type="button" className="btn primary" disabled={writing || !adds} onClick={() => void apply()}>{adds ? `导入 ${adds} 个` : "导入"}</button>}>
    <Field label="粘贴配置" hint="支持 Claude Desktop、Cursor 等常见格式：mcpServers 那一段，或整个配置文件。">
      <textarea className="mono" rows={10} value={raw} readOnly={writing} autoComplete="off" spellCheck={false} onChange={(e) => setRaw(e.target.value)} />
    </Field>
    {(error || status?.tone === "err") && <p className="notice error" role="alert">{error || status?.text}</p>}
    {preview && <ul className="settings-list mcp-import-list" aria-label="将导入的服务">{preview.map((entry, i) => <li className="settings-list-row" key={i}>
      <div><strong>{entry.name}</strong><span className={entry.transport === "sse" ? "meta-tag warn" : "meta-tag"}>{TYPE[entry.transport] ?? entry.transport}</span>
        <p className="hint">{[entry.secret_count ? `${entry.secret_count} 个密钥项` : "",
          entry.skipped ? "同名服务已存在，跳过" : entry.transport === "sse" ? "将添加，要改填新地址后才能开启" : "将添加，默认关闭"].filter(Boolean).join(" · ")}</p></div>
    </li>)}</ul>}
  </Dialog>;
}
