import { useState, type KeyboardEvent } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import type { CapabilitySupport, CapsOverrideView, ModelChoice, PoolEntryView, ProviderView, UpdatePoolModelArgs } from "../lib/api";
import { fuzzyMatch } from "../lib/fuzzy";
import { formatNotice } from "../lib/notice";
import { aiMutationOptions, bindingsQueryOptions, poolQueryOptions } from "./aiQuery";
import { ReplacePrompt } from "./AddModelsDialog";
import { ChipBar, Toggle } from "./chips";
import { CompatEditor, jsonObject, prettyJson } from "./compat";
import { Dialog } from "./Dialog";
import { useSettingsWrite, type SettingsEditState } from "./editState";
import { chatDefault, choiceKey, modelName, sameChoice, thinkingOptions } from "./modelChoices";
import { piQueryOptions } from "./piQuery";
import type { Ask } from "./SettingsPanel";
import { locale, useTranslation } from "../i18n";

export type ModelFilter = "" | "chat" | "untested" | "unused";
const FILTERS: ModelFilter[] = ["", "chat", "untested", "unused"];
const CAPS: Array<keyof CapsOverrideView & ("tool_calling" | "structured_output" | "image_input" | "reasoning")> = [
  "tool_calling", "structured_output", "image_input", "reasoning",
];

export const untested = (e: PoolEntryView) => !e.tested_at;
export const unused = (e: PoolEntryView) => !e.uses.length && !e.chat;

type Text = { name: string; ctx: string; out: string };
const textOf = (e: PoolEntryView): Text => ({ name: e.display_name, ctx: e.context_window ? String(e.context_window) : "", out: e.max_output ? String(e.max_output) : "" });

/**
 * 管理模型（阶段 19-7）：覆盖设置面板的弹出页，左侧列表，右侧选中模型的设置；改动立即保存。
 * 文字项在失焦或回车时保存，离开时有「保存并离开」。窄屏先列表后详情。
 */
