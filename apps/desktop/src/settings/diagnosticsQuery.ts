import { queryOptions, type QueryClient } from "@tanstack/react-query";
import { api, type DiagnosticsPage } from "../lib/api";
import { isTauri } from "../lib/preview";

const PREVIEW_DIAGNOSTICS: DiagnosticsPage = {
  scheduler: null,
  vec: "浏览器预览", stats: "仅展示样例，未读取本机数据", db: "未连接本机数据库", ai: "未连接模型", log: null,
};
export const diagnosticsKeys = {
  page: ["diagnostics", "page"] as const,
  write: ["diagnostics", "write"] as const,
};
export function diagnosticsQueryOptions() {
  return queryOptions({
    queryKey: diagnosticsKeys.page,
    queryFn: ({ signal }) => {
      void signal; // IPC 不可取消；消费 signal 让 query 取消隔离迟到快照。
      return isTauri() ? api.diagnostics() : Promise.resolve(PREVIEW_DIAGNOSTICS);
    },
    staleTime: 30_000, gcTime: 5 * 60_000,
    retry: false, retryOnMount: false, refetchOnMount: true,
    refetchOnWindowFocus: false, refetchOnReconnect: false, networkMode: "always",
  });
}
export function prefetchDiagnostics(client: QueryClient) {
  if (!client.isMutating({ mutationKey: diagnosticsKeys.write })) {
    return client.prefetchQuery(diagnosticsQueryOptions());
  }
}
