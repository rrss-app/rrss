import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type NetworkProxyMode, type NetworkProxyTestView, type SystemProxyView } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { MenuSelect } from "../components/MenuSelect";
import { Pending } from "../components/Pending";
import { Field, Fold, SaveBar, SettingRow } from "./chips";
import { Feedback } from "./ConnectionDialog";
import { readGate, useEditReport, useSettingsWrite, type SettingsEditState } from "./editState";
import {
  routeSummary, PROXY_USES, argsFromDraft, draftFromView, effectiveRule, isSocks, networkProxyMutationOptions, networkProxyQueryOptions, sameDraft, testArgsFromDraft,
  validateProxyDraft, withPastedAuth,
  type NetworkProxyDraft, type NetworkProxyDraftRule, type ProxyScope, type ProxyUse,
} from "./networkProxyQuery";

type Report = (state: SettingsEditState | null) => void;
type Result = { tone: "ok" | "err"; text: string };
type Check = { state: "pending" } | { state: "done"; view: NetworkProxyTestView } | { state: "failed"; text: string };

const SCOPE_TEXT: Record<ProxyScope, { name: string; hint: string }> = {
  global: { name: "全局", hint: "下面三个用途没有单独设置时都走这里。" },
  content: { name: "内容网络", hint: "订阅、全文、图片、网站图标、媒体与网页读取。" },
  api: { name: "模型 API", hint: "摘要、简报、嵌入、翻译、模型列表和连接测试。" },
  pi: { name: "Pi", hint: "对话引擎的下载、更新和对话。" },
};
const MODE_LABEL: Record<NetworkProxyMode, string> = { inherit: "跟随全局", direct: "无代理", system: "系统代理", custom: "自定义代理" };
const globalModes = (["direct", "system", "custom"] as const).map((value) => ({ value, label: MODE_LABEL[value] }));
const useModes = (["inherit", "direct", "system", "custom"] as const).map((value) => ({ value, label: MODE_LABEL[value] }));
const AUTH_TEXT = {
  clear: "保存后清除",
  replace: "保存后替换",
  enable: "保存后启用",
  saved: "已设置，不回显",
  unset: "未设置",
  endpointChanged:
    "代理协议、主机或端口已改变。为避免将已保存的认证发送到另一代理，保存后将清除原认证；如有需要，请填写新认证。",
};

function sameProxyEndpoint(draft: string, saved: string): boolean {
  try {
    const next = new URL(draft.trim());
    const old = new URL(saved.trim());
    return (
      next.protocol === old.protocol &&
      next.hostname.toLowerCase() === old.hostname.toLowerCase() &&
      next.port === old.port
    );
  } catch {
    return false;
  }
}

/**
 * 网络代理（规格 23 · P3）：全局一行 + 折叠的三个用途覆盖，模式的细节在所属行下的子面板里；
 * 页底「连接检查」列出每个用途实际走的线路，用未保存的草稿统一测活。一份草稿统一保存，认证只写不读。
 */
