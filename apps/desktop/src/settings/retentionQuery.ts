import { settingsQueryPolicy } from "./queryPolicy";
import { queryOptions } from "@tanstack/react-query";
import { api, type RetentionPage } from "../lib/api";
import { isTauri } from "../lib/preview";
import { i18n } from "../i18n";

// 浏览器预览只展示结构，不代表本机数据。
const previewRetention = (): RetentionPage => ({
  days: 0, default_days: 90, custom_feeds: 0, preview: "", preview_hint: "", can_run: false,
  confirm_title: i18n.t("retention.previewConfirmTitle", { ns: "settingsContent" }), confirm: [],
  confirm_ok: i18n.t("retention.previewConfirmOk", { ns: "settingsContent" }),
});
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
      return isTauri() ? api.retentionPage() : Promise.resolve(previewRetention());
    },
    ...settingsQueryPolicy,
  });
}
