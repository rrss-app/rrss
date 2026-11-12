import { useEffect, useId, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import type { SiteRuleView, SiteRuleTestView, StoredSiteRuleView } from "../bindings";
import { api } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { Dialog } from "./Dialog";
import { useSettingsWrite, type SettingsEditState } from "./editState";
import { siteRulesMutation } from "./siteRulesQuery";
import type { Ask } from "./SettingsPanel";

export type SiteRuleSeed = { domain: string; url: string };
const emptySiteRule = (domain = ""): SiteRuleView => ({ javascript_disabled: false, domain, body: null, title: null, author: null, date: null, strip: [] });
const fields = [["domain", "域名"], ["body", "正文"], ["title", "标题"], ["author", "作者"], ["date", "时间"]] as const;

export function SiteRuleDialog({ stored, seed, onClose, onAsk, onEditStateChange }: {
  stored?: StoredSiteRuleView; seed?: SiteRuleSeed; onClose: () => void; onAsk: Ask;
  onEditStateChange?: (state: SettingsEditState | null) => void;
}) {
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
    if (!payload.domain) { setError("请填写域名。"); return false; }
    const result = await run(() => api.saveSiteRule(payload, stored?.enabled ?? true));
    if (result.status === "busy" || !result.mounted) return false;
    if (result.status === "failed") { setError(formatNotice("保存失败：", result.error)); return false; }
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
      if (current === ticket.current) setError(formatNotice("测试失败：", cause && typeof cause === "object" && "message" in cause ? cause.message : cause));
    } finally { if (current === ticket.current) setTesting(false); }
  };
  const resetTest = () => { ++ticket.current; setTesting(false); setReport(null); };
  const change = (key: typeof fields[number][0], value: string) => {
    resetTest(); setError("");
    setDraft(old => ({ ...old, [key]: key === "domain" ? value : value || null }));
  };
  return <Dialog closeInHeader title="站点规则" className="wide site-rule-dialog" dirty={dirty} busy={writing} onAsk={onAsk} onClose={onClose} onSave={save} onEditStateChange={onEditStateChange}
    actions={<button type="button" className="btn primary" disabled={writing} aria-busy={writing} onClick={() => void save()}>保存</button>}>
    <div className="site-rule-fields">
      {fields.map(([key, label]) => <label key={key} htmlFor={`${id}-${key}`}><span>{label}</span><input id={`${id}-${key}`} className="input" value={draft[key] ?? ""} disabled={writing} readOnly={key === "domain" && !!stored} placeholder={key === "domain" ? "example.com" : "留空则用通用识别"} onChange={e => change(key, e.target.value)} /></label>)}
      <label htmlFor={`${id}-strip`}><span>删除</span><textarea id={`${id}-strip`} className="input" rows={3} value={strip} disabled={writing} placeholder="一行一个 CSS 选择器" onChange={e => { resetTest(); setStrip(e.target.value); }} /></label>
    </div>
    <label className="site-rule-javascript"><span>在原网页禁用本站 JavaScript</span><button type="button" className="switch" role="switch" aria-label="在原网页禁用本站 JavaScript" aria-checked={!!draft.javascript_disabled} disabled={writing} onClick={() => { resetTest(); setDraft(old => ({ ...old, javascript_disabled: !old.javascript_disabled })); }} /></label>
    <p className="hint">{stored ? "要换域名请新建规则。" : ""}用 CSS 选择器指定正文和要删掉的部分。「*.example.com」匹配所有子域名，不含 example.com 本身。</p>
    <div className="site-rule-test"><label htmlFor={`${id}-url`}>测试链接</label><div><input id={`${id}-url`} className="input" value={url} disabled={writing} onChange={e => { resetTest(); setUrl(e.target.value); }} /><button type="button" className="btn sm" disabled={writing || !url.trim()} aria-busy={testing} onClick={() => void test()}>测试</button></div></div>
    {error && <p role="alert" className="form-status err">{error}</p>}
    {report && <div className="site-rule-result" role="status">
      <div className="site-rule-layers"><span data-hit={report.layer === "UserRule"}>你的规则 · {report.layer === "UserRule" ? "命中" : "未命中"}</span><span>内置规则 · 未用</span><span data-hit={report.layer !== "UserRule"}>{report.layer === "Container" ? "article / main" : "通用识别"} · {report.layer === "UserRule" ? "未用" : "命中"}{report.layer === "Fallback" ? "（回退）" : ""}</span></div>
      <div>标题「{report.title}」 · 作者 {report.author || "未提取"} · {report.date || "未提取时间"} · 正文 {report.text_chars.toLocaleString()} 字 · {report.image_count} 张图 · {(report.elapsed_ms / 1000).toFixed(1)} 秒</div>
      <div className="site-rule-preview">{report.paragraphs.map((p, i) => <p key={i}>{p}</p>)}</div>
      <div className="muted">删除规则去掉了 {report.stripped_elements} 处。</div>
    </div>}
  </Dialog>;
}
