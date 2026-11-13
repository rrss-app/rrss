import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type NetworkProxyMode, type NetworkProxyTestView, type SystemProxyView } from "../lib/api";
import { i18n, useTranslation } from "../i18n";
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

const GLOBAL_MODES = ["direct", "system", "custom"] as const satisfies readonly NetworkProxyMode[];
const USE_MODES = ["inherit", "direct", "system", "custom"] as const satisfies readonly NetworkProxyMode[];

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
  const { t } = useTranslation("settingsSystem");
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
        setFeedback({ tone: "ok", text: t("proxy.saved") });
      }
      return true;
    }
    if (outcome.status === "failed" && outcome.mounted) setFeedback({ tone: "err", text: formatNotice(t("proxy.saveFailed"), outcome.error) });
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
  useEditReport(report, t("proxy.title"), dirty, busy, commit);
  const overrides = current ? PROXY_USES.filter((use) => current[use].mode !== "inherit").length : 0;
  const system = query.data?.system ?? null;
  return (
    <section className="settings-group settings-section" id="advanced-network-proxy" aria-labelledby="advanced-network-proxy-title">
      <div className="settings-section-meta">
        <div className="settings-group-head"><h4 id="advanced-network-proxy-title">{t("proxy.title")}</h4></div>
        <p className="hint">{t("proxy.hint")}</p>
      </div>
      <div className="settings-section-body proxy-body">
        {query.isPending && <Pending>{t("proxy.loading")}</Pending>}
        {query.isError && (
          <p className="notice error" role="alert">
            {formatNotice(t("proxy.loadFailed"), query.error)}
            <button type="button" className="btn sm" onClick={() => void query.refetch()}>{t("proxy.reload")}</button>
          </p>
        )}
        {current && <>
          <ProxyRuleEditor scope="global" rule={current.global} savedUrl={saved?.global.url ?? ""} system={system} disabled={busy} onChange={(rule) => update("global", rule)} />
          <Fold className="proxy-uses" title={t("proxy.uses.title")} summary={overrides ? t("proxy.uses.summary", { count: overrides }) : t("proxy.uses.allInherit")}
            open={open} onToggle={() => setOpen((value) => !value)}>
            {PROXY_USES.map((use) => (
              <ProxyRuleEditor key={use} scope={use} rule={current[use]} savedUrl={saved?.[use].url ?? ""} system={system} disabled={busy} onChange={(rule) => update(use, rule)} />
            ))}
          </Fold>
          <ProxyCheck draft={current} checks={checks} testing={testing} disabled={writing} onTest={() => void test()} />
          {feedback && <Feedback result={feedback} />}
          {dirty && <SaveBar label={t("proxy.title")} busy={writing} onSave={() => void commit()} onUndo={() => { setDraft(null); setFeedback(null); setChecks({}); }} />}
        </>}
      </div>
    </section>
  );
}

