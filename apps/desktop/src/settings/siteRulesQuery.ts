import { queryOptions, type QueryClient } from "@tanstack/react-query";
import { api } from "../lib/api";
import { isTauri } from "../lib/preview";
import type { SiteRulesPage } from "../bindings";

export const siteRulesKey = ["settings", "site-rules"] as const;
export const siteRulesWriteKey = ["settings", "web-write"] as const;
export const siteRulesOptions = () => queryOptions({
  queryKey: siteRulesKey,
  queryFn: ({ signal }) => {
    void signal;
    return isTauri() ? api.siteRulesPage() : Promise.resolve<SiteRulesPage>({ users: [], builtin_count: 1780, source: "fivefilters/ftr-site-config", commit: "045f380" });
  },
  staleTime: 30_000, gcTime: 300_000, retry: false, retryOnMount: false,
  refetchOnWindowFocus: false, refetchOnReconnect: false, networkMode: "always" as const,
});
export const siteRulesMutation = (client: QueryClient) => ({
  mutationKey: siteRulesWriteKey,
  mutationFn: async (action: () => Promise<void>) => {
    await client.cancelQueries({ queryKey: siteRulesKey });
    await action();
    // Do not turn a successful write into a failed save when refreshing fails.
    await client.invalidateQueries({ queryKey: siteRulesKey, refetchType: "none" });
    await client.fetchQuery(siteRulesOptions()).catch(() => undefined);
  },
});
