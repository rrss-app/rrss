import { settingsQueryPolicy } from "./queryPolicy";
import { queryOptions } from "@tanstack/react-query";
import { api, type BackupPage } from "../lib/api";
import { i18n } from "../i18n";
import { isTauri } from "../lib/preview";

// 浏览器预览只展示结构，不代表本机数据。
const previewBackup = (): BackupPage => ({
  last_manual: i18n.t("backup.preview.lastManual", { ns: "settingsSystem" }),
  schedule: "off",
  auto_summary: i18n.t("backup.preview.autoSummary", { ns: "settingsSystem" }),
  entries: [],
  restore_notice: null,
  rules: 0,
  interests: 0,
  memories: 0,
});
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
      return isTauri() ? api.backupPage() : Promise.resolve(previewBackup());
    },
    ...settingsQueryPolicy,
  });
}
