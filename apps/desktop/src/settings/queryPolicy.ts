export const settingsQueryPolicy = {
  staleTime: 30_000,
  gcTime: 5 * 60_000,
  retry: false,
  retryOnMount: false,
  refetchOnMount: true,
  refetchOnWindowFocus: false,
  refetchOnReconnect: false,
  networkMode: "always" as const,
};
