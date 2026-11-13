import { useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { FeedTree, FeedView, FolderView } from "../lib/api";
import { feedKeys, feedQueryOptions } from "../lib/feedQuery";
import { formatNotice } from "../lib/notice";
import { useTranslation } from "../i18n";
import { readGate, settingsChanged, useEditReport, useSettingsWrite, type SettingsEditState } from "./editState";

type Editor<T> = { baseline: T; draft: T };
type WriteResult = { tree?: FeedTree; message?: string };
type Operation = { label: string; write: () => Promise<WriteResult>; readOnly: boolean };
export function useSubscribeSettings(
  onChanged?: (tree?: FeedTree) => void,
  onEditStateChange?: (state: SettingsEditState | null) => void,
) {
  const { t } = useTranslation("settingsContent");
  const client = useQueryClient();
  const changed = useRef(onChanged);
  changed.current = onChanged;
  const { writing: busy, run: write } = useSettingsWrite({
    mutationKey: feedKeys.write,
    mutationFn: async ({ label, write, readOnly }: Operation) => {
      // 底层 IPC 不可取消；取消查询仅阻止旧结果重新覆盖共享快照。
      if (!readOnly) await client.cancelQueries({ queryKey: feedKeys.tree, exact: true });
      let result: WriteResult;
      try { result = await write(); }
      catch (error) { return { completed: false, snapshotFresh: true, status: { tone: "err" as const, text: formatNotice(t("subscribe.write.failed", { label }), error) } }; }
      let status = { tone: "ok" as "ok" | "err", text: result.message ?? t("subscribe.write.done", { label }) };
      if (readOnly) return { completed: true, snapshotFresh: true, status };
      await settingsChanged(client);
      let tree = result.tree;
      let snapshotFresh = !!tree;
      if (tree) client.setQueryData(feedKeys.tree, tree);
      else {
        try { tree = await client.fetchQuery(feedQueryOptions()); snapshotFresh = true; }
        catch (error) { status = { tone: "err", text: status.text + " " + formatNotice(t("subscribe.write.refreshFailed"), error) }; }
      }
      // 只有拿到权威树后才刷新依赖方；回读失败时避免传播旧树。
      if (snapshotFresh) changed.current?.(tree);
      return { completed: true, snapshotFresh, status };
    },
  });
  const query = useQuery({ ...feedQueryOptions(), ...readGate(busy) });
  const [url, setUrl] = useState("");
  const [folderName, setFolderName] = useState("");
  // 草稿放在分类中，订阅移动文件夹导致行组件重挂载时也不丢输入。
  const [feeds, setFeeds] = useState<Record<number, Editor<FeedView>>>({});
  const [folders, setFolders] = useState<Record<number, Editor<FolderView>>>({});
  const [status, setStatus] = useState<{ tone: "ok" | "err"; text: string } | null>(null);
  const dirty = !!url || !!folderName || Object.values(feeds).some(({ baseline: b, draft: d }) =>
    b.title !== d.title || b.url !== d.url || b.folder_id !== d.folder_id || b.retention_days !== d.retention_days) ||
    Object.values(folders).some(({ baseline, draft }) => baseline.name !== draft.name);
  useEditReport(onEditStateChange, t("subscribe.editLabel"), dirty, busy);

  const run = async (label: string, work: Operation["write"], onSuccess?: () => void, readOnly = false) => {
    if (busy) return;
    if (!readOnly && query.isError) {
      setStatus({ tone: "err", text: t("subscribe.write.stale") });
      return;
    }
    setStatus(null);
    const result = await write({ label, write: work, readOnly });
    if (result.status !== "done" || !result.mounted) return;
    if (result.data.completed && result.data.snapshotFresh) onSuccess?.();
    setStatus(result.data.status);
  };
  const editFeed = (id: number, editor?: Editor<FeedView>) => setFeeds((old) => {
    const next = { ...old }; if (editor) next[id] = editor; else delete next[id]; return next;
  });
  const editFolder = (id: number, editor?: Editor<FolderView>) => setFolders((old) => {
    const next = { ...old }; if (editor) next[id] = editor; else delete next[id]; return next;
  });
  const reload = async () => {
    if (busy) return;
    const result = await query.refetch();
    if (result.isSuccess) { setStatus(null); changed.current?.(result.data); }
  };
  return { query, busy, dirty, status, url, setUrl, folderName, setFolderName, feeds, folders, editFeed, editFolder, run, reload };
}
export type SubscribeSettings = ReturnType<typeof useSubscribeSettings>;
