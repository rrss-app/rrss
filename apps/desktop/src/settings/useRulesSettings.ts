import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type NewRuleSpec, type RulesPage } from "../lib/api";
import { formatNotice } from "../lib/notice";
import { readGate, settingsChanged, useEditReport, useSettingsWrite, type SettingsEditState } from "./editState";
import { rulesKeys, rulesQueryOptions } from "./rulesQuery";
import { useTranslation } from "../i18n";

const emptySpec = (): NewRuleSpec => ({
  polarity: "like", dimension: "title_kw", pattern: "", weight: 10, scope_kind: "global", scope_id: 0,
});
type Write = { label: string; run: () => Promise<unknown>; focus?: boolean };
type WriteResult = { ok: boolean; tone: "ok" | "err"; text: string; value?: unknown; page?: RulesPage };

export function useRulesSettings(onChanged?: () => void, onEditStateChange?: (state: SettingsEditState | null) => void) {
  const { t } = useTranslation("settingsContent");
  const client = useQueryClient();
  const [filter, setFilter] = useState("all");
  const changed = useRef(onChanged);
  changed.current = onChanged;
  const { writing: busy, run: write } = useSettingsWrite({
    mutationKey: rulesKeys.write,
    mutationFn: async ({ label, run, focus = true }: Write): Promise<WriteResult> => {
      await client.cancelQueries({ queryKey: rulesKeys.pages });
      let value: unknown;
      try { value = await run(); }
      catch (error) { return { ok: false, tone: "err", text: formatNotice(t("rules.write.failed", { label }), error) }; }
      await settingsChanged(client);
      let page: RulesPage | undefined;
      let tone: "ok" | "err" = "ok";
      let text = t("rules.write.done", { label });
      try { page = await client.fetchQuery(rulesQueryOptions(filter)); }
      catch (error) {
        tone = "err";
        text += " " + formatNotice(t("rules.write.refreshFailed"), error);
      }
      if (focus) changed.current?.();
      return { ok: true, tone, text, value, page };
    },
  });
  const query = useQuery({ ...rulesQueryOptions(filter), ...readGate(busy) });
  const [editId, setEditId] = useState(0);
  const [spec, setSpec] = useState<NewRuleSpec>(emptySpec);
  const [formBaseline, setFormBaseline] = useState(() => JSON.stringify(emptySpec()));
  const [threshold, setThreshold] = useState(() => query.data ? String(query.data.hide_threshold) : "");
  const [thresholdBaseline, setThresholdBaseline] = useState(threshold);
  const [status, setStatus] = useState<{ tone: "ok" | "err"; text: string } | null>(null);
  const thresholdDirty = threshold !== thresholdBaseline;
  const formDirty = JSON.stringify(spec) !== formBaseline;
  useEditReport(onEditStateChange, t("rules.editLabel"), thresholdDirty || formDirty, busy);
  useEffect(() => {
    // 回读失败或切换到已失效的筛选缓存时，旧快照不能覆盖已提交基线。
    if (!query.isSuccess || query.isFetching || query.isStale || busy || thresholdDirty) return;
    const next = String(query.data.hide_threshold);
    setThreshold(next);
    setThresholdBaseline(next);
  }, [query.data, query.isSuccess, query.isFetching, query.isStale, busy, thresholdDirty]);

  const run = async (label: string, action: () => Promise<unknown>, onSuccess?: (value?: unknown, page?: RulesPage) => void, focus = true) => {
    setStatus(null);
    const result = await write({ label, run: action, focus });
    if (result.status !== "done" || !result.mounted) return;
    setStatus({ tone: result.data.tone, text: result.data.text });
    if (result.data.ok) onSuccess?.(result.data.value, result.data.page);
  };

  const begin = (id: number, next: NewRuleSpec) => {
    setEditId(id);
    setSpec(next);
    setFormBaseline(JSON.stringify(next));
    setStatus(null);
  };
  const closeForm = () => { setEditId(0); setSpec(emptySpec()); setFormBaseline(JSON.stringify(emptySpec())); };
  const changeFilter = (value: string) => { if (!busy) { setFilter(value); setStatus(null); } };
  const save = () => {
    const payload = { ...spec, pattern: spec.pattern.trim(), weight: Number.isFinite(spec.weight) && spec.weight !== 0 ? spec.weight : 10,
      scope_id: spec.scope_kind === "global" ? 0 : spec.scope_id };
    if (!payload.pattern) { setStatus({ tone: "err", text: t("rules.write.patternRequired") }); return; }
    void run(editId > 0 ? t("rules.action.save") : t("rules.action.create"), () => editId > 0 ? api.updateRule(editId, payload) : api.createRule(payload), closeForm);
  };
  const saveThreshold = (nextThreshold = threshold) => {
    if (nextThreshold === thresholdBaseline || busy) return;
    const value = Number(nextThreshold);
    if (!nextThreshold.trim() || !Number.isFinite(value)) {
      setStatus({ tone: "err", text: t("rules.write.thresholdInvalid") });
      return;
    }
    void run(t("rules.action.saveThreshold"), () => api.setSetting("hide_threshold", String(value)), (_value, page) => {
      // 写入已成功：优先采用回读值；回读失败则保留已提交值，不伪造规则快照。
      const next = String(page?.hide_threshold ?? value);
      setThreshold(next);
      setThresholdBaseline(next);
    });
  };
  const refresh = () => {
    if (busy) return;
    setStatus(null);
    void query.refetch();
  };
  return { query, filter, changeFilter, editId, spec, setSpec, begin, closeForm, save, threshold, setThreshold, saveThreshold, status, busy,
    run, refresh };
}
