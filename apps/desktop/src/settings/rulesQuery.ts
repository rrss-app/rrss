import { settingsQueryPolicy } from "./queryPolicy";
import { queryOptions } from "@tanstack/react-query";
import { api } from "../lib/api";
import { isTauri, PREVIEW_RULES } from "../lib/preview";

export const rulesKeys = {
  pages: ["rules", "page"] as const,
  page: (filter: string) => ["rules", "page", filter] as const,
  write: ["rules", "write"] as const,
};

export const rulesQueryOptions = (filter: string) => queryOptions({
  queryKey: rulesKeys.page(filter),
  queryFn: ({ signal }) => {
    void signal; // IPC 不可取消，但 TanStack 必须丢弃已取消的迟到结果。
    return isTauri() ? api.rules(filter === "all" ? "" : filter) : Promise.resolve(PREVIEW_RULES);
  },
  ...settingsQueryPolicy,
});
