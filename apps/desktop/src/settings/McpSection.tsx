import { useEffect, useId, useRef, useState } from "react";
import type { McpInput, McpProbeView, McpToolView, McpView } from "../bindings";
import { useReveal } from "../lib/motion";
import { Toggle } from "./chips";
import {
  cancelLogin, clause, includeFrom, inputFromView, openTools, probeServer, sentence, startLogin, takeLogin,
  useLogins, useProbes, type LoginEntry, type ProbeEntry,
} from "./mcpRuntime";
import { joinCommand } from "./mcpCommand";

const TYPE: Record<string, string> = { stdio: "本地命令", http: "远程 HTTP", sse: "旧式 SSE" };
const seconds = (ms: number) => `${(Math.max(0, ms) / 1000).toFixed(1)} 秒`;

type Props = {
  servers: McpView[];
  writing: boolean;
  onAdd: () => void;
  onImport: () => void;
  onEdit: (server: McpView) => void;
  /** 开关写入；busy 未取得写入通道，false 表示已提示的实际失败。 */
  onToggle: (server: McpView, enabled: boolean) => Promise<boolean | "busy">;
  /** 保存（勾选开放的工具）；成功返回 true。 */
  onSave: (input: McpInput) => Promise<boolean>;
  onLoginFailed: (message: string) => void;
};

/**
 * 对话助手页的「外部工具（MCP）」一区（阶段 19 · 内置 MCP 的界面）：每个服务单独开关；
 * 检测只在点按或打开开关时运行，结果默认收起；浏览器登录的结果在这里取走：成功后开启并检测一次。
 */
export function McpSection({ servers, writing, onAdd, onImport, onEdit, onToggle, onSave, onLoginFailed }: Props) {
  const probes = useProbes();
  const logins = useLogins();
  const applyingLogin = useRef(false);
  const [loginSettled, setLoginSettled] = useState(0);

  useEffect(() => {
    if (writing || applyingLogin.current) return;
    for (const [id, entry] of logins) {
      const server = servers.find((s) => s.id === id);
      // 新服务的写后读取可能尚未返回；保留结果，等列表里能找到它。
      if (!server) continue;
      if (entry.state !== "done") continue;
      // 宿主的原因已是一句完整的话（如「登录超时，请重试」），前面只补上是哪一步。
      if (!entry.ok) {
        if (takeLogin(id, entry)) onLoginFailed(entry.message ? `浏览器登录没有完成：${clause(entry.message)}。` : "浏览器登录没有完成。");
        continue;
      }
      applyingLogin.current = true;
      let busy = false;
      void onToggle(server, true).then((outcome) => {
        busy = outcome === "busy";
        if (busy) return; // 保留结果，等写入通道变化；不自触发重试循环。
        if (takeLogin(id, entry) && outcome === true) probeServer(inputFromView(server));
      })
        .finally(() => {
          applyingLogin.current = false;
          if (!busy) setLoginSettled((version) => version + 1);
        });
      break;
    }
  }, [logins, servers, writing, loginSettled, onToggle, onLoginFailed]);

  return <section className="settings-group settings-section" aria-labelledby="assistant-mcp">
    <div className="settings-section-meta">
      <div className="settings-group-head"><h4 id="assistant-mcp">外部工具（MCP）</h4></div>
      <p className="hint">让助手用上你添加的工具，比如读写本机笔记、查询在线服务。每个服务单独开关，下次对话起生效。</p>
    </div>
    <div className="settings-section-body">
      <div className="settings-group-actions">
        <button type="button" className="btn sm ghost" disabled={writing} onClick={onImport}>从 JSON 导入</button>
        <button type="button" className="btn sm" disabled={writing} onClick={onAdd}>添加服务</button>
      </div>
      {servers.length === 0 ? <p className="settings-empty">还没有服务。可以添加服务，或直接粘贴服务说明里的 JSON 配置。</p>
        : <ul className="mcp-list" aria-label="外部工具服务">{servers.map((m) => <McpRow key={m.id} server={m} writing={writing}
          probe={probes.get(m.id)} login={logins.get(m.id)} onEdit={onEdit} onToggle={onToggle} onSave={onSave} />)}</ul>}
    </div>
  </section>;
}

