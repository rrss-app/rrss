import { useState } from "react";
import { applyLanguage, LANGUAGES, resolveLanguage, uiLanguageSetting, useTranslation } from "../i18n";
import { api } from "../lib/api";
import { MenuSelect } from "../components/MenuSelect";

type Option = "system" | "zh-CN" | "en";
const OPTIONS = ["system", ...LANGUAGES] as Option[];

/**
 * 设置导航底部的界面语言切换，紧挨「外观」。写设置后再问一次宿主解析结果（跟随系统时由宿主读系统语言），
 * 立即切换，不用重启。只有一种语言时不显示。选项名用各自的语言写（「简体中文」「English」）。
 */
export function LanguageSwitch() {
  const { t } = useTranslation("settingsSystem");
  const { t: shell } = useTranslation("shell");
  const [value, setValue] = useState(uiLanguageSetting);
  if (LANGUAGES.length < 2) return null;
  const choose = async (next: string) => {
    setValue(next);
    await api.setSetting("ui_language", next).catch(() => {});
    const host = await api.uiLanguage().catch(() => null);
    await applyLanguage(host?.language ?? resolveLanguage(next, navigator.languages), next);
  };
  // 三个选项放不进导航底部的宽度，用下拉菜单而不是分段控件。
  return <div className="settings-theme"><span>{shell("settings.language")}</span>
    <MenuSelect label={shell("settings.language")} value={value} options={OPTIONS.map((id) => ({ value: id, label: t(`registry.ui_language.options.${id}`) }))}
      onChange={(next) => void choose(next)} /></div>;
}