function schemeHint(url: string): string {
  const hint = (key: "socks5h" | "socks5" | "default") => i18n.t(`proxy.scheme.${key}`, { ns: "settingsSystem" });
  if (/^socks5h:\/\//i.test(url)) return hint("socks5h");
  if (/^socks5:\/\//i.test(url)) return hint("socks5");
  return hint("default");
}

function ProxyRuleEditor({ scope, rule, savedUrl, system, disabled, onChange }: {
  scope: ProxyScope; rule: NetworkProxyDraftRule; savedUrl: string; system: SystemProxyView | null; disabled: boolean;
  onChange: (rule: NetworkProxyDraftRule) => void;
}) {
  const { t } = useTranslation("settingsSystem");
  const name = t(`proxy.scope.${scope}.name`);
  const modes = (scope === "global" ? GLOBAL_MODES : USE_MODES).map((value) => ({ value, label: t(`proxy.mode.${value}`) }));
  return (
    <div className="proxy-rule" data-proxy-scope={scope} data-mode={rule.mode}>
      <SettingRow label={scope === "global" ? t("proxy.rule.globalLabel") : name} hint={t(`proxy.scope.${scope}.hint`)}>
        <MenuSelect label={t("proxy.rule.modeAria", { name })} value={rule.mode} options={modes} disabled={disabled}
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
  const { t } = useTranslation("settingsSystem");
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
      <Field label={t("proxy.custom.url")} hint={schemeHint(rule.url)}>
        <input data-proxy-field="url" aria-label={t("proxy.custom.urlAria", { name })} className="mono" value={rule.url} disabled={disabled} spellCheck={false} autoComplete="off"
          placeholder="http://127.0.0.1:7890" onChange={(e) => onChange(withPastedAuth(rule, e.target.value))} />
      </Field>
      <Field label={t("proxy.custom.noProxy")} hint={t("proxy.custom.noProxyHint")}>
        <input aria-label={t("proxy.custom.noProxyAria", { name })} className="mono" value={rule.no_proxy} disabled={disabled} spellCheck={false} autoComplete="off"
          placeholder={t("proxy.custom.noProxyPlaceholder")} onChange={(e) => onChange({ ...rule, no_proxy: e.target.value })} />
      </Field>
      <div className="proxy-auth" role="group" aria-label={t("proxy.auth.groupAria", { name })}>
        <div className="proxy-auth-head">
          <span className="proxy-auth-title">{t("proxy.auth.title")}</span>
          <span className="proxy-auth-state" data-auth-state={authState}>{t(`proxy.auth.state.${authState}`)}</span>
          <span className="proxy-auth-actions">
            {rule.clear_auth ? (
              <button type="button" className="btn sm ghost" disabled={disabled} onClick={() => onChange({ ...rule, clear_auth: false })}>{t("proxy.auth.keep")}</button>
            ) : showAuth ? (
              <button type="button" className="btn sm ghost" disabled={disabled} onClick={closeAuth}>{rule.has_auth ? t("proxy.auth.noReplace") : t("proxy.auth.noAuth")}</button>
            ) : <>
              <button type="button" className="btn sm ghost" disabled={disabled} onClick={() => setEditingAuth(true)}>{rule.has_auth ? t("proxy.auth.replace") : t("proxy.auth.add")}</button>
              {rule.has_auth && (
                <button type="button" className="btn sm ghost" disabled={disabled}
                  onClick={() => onChange({ ...rule, username: "", password: "", clear_auth: true })}>{t("proxy.auth.clear")}</button>
              )}
            </>}
          </span>
        </div>
        {endpointChanged && <p className="hint" data-auth-clear-reason="endpoint-changed">{t("proxy.auth.endpointChanged")}</p>}
        {showAuth && !rule.clear_auth && <>
          <div className="proxy-auth-fields">
            <input aria-label={t("proxy.auth.usernameAria", { name })} value={rule.username} disabled={disabled} autoComplete="off" spellCheck={false}
              placeholder={t("proxy.auth.username")} onChange={(e) => onChange({ ...rule, username: e.target.value, clear_auth: false })} />
            <input aria-label={t("proxy.auth.passwordAria", { name })} type="password" value={rule.password} disabled={disabled} autoComplete="off"
              placeholder={t("proxy.auth.password")} onChange={(e) => onChange({ ...rule, password: e.target.value, clear_auth: false })} />
          </div>
          <p className="hint">{t("proxy.auth.hint")}</p>
        </>}
      </div>
    </div>
  );
}

/** 系统代理模式下给出这台机器上实际读到的值；只读，凭据已在后端遮盖。 */
function SystemProxyDetails({ system }: { system: SystemProxyView }) {
  const { t } = useTranslation("settingsSystem");
  const rows = ([["HTTP", system.http], ["HTTPS", system.https], ["SOCKS / ALL", system.all], [t("proxy.system.noProxy"), system.no_proxy]] as const)
    .filter(([, value]) => value);
  return (
    <div className="proxy-detail proxy-system" role="group" aria-label={t("proxy.system.title")}>
      <div className="proxy-system-head">
        <span>{t("proxy.system.title")}</span>
        <span className="proxy-system-source">{t("proxy.system.source", { source: system.source })}</span>
      </div>
      {rows.length > 0 ? (
        <dl className="proxy-kv">
          {rows.map(([label, value]) => <div key={label}><dt>{label}</dt><dd className="mono">{value}</dd></div>)}
        </dl>
      ) : <p className="hint">{t("proxy.system.none")}</p>}
      {system.pac && <p className="hint">{t("proxy.system.pac")}</p>}
    </div>
  );
}

function ProxyCheck({ draft, checks, testing, disabled, onTest }: {
  draft: NetworkProxyDraft; checks: Partial<Record<ProxyUse, Check>>; testing: boolean; disabled: boolean; onTest: () => void;
}) {
  const { t } = useTranslation("settingsSystem");
  return (
    <div className="proxy-check" role="group" aria-labelledby="advanced-network-proxy-check">
      <div className="proxy-check-head">
        <div>
          <strong id="advanced-network-proxy-check">{t("proxy.check.title")}</strong>
          <p className="hint">{t("proxy.check.hint")}</p>
        </div>
        <button type="button" className="btn sm" disabled={disabled || testing} aria-busy={testing} onClick={onTest}>{t("proxy.check.test")}</button>
      </div>
      <ul className="proxy-check-list">
        {PROXY_USES.map((use) => {
          const own = draft[use];
          const rule = effectiveRule(draft, use);
          const check = checks[use];
          return (
            <li key={use} data-proxy-check={use} data-state={check ? (check.state === "done" ? (check.view.ok ? "ok" : "warn") : check.state) : undefined}>
              <span className="proxy-check-name">{t(`proxy.scope.${use}.name`)}</span>
              <span className="proxy-check-route">{own.mode === "inherit" ? t("proxy.check.inheritRoute", { route: routeSummary(rule) }) : routeSummary(rule)}</span>
              <CheckResult check={check} />
              {use === "pi" && isSocks(rule) && (
                <span className="proxy-check-note">{t("proxy.check.socksNote")}</span>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

function CheckResult({ check }: { check: Check | undefined }) {
  const { t } = useTranslation("settingsSystem");
  if (!check) return <span className="proxy-check-result" />;
  if (check.state === "pending") return <span className="proxy-check-result" aria-busy="true"><span className="proxy-check-dot" aria-hidden="true" />{t("proxy.check.testing")}</span>;
  if (check.state === "failed") return <span className="proxy-check-result" role="alert" title={check.text}><span className="proxy-check-dot" aria-hidden="true" />{check.text}</span>;
  const { view } = check;
  return (
    <span className="proxy-check-result" role="status" title={view.message}>
      <span className="proxy-check-dot" aria-hidden="true" />{view.ok ? t("proxy.check.ok", { ms: view.latency_ms }) : `HTTP ${view.status}`}
    </span>
  );
}
