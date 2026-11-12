import { useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { formatNotice } from "../lib/notice";
import { diagnosticsKeys, diagnosticsQueryOptions } from "./diagnosticsQuery";
import { readGate, settingsChanged, useEditReport, useSettingsWrite, type SettingsEditState } from "./editState";

type Operation = { label: string; action: () => Promise<unknown>; success: string; indexChanged: boolean };
export function useDiagnosticsSettings(onEditStateChange?: (state: SettingsEditState | null) => void, onChanged?: () => void) {
  const client = useQueryClient();
  const changed = useRef(onChanged);
  changed.current = onChanged;
  const { writing, run } = useSettingsWrite({
    mutationKey: diagnosticsKeys.write,
    mutationFn: async ({ label, action, success, indexChanged }: Operation) => {
      await client.cancelQueries({ queryKey: diagnosticsKeys.page, exact: true });
      try { await action(); }
      catch (error) { return { tone: "err" as const, text: formatNotice(label + "失败：", error) }; }
      await settingsChanged(client);
      if (indexChanged) changed.current?.();
      try {
        await client.fetchQuery(diagnosticsQueryOptions());
        return { tone: "ok" as const, text: success };
      } catch (error) {
        return { tone: "err" as const, text: success + " " + formatNotice("诊断信息没能刷新，显示的还是之前的内容；维护已经执行，刷新状态即可：", error) };
      }
    },
  });
  const [pending, setPending] = useState("");
  const busy = writing || !!pending;
  const query = useQuery({ ...diagnosticsQueryOptions(), ...readGate(busy) });
  const [status, setStatus] = useState<{ tone: "ok" | "err"; text: string } | null>(null);
  useEditReport(onEditStateChange, "诊断维护", false, busy);

  const maintain = async (label: string, action: () => Promise<unknown>, success: string, indexChanged = false) => {
    if (busy || query.isFetching || client.isMutating({ mutationKey: diagnosticsKeys.write })) return;
    setPending(label);
    setStatus(null);
    const result = await run({ label, action, success, indexChanged });
    setPending("");
    if (result.status === "done" && result.mounted) setStatus(result.data);
  };
  const refresh = async () => {
    if (busy || query.isFetching) return;
    const result = await query.refetch();
    if (result.isSuccess) setStatus(null);
  };
  return { query, busy, pending, status, maintain, refresh };
}
