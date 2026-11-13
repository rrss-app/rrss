import { useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { i18n, useTranslation } from "../i18n";
import { formatNotice } from "../lib/notice";
import { diagnosticsKeys, diagnosticsQueryOptions } from "./diagnosticsQuery";
import { readGate, settingsChanged, useEditReport, useSettingsWrite, type SettingsEditState } from "./editState";

type OperationId = "embed" | "resetFailed";
type Operation = { id: OperationId; action: () => Promise<unknown>; success: string; indexChanged: boolean };
export function useDiagnosticsSettings(onEditStateChange?: (state: SettingsEditState | null) => void, onChanged?: () => void) {
  const { t } = useTranslation("settingsSystem");
  const client = useQueryClient();
  const changed = useRef(onChanged);
  changed.current = onChanged;
  const { writing, run } = useSettingsWrite({
    mutationKey: diagnosticsKeys.write,
    mutationFn: async ({ id, action, success, indexChanged }: Operation) => {
      await client.cancelQueries({ queryKey: diagnosticsKeys.page, exact: true });
      try { await action(); }
      catch (error) { return { tone: "err" as const, text: formatNotice(i18n.t(`diag.opFailed.${id}`, { ns: "settingsSystem" }), error) }; }
      await settingsChanged(client);
      if (indexChanged) changed.current?.();
      try {
        await client.fetchQuery(diagnosticsQueryOptions());
        return { tone: "ok" as const, text: success };
      } catch (error) {
        return { tone: "err" as const, text: success + " " + formatNotice(i18n.t("diag.refreshFailed", { ns: "settingsSystem" }), error) };
      }
    },
  });
  const [pending, setPending] = useState("");
  const busy = writing || !!pending;
  const query = useQuery({ ...diagnosticsQueryOptions(), ...readGate(busy) });
  const [status, setStatus] = useState<{ tone: "ok" | "err"; text: string } | null>(null);
  useEditReport(onEditStateChange, t("diag.report"), false, busy);

  const maintain = async (id: OperationId, action: () => Promise<unknown>, success: string, indexChanged = false) => {
    if (busy || query.isFetching || client.isMutating({ mutationKey: diagnosticsKeys.write })) return;
    setPending(id);
    setStatus(null);
    const result = await run({ id, action, success, indexChanged });
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
