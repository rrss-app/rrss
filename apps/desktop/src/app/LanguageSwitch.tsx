import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { applyLanguage, LANGUAGES, PSEUDO_LANGUAGE, resolveLanguage, uiLanguageSetting, useTranslation } from "../i18n";
import { api } from "../lib/api";
import { MenuSelect } from "../components/MenuSelect";
import { settingsSchemaOptions } from "../settings/settingsQuery";

type Option = "system" | "zh-CN" | "en";
const OPTIONS = ["system", ...LANGUAGES] as Option[];

/**
 * 设置导航底部的界面语言切换，在「外观」下面。写设置后再问一次宿主解析结果（跟随系统时由宿主读系统语言），
 * 立即切换，不用重启。只有一种语言时不显示。选项名用各自的语言写（「简体中文」「English」）。
 * 界面调试模式下多一个伪本地化 `en-XA`，只在本次运行生效，不写设置。
 */
export function LanguageSwitch() {
  const { t } = useTranslation("settingsSystem");
  const { t: shell } = useTranslation("shell");
  const [value, setValue] = useState<string>(uiLanguageSetting);
  const settings = useQuery(settingsSchemaOptions());
  const debug = settings.data?.values.some((item) => item.id === "ui_debug_mode" && item.value === "1") ?? false;
  if (LANGUAGES.length < 2) return null;
  const choose = async (next: string) => {
    setValue(next);
    if (next === PSEUDO_LANGUAGE) { await applyLanguage(PSEUDO_LANGUAGE); return; }
    await api.setSetting("ui_language", next).catch(() => {});
    const host = await api.uiLanguage().catch(() => null);
    await applyLanguage(host?.language ?? resolveLanguage(next, navigator.languages), next);
  };
  const options = OPTIONS.map((id) => ({ value: id as string, label: t(`registry.ui_language.options.${id}`) }));
  if (debug) options.push({ value: PSEUDO_LANGUAGE, label: `${PSEUDO_LANGUAGE} (pseudo)` });
  // 三个选项放不进导航底部的宽度，用下拉菜单而不是分段控件。
  return <div className="settings-theme"><span>{shell("settings.language")}</span>
    <MenuSelect label={shell("settings.language")} value={value} options={options} onChange={(next) => void choose(next)} /></div>;
}