export function NetworkProxyGroup({ report }: { report: Report }) {
  const client = useQueryClient();
  const { writing, run } = useSettingsWrite(networkProxyMutationOptions(client));
  const query = useQuery({ ...networkProxyQueryOptions(), ...readGate(writing) });
  const [draft, setDraft] = useState<NetworkProxyDraft | null>(null);
  const [open, setOpen] = useState(false);
  const [feedback, setFeedback] = useState<Result | null>(null);
  const [checks, setChecks] = useState<Partial<Record<ProxyUse, Check>>>({});
  const saved = query.data ? draftFromView(query.data) : null;
  const current = draft ?? saved;
  const dirty = !!(current && saved && !sameDraft(current, saved));
  const testing = Object.values(checks).some((check) => check?.state === "pending");
  const busy = writing || testing;
  const update = (scope: ProxyScope, rule: NetworkProxyDraftRule) => {
    if (!current) return;
    setDraft({ ...current, [scope]: rule });
    setFeedback(null);
    // 草稿变了，上一次的检查结果不再对应当前线路。
    setChecks({});
  };
  const commit = async (): Promise<boolean> => {
    if (!current) return false;
    const invalid = validateProxyDraft(current);
    if (invalid) { setFeedback({ tone: "err", text: invalid }); return false; }
    const outcome = await run(argsFromDraft(current));
    if (outcome.status === "done") {
      if (outcome.mounted) {
        setDraft(null);
        setFeedback({ tone: "ok", text: "已保存。新的请求马上生效；正在进行的对话要等对话引擎下次启动后才用新设置。" });
      }
      return true;
    }
    if (outcome.status === "failed" && outcome.mounted) setFeedback({ tone: "err", text: formatNotice("保存网络代理失败：", outcome.error) });
    return false;
  };
  const test = async () => {
    if (!current || busy) return;
    const invalid = validateProxyDraft(current);
    if (invalid) { setFeedback({ tone: "err", text: invalid }); return; }
    setFeedback(null);
    setChecks(Object.fromEntries(PROXY_USES.map((use) => [use, { state: "pending" }])));
    await Promise.all(PROXY_USES.map(async (use) => {
      let check: Check;
      try {
        check = { state: "done", view: await api.testNetworkProxy(testArgsFromDraft(use, current)) };
      } catch (error) {
        check = { state: "failed", text: formatNotice("", error) };
      }
      setChecks((prev) => (prev[use]?.state === "pending" ? { ...prev, [use]: check } : prev));
    }));
  };
  useEditReport(report, "网络代理", dirty, busy, commit);
  const overrides = current ? PROXY_USES.filter((use) => current[use].mode !== "inherit").length : 0;
  const system = query.data?.system ?? null;
  return (
    <section className="settings-group settings-section" id="advanced-network-proxy" aria-labelledby="advanced-network-proxy-title">
      <div className="settings-section-meta">
        <div className="settings-group-head"><h4 id="advanced-network-proxy-title">网络代理</h4></div>
        <p className="hint">文章、模型服务和对话引擎可以分别设置连接方式。一般不用改。</p>
      </div>
      <div className="settings-section-body proxy-body">
        {query.isPending && <Pending>正在读取网络代理设置…</Pending>}
        {query.isError && (
          <p className="notice error" role="alert">
            {formatNotice("读取网络代理设置失败：", query.error)}
            <button type="button" className="btn sm" onClick={() => void query.refetch()}>重新读取</button>
          </p>
        )}
        {current && <>
          <ProxyRuleEditor scope="global" rule={current.global} savedUrl={saved?.global.url ?? ""} system={system} disabled={busy} onChange={(rule) => update("global", rule)} />
          <Fold className="proxy-uses" title="按用途单独设置" summary={overrides ? `${overrides} 项单独设置` : "全部跟随全局"}
            open={open} onToggle={() => setOpen((value) => !value)}>
            {PROXY_USES.map((use) => (
              <ProxyRuleEditor key={use} scope={use} rule={current[use]} savedUrl={saved?.[use].url ?? ""} system={system} disabled={busy} onChange={(rule) => update(use, rule)} />
            ))}
          </Fold>
          <ProxyCheck draft={current} checks={checks} testing={testing} disabled={writing} onTest={() => void test()} />
          {feedback && <Feedback result={feedback} />}
          {dirty && <SaveBar label="网络代理" busy={writing} onSave={() => void commit()} onUndo={() => { setDraft(null); setFeedback(null); setChecks({}); }} />}
        </>}
      </div>
    </section>
  );
}

