import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import chat from "../locales/zh-CN/chat.json" with { type: "json" };
import common from "../locales/zh-CN/common.json" with { type: "json" };
import digest from "../locales/zh-CN/digest.json" with { type: "json" };
import errors from "../locales/zh-CN/errors.json" with { type: "json" };
import feeds from "../locales/zh-CN/feeds.json" with { type: "json" };
import focus from "../locales/zh-CN/focus.json" with { type: "json" };
import reader from "../locales/zh-CN/reader.json" with { type: "json" };
import reasons from "../locales/zh-CN/reasons.json" with { type: "json" };
import search from "../locales/zh-CN/search.json" with { type: "json" };
import settingsAssistant from "../locales/zh-CN/settingsAssistant.json" with { type: "json" };
import settingsContent from "../locales/zh-CN/settingsContent.json" with { type: "json" };
import settingsJobs from "../locales/zh-CN/settingsJobs.json" with { type: "json" };
import settingsModels from "../locales/zh-CN/settingsModels.json" with { type: "json" };
import settingsSystem from "../locales/zh-CN/settingsSystem.json" with { type: "json" };
import shell from "../locales/zh-CN/shell.json" with { type: "json" };
import enChat from "../locales/en/chat.json" with { type: "json" };
import enCommon from "../locales/en/common.json" with { type: "json" };
import enDigest from "../locales/en/digest.json" with { type: "json" };
import enErrors from "../locales/en/errors.json" with { type: "json" };
import enFeeds from "../locales/en/feeds.json" with { type: "json" };
import enFocus from "../locales/en/focus.json" with { type: "json" };
import enReader from "../locales/en/reader.json" with { type: "json" };
import enReasons from "../locales/en/reasons.json" with { type: "json" };
import enSearch from "../locales/en/search.json" with { type: "json" };
import enSettingsAssistant from "../locales/en/settingsAssistant.json" with { type: "json" };
import enSettingsContent from "../locales/en/settingsContent.json" with { type: "json" };
import enSettingsJobs from "../locales/en/settingsJobs.json" with { type: "json" };
import enSettingsModels from "../locales/en/settingsModels.json" with { type: "json" };
import enSettingsSystem from "../locales/en/settingsSystem.json" with { type: "json" };
import enShell from "../locales/en/shell.json" with { type: "json" };

/** 源语言，也是缺译文时的回退语言。 */
export const SOURCE_LANGUAGE = "zh-CN";

/** 语言包按功能分命名空间，与 `features/` 对应（设置页按分区再分，壳层与 `lib/` 用 `shell`）；新增命名空间在这里登记。 */
export const resources = {
  "zh-CN": {
    chat, common, digest, errors, feeds, focus, reader, reasons, search,
    settingsAssistant, settingsContent, settingsJobs, settingsModels, settingsSystem, shell,
  },
  en: {
    chat: enChat, common: enCommon, digest: enDigest, errors: enErrors, feeds: enFeeds,
    focus: enFocus, reader: enReader, reasons: enReasons, search: enSearch, settingsAssistant: enSettingsAssistant,
    settingsContent: enSettingsContent, settingsJobs: enSettingsJobs, settingsModels: enSettingsModels, settingsSystem: enSettingsSystem, shell: enShell,
  },
} as const;

// 语言包随包内联，同步初始化：首帧与测试都不用等加载。
void i18n.use(initReactI18next).init({
  lng: SOURCE_LANGUAGE,
  fallbackLng: SOURCE_LANGUAGE,
  resources,
  defaultNS: "common",
  initAsync: false,
  // React 自己转义；标题、查询词等内容原样插值。
  interpolation: { escapeValue: false },
});

if (typeof document !== "undefined") {
  document.documentElement.lang = i18n.language;
  i18n.on("languageChanged", (language) => { document.documentElement.lang = language; });
}

/** 有完整语言包、可以切换过去的语言（`check:locales` 保证与中文的键一致）。 */
export const LANGUAGES: readonly string[] = [SOURCE_LANGUAGE, "en"];

/**
 * 与宿主 `rrss_app::ui_language::resolve` 同一套规则，给没有宿主的浏览器预览用：
 * 显式设置优先；跟随系统时取第一个认识的（`zh*` → 简体中文，`en*` → 英文），都不认识落到英文。
 */
export function resolveLanguage(setting: string, system: readonly string[]): string {
  if (setting === "zh-CN" || setting === "en") return setting;
  for (const tag of system) {
    const primary = tag.trim().split(/[-_]/)[0]?.toLowerCase();
    if (primary === "zh") return "zh-CN";
    if (primary === "en") return "en";
  }
  return "en";
}

let languageSetting = "system";
/** 设置里的原值（`system` / `zh-CN` / `en`），给语言切换控件显示当前选择。 */
export const uiLanguageSetting = (): string => languageSetting;

/** 切到解析出的语言；还没有语言包的语言先留在源语言。 */
export function applyLanguage(language: string, setting = languageSetting): Promise<unknown> {
  languageSetting = setting;
  return i18n.changeLanguage(LANGUAGES.includes(language) ? language : SOURCE_LANGUAGE);
}

/** 当前界面语言：`Intl` 与 `toLocale*String` 都从这里取，不写死语言。 */
export const locale = (): string => i18n.language;

export { Trans, useTranslation } from "react-i18next";
export { i18n };
