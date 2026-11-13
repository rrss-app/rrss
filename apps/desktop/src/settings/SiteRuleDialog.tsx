import { useEffect, useId, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import type { SiteRuleView, SiteRuleTestView, StoredSiteRuleView } from "../bindings";
import { api } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { Dialog } from "./Dialog";
import { useSettingsWrite, type SettingsEditState } from "./editState";
import { siteRulesMutation } from "./siteRulesQuery";
import type { Ask } from "./SettingsPanel";
import { locale, useTranslation } from "../i18n";

export type SiteRuleSeed = { domain: string; url: string };
const emptySiteRule = (domain = ""): SiteRuleView => ({ javascript_disabled: false, domain, body: null, title: null, author: null, date: null, strip: [] });
const fields = ["domain", "body", "title", "author", "date"] as const;

export function SiteRuleDialog({ stored, seed, onClose, onAsk, onEditStateChange }: {
  stored?: StoredSiteRuleView; seed?: SiteRuleSeed; onClose: () => void; onAsk: Ask;
  onEditStateChange?: (state: SettingsEditState | null) => void;
}) {
  const { t } = useTranslation("settingsContent");
  const [baseline] = useState(() => stored?.rule ?? emptySiteRule(seed?.domain));
  const [draft, setDraft] = useState(baseline);
  const [strip, setStrip] = useState(baseline.strip.join("\n"));
  const [url, setUrl] = useState(seed?.url ?? "");
  const [error, setError] = useState("");
  const [report, setReport] = useState<SiteRuleTestView | null>(null);
  const [testing, setTesting] = useState(false);
  const ticket = useRef(0);
  useEffect(() => () => { ++ticket.current; }, []);
  const client = useQueryClient();
  const { writing, run } = useSettingsWrite(siteRulesMutation(client));
  const id = useId();
  const rule = (): SiteRuleView => ({ ...draft, domain: draft.domain.trim(), strip: strip.split("\n").map(s => s.trim()).filter(Boolean) });
  const dirty = JSON.stringify(draft) !== JSON.stringify(baseline) || strip !== baseline.strip.join("\n");
  const save = async () => {
    const payload = rule();
    if (!payload.domain) { setError(t("siteRule.domainRequired")); return false; }
    const result = await run(() => api.saveSiteRule(payload, stored?.enabled ?? true));
    if (result.status === "busy" || !result.mounted) return false;
    if (result.status === "failed") { setError(formatNotice(t("siteRule.saveFailed"), result.error)); return false; }
    onClose();
    return true;
  };
  const test = async () => {
    const current = ++ticket.current;
    setTesting(true); setError(""); setReport(null);
    try {
      const next = await api.testSiteRule(url.trim(), rule());
      if (current === ticket.current) setReport(next);
    } catch (cause) {
      if (current === ticket.current) setError(formatNotice(t("siteRule.testFailed"), cause && typeof cause === "object" && "message" in cause ? cause.message : cause));
    } finally { if (current === ticket.current) setTesting(false); }
  };
  const resetTest = () => { ++ticket.current; setTesting(false); setReport(null); };
  const change = (key: typeof fields[number], value: string) => {
    resetTest(); setError("");
    setDraft(old => ({ ...old, [key]: key === "domain" ? value : value || null }));
  };
  return <Dialog closeInHeader title={t("siteRule.title")} className="wide site-rule-dialog" dirty={dirty} busy={writing} onAsk={onAsk} onClose={onClose} onSave={save} onEditStateChange={onEditStateChange}
    actions={<button type="button" className="btn primary" disabled={writing} aria-busy={writing} onClick={() => void save()}>{t("siteRule.save")}</button>}>
    <div className="site-rule-fields">
      {fields.map((key) => <label key={key} htmlFor={`${id}-${key}`}><span>{t(`siteRule.field.${key}`)}</span><input id={`${id}-${key}`} className="input" value={draft[key] ?? ""} disabled={writing} readOnly={key === "domain" && !!stored} placeholder={key === "domain" ? "example.com" : t("siteRule.fieldPlaceholder")} onChange={e => change(key, e.target.value)} /></label>)}
      <label htmlFor={`${id}-strip`}><span>{t("siteRule.strip")}</span><textarea id={`${id}-strip`} className="input" rows={3} value={strip} disabled={writing} placeholder={t("siteRule.stripPlaceholder")} onChange={e => { resetTest(); setStrip(e.target.value); }} /></label>
    </div>
    <label className="site-rule-javascript"><span>{t("siteRule.disableJs")}</span><button type="button" className="switch" role="switch" aria-label={t("siteRule.disableJs")} aria-checked={!!draft.javascript_disabled} disabled={writing} onClick={() => { resetTest(); setDraft(old => ({ ...old, javascript_disabled: !old.javascript_disabled })); }} /></label>
    <p className="hint">{stored ? t("siteRule.hintStored") : ""}{t("siteRule.hint")}</p>
    <div className="site-rule-test"><label htmlFor={`${id}-url`}>{t("siteRule.testUrl")}</label><div><input id={`${id}-url`} className="input" value={url} disabled={writing} onChange={e => { resetTest(); setUrl(e.target.value); }} /><button type="button" className="btn sm" disabled={writing || !url.trim()} aria-busy={testing} onClick={() => void test()}>{t("siteRule.test")}</button></div></div>
    {error && <p role="alert" className="form-status err">{error}</p>}
    {report && <div className="site-rule-result" role="status">
      <div className="site-rule-layers"><span data-hit={report.layer === "UserRule"}>{report.layer === "UserRule" ? t("siteRule.result.userHit") : t("siteRule.result.userMiss")}</span><span>{t("siteRule.result.builtinUnused")}</span><span data-hit={report.layer !== "UserRule"}>{t(report.layer === "UserRule" ? "siteRule.result.layerUnused" : "siteRule.result.layerHit", { name: report.layer === "Container" ? "article / main" : t("siteRule.result.generic") })}{report.layer === "Fallback" ? t("siteRule.result.fallbackTag") : ""}</span></div>
      <div>{t("siteRule.result.summary", { title: report.title, author: report.author || t("siteRule.result.noAuthor"), date: report.date || t("siteRule.result.noDate"), chars: report.text_chars.toLocaleString(locale()), images: report.image_count, seconds: (report.elapsed_ms / 1000).toFixed(1) })}</div>
      <div className="site-rule-preview">{report.paragraphs.map((p, i) => <p key={i}>{p}</p>)}</div>
      <div className="muted">{t("siteRule.result.stripped", { count: report.stripped_elements })}</div>
    </div>}
  </Dialog>;
}
