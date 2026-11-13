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
import { useTranslation } from "../i18n";

export function WebReadingPane({ onEditStateChange, onAsk, siteRuleSeed, onSiteRuleSeedUsed }: { onAsk: Ask; onEditStateChange?: (state: SettingsEditState | null) => void; siteRuleSeed?: SiteRuleSeed; onSiteRuleSeedUsed?: () => void }) {
  const { t } = useTranslation("settingsContent");
  const client = useQueryClient();
  const query = useQuery(settingsSchemaOptions());
  const { writing, run } = useSettingsWrite({ ...settingsMutationOptions(client), mutationKey: siteRulesWriteKey });
  const { run: runClear } = useSettingsWrite({ mutationKey: siteRulesWriteKey, mutationFn: () => api.clearWebData() });
  const busy = writing;
  const [message, setMessage] = useState("");
  const { report } = useMergedEditState(onEditStateChange);
  useEditReport(report("settings"), t("webReading.editLabel"), false, busy);
  const save = async (id: "web_open_mode" | "web_first_tip" | "web_javascript", value: string) => {
    setMessage("");
    const result = await run(id === "web_first_tip" ? [{ id, value }, { id: "web_original_tip", value }] : [{ id, value }]);
    if (result.status === "busy" || !result.mounted) return;
    const error = result.status === "failed" ? result.error : result.data.failure?.error;
    setMessage(error ? formatNotice(t("webReading.saveFailed"), error) : id === "web_first_tip" ? t("webReading.firstTipReset") : "");
  };
  const clear = async () => {
    if (busy) return;
    setMessage("");
    const result = await runClear(undefined);
    if (result.status === "busy" || !result.mounted) return;
    setMessage(result.status === "failed" ? formatNotice(t("webReading.clearFailed"), result.error) : t("webReading.cleared"));
  };
  if (!query.data) return query.isError ? <div role="alert">{t("webReading.readFailed")}<button type="button" className="btn" onClick={() => void query.refetch()}>{t("webReading.retry")}</button></div> : <Pending>{t("webReading.loading")}</Pending>;
  const mode = query.data.values.find(v => v.id === "web_open_mode")?.value ?? "reader";
  const kind = query.data.specs.find(s => s.id === "web_open_mode")?.kind;
  const options = kind?.type === "enum" ? kind.options : [];
  return <div className="web-reading-settings">
    <p className="hint">{t("webReading.intro")}</p>
    <section className="settings-group"><div className="settings-group-head"><h4>{t("webReading.open.title")}</h4></div>
      <SettingRow label={t("webReading.open.click")} hint={t("webReading.open.clickHint")}>
        <MenuButton label={t("webReading.open.click")} className="menu-select" disabled={busy} face={<><span>{options.find(o => o.id === mode)?.label}</span><Icon name="chevronDown" size={12} /></>}>
          {close => options.map(option => <button type="button" role="menuitemradio" key={option.id} aria-checked={mode === option.id} onClick={() => { close(); void save("web_open_mode", option.id); }}>{option.label}{option.hint ? t("webReading.open.optionHint", { hint: option.hint }) : ""}</button>)}
        </MenuButton>
      </SettingRow>
      <SettingRow label={t("webReading.tip.label")} hint={t("webReading.tip.hint")}>
        <button type="button" className="btn sm" disabled={busy} aria-busy={writing} onClick={() => void save("web_first_tip", "1")}>{t("webReading.tip.reset")}</button>
      </SettingRow>
    </section>
    <section className="settings-group"><div className="settings-group-head"><h4>{t("webReading.page.title")}</h4></div>
      <SettingRow label={t("webReading.page.js")} hint={t("webReading.page.jsHint")}>
        <button type="button" className="switch" role="switch" aria-label={t("webReading.page.jsAria")} aria-checked={query.data.values.find(v => v.id === "web_javascript")?.value === "1"} disabled={busy} onClick={() => void save("web_javascript", query.data.values.find(v => v.id === "web_javascript")?.value === "1" ? "0" : "1")} />
      </SettingRow>
      <SettingRow label={t("webReading.page.data")} hint={t("webReading.page.dataHint")}>
        <button type="button" className="btn sm" disabled={busy} aria-busy={writing} onClick={() => onAsk(t("webReading.page.clearTitle"), t("webReading.page.clearBody"), () => void clear(), { okLabel: t("webReading.page.clear") })}>{t("webReading.page.clear")}</button>
      </SettingRow>
    </section>
    {message ? <p role="status" className="hint">{message}</p> : null}
    <SiteRulesSection onAsk={onAsk} onEditStateChange={report("rules")} seed={siteRuleSeed} onSeedUsed={onSiteRuleSeedUsed} />
  </div>;
}
