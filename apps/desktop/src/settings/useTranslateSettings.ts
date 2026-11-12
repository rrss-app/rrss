import { useEffect, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { readGate, useEditReport, useSettingsWrite, type SettingsEditState } from "./editState";
import { sameTranslateDraft, translateDraft, translateKeys, translateMutationOptions, translateQueryOptions,
  type TranslateAction, type TranslateDraft, type TranslateStatus } from "./translateQuery";

type Editor = { baseline: TranslateDraft; draft: TranslateDraft };
/** 点选即存的字段（阶段 19-7）；其余文字字段在保存栏。 */
export type InstantKey = "enabled" | "backend" | "target_lang";

export function useTranslateSettings(onEditStateChange?: (state: SettingsEditState | null) => void) {
  const client = useQueryClient();
  const { writing, run: write } = useSettingsWrite(translateMutationOptions(client));
  const [pending, setPending] = useState<TranslateAction | "pick" | null>(null);
  const busy = writing || pending !== null;
  const query = useQuery({ ...translateQueryOptions(), ...readGate(busy) });
  const [editor, setEditor] = useState<Editor | null>(() => {
    const page = query.data;
    return page ? { baseline: translateDraft(page), draft: translateDraft(page) } : null;
  });
  const [status, setStatus] = useState<TranslateStatus | null>(null);
  const dirty = editor !== null && !sameTranslateDraft(editor.baseline, editor.draft);

  // 查询只拥有宿主快照。新快照可以更新未编辑表单，绝不覆盖脏草稿。
  useEffect(() => {
    if (!query.data || busy || dirty) return;
    const next = translateDraft(query.data);
    setEditor((old) => old && sameTranslateDraft(old.baseline, next) ? old : { baseline: next, draft: next });
  }, [query.data, busy, dirty]);

  const change = <K extends keyof TranslateDraft>(key: K, value: TranslateDraft[K]) => {
    if (busy) return;
    setEditor((old) => old ? { ...old, draft: { ...old.draft, [key]: value } } : old);
    setStatus(null);
  };

  /** 写入 `submit`，成功后以回读为基线；`keep` 决定草稿里哪些未保存的输入留下。 */
  const commit = async (action: TranslateAction, label: TranslateAction | "pick", submit: TranslateDraft,
    keep: (baseline: TranslateDraft, old: TranslateDraft) => TranslateDraft) => {
    if (busy || client.isMutating({ mutationKey: translateKeys.write })) return false;
    setPending(label);
    setStatus(null);
    const result = await write({ action, draft: submit });
    setPending(null);
    if (result.status !== "done" || !result.mounted) return false;
    const { saved, page, submitted } = result.data;
    if (saved) {
      const baseline = page ? translateDraft(page) : submitted;
      setEditor((old) => ({ baseline, draft: old ? keep(baseline, old.draft) : baseline }));
    }
    const ok = saved && result.data.status.tone === "ok";
    // 点选即存成功时不报「已保存」：控件本身就是结果，只在失败时说明。
    setStatus(ok && label === "pick" ? null : result.data.status);
    return ok;
  };

  const run = (action: TranslateAction) => editor
    ? commit(action, action, editor.draft, (baseline) => baseline)
    : Promise.resolve(false);

  /** 点选类只提交这一项（其余取已保存的值），保存栏里没保存的文字输入原样保留。 */
  const pick = <K extends InstantKey>(key: K, value: TranslateDraft[K]) => {
    if (!editor || busy) return;
    const edited = (Object.keys(editor.draft) as (keyof TranslateDraft)[])
      .filter((k) => k !== key && editor.draft[k] !== editor.baseline[k]);
    void commit("save", "pick", { ...editor.baseline, [key]: value }, (baseline, old) => {
      const next: TranslateDraft = { ...baseline };
      for (const k of edited) (next as Record<string, unknown>)[k] = old[k];
      return next;
    });
  };

  useEditReport(onEditStateChange, "翻译设置", dirty, busy, () => run("save"));

  /** 读取失败时的重试；正常情况下切进来就按新鲜度重读，不再给「重新读取」按钮。 */
  const reload = () => {
    if (busy) return;
    setStatus(null);
    void query.refetch();
  };

  const discard = () => {
    if (busy) return;
    setEditor((old) => old ? { ...old, draft: old.baseline } : old);
    setStatus(null);
  };

  return { query, draft: editor?.draft, baseline: editor?.baseline, dirty, busy, pending, status, change, pick, run, reload, discard };
}
