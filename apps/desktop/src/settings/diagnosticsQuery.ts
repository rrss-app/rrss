import { settingsQueryPolicy } from "./queryPolicy";
import { queryOptions, type QueryClient } from "@tanstack/react-query";
import { api, type DiagnosticsPage } from "../lib/api";
import { isTauri } from "../lib/preview";

// 浏览器预览：形状齐全的空页，诊断页按预览显示说明文字，不展示这些数。
const previewDiagnostics = (): DiagnosticsPage => ({
  scheduler: null, vectors: null, log: null,
  db: { path: "", size_bytes: 0, user_version: 0, journal_mode: "", fts_rows: 0 },
  vec: { available: false, version: null, reason: null },
  ai: { job_tokens: 0, budget_tokens: 0, cache_rows: 0, chat_input_tokens: 0, chat_output_tokens: 0, chat_cost: 0 },
});
export const diagnosticsKeys = {
  page: ["diagnostics", "page"] as const,
  write: ["diagnostics", "write"] as const,
};
export function diagnosticsQueryOptions() {
  return queryOptions({
    queryKey: diagnosticsKeys.page,
    queryFn: ({ signal }) => {
      void signal; // IPC 不可取消；消费 signal 让 query 取消隔离迟到快照。
      return isTauri() ? api.diagnostics() : Promise.resolve(previewDiagnostics());
    },
    ...settingsQueryPolicy,
  });
}
export function prefetchDiagnostics(client: QueryClient) {
  if (!client.isMutating({ mutationKey: diagnosticsKeys.write })) {
    return client.prefetchQuery(diagnosticsQueryOptions());
  }
}
