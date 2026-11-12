import { useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type MemoryView } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { memoryKeys, memoryQueryOptions } from "./memoryQuery";
import { readGate, settingsChanged, useEditReport, useSettingsWrite, type SettingsEditState } from "./editState";

type Status = { tone: "ok" | "err"; text: string };
type Operation = { label: string; write: () => Promise<unknown>; filter: string; interests?: boolean; readOnly?: boolean; quiet?: boolean };
type Ask = (title: string, message: string, onOk: () => void) => void;

export function useMemorySettings(onAsk: Ask, onChanged?: () => void, onEditStateChange?: (state: SettingsEditState | null) => void) {
  const client = useQueryClient();
  const changed = useRef(onChanged);
  changed.current = onChanged;
  const { writing: busy, run: write } = useSettingsWrite({
    mutationKey: memoryKeys.write,
    mutationFn: async ({ label: title, write, filter: currentFilter, interests, readOnly, quiet }: Operation) => {
      if (!readOnly) await client.cancelQueries({ queryKey: memoryKeys.pages });
      let value: unknown;
      try { value = await write(); }
      catch (error) {
        // 失败常说明列表已过期（如记忆已在别处确认）；标记过期，解锁后按数据库重读，不当作写入完成。
        if (!readOnly) await client.invalidateQueries({ queryKey: memoryKeys.pages, refetchType: "none" });
        return { completed: false as const, status: { tone: "err" as const, text: formatNotice(title + "失败：", error) } };
      }
      let nextStatus: Status | null = quiet ? null : { tone: "ok", text: title + "已完成。" };
      if (!readOnly) {
        // 写期间的新读取也须隔离；先失效再 fetch，不能命中写前 fresh 快照。
        await settingsChanged(client);
        // 成功写入即通知排序，即使随后回读失败或当前分类已卸载。
        if (interests) changed.current?.();
        const filters = interests && currentFilter !== "" ? ["", currentFilter] : [currentFilter];
        const reads = await Promise.allSettled(filters.map((filter) => client.fetchQuery(memoryQueryOptions(filter))));
        const failedRead = reads.find((read): read is PromiseRejectedResult => read.status === "rejected");
        if (failedRead) nextStatus = { tone: "err", text: title + "已完成；" + formatNotice("记忆列表没能刷新，显示的还是之前的内容；刚才的操作已完成，重新读取即可：", failedRead.reason) };
      }
      return { completed: true as const, status: nextStatus, value };
    },
  });
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState("");
  const [sub, setSub] = useState<"memory" | "interest">("memory");
  const query = useQuery({ ...memoryQueryOptions(filter), ...readGate(busy) });
  const interestQuery = useQuery({ ...memoryQueryOptions(""), ...readGate(busy) });
  const [label, setLabel] = useState("");
  const [editing, setEditing] = useState<{ baseline: MemoryView; content: string } | null>(null);
  const [status, setStatus] = useState<Status | null>(null);
  const editDirty = !!editing && editing.content !== editing.baseline.content;
  useEditReport(onEditStateChange, "记忆与兴趣", !!label || editDirty, busy);

  const run = async <T,>(title: string, work: () => Promise<T>, options: { onSuccess?: (value: T) => void; interests?: boolean; readOnly?: boolean; quiet?: boolean } = {}) => {
    if (busy) return;
    setStatus(null);
    const result = await write({ label: title, write: work, filter, ...options });
    if (result.status !== "done" || !result.mounted) return;
    if (result.data.completed) options.onSuccess?.(result.data.value as T);
    setStatus(result.data.status);
  };

  const replaceEditor = (replace: () => void) => {
    if (busy) return;
    // 确认框关闭前可能已开始别的写入，确认时再查一次。
    const proceed = () => { if (!client.isMutating({ mutationKey: memoryKeys.write })) replace(); };
    if (editDirty) onAsk("放弃记忆修改", "当前记忆有未保存的修改，放弃后继续？", proceed);
    else proceed();
  };
  const beginEdit = (memory: MemoryView) => {
    if (editing?.baseline.id === memory.id) return;
    replaceEditor(() => setEditing({ baseline: memory, content: memory.content }));
  };
  const submitSearch = () => {
    const nextFilter = search.trim();
    if (busy) return;
    if (nextFilter === filter) { void query.refetch(); return; }
    replaceEditor(() => { setEditing(null); setFilter(nextFilter); });
  };
  const refresh = async () => {
    if (busy) return;
    const result = await (sub === "interest" ? interestQuery.refetch() : query.refetch());
    if (result.isSuccess) setStatus(null);
  };
  const addInterest = () => {
    const text = label.trim();
    if (text) return run("添加兴趣", () => api.addInterest(text), { interests: true, onSuccess: () => setLabel("") });
  };
  const saveEdit = () => {
    if (editing?.content.trim()) return run("保存记忆", () => api.editMemory(editing.baseline.id, editing.content), { onSuccess: () => setEditing(null) });
  };
  // 若后台刷新移除了正在编辑的条目，仍保留其编辑器，直到明确保存或取消。
  const memories = query.data?.memories ?? [];
  const visibleMemories = editing && !memories.some((memory) => memory.id === editing.baseline.id)
    ? [editing.baseline, ...memories] : memories;
  return { query, interestQuery, sub, setSub, search, setSearch, filter, submitSearch, refresh, label, setLabel, editing, setEditing, beginEdit, saveEdit, addInterest, status, busy, run, visibleMemories };
}