function schemeHint(url: string): string {
  if (/^socks5h:\/\//i.test(url)) return "SOCKS5H：由代理解析目标域名。";
  if (/^socks5:\/\//i.test(url)) return "SOCKS5：由本机解析目标域名；需要代理解析时改用 socks5h://。";
  return "支持 http、https、socks5、socks5h；推荐 socks5h:// 让代理解析域名。";
}

function ProxyRuleEditor({ scope, rule, savedUrl, system, disabled, onChange }: {
  scope: ProxyScope; rule: NetworkProxyDraftRule; savedUrl: string; system: SystemProxyView | null; disabled: boolean;
  onChange: (rule: NetworkProxyDraftRule) => void;
}) {
  const { name, hint } = SCOPE_TEXT[scope];
  return (
    <div className="proxy-rule" data-proxy-scope={scope} data-mode={rule.mode}>
      <SettingRow label={scope === "global" ? "全局代理" : name} hint={hint}>
        <MenuSelect label={`${name}代理模式`} value={rule.mode} options={scope === "global" ? globalModes : useModes} disabled={disabled}
          onChange={(mode) => onChange({ ...rule, mode })} />
      </SettingRow>
      {rule.mode === "custom" && <CustomProxyFields name={name} rule={rule} savedUrl={savedUrl} disabled={disabled} onChange={onChange} />}
      {rule.mode === "system" && system && <SystemProxyDetails system={system} />}
    </div>
  );
}

function CustomProxyFields({ name, rule, savedUrl, disabled, onChange }: {
  name: string; rule: NetworkProxyDraftRule; savedUrl: string; disabled: boolean; onChange: (rule: NetworkProxyDraftRule) => void;
}) {
  const typed = !!(rule.username || rule.password);
  const [editingAuth, setEditingAuth] = useState(false);
  // 粘贴含认证的地址会直接填好两栏，这时也要展开给用户看到。
  const showAuth = editingAuth || typed;
  // 保存后认证状态翻转（新设 / 清除），输入栏收起。
  useEffect(() => { setEditingAuth(false); }, [rule.has_auth]);
  const endpointChanged = rule.has_auth && !typed && !sameProxyEndpoint(rule.url, savedUrl);
  const authState = rule.clear_auth || endpointChanged ? "clear" : typed ? (rule.has_auth ? "replace" : "enable")
    : rule.has_auth ? "saved" : "unset";
  const closeAuth = () => { setEditingAuth(false); onChange({ ...rule, username: "", password: "" }); };
  return (
    <div className="proxy-detail">
      <Field label="代理地址" hint={schemeHint(rule.url)}>
        <input data-proxy-field="url" aria-label={`${name}代理地址`} className="mono" value={rule.url} disabled={disabled} spellCheck={false} autoComplete="off"
          placeholder="http://127.0.0.1:7890" onChange={(e) => onChange(withPastedAuth(rule, e.target.value))} />
      </Field>
      <Field label="不代理的地址" hint="域名、IP 或网段，逗号或空格分隔；* 表示全部直连。本机地址始终直连，局域网需要在这里写网段。">
        <input aria-label={`${name}不代理地址`} className="mono" value={rule.no_proxy} disabled={disabled} spellCheck={false} autoComplete="off"
          placeholder="例如 example.com, 192.168.0.0/16" onChange={(e) => onChange({ ...rule, no_proxy: e.target.value })} />
      </Field>
      <div className="proxy-auth" role="group" aria-label={`${name}代理认证`}>
        <div className="proxy-auth-head">
          <span className="proxy-auth-title">代理认证</span>
          <span className="proxy-auth-state" data-auth-state={authState}>{AUTH_TEXT[authState]}</span>
          <span className="proxy-auth-actions">
            {rule.clear_auth ? (
              <button type="button" className="btn sm ghost" disabled={disabled} onClick={() => onChange({ ...rule, clear_auth: false })}>保留原认证</button>
            ) : showAuth ? (
              <button type="button" className="btn sm ghost" disabled={disabled} onClick={closeAuth}>{rule.has_auth ? "不替换" : "不使用认证"}</button>
            ) : <>
              <button type="button" className="btn sm ghost" disabled={disabled} onClick={() => setEditingAuth(true)}>{rule.has_auth ? "替换" : "添加认证"}</button>
              {rule.has_auth && (
                <button type="button" className="btn sm ghost" disabled={disabled}
                  onClick={() => onChange({ ...rule, username: "", password: "", clear_auth: true })}>清除认证</button>
              )}
            </>}
          </span>
        </div>
        {endpointChanged && <p className="hint" data-auth-clear-reason="endpoint-changed">{AUTH_TEXT.endpointChanged}</p>}
        {showAuth && !rule.clear_auth && <>
          <div className="proxy-auth-fields">
            <input aria-label={`${name}代理用户名`} value={rule.username} disabled={disabled} autoComplete="off" spellCheck={false}
              placeholder="用户名" onChange={(e) => onChange({ ...rule, username: e.target.value, clear_auth: false })} />
            <input aria-label={`${name}代理密码`} type="password" value={rule.password} disabled={disabled} autoComplete="off"
              placeholder="密码" onChange={(e) => onChange({ ...rule, password: e.target.value, clear_auth: false })} />
          </div>
          <p className="hint">保存后不回显。粘贴含认证的地址时，认证会自动移到这两栏。</p>
        </>}
      </div>
    </div>
  );
}

/** 系统代理模式下给出这台机器上实际读到的值；只读，凭据已在后端遮盖。 */
function SystemProxyDetails({ system }: { system: SystemProxyView }) {
  const rows = ([["HTTP", system.http], ["HTTPS", system.https], ["SOCKS / ALL", system.all], ["不代理", system.no_proxy]] as const)
    .filter(([, value]) => value);
  return (
    <div className="proxy-detail proxy-system" role="group" aria-label="检测到的系统代理">
      <div className="proxy-system-head">
        <span>检测到的系统代理</span>
        <span className="proxy-system-source">来源：{system.source}</span>
      </div>
      {rows.length > 0 ? (
        <dl className="proxy-kv">
          {rows.map(([label, value]) => <div key={label}><dt>{label}</dt><dd className="mono">{value}</dd></div>)}
        </dl>
      ) : <p className="hint">没有读到代理设置，这个模式下实际为直连。</p>}
      {system.pac && <p className="hint">系统还设置了自动配置脚本（PAC），RRSS 不使用它，只按上面的地址连接。</p>}
    </div>
  );
}

function ProxyCheck({ draft, checks, testing, disabled, onTest }: {
  draft: NetworkProxyDraft; checks: Partial<Record<ProxyUse, Check>>; testing: boolean; disabled: boolean; onTest: () => void;
}) {
  return (
    <div className="proxy-check" role="group" aria-labelledby="advanced-network-proxy-check">
      <div className="proxy-check-head">
        <div>
          <strong id="advanced-network-proxy-check">连接检查</strong>
          <p className="hint">按当前填写（含未保存的修改）访问 GitHub，测完不保存。</p>
        </div>
        <button type="button" className="btn sm" disabled={disabled || testing} aria-busy={testing} onClick={onTest}>测试连接</button>
      </div>
      <ul className="proxy-check-list">
        {PROXY_USES.map((use) => {
          const own = draft[use];
          const rule = effectiveRule(draft, use);
          const check = checks[use];
          return (
            <li key={use} data-proxy-check={use} data-state={check ? (check.state === "done" ? (check.view.ok ? "ok" : "warn") : check.state) : undefined}>
              <span className="proxy-check-name">{SCOPE_TEXT[use].name}</span>
              <span className="proxy-check-route">{own.mode === "inherit" ? `全局 · ${routeSummary(rule)}` : routeSummary(rule)}</span>
              <CheckResult check={check} />
              {use === "pi" && isSocks(rule) && (
                <span className="proxy-check-note">对话能否走 SOCKS 代理取决于对话引擎和模型服务；文章和图片的下载都支持。</span>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

function CheckResult({ check }: { check: Check | undefined }) {
  if (!check) return <span className="proxy-check-result" />;
  if (check.state === "pending") return <span className="proxy-check-result" aria-busy="true"><span className="proxy-check-dot" aria-hidden="true" />测试中</span>;
  if (check.state === "failed") return <span className="proxy-check-result" role="alert" title={check.text}><span className="proxy-check-dot" aria-hidden="true" />{check.text}</span>;
  const { view } = check;
  return (
    <span className="proxy-check-result" role="status" title={view.message}>
      <span className="proxy-check-dot" aria-hidden="true" />{view.ok ? `可访问 · ${view.latency_ms} ms` : `HTTP ${view.status}`}
    </span>
  );
}
