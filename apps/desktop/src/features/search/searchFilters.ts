import type { SearchScope } from "../../lib/api";

export type SearchTime = "any" | "today" | "7d" | "30d";

/** 浮层筛选：范围 / 来源（空 = 全部订阅）/ 发布时间。 */
export type SearchFilters = { scope: SearchScope; feedIds: number[]; time: SearchTime };

export const DEFAULT_FILTERS: SearchFilters = { scope: "all", feedIds: [], time: "any" };

/** 浮层一次取的结果数；列表与预览各自滚动，不分页。 */
export const SEARCH_LIMIT = 50;

export const SCOPE_OPTIONS: { value: SearchScope; label: string }[] = [
  { value: "all", label: "全部" },
  { value: "unread", label: "未读" },
  { value: "starred", label: "收藏" },
];

export const TIME_OPTIONS: { value: SearchTime; label: string }[] = [
  { value: "any", label: "不限" },
  { value: "today", label: "今天" },
  { value: "7d", label: "7 天" },
  { value: "30d", label: "30 天" },
];

const DAY = 86_400_000;

/** 时间筛选换算为发布时间下界（Unix 毫秒）；「今天」从本地零点起。 */
export function publishedSince(time: SearchTime, now = Date.now()): number | null {
  if (time === "any") return null;
  if (time === "today") {
    const start = new Date(now);
    start.setHours(0, 0, 0, 0);
    return start.getTime();
  }
  return now - (time === "7d" ? 7 : 30) * DAY;
}

export const filtersActive = (filters: SearchFilters) =>
  filters.scope !== "all" || filters.feedIds.length > 0 || filters.time !== "any";
