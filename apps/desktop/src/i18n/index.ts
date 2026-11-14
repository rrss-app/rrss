import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import chat from "../locales/zh-CN/chat.json" with { type: "json" };
import common from "../locales/zh-CN/common.json" with { type: "json" };
import digest from "../locales/zh-CN/digest.json" with { type: "json" };
import errors from "../locales/zh-CN/errors.json" with { type: "json" };
import feeds from "../locales/zh-CN/feeds.json" with { type: "json" };
import focus from "../locales/zh-CN/focus.json" with { type: "json" };
import reader from "../locales/zh-CN/reader.json" with { type: "json" };
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
    chat, common, digest, errors, feeds, focus, reader, search,
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

/** 当前界面语言：`Intl` 与 `toLocale*String` 都从这里取，不写死语言。 */
export const locale = (): string => i18n.language;

export { Trans, useTranslation } from "react-i18next";
export { i18n };
