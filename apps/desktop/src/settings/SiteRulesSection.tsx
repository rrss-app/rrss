import { useEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { StoredSiteRuleView } from "../bindings";
import { api } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { Pending } from "../components/Pending";
import { readGate, useEditReport, useMergedEditState, useSettingsWrite, type SettingsEditState } from "./editState";
import { siteRulesMutation, siteRulesOptions } from "./siteRulesQuery";
import { SiteRuleDialog, type SiteRuleSeed } from "./SiteRuleDialog";
import type { Ask } from "./SettingsPanel";
import { locale, useTranslation } from "../i18n";

export function SiteRulesSection({ onAsk, onEditStateChange, seed, onSeedUsed }: {
  onAsk: Ask; onEditStateChange?: (state: SettingsEditState | null) => void;
  seed?: SiteRuleSeed; onSeedUsed?: () => void;
}) {
  const { t } = useTranslation("settingsContent");
  const [editor, setEditor] = useState<{ stored?: StoredSiteRuleView; seed?: SiteRuleSeed } | null>(() => seed ? { seed } : null);
  useEffect(() => { if (seed) onSeedUsed?.(); }, [seed, onSeedUsed]);
  const client = useQueryClient();
  const { writing, run } = useSettingsWrite(siteRulesMutation(client));
  const query = useQuery({ ...siteRulesOptions(), ...readGate(writing) });
  const scroll = useRef<HTMLDivElement>(null);
  const users = query.data?.users ?? [];
  const virtualized = users.length > 20;
  const virtual = useVirtualizer({ count: virtualized ? users.length : 0, getScrollElement: () => scroll.current, estimateSize: () => 72, overscan: 4, getItemKey: i => users[i].rule.domain });
  const rows = virtualized ? virtual.getVirtualItems().map(v => ({ item: users[v.index], index: v.index, start: v.start })) : users.map((item, index) => ({ item, index, start: 0 }));
  const [error, setError] = useState("");
  const { report } = useMergedEditState(onEditStateChange);
  useEditReport(report("list"), t("siteRules.title"), false, writing);
  const write = async (action: () => Promise<void>) => {
    const result = await run(action);
    if (result.status === "busy" || !result.mounted) return;
    setError(result.status === "failed" ? formatNotice(t("siteRules.writeFailed"), result.error) : "");
  };
  return <section className="settings-group site-rules-section">
    <div className="settings-group-head"><h4>{t("siteRules.title")}</h4><button type="button" className="btn sm" disabled={writing} onClick={() => setEditor({})}>{t("siteRules.new")}</button></div>
    {!query.data ? query.isError ? <p role="alert">{t("siteRules.readFailed")}<button type="button" className="btn sm" onClick={() => void query.refetch()}>{t("siteRules.retry")}</button></p> : <Pending>{t("siteRules.loading")}</Pending> : <div className="site-rules-list">
      <div ref={scroll} className={virtualized ? "site-rules-scroll" : undefined}><div style={virtualized ? { height: virtual.getTotalSize(), position: "relative" } : undefined}>
      {rows.map(({ item, index, start }) => <div className="site-rule-row" key={item.rule.domain} data-index={index} ref={virtualized ? virtual.measureElement : undefined} style={virtualized ? { position: "absolute", top: 0, left: 0, width: "100%", transform: `translateY(${start}px)` } : undefined}>
        <div className="site-rule-main"><strong>{item.rule.domain}</strong><span>{t("siteRules.row.user")}{item.rule.body ? ` · ${t("siteRules.row.body", { body: item.rule.body })}` : ""}{item.rule.strip.length ? ` · ${t("siteRules.row.strip", { selectors: item.rule.strip.join(", ") })}` : ""} · {t("siteRules.row.updated", { date: new Date(item.updated_at).toLocaleDateString(locale()) })}</span></div>
        <button type="button" className="switch" role="switch" aria-label={t("siteRules.row.enableAria", { domain: item.rule.domain })} aria-checked={item.enabled} disabled={writing} onClick={() => void write(() => api.setSiteRuleEnabled(item.rule.domain, !item.enabled))} />
        <button type="button" className="btn sm ghost" disabled={writing} onClick={() => setEditor({ stored: item })}>{t("siteRules.row.edit")}</button>
        <button type="button" className="btn sm ghost" disabled={writing} onClick={() => onAsk(t("siteRules.row.deleteTitle"), t("siteRules.row.deleteConfirm", { domain: item.rule.domain }), () => void write(() => api.deleteSiteRule(item.rule.domain)), { okLabel: t("siteRules.row.delete") })}>{t("siteRules.row.delete")}</button>
      </div>)}
      </div></div>
      <div className="site-rule-row"><div className="site-rule-main"><strong>{t("siteRules.builtin.title", { count: query.data.builtin_count })}</strong><span>{t("siteRules.builtin.detail", { source: query.data.source, commit: query.data.commit })}</span></div></div>
    </div>}
    {query.data && query.isError && <p role="alert">{t("siteRules.refreshFailed")}<button type="button" className="btn sm" onClick={() => void query.refetch()}>{t("siteRules.retry")}</button></p>}
    {error && <p role="alert">{error}</p>}
    <p className="hint">{t("siteRules.hint")}</p>
    {editor && <SiteRuleDialog stored={editor.stored} seed={editor.seed} onAsk={onAsk} onClose={() => setEditor(null)} onEditStateChange={report("editor")} />}
  </section>;
}
