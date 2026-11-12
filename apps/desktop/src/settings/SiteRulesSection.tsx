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

export function SiteRulesSection({ onAsk, onEditStateChange, seed, onSeedUsed }: {
  onAsk: Ask; onEditStateChange?: (state: SettingsEditState | null) => void;
  seed?: SiteRuleSeed; onSeedUsed?: () => void;
}) {
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
  useEditReport(report("list"), "站点规则", false, writing);
  const write = async (action: () => Promise<void>) => {
    const result = await run(action);
    if (result.status === "busy" || !result.mounted) return;
    setError(result.status === "failed" ? formatNotice("修改失败：", result.error) : "");
  };
  return <section className="settings-group site-rules-section">
    <div className="settings-group-head"><h4>站点规则</h4><button type="button" className="btn sm" disabled={writing} onClick={() => setEditor({})}>新建规则</button></div>
    {!query.data ? query.isError ? <p role="alert">无法读取站点规则。<button type="button" className="btn sm" onClick={() => void query.refetch()}>重试</button></p> : <Pending>正在读取站点规则…</Pending> : <div className="site-rules-list">
      <div ref={scroll} className={virtualized ? "site-rules-scroll" : undefined}><div style={virtualized ? { height: virtual.getTotalSize(), position: "relative" } : undefined}>
      {rows.map(({ item, index, start }) => <div className="site-rule-row" key={item.rule.domain} data-index={index} ref={virtualized ? virtual.measureElement : undefined} style={virtualized ? { position: "absolute", top: 0, left: 0, width: "100%", transform: `translateY(${start}px)` } : undefined}>
        <div className="site-rule-main"><strong>{item.rule.domain}</strong><span>你的规则{item.rule.body ? ` · 正文 ${item.rule.body}` : ""}{item.rule.strip.length ? ` · 删除 ${item.rule.strip.join(", ")}` : ""} · 更新 {new Date(item.updated_at).toLocaleDateString()}</span></div>
        <button type="button" className="switch" role="switch" aria-label={`启用 ${item.rule.domain}`} aria-checked={item.enabled} disabled={writing} onClick={() => void write(() => api.setSiteRuleEnabled(item.rule.domain, !item.enabled))} />
        <button type="button" className="btn sm ghost" disabled={writing} onClick={() => setEditor({ stored: item })}>编辑</button>
        <button type="button" className="btn sm ghost" disabled={writing} onClick={() => onAsk("删除站点规则？", `删除 ${item.rule.domain} 的规则后，将使用内置规则或通用识别。`, () => void write(() => api.deleteSiteRule(item.rule.domain)), { okLabel: "删除" })}>删除</button>
      </div>)}
      </div></div>
      <div className="site-rule-row"><div className="site-rule-main"><strong>内置规则 {query.data.builtin_count.toLocaleString()} 条</strong><span>来自 {query.data.source}（公有领域） · 版本 {query.data.commit} · 随 RRSS 更新 · 中文网站收录较少，多数靠自动识别</span></div></div>
    </div>}
    {query.data && query.isError && <p role="alert">列表没能刷新，刚才的修改已经保存，重新读取即可。<button type="button" className="btn sm" onClick={() => void query.refetch()}>重试</button></p>}
    {error && <p role="alert">{error}</p>}
    <p className="hint">取正文时先用你的规则，没有就用内置规则，再没有就自动识别。</p>
    {editor && <SiteRuleDialog stored={editor.stored} seed={editor.seed} onAsk={onAsk} onClose={() => setEditor(null)} onEditStateChange={report("editor")} />}
  </section>;
}
