import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { ModelChoice, ModelPoolView, ProviderView } from "../lib/api";
import { fuzzyMatch } from "../lib/fuzzy";
import { formatNotice } from "../lib/notice";
import { aiKeys, aiMutationOptions, bindingsQueryOptions } from "./aiQuery";
import { ChipBar } from "./chips";
import { Dialog } from "./Dialog";
import { useSettingsWrite, type SettingsEditState } from "./editState";
import { choiceKey, modelName, parseChoice, sameChoice } from "./modelChoices";
import type { Ask } from "./SettingsPanel";
import { MenuSelect } from "../components/MenuSelect";
import { locale } from "../i18n";

type Note = { tone: "ok" | "err"; text: string };

/** 一次新勾选超过这么多先确认：模型会出现在所有选模型的地方。 */
export const BULK_CONFIRM = 50;

const fetchedLabel = (at: number) => new Date(at).toLocaleString(locale(), { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" });

/**
 * 添加模型（阶段 19-7）：按连接切换；勾选只改草稿，底部「应用」一次写入，左侧写明将加入 / 移出几个。
 * 取消勾选的模型正被用途使用时，应用前先问改用哪个模型。「重新获取」失败时目录不变，仍可勾选或手填。
 */
export function AddModelsDialog({ providers, pool, initial, note: firstNote, onAsk, onClose, onApplied, onEditStateChange }: {
  providers: ProviderView[];
  pool: ModelPoolView | undefined;
  initial: number;
  note?: string;
  onAsk: Ask;
  onClose: () => void;
  /** 应用成功；`firstRun` 是之前一个模型都没有时新加入的第一个。 */
  onApplied: (message: string, firstRun: ModelChoice | null) => void;
  onEditStateChange?: (state: SettingsEditState | null) => void;
}) {
  const client = useQueryClient();
  const { writing, run } = useSettingsWrite(aiMutationOptions(client));
  const bindings = useQuery(bindingsQueryOptions()).data;
  const [providerId, setProviderId] = useState(initial);
  const [filter, setFilter] = useState("");
  const [manual, setManual] = useState("");
  const [staged, setStaged] = useState<Record<string, boolean>>({});
  const [note, setNote] = useState<Note | null>(firstNote ? { tone: "ok", text: firstNote } : null);
  const [fetching, setFetching] = useState(false);
  const [replacing, setReplacing] = useState(false);
  const provider = providers.find((p) => p.id === providerId);
  const catalog = pool?.catalogs.find((c) => c.provider_id === providerId);
  const inPool = (key: string) => !!pool?.pool.some((e) => choiceKey(e) === key);
  const checked = (key: string) => staged[key] ?? inPool(key);
  // 手填的 id 也列出来，和目录一样勾选。
  const manualIds = Object.keys(staged).map(parseChoice).filter((c): c is ModelChoice => !!c && c.provider_id === providerId)
    .map((c) => c.model).filter((m) => !catalog?.models.includes(m));
  const listed = [...manualIds, ...(catalog?.models ?? [])].filter((m) => !filter.trim() || fuzzyMatch(m, filter.trim()));
  const adds = Object.entries(staged).filter(([k, v]) => v && !inPool(k)).map(([k]) => parseChoice(k)!);
  const removes = Object.entries(staged).filter(([k, v]) => !v && inPool(k)).map(([k]) => parseChoice(k)!);
  const dirty = adds.length + removes.length > 0 || !!manual.trim();
  const busy = writing || fetching;
  const used = removes.flatMap((c) => bindings?.tasks.filter((t) => sameChoice(t, c)) ?? []);

  const toggle = (model: string) => {
    const key = choiceKey({ provider_id: providerId, model });
    setStaged((old) => {
      const next = { ...old, [key]: !(old[key] ?? inPool(key)) };
      if (next[key] === inPool(key) && catalog?.models.includes(model)) delete next[key];
      return next;
    });
    setReplacing(false);
  };
  const all = listed.length > 0 && listed.every((m) => checked(choiceKey({ provider_id: providerId, model: m })));
  const selectAll = () => {
    const apply = () => setStaged((old) => {
      const next = { ...old };
      for (const m of listed) {
        const key = choiceKey({ provider_id: providerId, model: m });
        if (!all === inPool(key)) delete next[key]; else next[key] = !all;
      }
      return next;
    });
    const fresh = listed.filter((m) => !checked(choiceKey({ provider_id: providerId, model: m }))).length;
    if (!all && fresh > BULK_CONFIRM) {
      onAsk(`一次勾选 ${fresh} 个模型？`, "加进来的模型会出现在所有选模型的地方。通常挑几个常用的就够。", apply, { okLabel: "全部勾选" });
    } else apply();
  };
  const addManual = () => {
    const model = manual.trim();
    if (!model) return;
    setManual("");
    setStaged((old) => ({ ...old, [choiceKey({ provider_id: providerId, model })]: true }));
    setNote({ tone: "ok", text: `已勾选手填的 ${model}，应用后加入。` });
  };
  const refetch = async () => {
    setNote(null);
    setFetching(true);
    const outcome = await run({ type: "catalog", providerId });
    if (outcome.status === "busy") { setFetching(false); return; }
    if (!outcome.mounted) return;
    setFetching(false);
    if (outcome.status === "failed") {
      setNote({ tone: "err", text: formatNotice(catalog
        ? `重新获取失败，已记住的 ${catalog.models.length} 个模型不受影响：`
        : "获取失败，可以先手填模型 id：", outcome.error) });
    } else {
      const next = client.getQueryData<ModelPoolView>(aiKeys.pool)?.catalogs.find((c) => c.provider_id === providerId);
      setNote({ tone: "ok", text: next ? `已更新，共 ${next.models.length} 个模型。` : "服务没有返回模型，可以手填。" });
    }
  };

  const apply = async (replace?: ModelChoice | null): Promise<boolean> => {
    if (!adds.length && !removes.length) { if (manual.trim()) addManual(); return !manual.trim(); }
    if (used.length && replace === undefined) { setReplacing(true); return false; }
    const firstRun = !pool?.pool.length && adds.length ? adds[0] : null;
    const outcome = await run({ type: "pool", args: { add: adds, remove: removes, replace: replace ?? undefined } });
    if (outcome.status === "busy" || !outcome.mounted) return false;
    if (outcome.status === "failed") { setNote({ tone: "err", text: formatNotice("没有应用：", outcome.error) }); return false; }
    const view = outcome.data.pool;
    const moved = view?.replaced.length ? `，${view.replaced.length} 项用途改用替换模型` : view?.unset.length ? `，${view.unset.length} 项用途暂停，等你另选` : "";
    onApplied(`已加入 ${adds.length} 个、移出 ${removes.length} 个模型${moved}。`, firstRun);
    onClose();
    return true;
  };

  return (
    <Dialog title="添加模型" dirty={dirty} busy={busy} onAsk={onAsk} onClose={onClose} onSave={() => apply()}
      onEditStateChange={onEditStateChange} className="wide"
      leading={<span className="hint">{adds.length || removes.length ? `将加入 ${adds.length} 个、移出 ${removes.length} 个` : "勾选要用的模型，最后一次应用。"}</span>}
      actions={<button type="button" className="btn primary" disabled={busy || (!adds.length && !removes.length)} aria-busy={writing || undefined} onClick={() => void apply()}>
        应用
      </button>}>
      <ChipBar value={String(providerId)} options={providers.map((p) => ({ id: String(p.id), label: p.name }))} disabled={busy}
        onChange={(id) => { setProviderId(Number(id)); setFilter(""); setNote(null); setReplacing(false); }} />
      {note && <p className={note.tone === "err" ? "form-status error" : "form-status"} role={note.tone === "err" ? "alert" : "status"}>{note.text}</p>}
      {replacing && (
        <ReplacePrompt pool={pool} providers={providers} removing={removes} uses={used.map((t) => t.title)} disabled={busy}
          onConfirm={(r) => void apply(r)} onCancel={() => setReplacing(false)} />
      )}
      {catalog ? <>
        <div className="row catalog-meta">
          <span className="hint">目录 {catalog.models.length} 个 · {fetchedLabel(catalog.fetched_at)} 获取 · 已加入 {pool?.pool.filter((e) => e.provider_id === providerId).length ?? 0} 个</span>
          <button type="button" className="btn sm" disabled={busy} aria-busy={fetching || undefined} onClick={() => void refetch()}>重新获取</button>
        </div>
        <div className="row catalog-filter">
          <input type="search" value={filter} placeholder="搜索模型 id，例如 gpt5m、qwen coder" aria-label="搜索模型" onChange={(e) => setFilter(e.target.value)} />
          {listed.length > 0 && (
            <button type="button" className="btn sm" disabled={busy} onClick={selectAll}>
              {all ? "取消勾选" : "全选"}{filter.trim() ? `筛选出的 ${listed.length} 个` : `全部 ${listed.length} 个`}
            </button>
          )}
        </div>
      </> : (
        <div className="catalog-empty">
          <p>还没有获取过「{provider?.name}」的模型列表。获取成功后会记住，下次打开直接显示。</p>
          <button type="button" className="btn sm primary" disabled={busy} aria-busy={fetching || undefined} onClick={() => void refetch()}>获取模型列表</button>
        </div>
      )}
      {(catalog || manualIds.length > 0) && (
        <ul className="catalog-list tall" aria-label={`${provider?.name ?? ""} 的模型`}>
          {listed.map((model) => {
            const key = choiceKey({ provider_id: providerId, model });
            const s = staged[key];
            const entry = pool?.pool.find((e) => choiceKey(e) === key);
            const uses = entry?.uses.length ?? 0;
            return (
              <li key={model}>
                <label>
                  <input type="checkbox" checked={checked(key)} disabled={busy} onChange={() => toggle(model)} />
                  <span className="mono">{model}</span>
                  {entry?.display_name && <span className="hint">{modelName(entry)}</span>}
                  {s === true && !inPool(key) ? <span className="meta-tag accent">将加入</span>
                    : s === false && inPool(key) ? <span className="meta-tag warn">将移出{uses ? ` · 用于 ${uses} 项` : ""}</span>
                    : inPool(key) ? <span className="hint">已加入</span> : null}
                </label>
              </li>
            );
          })}
          {!listed.length && <li className="hint">没有匹配「{filter.trim()}」的模型。可以在下面手填。</li>}
        </ul>
      )}
      <div className="field">
        <span>手填模型 id</span>
        <div className="row catalog-filter">
          <input value={manual} placeholder="例如 my-proxy-model" aria-label="手填模型 id" disabled={busy}
            onChange={(e) => setManual(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter" && !e.nativeEvent.isComposing) addManual(); }} />
          <button type="button" className="btn sm" disabled={busy || !manual.trim()} onClick={addManual}>加入勾选</button>
        </div>
        <span className="hint">目录里没有、或获取失败时用。挂在「{provider?.name}」下。</span>
      </div>
    </Dialog>
  );
}

/**
 * 移出正被用途使用的模型前：列出受影响的用途，当场选替换模型（或让这些用途暂停）。
 * 对话默认只会换成用于对话的模型，否则由宿主让对话暂停。
 */
export function ReplacePrompt({ pool, providers, removing, uses, disabled, onConfirm, onCancel }: {
  pool: ModelPoolView | undefined;
  providers: ProviderView[];
  removing: ModelChoice[];
  uses: string[];
  disabled?: boolean;
  onConfirm: (replace: ModelChoice | null) => void;
  onCancel: () => void;
}) {
  const gone = new Set(removing.map(choiceKey));
  const others = (pool?.pool ?? []).filter((e) => !gone.has(choiceKey(e)));
  const [target, setTarget] = useState(others[0] ? choiceKey(others[0]) : "");
  const name = (id: number) => providers.find((p) => p.id === id)?.name ?? "已删除的连接";
  return (
    <div className="replace-prompt" role="group" aria-label="改用哪个模型">
      <p><strong>{removing.length === 1 ? removing[0].model : `这 ${removing.length} 个模型`}</strong> 正在用于：{uses.join("、")}。</p>
      <label className="field">
        <span>这些用途改用</span>
        <MenuSelect label="这些用途改用" value={target} disabled={disabled} onChange={setTarget} options={[
          ...others.map((e) => ({ value: choiceKey(e), label: `${modelName(e)}（${name(e.provider_id)}）` })),
          { value: "", label: "不选，这些用途暂停" },
        ]} />
      </label>
      <div className="row">
        <button type="button" className="btn primary sm" disabled={disabled} onClick={() => onConfirm(target ? parseChoice(target) : null)}>移出</button>
        <button type="button" className="btn ghost sm" disabled={disabled} onClick={onCancel}>取消</button>
      </div>
    </div>
  );
}