export function ManageModels({ providers, initialFilter, initialModel, onAsk, onClose, onAddModels, onShowUses, onEditStateChange }: {
  providers: ProviderView[];
  initialFilter: ModelFilter;
  initialModel?: ModelChoice | null;
  onAsk: Ask;
  onClose: () => void;
  onAddModels: () => void;
  /** 关掉弹出页，回到模型页的「各用途」。 */
  onShowUses: () => void;
  onEditStateChange?: (state: SettingsEditState | null) => void;
}) {
  const { t } = useTranslation("settingsModels");
  const support = (state: CapabilitySupport) => t(`manage.support.${state}`);
  const client = useQueryClient();
  const { writing, run } = useSettingsWrite(aiMutationOptions(client));
  const pool = useQuery(poolQueryOptions()).data;
  const bindings = useQuery(bindingsQueryOptions()).data;
  const [filter, setFilter] = useState<ModelFilter>(initialFilter);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string | null>(initialModel ? choiceKey(initialModel) : null);
  const [showDetail, setShowDetail] = useState(!!initialModel);
  const [text, setText] = useState<{ key: string; value: Text } | null>(null);
  const [status, setStatus] = useState<{ tone: "ok" | "err"; text: string } | null>(null);
  const [probing, setProbing] = useState(false);
  const [removing, setRemoving] = useState(false);

  const entries = pool?.pool ?? [];
  const name = (id: number) => providers.find((p) => p.id === id)?.name ?? t("manage.deletedConnection");
  const q = query.trim();
  const shown = entries.filter((e) => choiceKey(e) === selected || (
    (!q || fuzzyMatch(modelName(e), q) || fuzzyMatch(e.model, q) || fuzzyMatch(name(e.provider_id), q))
    && (!filter || (filter === "chat" && e.chat) || (filter === "untested" && untested(e)) || (filter === "unused" && unused(e)))));
  const current = entries.find((e) => choiceKey(e) === selected) ?? shown[0];
  const key = current ? choiceKey(current) : "";
  const draft = text?.key === key ? text.value : current ? textOf(current) : { name: "", ctx: "", out: "" };
  const dirty = !!current && text?.key === key && JSON.stringify(text.value) !== JSON.stringify(textOf(current));

  const write = async (args: Omit<UpdatePoolModelArgs, "provider_id" | "model">, done?: string) => {
    if (!current) return false;
    setStatus(null);
    const outcome = await run({ type: "model", args: { provider_id: current.provider_id, model: current.model, ...args } });
    if (outcome.status === "busy" || !outcome.mounted) return outcome.status === "done";
    if (outcome.status === "failed") { setStatus({ tone: "err", text: formatNotice(t("manage.saveFailed"), outcome.error) }); return false; }
    setStatus({ tone: "ok", text: done ?? t("manage.saved") });
    return true;
  };
  const saveText = async () => {
    if (!dirty || !current) return true;
    const count = (raw: string, label: string) => {
      if (!raw.trim()) return 0;
      const n = Number(raw);
      if (!Number.isInteger(n) || n <= 0) { setStatus({ tone: "err", text: t("manage.positiveInt", { label }) }); return null; }
      return n;
    };
    const ctx = count(draft.ctx, t("manage.ctxLabel"));
    const out = count(draft.out, t("manage.outLabel"));
    if (ctx === null || out === null) return false;
    const ok = await write({ display_name: draft.name.trim(), context_window: ctx, max_output: out });
    if (ok) setText(null);
    return ok;
  };

  // 写入中文字框只读而不禁用：禁用会让焦点被设置层的焦点陷阱移到「返回」，同一次回车就把设置关了。
  const enterSaves = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key !== "Enter" || e.nativeEvent.isComposing) return;
    e.preventDefault();
    void saveText();
  };

  const probe = async () => {
    if (!current) return;
    setStatus(null);
    setProbing(true);
    const outcome = await run({ type: "probe", providerId: current.provider_id, model: current.model });
    setProbing(false);
    if (outcome.status === "busy" || !outcome.mounted) return;
    if (outcome.status === "failed") { setStatus({ tone: "err", text: formatNotice(t("manage.probeFailed"), outcome.error) }); return; }
    const e = outcome.data.pool?.pool.find((x) => choiceKey(x) === key);
    setStatus({ tone: "ok", text: e ? t("manage.probeDone", { caps: CAPS.filter((c) => c !== "image_input").map((c) => t("manage.capResult", { label: t(`manage.cap.${c}.label`), support: support(e.detected[c]) })).join(t("manage.capSep")) }) : t("manage.probeDoneBare") });
  };

  const remove = async (replace?: ModelChoice | null) => {
    if (!current) return;
    if (current.uses.length && replace === undefined) { setRemoving(true); return; }
    const outcome = await run({ type: "pool", args: { add: [], remove: [{ provider_id: current.provider_id, model: current.model }], replace: replace ?? undefined } });
    if (outcome.status === "busy" || !outcome.mounted) return;
    setRemoving(false);
    if (outcome.status === "failed") { setStatus({ tone: "err", text: formatNotice(t("manage.removeFailed"), outcome.error) }); return; }
    const view = outcome.data.pool;
    setSelected(null);
    setShowDetail(false);
    setStatus({ tone: "ok", text: view?.replaced.length ? t("manage.removedReplaced", { name: modelName(current), count: view.replaced.length })
      : view?.unset.length ? t("manage.removedUnset", { name: modelName(current), count: view.unset.length })
      : t("manage.removed", { name: modelName(current) }) });
  };

  const isDefault = !!current && sameChoice(chatDefault(bindings), current);
  const uses = current ? current.uses.map((use) => bindings?.tasks.find((x) => x.task === use)?.title ?? use) : [];
  const reasoningOff = current?.effective.reasoning === "unsupported";
  // 引擎握手报告过的更高档位；只读缓存，不为它发请求。
  const reported = useQuery({ ...piQueryOptions(), enabled: false }).data?.thinking_levels;

  return (
    <Dialog title={t("manage.title")} dirty={dirty} busy={writing} onAsk={onAsk} onClose={onClose} onSave={saveText} className="page"
      onEditStateChange={onEditStateChange} cancelLabel={t("manage.done")} actions={null}
      leading={<span className="hint">{t("manage.autosave")}</span>}>
      <div className="manage-models" data-detail={showDetail ? "1" : "0"}>
        <div className="manage-list">
          <div className="manage-list-tools">
            <input type="search" value={query} placeholder={t("manage.searchPlaceholder")} aria-label={t("manage.searchAria")} onChange={(e) => setQuery(e.target.value)} />
            <ChipBar value={filter} options={FILTERS.map((id) => ({ id, label: t(`manage.filter.${id || "all"}`) }))} onChange={(v) => setFilter(v as ModelFilter)} />
          </div>
          <ul className="manage-items" aria-label={t("manage.listAria")}>
            {shown.map((e, i) => {
              const group = i === 0 || shown[i - 1].provider_id !== e.provider_id ? name(e.provider_id) : null;
              const limited = e.effective.tool_calling === "unsupported" || e.effective.structured_output === "unsupported";
              return (
                <li key={choiceKey(e)}>
                  {group && <div className="manage-group">{group}</div>}
                  <button type="button" className="manage-item" aria-current={choiceKey(e) === key ? "true" : undefined}
                    onClick={() => { if (dirty) void saveText(); setSelected(choiceKey(e)); setShowDetail(true); setStatus(null); setRemoving(false); }}>
                    <span className="mono">{modelName(e)}</span>
                    <span className="manage-meta">
                      {e.chat && <span className="dot-chat" title={t("manage.forChat")} />}
                      {untested(e) ? <span className="warn">{t("manage.untested")}</span> : limited ? <span className="warn">{t("manage.limited")}</span> : null}
                      {e.uses.length > 0 && <span>{t("manage.usesCount", { count: e.uses.length })}</span>}
                    </span>
                  </button>
                </li>
              );
            })}
            {!shown.length && <li className="hint">{t(entries.length ? "manage.noMatch" : "manage.noModels")}</li>}
          </ul>
          <div className="manage-list-foot">
            <button type="button" className="btn sm" onClick={onAddModels}>{t("manage.add")}</button>
            <span className="hint">{t("manage.count", { count: entries.length })}</span>
          </div>
        </div>

        <div className="manage-detail">
          {!current ? <p className="settings-empty">{t("manage.pickOne")}</p> : <>
            <button type="button" className="link-btn manage-back" onClick={() => setShowDetail(false)}>{t("manage.back")}</button>
            <div className="manage-detail-head">
              <div>
                <h4 className="mono">{current.model}</h4>
                <span className="hint"><span className="model-picker-tag">{name(current.provider_id)}</span>
                  {current.tested_at ? t("manage.testedAt", { time: new Date(current.tested_at).toLocaleString(locale(), { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" }) }) : t("manage.neverTested")}</span>
              </div>
              <button type="button" className="btn sm" disabled={writing} aria-busy={probing || undefined} onClick={() => void probe()}>{t("manage.probe")}</button>
            </div>
            {status && <p className={status.tone === "err" ? "form-status error" : "form-status"} role={status.tone === "err" ? "alert" : "status"}>{status.text}</p>}
            {removing && (
              <ReplacePrompt pool={pool} providers={providers} removing={[current]} uses={uses} disabled={writing}
                onConfirm={(r) => void remove(r)} onCancel={() => setRemoving(false)} />
            )}

            <section className="manage-section" aria-label={t("manage.useSection")}>
              <h5>{t("manage.useSection")}</h5>
              <div className="settings-option">
                <div><strong>{t("manage.forChat")}</strong>
                  <p className="hint">{t(isDefault ? "manage.chatHintDefault" : "manage.chatHint")}</p></div>
                <Toggle label={t("manage.forChat")} on={current.chat} onLabel={t("toggle.on")} offLabel={t("toggle.off")} disabled={writing || (current.chat && isDefault)}
                  onClick={() => void write({ chat: !current.chat })} />
              </div>
              <div className="settings-option">
                <div><strong>{t("manage.usedAt")}</strong><p className="hint">{uses.length ? uses.join(t("manage.listSep")) : t("manage.noUses")}</p></div>
                <button type="button" className="link-btn" onClick={onShowUses}>{t("manage.editUses")}</button>
              </div>
              <div className="settings-option">
                <div><strong>{t("manage.displayName")}</strong><p className="hint">{t("manage.displayNameHint")}</p></div>
                <input value={draft.name} aria-label={t("manage.displayName")} placeholder={current.model} readOnly={writing}
                  onChange={(e) => setText({ key, value: { ...draft, name: e.target.value } })} onBlur={() => void saveText()} onKeyDown={enterSaves} />
              </div>
            </section>

            <section className="manage-section" aria-label={t("manage.capSection")}>
              <h5>{t("manage.capSection")}</h5>
              <p className="hint">{t("manage.capIntro")}</p>
              {CAPS.map((c) => {
                const over = current.overrides[c];
                return (
                  <div className="settings-option" key={c}>
                    <div><strong>{t(`manage.cap.${c}.label`)}</strong><p className="hint">{t(`manage.cap.${c}.hint`)}</p></div>
                    <ChipBar className="segmented" disabled={writing}
                      value={over === true ? "yes" : over === false ? "no" : "auto"}
                      options={[{ id: "auto", label: t("manage.byTest", { support: support(current.detected[c]) }) }, { id: "yes", label: t("manage.support.supported") }, { id: "no", label: t("manage.support.unsupported") }]}
                      onChange={(v) => void write({ overrides: { ...current.overrides, [c]: v === "auto" ? undefined : v === "yes" } })} />
                  </div>
                );
              })}
            </section>

            <section className="manage-section" aria-label={t("manage.chatSection")}>
              <h5>{t("manage.chatSection")}</h5>
              <div className="settings-option">
                <div><strong>{t("manage.thinking")}</strong>
                  <p className="hint">{t(reasoningOff ? "manage.thinkingOff" : "manage.thinkingHint")}</p></div>
                {!reasoningOff && <ChipBar className="segmented" value={current.thinking} options={thinkingOptions(reported)} disabled={writing}
                  onChange={(v) => void write({ thinking: v })} />}
              </div>
              <div className="settings-option">
                <div><strong>{t("manage.ctxLabel")}</strong><p className="hint">{t(current.engine_context_window ? "manage.ctxHint" : "manage.ctxHintPending")}</p></div>
                <input value={draft.ctx} aria-label={t("manage.ctxLabel")} inputMode="numeric" readOnly={writing}
                  placeholder={current.engine_context_window ? t("manage.ctxDefaultN", { n: current.engine_context_window.toLocaleString("en-US") }) : t("manage.ctxDefault")}
                  onChange={(e) => setText({ key, value: { ...draft, ctx: e.target.value } })} onBlur={() => void saveText()} onKeyDown={enterSaves} />
              </div>
              <div className="settings-option">
                <div><strong>{t("manage.outLabel")}</strong><p className="hint">{t("manage.outHint")}</p></div>
                <input value={draft.out} aria-label={t("manage.outLabel")} inputMode="numeric" placeholder={t("manage.outPlaceholder")} readOnly={writing}
                  onChange={(e) => setText({ key, value: { ...draft, out: e.target.value } })} onBlur={() => void saveText()} onKeyDown={enterSaves} />
              </div>
            </section>

            <ModelCompat key={key} entry={current} connection={name(current.provider_id)} disabled={writing}
              onSave={(compat) => write({ compat_json: compat }, compat ? t("manage.compatSaved") : t("manage.compatReset"))} />

            <div className="manage-detail-foot">
              <button type="button" className="btn danger" disabled={writing} onClick={() => void remove()}>{t("manage.removeBtn")}</button>
            </div>
          </>}
        </div>
      </div>
    </Dialog>
  );
}

/** 单个模型的兼容设置：默认用连接的；单独设置时编辑 JSON，按「保存」提交。 */
function ModelCompat({ entry, connection, disabled, onSave }: {
  entry: PoolEntryView;
  connection: string;
  disabled: boolean;
  onSave: (compat: string) => Promise<boolean>;
}) {
  const { t } = useTranslation("settingsModels");
  const own = !!entry.compat_json;
  const [text, setText] = useState(prettyJson(entry.compat_json ?? entry.recommended_compat_json ?? "{}"));
  const valid = jsonObject(text) !== null;
  return (
    <details className="manage-section advanced-fold" open={own || undefined}>
      <summary>{t("manage.compatAdvanced")}</summary>
      <p className="hint">{t(entry.recommended_compat_json ? "manage.compatDefaultRecommended" : "manage.compatDefault", { connection })}</p>
      <div className="settings-option">
        <div><strong>{t("manage.compatOwn")}</strong></div>
        <Toggle label={t("manage.compatToggle")} on={own} onLabel={t("toggle.on")} offLabel={t("toggle.off")} disabled={disabled || (!own && !valid)}
          onClick={() => void onSave(own ? "" : text)} />
      </div>
      {own && <>
        <CompatEditor value={text} onChange={setText} disabled={disabled} label={t("manage.compatLabel", { model: entry.model })} />
        <button type="button" className="btn sm" disabled={disabled || !valid} onClick={() => void onSave(text)}>{t("manage.compatSave")}</button>
      </>}
    </details>
  );
}