function McpRow({ server: m, writing, probe, login, onEdit, onToggle, onSave }: {
  server: McpView;
  writing: boolean;
  probe?: ProbeEntry;
  login?: LoginEntry;
  onEdit: (server: McpView) => void;
  onToggle: Props["onToggle"];
  onSave: Props["onSave"];
}) {
  const [open, setOpen] = useState(false);
  // 勾选后先按新的开放范围显示，写入失败再退回；宿主读回新值后以读回的为准。
  const [pending, setPending] = useState<string[] | null>(null);
  const saved = m.include_tools.join("\n");
  useEffect(() => setPending(null), [saved]);
  const include = pending ?? m.include_tools;
  const panelId = useId();
  const legacy = m.transport === "sse";
  const oauth = m.auth === "oauth";
  const needsLogin = oauth && !m.logged_in;
  const waiting = login?.state === "waiting";
  const busy = !!probe?.busy;
  const result = !legacy && !needsLogin ? probe?.result ?? null : null;
  const missing = [...m.env, ...m.headers].filter((v) => !v.has_value).map((v) => v.name);
  const where = m.transport === "stdio" ? joinCommand([m.command, ...m.args]) : m.url;
  const tag = TYPE[m.transport] ?? m.transport;
  const probeNow = () => probeServer(inputFromView(m));
  const toggle = () => {
    if (m.enabled) void onToggle(m, false);
    // 打开开关时自动检测一次：下次对话反正要连它。
    else void onToggle(m, true).then((ok) => { if (ok === true) probeNow(); });
  };

  const health: { tone?: "ok" | "bad" | "busy" | "warn"; text: string } =
    legacy ? { tone: "warn", text: "这种旧式地址已连不上，换成新地址后再开启" }
    : needsLogin ? waiting ? { tone: "busy", text: "等你在浏览器里完成登录…" } : { text: "需要登录 · 在浏览器里授权后才能使用" }
    : busy ? { tone: "busy", text: "正在连接…" }
    : result?.ok ? { tone: "ok", text: `已连接 · 开放 ${openTools(result.tools, include).size} / ${result.tools.length} 个工具 · ${seconds(result.elapsed_ms)}` }
    : result ? { tone: "bad", text: `连接失败 · ${clause(result.error) || "没有连上"}` }
    : { text: oauth ? "已登录 · 未检测" : "未检测" };

  const actions = legacy
    ? <button type="button" className="btn sm primary" disabled={writing} aria-label={`改填地址 ${m.name}`} onClick={() => onEdit(m)}>改填地址</button>
    : <>
      {result && !busy && <button type="button" className="btn sm ghost" aria-expanded={open} aria-controls={panelId} onClick={() => setOpen((v) => !v)}>
        {result.ok ? (open ? "收起工具" : "查看工具") : (open ? "收起详情" : "查看详情")}</button>}
      {needsLogin
        ? waiting
          ? <button type="button" className="btn sm" aria-label={`取消登录 ${m.name}`} onClick={() => cancelLogin(m.id)}>取消</button>
          : <button type="button" className="btn sm primary" disabled={writing} aria-label={`登录 ${m.name}`} onClick={() => startLogin(m.id)}>登录</button>
        : <button type="button" className="btn sm" disabled={busy} aria-label={`检测 ${m.name}`} onClick={probeNow}>检测</button>}
      <button type="button" className="btn sm" disabled={writing} aria-label={`编辑服务 ${m.name}`} onClick={() => onEdit(m)}>编辑</button>
    </>;

  return <li className="mcp-row">
    <div className="mcp-row-head">
      <div className="mcp-row-main">
        <div className="mcp-row-title"><strong>{m.name}</strong>
          <span className={legacy ? "meta-tag warn" : "meta-tag"}>{oauth && !legacy ? `${tag} · 浏览器登录` : tag}</span></div>
        <p className="mcp-where" title={where}>{where}</p>
        <p className="mcp-health" data-tone={health.tone} role="status">{health.text}</p>
        {missing.length > 0 && <p className="hint warn">需要重新填写：{missing.join("、")}</p>}
      </div>
      <div className="mcp-row-actions">
        {actions}
        {/* 旧式 SSE 与没登录的服务只能关、不能开。 */}
        <Toggle warning label={`服务 ${m.name}`} on={m.enabled} onLabel="已开启" offLabel="已关闭"
          disabled={writing || (!m.enabled && (legacy || needsLogin))} onClick={toggle} />
      </div>
    </div>
    {result && <McpPanel id={panelId} open={open} result={result} include={include} disabled={writing}
      onInclude={(include_tools) => {
        setPending(include_tools);
        void onSave(inputFromView(m, { include_tools })).then((ok) => { if (!ok) setPending(null); });
      }} />}
  </li>;
}

