import { queryOptions } from "@tanstack/react-query";
import { api, type BackupPage } from "../lib/api";
import { isTauri } from "../lib/preview";

// 浏览器预览只展示结构，不代表本机数据。
const PREVIEW_BACKUP: BackupPage = {
  last_manual: "未连接本机数据。",
  schedule: "off",
  auto_summary: "升级前自动 · 定时关闭",
  entries: [],
  restore_notice: null,
  rules: 0,
  interests: 0,
  memories: 0,
};
export const backupKeys = {
  page: ["storage", "backup"] as const,
  write: ["storage", "backup", "write"] as const,
};
/** 只读设置与备份目录清单，很快；与占用统计分开，打开页面不必等扫库。 */
export function backupQueryOptions() {
  return queryOptions({
    queryKey: backupKeys.page,
    queryFn: ({ signal }) => {
      void signal; // IPC 不可取消；消费 signal 让 query 取消隔离迟到快照。
      return isTauri() ? api.backupPage() : Promise.resolve(PREVIEW_BACKUP);
    },
    staleTime: 30_000, gcTime: 5 * 60_000,
    retry: false, retryOnMount: false, refetchOnMount: true,
    refetchOnWindowFocus: false, refetchOnReconnect: false, networkMode: "always",
  });
}
