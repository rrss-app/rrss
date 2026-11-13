import { settingsQueryPolicy } from "./queryPolicy";
import { queryOptions } from "@tanstack/react-query";
import { api, type RetentionPage } from "../lib/api";
import { isTauri } from "../lib/preview";

// 浏览器预览只展示结构，不代表本机数据。
const PREVIEW_RETENTION: RetentionPage = {
  days: 0, default_days: 90, custom_feeds: 0, preview: "", preview_hint: "", can_run: false,
  confirm_title: "立即清理旧文章正文", confirm: [], confirm_ok: "清理",
};
export const retentionKeys = {
  page: ["storage", "retention"] as const,
  write: ["storage", "retention", "write"] as const,
};
/** 预览要按条件扫一遍文章和引用：只在打开存储页时读。 */
export function retentionQueryOptions() {
  return queryOptions({
    queryKey: retentionKeys.page,
    queryFn: ({ signal }) => {
      void signal; // IPC 不可取消；消费 signal 让 query 取消隔离迟到快照。
      return isTauri() ? api.retentionPage() : Promise.resolve(PREVIEW_RETENTION);
    },
    ...settingsQueryPolicy,
  });
}
