import { queryOptions } from "@tanstack/react-query";
import { api } from "./api";
import { isTauri, PREVIEW_FEEDS } from "./preview";

// 焦点来源、启动预载和订阅设置共用一份宿主快照。
export const feedKeys = { tree: ["feeds", "tree"] as const, write: ["feeds", "write"] as const };
export const feedQueryOptions = () => queryOptions({
  // 浏览器预览显式选择样例；真实宿主失败绝不回退为样例。
  queryKey: feedKeys.tree, queryFn: ({ signal }) => {
    // 取消只隔离 TanStack 快照；Tauri IPC 本身不支持取消。
    void signal;
    return isTauri() ? api.feeds() : Promise.resolve(PREVIEW_FEEDS);
  },
  staleTime: 30_000, gcTime: 5 * 60_000, retry: false, retryOnMount: false,
  refetchOnMount: true, refetchOnWindowFocus: false, refetchOnReconnect: false, networkMode: "always",
});
