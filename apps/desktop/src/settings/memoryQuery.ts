import { settingsQueryPolicy } from "./queryPolicy";
import { queryOptions, type QueryClient } from "@tanstack/react-query";
import { api } from "../lib/api";
import { isTauri, PREVIEW_MEMORY } from "../lib/preview";

export const memoryKeys = {
  pages: ["memory", "page"] as const,
  page: (query: string) => ["memory", "page", query] as const,
  write: ["memory", "write"] as const,
};

export function memoryQueryOptions(query = "") {
  return queryOptions({
    queryKey: memoryKeys.page(query),
    queryFn: ({ signal }) => {
      void signal; // IPC 不可取消；消费 signal 让 query 取消隔离迟到快照。
      return isTauri() ? api.memory(query) : Promise.resolve({
        ...PREVIEW_MEMORY,
        memories: PREVIEW_MEMORY.memories.filter((memory) => memory.content.includes(query)),
      });
    },
    ...settingsQueryPolicy,
  });
}

export function prefetchMemory(client: QueryClient) {
  if (!client.isMutating({ mutationKey: memoryKeys.write })) {
    return client.prefetchQuery(memoryQueryOptions());
  }
}
