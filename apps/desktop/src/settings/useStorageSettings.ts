import { useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type StorageItemView } from "../lib/api";
import { i18n, useTranslation } from "../i18n";
import { formatNotice } from "../lib/notice";
import { readGate, settingsChanged, useEditReport, useSettingsWrite, type SettingsEditState } from "./editState";
import { storageKeys, storageQueryOptions } from "./storageQuery";
import type { Ask } from "./SettingsPanel";

type Status = { tone: "ok" | "err"; text: string };
/** 会改变检索结果的清理：完成后通知焦点等依赖索引的视图。 */
const INDEX_ACTIONS = new Set(["old_vectors"]);

export function useStorageSettings(
  onAsk: Ask,
  onEditStateChange?: (state: SettingsEditState | null) => void,
  onChanged?: () => void,
) {
  const { t } = useTranslation("settingsSystem");
  const client = useQueryClient();
  const changed = useRef(onChanged);
  changed.current = onChanged;
  const { writing, run: write } = useSettingsWrite({
    mutationKey: storageKeys.write,
    mutationFn: async (item: StorageItemView): Promise<Status> => {
      await client.cancelQueries({ queryKey: storageKeys.page, exact: true });
      let done: string;
      try { done = await api.storageClean(item.action); }
      catch (error) { return { tone: "err", text: formatNotice(i18n.t("storage.cleanFailed", { ns: "settingsSystem", title: item.confirm_title }), error) }; }
      await settingsChanged(client);
      if (INDEX_ACTIONS.has(item.action)) changed.current?.();
      try {
        await client.fetchQuery(storageQueryOptions());
        return { tone: "ok", text: done };
      } catch (error) {
        return { tone: "err", text: done + " " + formatNotice(i18n.t("storage.rescanFailed", { ns: "settingsSystem" }), error) };
      }
    },
  });
  const [pending, setPending] = useState("");
  const busy = writing || !!pending;
  const query = useQuery({ ...storageQueryOptions(), ...readGate(busy) });
  const [status, setStatus] = useState<Status | null>(null);
  useEditReport(onEditStateChange, t("storage.report"), false, busy);

  const run = async (item: StorageItemView) => {
    if (client.isMutating({ mutationKey: storageKeys.write })) return;
    setPending(item.action);
    setStatus(null);
    const result = await write(item);
    setPending("");
    if (result.status === "done" && result.mounted) setStatus(result.data);
  };
  const clean = (item: StorageItemView) => {
    if (!item.action || busy || query.isFetching) return;
    onAsk(item.confirm_title, item.confirm.join("\n"), () => { void run(item); });
  };
  const refresh = async () => {
    if (busy || query.isFetching) return;
    const result = await query.refetch();
    if (result.isSuccess) setStatus(null);
  };
  return { query, busy, pending, status, clean, refresh };
}
