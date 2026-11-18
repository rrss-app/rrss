import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "../../i18n";
import { api, onThreadTitleEvent, type ThreadView } from "../../lib/api";
import { bindingsQueryOptions, poolQueryOptions } from "../../settings/aiQuery";
import { findEntry, modelName, taskChoice } from "../../settings/modelChoices";
import type { Ask } from "../../settings/SettingsPanel";
import type { ThreadNotice } from "./ThreadList";

export function useThreadTitles(onAsk: Ask, onSettings: () => void, notify: (notice: ThreadNotice) => void, onChanged: () => void) {
  const { t } = useTranslation("chat");
  const client = useQueryClient();
  const [pending, setPending] = useState<ReadonlySet<number>>(() => new Set());
  useEffect(() => {
    let disposed = false;
    const subscriptions: Array<() => void> = [];
    const update = (id: number, active: boolean) => setPending((current) => {
      if (current.has(id) === active) return current;
      const next = new Set(current);
      if (active) next.add(id);
      else next.delete(id);
      return next;
    });
    const callbacks = {
      started: (id: number) => update(id, true),
      finished: (id: number) => update(id, false),
      changed: onChanged,
    };
    for (const event of ["started", "finished", "changed"] as const) {
      void onThreadTitleEvent(event, (id) => { if (!disposed) callbacks[event](id); })
        .then((stop) => { if (disposed) stop(); else subscriptions.push(stop); }, () => undefined);
    }
    return () => { disposed = true; for (const stop of subscriptions) stop(); };
  }, [onChanged]);

  const noModel = () => notify({ message: t("titles.noModel"), actionLabel: t("titles.settings"), onAction: onSettings });
  const generate = async (thread: ThreadView) => {
    if (pending.has(thread.id)) return;
    try {
      const choice = taskChoice(await client.fetchQuery(bindingsQueryOptions()), "chat_title");
      if (!choice) { noModel(); return; }
      const entry = findEntry(client.getQueryData(poolQueryOptions().queryKey), choice);
      onAsk(t("titles.confirm"), t("titles.message", { model: entry ? modelName(entry) : choice.model }), () => {
        void api.generateThreadTitle(thread.id, choice).then((outcome) => {
          if (outcome === "no_model") noModel();
          else if (outcome !== "applied") notify({ message: t(`titles.outcome.${outcome}`) });
        }, () => notify({ message: t("titles.failed") }));
      }, { okLabel: t("titles.generate") });
    } catch {
      notify({ message: t("titles.loadFailed") });
    }
  };
  return { pending, generate };
}
