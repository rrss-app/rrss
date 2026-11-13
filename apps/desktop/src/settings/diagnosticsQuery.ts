import { settingsQueryPolicy } from "./queryPolicy";
import { queryOptions, type QueryClient } from "@tanstack/react-query";
import { api, type DiagnosticsPage } from "../lib/api";
import { i18n } from "../i18n";
import { isTauri } from "../lib/preview";

const previewDiagnostics = (): DiagnosticsPage => {
  const t = (key: "vec" | "stats" | "db" | "ai") => i18n.t(`diag.preview.${key}`, { ns: "settingsSystem" });
  return { scheduler: null, vec: t("vec"), stats: t("stats"), db: t("db"), ai: t("ai"), log: null };
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