/** 检测结果的展开面板：成功是工具列表，失败是原因与输出尾部。重测时内容原地更新，不重播展开。 */
export function McpPanel({ id, open, result, include, disabled, onInclude }: {
  id: string;
  open: boolean;
  result: McpProbeView;
  include: string[];
  disabled: boolean;
  onInclude: (include: string[]) => void;
}) {
  const reveal = useReveal(open, { openMs: 240, closeMs: 180 });
  return <div className="mcp-reveal" data-state={reveal} id={id}>
    <div className="mcp-reveal-clip">
      {result.ok ? <McpToolList tools={result.tools} include={include} disabled={disabled} onChange={onInclude} />
        : <div className="mcp-error">
          <p><strong>{sentence(result.error) || "没有连上。"}</strong>{result.hint ? ` ${sentence(result.hint)}` : ""}</p>
          {result.output && <pre>{result.output}</pre>}
        </div>}
    </div>
  </div>;
}

/** 勾选开放给助手的工具。至少留一个：一个都不勾等于存成空，空表示全部开放。 */
export function McpToolList({ tools, include, disabled, onChange }: {
  tools: McpToolView[];
  include: string[];
  disabled: boolean;
  onChange: (include: string[]) => void;
}) {
  const chosen = openTools(tools, include);
  if (tools.length === 0) return <div className="mcp-tools"><p className="hint">这个服务没有提供工具。</p></div>;
  const set = (name: string, on: boolean) => {
    const next = new Set(chosen);
    if (on) next.add(name); else next.delete(name);
    onChange(includeFrom(tools, next));
  };
  return <div className="mcp-tools">
    <div className="mcp-tools-head">
      <span>勾选的工具会开放给助手；会改文件的默认不勾，勾上后每次调用前先问你。</span>
      <button type="button" className="btn sm ghost" disabled={disabled || chosen.size === tools.length} onClick={() => onChange([])}>全选</button>
    </div>
    <ul aria-label="开放给助手的工具">{tools.map((tool) => {
      const on = chosen.has(tool.name);
      return <li key={tool.name}>
        <label className="mcp-tool">
          <input type="checkbox" checked={on} disabled={disabled || (on && chosen.size === 1)} onChange={(e) => set(tool.name, e.target.checked)} />
          <span className="mcp-tool-text">
            <code>{tool.name}</code>
            {tool.description && <span className="mcp-tool-desc">{tool.description}</span>}
            {tool.writes && <span className="mcp-tool-ask" data-on={on ? "" : undefined}><span>调用前会先问你</span></span>}
          </span>
          {tool.writes && <span className="meta-tag warn">会改文件</span>}
        </label>
      </li>;
    })}</ul>
  </div>;
}
