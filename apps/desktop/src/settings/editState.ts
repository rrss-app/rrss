import { useCallback, useEffect, useRef, useState } from "react";
import { useIsMutating, useMutation, useQueryClient, type QueryClient, type UseMutationOptions } from "@tanstack/react-query";

/**
 * 设置分类的通用离开保护。`save` 让离开确认可以「保存并离开」：成功返回 true，
 * 失败时由这一块自己显示原因并返回 false，离开取消。
 */
export type SettingsEditState = { label: string; dirty: boolean; busy: boolean; save?: () => Promise<boolean> };

type Report = (state: SettingsEditState | null) => void;

/**
 * 同一分类里几块各自上报离开状态，合并成一份交给设置壳：忙的优先，其次有未保存修改的。
 * 每块有修改的都能保存时，合并后的 `save` 依次保存，任一失败即停。
 */
export function mergeEditStates(states: Array<SettingsEditState | null>): SettingsEditState | null {
  const busy = states.find((s) => s?.busy);
  if (busy) return busy;
  const dirty = states.filter((s): s is SettingsEditState => !!s?.dirty);
  if (!dirty.length) return states.find((s) => s) ?? null;
  if (dirty.length === 1) return dirty[0];
  const saves = dirty.map((s) => s.save);
  return {
    label: dirty[0].label, dirty: true, busy: false,
    save: saves.every(Boolean) ? async () => {
      for (const save of saves) if (!(await save!())) return false;
      return true;
    } : undefined,
  };
}

/**
 * 给每一块一个稳定的上报函数（按名字缓存，子组件可以放进 ref / effect）。
 * `busy` 记着各块是否在写，页面可以据此让别的块暂停写入。
 */
export function useMergedEditState(onEditStateChange?: Report) {
  const notify = useRef(onEditStateChange);
  notify.current = onEditStateChange;
  const states = useRef(new Map<string, SettingsEditState | null>());
  const reporters = useRef(new Map<string, Report>());
  const [busy, setBusy] = useState<Record<string, boolean>>({});
  const report = useCallback((part: string): Report => {
    let fn = reporters.current.get(part);
    if (!fn) {
      fn = (state) => {
        states.current.set(part, state);
        setBusy((old) => (!!old[part] === !!state?.busy ? old : { ...old, [part]: !!state?.busy }));
        notify.current?.(mergeEditStates([...states.current.values()]));
      };
      reporters.current.set(part, fn);
    }
    return fn;
  }, []);
  return { report, busy };
}

/** 把一块的离开状态上报给设置壳，卸载时撤销。`save` 可选，给了离开确认才有「保存并离开」。 */
export function useEditReport(onEditStateChange: Report | undefined, label: string, dirty: boolean, busy: boolean,
  save?: () => Promise<boolean>) {
  const notify = useRef(onEditStateChange);
  notify.current = onEditStateChange;
  const saveRef = useRef(save);
  saveRef.current = save;
  const canSave = !!save;
  useEffect(() => {
    notify.current?.({ label, dirty, busy, save: canSave ? () => saveRef.current?.() ?? Promise.resolve(false) : undefined });
  }, [label, dirty, busy, canSave]);
  useEffect(() => () => notify.current?.(null), []);
}

export type WriteOutcome<T> =
  | { status: "busy" }
  | { status: "done"; data: T; mounted: boolean }
  | { status: "failed"; error: unknown; mounted: boolean };

/** 设置页的读取门：写入中不读；读失败后不隐式重试，由用户点重新读取。 */
export const readGate = (writing: boolean) => ({ enabled: (q: { state: { status: string } }) => !writing && q.state.status !== "error" });

/** 设置页全部读取的 key 根。新增设置读取时 key 以其中之一开头即可，不必登记依赖关系。 */
const SETTINGS_READS = new Set(["settings", "rules", "memory", "diagnostics", "storage", "feeds"]);

/**
 * 任一设置写入成功后调用：取消并标记全部设置读取过期，打开的页面解锁后自行重读，其余页面下次打开时重读。
 * 写入方在它之后再回读或回写自己的快照。不逐项记录「谁影响谁」，漏记就会显示旧数据。
 */
export async function settingsChanged(client: QueryClient) {
  const filters = { predicate: (q: { queryKey: readonly unknown[] }) => SETTINGS_READS.has(String(q.queryKey[0])) };
  // IPC 不可取消；取消只让写入前发出的读取不能再覆盖失效标记。
  await client.cancelQueries(filters);
  await client.invalidateQueries({ ...filters, refetchType: "none" });
}

/**
 * 设置页共用的写入生命周期：同一写入通道（mutationKey）跨组件互斥，完成后回收 mutation。
 * 共享缓存的回写在 mutationFn 里完成、不依赖组件仍挂载；`mounted` 告诉调用方还能不能更新自己的草稿和提示。
 */
export function useSettingsWrite<TVariables, TData>(
  options: UseMutationOptions<TData, Error, TVariables> & { mutationKey: readonly unknown[] },
) {
  const client = useQueryClient();
  // 变量可能含密钥：完成且无观察者后立即回收；写入不重试。
  const mutation = useMutation({ retry: false, networkMode: "always", gcTime: 0, ...options });
  const writing = useIsMutating({ mutationKey: options.mutationKey }) > 0;
  const mounted = useRef(true);
  // StrictMode 会模拟卸载再挂载，挂载时要重新置真。
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const run = async (variables: TVariables): Promise<WriteOutcome<TData>> => {
    if (client.isMutating({ mutationKey: options.mutationKey })) return { status: "busy" };
    try {
      const data = await mutation.mutateAsync(variables);
      return { status: "done", data, mounted: mounted.current };
    } catch (error) {
      return { status: "failed", error, mounted: mounted.current };
    } finally {
      mutation.reset();
    }
  };
  return { writing, run };
}
