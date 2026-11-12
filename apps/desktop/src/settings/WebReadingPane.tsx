import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { MenuButton } from "../components/MenuSelect";
import { Icon } from "../components/Icon";
import { Pending } from "../components/Pending";
import { SettingRow } from "./chips";
import { useEditReport, useMergedEditState, useSettingsWrite, type SettingsEditState } from "./editState";
import { settingsMutationOptions, settingsSchemaOptions } from "./settingsQuery";
import { api } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { SiteRulesSection } from "./SiteRulesSection";
import { siteRulesWriteKey } from "./siteRulesQuery";
import type { SiteRuleSeed } from "./SiteRuleDialog";
import type { Ask } from "./SettingsPanel";

export function WebReadingPane({ onEditStateChange, onAsk, siteRuleSeed, onSiteRuleSeedUsed }: { onAsk: Ask; onEditStateChange?: (state: SettingsEditState | null) => void; siteRuleSeed?: SiteRuleSeed; onSiteRuleSeedUsed?: () => void }) {
  const client = useQueryClient();
  const query = useQuery(settingsSchemaOptions());
  const { writing, run } = useSettingsWrite({ ...settingsMutationOptions(client), mutationKey: siteRulesWriteKey });
  const { run: runClear } = useSettingsWrite({ mutationKey: siteRulesWriteKey, mutationFn: () => api.clearWebData() });
  const busy = writing;
  const [message, setMessage] = useState("");
  const { report } = useMergedEditState(onEditStateChange);
  useEditReport(report("settings"), "网页阅读", false, busy);
  const save = async (id: "web_open_mode" | "web_first_tip" | "web_javascript", value: string) => {
    setMessage("");
    const result = await run(id === "web_first_tip" ? [{ id, value }, { id: "web_original_tip", value }] : [{ id, value }]);
    if (result.status === "busy" || !result.mounted) return;
    const error = result.status === "failed" ? result.error : result.data.failure?.error;
    setMessage(error ? formatNotice("保存失败：", error) : id === "web_first_tip" ? "下次打开阅读视图或原网页时会重新显示首次提示。" : "");
  };
  const clear = async () => {
    if (busy) return;
    setMessage("");
    const result = await runClear(undefined);
    if (result.status === "busy" || !result.mounted) return;
    setMessage(result.status === "failed" ? formatNotice("清空失败：", result.error) : "原网页数据已清空。");
  };
  if (!query.data) return query.isError ? <div role="alert">无法读取网页阅读设置。<button type="button" className="btn" onClick={() => void query.refetch()}>重试</button></div> : <Pending>正在读取网页阅读设置…</Pending>;
  const mode = query.data.values.find(v => v.id === "web_open_mode")?.value ?? "reader";
  const kind = query.data.specs.find(s => s.id === "web_open_mode")?.kind;
  const options = kind?.type === "enum" ? kind.options : [];
  return <div className="web-reading-settings">
    <p className="hint">文章里的外部链接先在 RRSS 里打开。打开的网页默认不进库。</p>
    <section className="settings-group"><div className="settings-group-head"><h4>打开方式</h4></div>
      <SettingRow label="点击外部链接" hint="右键链接随时可以选别的方式。">
        <MenuButton label="点击外部链接" className="menu-select" disabled={busy} face={<><span>{options.find(o => o.id === mode)?.label}</span><Icon name="chevronDown" size={12} /></>}>
          {close => options.map(option => <button type="button" role="menuitemradio" key={option.id} aria-checked={mode === option.id} onClick={() => { close(); void save("web_open_mode", option.id); }}>{option.label}{option.hint ? `（${option.hint}）` : ""}</button>)}
        </MenuButton>
      </SettingRow>
      <SettingRow label="首次提示" hint="仿冒域名、链接文字与地址不符、非 HTTPS 的提示总会出现。">
        <button type="button" className="btn sm" disabled={busy} aria-busy={writing} onClick={() => void save("web_first_tip", "1")}>重新显示首次提示</button>
      </SettingRow>
    </section>
    <section className="settings-group"><div className="settings-group-head"><h4>原网页</h4></div>
      <SettingRow label="允许网页运行 JavaScript" hint="关掉更安全，但很多网站会显示不全。更改后对新打开的原网页生效，也可以在站点规则里只对某个站点关闭。">
        <button type="button" className="switch" role="switch" aria-label="允许 JavaScript" aria-checked={query.data.values.find(v => v.id === "web_javascript")?.value === "1"} disabled={busy} onClick={() => void save("web_javascript", query.data.values.find(v => v.id === "web_javascript")?.value === "1" ? "0" : "1")} />
      </SettingRow>
      <SettingRow label="浏览数据只保留到退出 RRSS" hint="这次运行里，原网页共用登录、Cookie 和缓存，通过一次人机验证就不用反复验证；关掉主窗口退出后全部删除，下次启动是全新状态。摄像头、麦克风、定位、通知一律拒绝；下载会被拦截，可交给系统浏览器。">
        <button type="button" className="btn sm" disabled={busy} aria-busy={writing} onClick={() => onAsk("立即清空原网页数据？", "这会关闭当前原网页，并清除这次运行保存的登录、Cookie 和缓存。", () => void clear(), { okLabel: "立即清空" })}>立即清空</button>
      </SettingRow>
    </section>
    {message ? <p role="status" className="hint">{message}</p> : null}
    <SiteRulesSection onAsk={onAsk} onEditStateChange={report("rules")} seed={siteRuleSeed} onSeedUsed={onSiteRuleSeedUsed} />
  </div>;
}
