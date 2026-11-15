import { i18n } from "../i18n";

/** 内置手动收录源的地址；它没有存名字，后端给空的 `feed_title` / `title`。 */
export const MANUAL_FEED_URL = "rrss:manual";

const manualName = () => String(i18n.t("manualFeed", { ns: "feeds" }));

/**
 * IPC 边界补上内置手动收录源的名字：后端只对它返回空的来源名（其余源总有名字），
 * 这里按当前语言填上，组件照常读 `feed_title` / `title`。原地改刚解析出的响应。
 */
export function nameBuiltInFeeds<T>(value: T, depth = 0): T {
  if (depth > 12 || !value || typeof value !== "object") return value;
  if (Array.isArray(value)) {
    for (const item of value) nameBuiltInFeeds(item, depth + 1);
    return value;
  }
  const record = value as Record<string, unknown>;
  if (record.feed_title === "") record.feed_title = manualName();
  if (record.url === MANUAL_FEED_URL && record.title === "") record.title = manualName();
  for (const child of Object.values(record)) {
    if (child && typeof child === "object") nameBuiltInFeeds(child, depth + 1);
  }
  return value;
}
