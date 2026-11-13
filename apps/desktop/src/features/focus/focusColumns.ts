import { useCallback, useEffect, useRef } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { FeedCountsView, FeedTree } from "../../lib/api";
import { i18n } from "../../i18n";
import { formatNotice } from "../../lib/notice";
import { feedQueryOptions } from "../../lib/feedQuery";
import { settingsMutationOptions, settingsSchemaOptions } from "../../settings/settingsQuery";

const MAX_COLUMNS = 5;

export type FocusColumn = { id: string; name: string; unread: number };
export type ColumnFolder = { id: string; name: string; depth: number; added: boolean };

/** 设置 `focus_columns` 里还在订阅树中的文件夹，去重，最多 5 项。 */
function liveColumns(raw: string, tree: FeedTree): string[] {
  const known = new Set(tree.folders.map((folder) => `folder:${folder.id}`));
  return [...new Set(raw.split(",").map((part) => part.trim()))].filter((id) => known.has(id)).slice(0, MAX_COLUMNS);
}

/** 侧栏栏目：对照订阅树丢掉已删除的文件夹，增删改顺序写 `focus_columns`。 */
export function useFocusColumns(counts: FeedCountsView | null, activeId: string | null, onGone: () => void, notify: (message: string) => void) {
  const client = useQueryClient();
  const schema = useQuery(settingsSchemaOptions());
  const tree = useQuery(feedQueryOptions()).data;
  const save = useMutation(settingsMutationOptions(client));
  const stored = schema.data?.values.find((item) => item.id === "focus_columns")?.value ?? "";
  const live = tree ? liveColumns(stored, tree) : [];
  const liveKey = live.join(",");
  const ready = schema.isSuccess && tree !== undefined;

  const { mutateAsync, isPending: pending } = save;
  const notifyRef = useRef(notify);
  notifyRef.current = notify;
  const write = useCallback((ids: string[]) => {
    void mutateAsync([{ id: "focus_columns", value: ids.join(",") }]).then((result) => {
      if (result.failure) notifyRef.current(formatNotice(i18n.t("columns.saveFailed", { ns: "focus" }), result.failure.error));
    });
  }, [mutateAsync]);

  // 文件夹被删后把它从设置里去掉。同一个设置值只清一次：写入失败时不反复重试。
  const cleaned = useRef<string | null>(null);
  useEffect(() => {
    if (!ready || pending || liveKey === stored || cleaned.current === stored) return;
    cleaned.current = stored;
    write(liveKey ? liveKey.split(",") : []);
  }, [ready, pending, liveKey, stored, write]);

  useEffect(() => {
    if (ready && activeId && !liveKey.split(",").includes(activeId)) onGone();
  }, [ready, activeId, liveKey, onGone]);

  const unread = (id: string) => counts?.folders.find((row) => `folder:${row.id}` === id)?.unread ?? 0;
  const columns: FocusColumn[] = ready ? live.map((id) => ({
    id, name: tree!.folders.find((folder) => `folder:${folder.id}` === id)?.name ?? "", unread: unread(id),
  })) : [];
  const choices: ColumnFolder[] = (tree?.folders ?? []).map((folder) => ({
    id: `folder:${folder.id}`, name: folder.name, depth: folder.indent, added: live.includes(`folder:${folder.id}`),
  }));
  const canEdit = ready && !pending;

  return {
    columns,
    choices,
    canAdd: canEdit && live.length < MAX_COLUMNS,
    add(id: string) {
      if (canEdit && !live.includes(id) && live.length < MAX_COLUMNS && choices.some((item) => item.id === id)) write([...live, id]);
    },
    move(id: string, delta: -1 | 1) {
      const index = live.indexOf(id);
      const next = index + delta;
      if (!canEdit || index < 0 || next < 0 || next >= live.length) return;
      const copy = live.slice();
      copy.splice(index, 1);
      copy.splice(next, 0, id);
      write(copy);
    },
    remove(id: string) { if (canEdit) write(live.filter((item) => item !== id)); },
  };
}

export function columnName(tree: FeedTree | undefined, id: string | null): string {
  if (!tree || !id) return "";
  return tree.folders.find((item) => `folder:${item.id}` === id)?.name ?? "";
}
