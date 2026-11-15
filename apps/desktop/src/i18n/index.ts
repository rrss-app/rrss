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

/** 源语言，也是缺译文时的回退语言。 */
export const SOURCE_LANGUAGE = "zh-CN";

/** 语言包按功能分命名空间，与 `features/` 对应（设置页按分区再分，壳层与 `lib/` 用 `shell`）；新增命名空间在这里登记。 */
export const resources = {
  "zh-CN": {
    chat, common, digest, errors, feeds, focus, reader, reasons, search,
    settingsAssistant, settingsContent, settingsJobs, settingsModels, settingsSystem, shell,
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

/** 有完整语言包、可以切换过去的语言。英文包补齐之前只有中文（阶段 27 · I4b 加 `en`）。 */
export const LANGUAGES: readonly string[] = [SOURCE_LANGUAGE];

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

/** 切到解析出的语言；还没有语言包的语言先留在源语言。 */
export function applyLanguage(language: string): Promise<unknown> {
  return i18n.changeLanguage(LANGUAGES.includes(language) ? language : SOURCE_LANGUAGE);
}

/** 当前界面语言：`Intl` 与 `toLocale*String` 都从这里取，不写死语言。 */
export const locale = (): string => i18n.language;

export { Trans, useTranslation } from "react-i18next";
export { i18n };
