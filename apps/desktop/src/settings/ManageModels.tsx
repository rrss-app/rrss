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
import { locale } from "../i18n";

export type ModelFilter = "" | "chat" | "untested" | "unused";
const FILTERS: Array<{ id: ModelFilter; label: string }> = [
  { id: "", label: "全部" }, { id: "chat", label: "对话" }, { id: "untested", label: "能力未测" }, { id: "unused", label: "没在用" },
];
const CAPS: Array<{ key: keyof CapsOverrideView; label: string; hint: string }> = [
  { key: "tool_calling", label: "工具调用", hint: "对话查阅文章、读取网页、调用外部工具都要它。" },
  { key: "structured_output", label: "结构化输出", hint: "摘要、简报、记忆整理按固定格式返回结果。" },
  { key: "image_input", label: "图片输入", hint: "对话里可以带上文章配图。不会自动测试。" },
  { key: "reasoning", label: "推理（思考）", hint: "模型支持时才能选思考强度。" },
];
const SUPPORT: Record<CapabilitySupport, string> = { supported: "支持", unsupported: "不支持", unknown: "未测" };

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
  const name = (id: number) => providers.find((p) => p.id === id)?.name ?? "已删除的连接";
  const q = query.trim();
  const shown = entries.filter((e) => choiceKey(e) === selected || (
    (!q || fuzzyMatch(modelName(e), q) || fuzzyMatch(e.model, q) || fuzzyMatch(name(e.provider_id), q))
    && (!filter || (filter === "chat" && e.chat) || (filter === "untested" && untested(e)) || (filter === "unused" && unused(e)))));
  const current = entries.find((e) => choiceKey(e) === selected) ?? shown[0];
  const key = current ? choiceKey(current) : "";
  const draft = text?.key === key ? text.value : current ? textOf(current) : { name: "", ctx: "", out: "" };
  const dirty = !!current && text?.key === key && JSON.stringify(text.value) !== JSON.stringify(textOf(current));

  const write = async (args: Omit<UpdatePoolModelArgs, "provider_id" | "model">, done = "已保存。") => {
    if (!current) return false;
    setStatus(null);
    const outcome = await run({ type: "model", args: { provider_id: current.provider_id, model: current.model, ...args } });
    if (outcome.status === "busy" || !outcome.mounted) return outcome.status === "done";
    if (outcome.status === "failed") { setStatus({ tone: "err", text: formatNotice("没有保存：", outcome.error) }); return false; }
    setStatus({ tone: "ok", text: done });
    return true;
  };
  const saveText = async () => {
    if (!dirty || !current) return true;
    const count = (raw: string, label: string) => {
      if (!raw.trim()) return 0;
      const n = Number(raw);
      if (!Number.isInteger(n) || n <= 0) { setStatus({ tone: "err", text: `${label}要填正整数，或留空。` }); return null; }
      return n;
    };
    const ctx = count(draft.ctx, "上下文长度");
    const out = count(draft.out, "单次最多输出");
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
    if (outcome.status === "failed") { setStatus({ tone: "err", text: formatNotice("测试没有完成：", outcome.error) }); return; }
    const e = outcome.data.pool?.pool.find((x) => choiceKey(x) === key);
    setStatus({ tone: "ok", text: e ? `测试完成：${CAPS.filter((c) => c.key !== "image_input").map((c) => `${c.label}${SUPPORT[e.detected[c.key]]}`).join("，")}。结果不准时可以在下面手动改。` : "测试完成。" });
  };

  const remove = async (replace?: ModelChoice | null) => {
    if (!current) return;
    if (current.uses.length && replace === undefined) { setRemoving(true); return; }
    const outcome = await run({ type: "pool", args: { add: [], remove: [{ provider_id: current.provider_id, model: current.model }], replace: replace ?? undefined } });
    if (outcome.status === "busy" || !outcome.mounted) return;
    setRemoving(false);
    if (outcome.status === "failed") { setStatus({ tone: "err", text: formatNotice("没有移出：", outcome.error) }); return; }
    const view = outcome.data.pool;
    setSelected(null);
    setShowDetail(false);
    setStatus({ tone: "ok", text: `已移出 ${modelName(current)}${view?.replaced.length ? `，${view.replaced.length} 项用途改用替换模型` : view?.unset.length ? `，${view.unset.length} 项用途暂停，等你另选` : ""}。` });
  };

  const isDefault = !!current && sameChoice(chatDefault(bindings), current);
  const uses = current ? current.uses.map((t) => bindings?.tasks.find((x) => x.task === t)?.title ?? t) : [];
  const reasoningOff = current?.effective.reasoning === "unsupported";
  // 引擎握手报告过的更高档位；只读缓存，不为它发请求。
  const reported = useQuery({ ...piQueryOptions(), enabled: false }).data?.thinking_levels;

  return (
    <Dialog title="管理模型" dirty={dirty} busy={writing} onAsk={onAsk} onClose={onClose} onSave={saveText} className="page"
      onEditStateChange={onEditStateChange} cancelLabel="完成" actions={null}
      leading={<span className="hint">改动立即保存。</span>}>
      <div className="manage-models" data-detail={showDetail ? "1" : "0"}>
        <div className="manage-list">
          <div className="manage-list-tools">
            <input type="search" value={query} placeholder="搜索模型或连接" aria-label="搜索模型" onChange={(e) => setQuery(e.target.value)} />
            <ChipBar value={filter} options={FILTERS} onChange={(v) => setFilter(v as ModelFilter)} />
          </div>
          <ul className="manage-items" aria-label="加进来的模型">
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
                      {e.chat && <span className="dot-chat" title="用于对话" />}
                      {untested(e) ? <span className="warn">能力未测</span> : limited ? <span className="warn">能力受限</span> : null}
                      {e.uses.length > 0 && <span>{e.uses.length} 项</span>}
                    </span>
                  </button>
                </li>
              );
            })}
            {!shown.length && <li className="hint">{entries.length ? "没有符合的模型。" : "还没有加进来的模型。"}</li>}
          </ul>
          <div className="manage-list-foot">
            <button type="button" className="btn sm" onClick={onAddModels}>添加模型</button>
            <span className="hint">{entries.length} 个</span>
          </div>
        </div>

        <div className="manage-detail">
          {!current ? <p className="settings-empty">选一个模型查看和修改它的设置。</p> : <>
            <button type="button" className="link-btn manage-back" onClick={() => setShowDetail(false)}>← 模型列表</button>
            <div className="manage-detail-head">
              <div>
                <h4 className="mono">{current.model}</h4>
                <span className="hint"><span className="model-picker-tag">{name(current.provider_id)}</span>
                  {current.tested_at ? ` 能力 ${new Date(current.tested_at).toLocaleString(locale(), { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" })} 测试` : " 能力还没测试"}</span>
              </div>
              <button type="button" className="btn sm" disabled={writing} aria-busy={probing || undefined} onClick={() => void probe()}>测试能力</button>
            </div>
            {status && <p className={status.tone === "err" ? "form-status error" : "form-status"} role={status.tone === "err" ? "alert" : "status"}>{status.text}</p>}
            {removing && (
              <ReplacePrompt pool={pool} providers={providers} removing={[current]} uses={uses} disabled={writing}
                onConfirm={(r) => void remove(r)} onCancel={() => setRemoving(false)} />
            )}

            <section className="manage-section" aria-label="使用">
              <h5>使用</h5>
              <div className="settings-option">
                <div><strong>用于对话</strong>
                  <p className="hint">打开后出现在对话的模型列表里。{isDefault ? "它是对话默认模型，要关掉先在模型页「对话」里改选别的。" : ""}</p></div>
                <Toggle label="用于对话" on={current.chat} onLabel="已开启" offLabel="已关闭" disabled={writing || (current.chat && isDefault)}
                  onClick={() => void write({ chat: !current.chat })} />
              </div>
              <div className="settings-option">
                <div><strong>用在这些地方</strong><p className="hint">{uses.length ? uses.join("、") : "还没有用途选它。"}</p></div>
                <button type="button" className="link-btn" onClick={onShowUses}>去改用途</button>
              </div>
              <div className="settings-option">
                <div><strong>显示名称</strong><p className="hint">选择列表和对话输入区显示它；留空显示模型 id。</p></div>
                <input value={draft.name} aria-label="显示名称" placeholder={current.model} readOnly={writing}
                  onChange={(e) => setText({ key, value: { ...draft, name: e.target.value } })} onBlur={() => void saveText()} onKeyDown={enterSaves} />
              </div>
            </section>

            <section className="manage-section" aria-label="能力">
              <h5>能力</h5>
              <p className="hint">「测试能力」会实际调用一次模型；结果不准可以手动改，以手动设置为准。</p>
              {CAPS.map((c) => {
                const over = current.overrides[c.key];
                return (
                  <div className="settings-option" key={c.key}>
                    <div><strong>{c.label}</strong><p className="hint">{c.hint}</p></div>
                    <ChipBar className="segmented" disabled={writing}
                      value={over === true ? "yes" : over === false ? "no" : "auto"}
                      options={[{ id: "auto", label: `按测试（${SUPPORT[current.detected[c.key]]}）` }, { id: "yes", label: "支持" }, { id: "no", label: "不支持" }]}
                      onChange={(v) => void write({ overrides: { ...current.overrides, [c.key]: v === "auto" ? undefined : v === "yes" } })} />
                  </div>
                );
              })}
            </section>

            <section className="manage-section" aria-label="对话参数">
              <h5>对话参数</h5>
              <div className="settings-option">
                <div><strong>思考强度</strong>
                  <p className="hint">{reasoningOff ? "这个模型不支持推理，对话时不思考。" : "在对话里用这个模型时的默认档位；单条对话可以在输入区另选。"}</p></div>
                {!reasoningOff && <ChipBar className="segmented" value={current.thinking} options={thinkingOptions(reported)} disabled={writing}
                  onChange={(v) => void write({ thinking: v })} />}
              </div>
              <div className="settings-option">
                <div><strong>上下文长度</strong><p className="hint">单位 token；留空按模型默认{current.engine_context_window ? "" : "（开始一次对话后显示）"}。</p></div>
                <input value={draft.ctx} aria-label="上下文长度" inputMode="numeric" readOnly={writing}
                  placeholder={current.engine_context_window ? `默认 ${current.engine_context_window.toLocaleString("en-US")}` : "默认"}
                  onChange={(e) => setText({ key, value: { ...draft, ctx: e.target.value } })} onBlur={() => void saveText()} onKeyDown={enterSaves} />
              </div>
              <div className="settings-option">
                <div><strong>单次最多输出</strong><p className="hint">单位 token；留空不限制。</p></div>
                <input value={draft.out} aria-label="单次最多输出" inputMode="numeric" placeholder="例如 8192" readOnly={writing}
                  onChange={(e) => setText({ key, value: { ...draft, out: e.target.value } })} onBlur={() => void saveText()} onKeyDown={enterSaves} />
              </div>
            </section>

            <ModelCompat key={key} entry={current} connection={name(current.provider_id)} disabled={writing}
              onSave={(compat) => write({ compat_json: compat }, compat ? "已保存这个模型的兼容设置。" : "已改回用连接的兼容设置。")} />

            <div className="manage-detail-foot">
              <button type="button" className="btn danger" disabled={writing} onClick={() => void remove()}>从模型列表移出</button>
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
  const own = !!entry.compat_json;
  const [text, setText] = useState(prettyJson(entry.compat_json ?? entry.recommended_compat_json ?? "{}"));
  const valid = jsonObject(text) !== null;
  return (
    <details className="manage-section advanced-fold" open={own || undefined}>
      <summary>高级：兼容设置</summary>
      <p className="hint">默认用连接「{connection}」的兼容设置。{entry.recommended_compat_json ? "测试能力给出了推荐值，已填在下面。" : ""}</p>
      <div className="settings-option">
        <div><strong>只对这个模型单独设置</strong></div>
        <Toggle label="单独设置兼容" on={own} onLabel="已开启" offLabel="已关闭" disabled={disabled || (!own && !valid)}
          onClick={() => void onSave(own ? "" : text)} />
      </div>
      {own && <>
        <CompatEditor value={text} onChange={setText} disabled={disabled} label={`${entry.model}的兼容设置`} />
        <button type="button" className="btn sm" disabled={disabled || !valid} onClick={() => void onSave(text)}>保存兼容设置</button>
      </>}
    </details>
  );
}
