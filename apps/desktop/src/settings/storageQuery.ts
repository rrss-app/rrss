import { queryOptions } from "@tanstack/react-query";
import { api, type StoragePage } from "../lib/api";
import { isTauri } from "../lib/preview";

// 浏览器预览只展示结构，不代表本机数据。
const PREVIEW_STORAGE: StoragePage = {
  data_dir: "浏览器预览",
  total: "—",
  summary: "未连接本机数据",
  groups: [],
  scanned_at: 0,
};
export const storageKeys = {
  page: ["storage", "page"] as const,
  write: ["storage", "write"] as const,
};
/** 统计要扫一遍库与数据目录：不进启动预载，只在打开存储页或点重新统计时读取。 */
export function storageQueryOptions() {
  return queryOptions({
    queryKey: storageKeys.page,
    queryFn: ({ signal }) => {
      void signal; // IPC 不可取消；消费 signal 让 query 取消隔离迟到快照。
      return isTauri() ? api.storagePage() : Promise.resolve(PREVIEW_STORAGE);
    },
    staleTime: 30_000, gcTime: 5 * 60_000,
    retry: false, retryOnMount: false, refetchOnMount: true,
    refetchOnWindowFocus: false, refetchOnReconnect: false, networkMode: "always",
  });
}
